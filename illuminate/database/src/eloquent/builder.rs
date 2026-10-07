//! The Eloquent query builder.

use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;

use futures::StreamExt;
use futures::stream::BoxStream;

use futures::TryStreamExt;
use illuminate_pagination::{
    LengthAwarePaginator, Paginator, PaginatorOptions, current_page, current_path,
};
use illuminate_support::{Collection, Conditionable, Map, Result, Str, Tappable, Value};

use super::errors::{ModelNotFoundException, RelationNotFoundException};
use super::events::{ModelEvent, fire};
use super::model::{Model, normalize_key, now};
use super::relations::{self, Constraint, DynRelation, EagerSpec};
use super::scope::{Scope, scope_name};
use super::state::{EloquentState, ScopeFn};
use super::{Attributes, IntoIds, IntoRelations};
use crate::error::MultipleRecordsFoundException;
use crate::expression::{Expression, Ident};
use crate::pagination::{Cursor, CursorPaginator, CursorPaginatorOptions, resolve_current_cursor};
use crate::query::Builder as QueryBuilder;
use crate::{Connection, DatabaseManager};

/// A callback run on the models a query returns (`after_query`).
pub type ModelsCallback<M> = Arc<dyn Fn(Collection<M>) -> Collection<M> + Send + Sync>;

/// Which soft deleted models a query includes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrashedMode {
    /// Exclude soft deleted models (the default).
    #[default]
    Without,
    /// Include soft deleted models.
    With,
    /// Only soft deleted models.
    Only,
}

/// A query for models of type `M`.
///
/// It wraps the base [query builder](crate::query::Builder), forwarding its
/// clause methods, and returns hydrated models. Global scopes and the soft
/// delete scope are applied when the query runs.
///
/// ```ignore
/// let users = User::query()
///     .where_("active", true)
///     .with("posts")
///     .latest()
///     .paginate(15)
///     .await?;
/// ```
pub struct Builder<M: Model> {
    pub(crate) query: QueryBuilder,
    pub(crate) eager: Vec<(String, EagerSpec)>,
    removed_scopes: Vec<String>,
    without_scopes: bool,
    trashed: TrashedMode,
    pub(crate) qualifier: String,
    scopes_applied: bool,
    local_scopes: Vec<(String, ScopeFn<M>)>,
    pending_attributes: Attributes,
    after_query_callbacks: Vec<ModelsCallback<M>>,
    _model: PhantomData<fn() -> M>,
}

impl<M: Model> Clone for Builder<M> {
    fn clone(&self) -> Self {
        Self {
            query: self.query.clone(),
            eager: self.eager.clone(),
            removed_scopes: self.removed_scopes.clone(),
            without_scopes: self.without_scopes,
            trashed: self.trashed,
            qualifier: self.qualifier.clone(),
            scopes_applied: self.scopes_applied,
            local_scopes: self.local_scopes.clone(),
            pending_attributes: self.pending_attributes.clone(),
            after_query_callbacks: self.after_query_callbacks.clone(),
            _model: PhantomData,
        }
    }
}

impl<M: Model> fmt::Debug for Builder<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Builder")
            .field("model", &M::class_name())
            .field("sql", &self.to_sql())
            .field("bindings", &self.applied().query.get_bindings())
            .field("eager", &self.eager)
            .finish()
    }
}

impl<M: Model> Default for Builder<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: Model> Conditionable for Builder<M> {}
impl<M: Model> Tappable for Builder<M> {}

impl<M: Model> Builder<M> {
    /// A new query for the model, on its connection.
    pub fn new() -> Self {
        Self::on_connection(M::get_connection())
    }

    /// A new query for the model on the named connection.
    pub fn on(connection: &str) -> Self {
        Self::on_connection(DatabaseManager::resolve().connection(connection))
    }

    /// A new query for the model on the given connection.
    pub fn on_connection(connection: Connection) -> Self {
        EloquentState::resolve().boot::<M>();
        Self {
            query: connection.table(M::table()),
            eager: Vec::new(),
            removed_scopes: Vec::new(),
            without_scopes: false,
            trashed: TrashedMode::Without,
            qualifier: M::table(),
            scopes_applied: false,
            local_scopes: Vec::new(),
            pending_attributes: Attributes::new(),
            after_query_callbacks: Vec::new(),
            _model: PhantomData,
        }
    }

    /// Wrap a base query targeting the model's table.
    pub fn from_query(query: QueryBuilder) -> Self {
        EloquentState::resolve().boot::<M>();
        let qualifier = query.table_name().map(table_alias).unwrap_or_else(M::table);
        Self {
            query,
            eager: Vec::new(),
            removed_scopes: Vec::new(),
            without_scopes: false,
            trashed: TrashedMode::Without,
            qualifier,
            scopes_applied: false,
            local_scopes: Vec::new(),
            pending_attributes: Attributes::new(),
            after_query_callbacks: Vec::new(),
            _model: PhantomData,
        }
    }

    /// Select the model's table under an alias.
    pub(crate) fn aliased(mut self, alias: &str) -> Self {
        self.query.from = Some(Ident::Name(format!("{} as {alias}", M::table())));
        self.qualifier = alias.to_string();
        self
    }

    // ------------------------------------------------------------------
    // Inspection
    // ------------------------------------------------------------------

    /// Qualify a column with the model's table (or alias): `users.name`.
    pub fn qualify_column(&self, column: &str) -> String {
        if column.contains('.') {
            column.to_string()
        } else {
            format!("{}.{column}", self.qualifier)
        }
    }

    /// The underlying base query (without global scopes applied).
    pub fn get_query(&self) -> &QueryBuilder {
        &self.query
    }

    /// The underlying base query, mutably.
    pub fn get_query_mut(&mut self) -> &mut QueryBuilder {
        &mut self.query
    }

    /// Transform the underlying base query.
    pub fn map_query(mut self, callback: impl FnOnce(QueryBuilder) -> QueryBuilder) -> Self {
        self.query = callback(self.query);
        self
    }

    /// The base query with global scopes applied (Laravel's `toBase()`).
    pub fn to_base(&self) -> QueryBuilder {
        self.applied().query
    }

    /// The query's SQL, with global scopes applied.
    pub fn to_sql(&self) -> String {
        self.applied().query.to_sql()
    }

    /// The query's SQL with bindings interpolated.
    pub fn to_raw_sql(&self) -> String {
        self.applied().query.to_raw_sql()
    }

    /// The relationships that will be eager loaded.
    pub fn get_eager_loads(&self) -> &[(String, EagerSpec)] {
        &self.eager
    }

    // ------------------------------------------------------------------
    // Clauses
    // ------------------------------------------------------------------

    forward_clauses!(query);

    /// Order by the model's "created at" column, newest first.
    pub fn latest(self) -> Self {
        let column = self.qualify_column(M::created_at_column());
        self.order_by(column, "desc")
    }

    /// Order by the model's "created at" column, oldest first.
    pub fn oldest(self) -> Self {
        let column = self.qualify_column(M::created_at_column());
        self.order_by(column, "asc")
    }

    /// Constrain the query to the given primary key(s).
    pub fn where_key(mut self, id: impl Into<Value>) -> Self {
        let column = self.qualify_column(M::primary_key());
        self.query = match id.into() {
            Value::Array(ids) => {
                let ids: Vec<Value> = ids.into_iter().map(normalize_key::<M>).collect();
                self.query.where_in(column, ids)
            }
            id => self.query.where_(column, normalize_key::<M>(id)),
        };
        self
    }

    /// Exclude the given primary key(s).
    pub fn where_key_not(mut self, id: impl Into<Value>) -> Self {
        let column = self.qualify_column(M::primary_key());
        self.query = match id.into() {
            Value::Array(ids) => {
                let ids: Vec<Value> = ids.into_iter().map(normalize_key::<M>).collect();
                self.query.where_not_in(column, ids)
            }
            id => self.query.where_op(column, "!=", normalize_key::<M>(id)),
        };
        self
    }

    /// Exclude the given model(s) from the results.
    ///
    /// ```ignore
    /// let others = User::query().except(&current_user).get().await?;
    /// ```
    pub fn except(self, models: impl super::RelatedModels<M>) -> Self {
        let (models, _) = models.into_related_models();
        let keys: Vec<Value> = models.iter().map(|model| model.get_key()).collect();
        self.where_key_not(Value::Array(keys))
    }

    /// Add an "or" clause matching the given primary key(s).
    pub fn or_where_key(mut self, id: impl Into<Value>) -> Self {
        let column = self.qualify_column(M::primary_key());
        self.query = match id.into() {
            Value::Array(ids) => {
                let ids: Vec<Value> = ids.into_iter().map(normalize_key::<M>).collect();
                self.query.or_where_in(column, ids)
            }
            id => self.query.or_where(column, normalize_key::<M>(id)),
        };
        self
    }

    /// Add an "or" clause excluding the given primary key(s).
    pub fn or_where_key_not(mut self, id: impl Into<Value>) -> Self {
        let column = self.qualify_column(M::primary_key());
        self.query = match id.into() {
            Value::Array(ids) => {
                let ids: Vec<Value> = ids.into_iter().map(normalize_key::<M>).collect();
                self.query.or_where_not_in(column, ids)
            }
            id => self.query.or_where_op(column, "!=", normalize_key::<M>(id)),
        };
        self
    }

    /// Constrain the query to the attributes, and give them to every model
    /// the builder creates (`create`, `first_or_create`, `make`, ...).
    ///
    /// ```ignore
    /// let drafts = Post::query().with_attributes(json!({"status": "draft"}));
    /// let post = drafts.create(json!({"title": "Hello"})).await?; // status = draft
    /// ```
    pub fn with_attributes(self, attributes: impl Into<Attributes>) -> Self {
        self.with_attributes_as(attributes, true)
    }

    /// Give the attributes to every model the builder creates, constraining
    /// the query to them only when `as_conditions` is true.
    pub fn with_attributes_as(
        mut self,
        attributes: impl Into<Attributes>,
        as_conditions: bool,
    ) -> Self {
        let attributes = attributes.into();
        if as_conditions {
            for (column, value) in &attributes.0 {
                let column = self.qualify_column(column);
                self.query = self.query.where_(column, value.clone());
            }
        }
        self.pending_attributes = std::mem::take(&mut self.pending_attributes).merge(attributes);
        self
    }

    /// A new model instance carrying the builder's pending attributes.
    pub fn new_model_instance(&self, attributes: impl Into<Attributes>) -> Result<M> {
        let mut model = M::template();
        if !self.pending_attributes.is_empty() {
            model.force_fill(self.pending_attributes.clone())?;
        }
        model.fill(attributes)?;
        Ok(model)
    }

    /// A new model instance, filled without mass assignment protection.
    fn new_model_instance_forced(&self, attributes: impl Into<Attributes>) -> Result<M> {
        let mut model = M::template();
        model.force_fill(self.pending_attributes.clone().merge(attributes))?;
        Ok(model)
    }

    /// Apply a local scope.
    ///
    /// ```ignore
    /// impl User {
    ///     pub fn popular(query: Builder<Self>) -> Builder<Self> {
    ///         query.where_op("votes", ">", 100)
    ///     }
    /// }
    ///
    /// let users = User::query().scope(User::popular).get().await?;
    /// ```
    pub fn scope(self, scope: impl FnOnce(Self) -> Self) -> Self {
        scope(self)
    }

    /// Include soft deleted models.
    pub fn with_trashed(mut self) -> Self {
        self.trashed = TrashedMode::With;
        self
    }

    /// Only include soft deleted models.
    pub fn only_trashed(mut self) -> Self {
        self.trashed = TrashedMode::Only;
        self
    }

    /// Exclude soft deleted models (the default).
    pub fn without_trashed(mut self) -> Self {
        self.trashed = TrashedMode::Without;
        self
    }

    /// Remove a registered global scope from the query.
    pub fn without_global_scope(mut self, name: &str) -> Self {
        self.removed_scopes.push(name.to_string());
        self
    }

    /// Remove a [`Scope`] object from the query (Laravel's
    /// `withoutGlobalScope(AncientScope::class)`).
    ///
    /// ```ignore
    /// let everyone = User::query()
    ///     .without_global_scope_object::<AncientScope>()
    ///     .get()
    ///     .await?;
    /// ```
    pub fn without_global_scope_object<S: Scope<M>>(self) -> Self {
        self.without_global_scope(&scope_name::<S>())
    }

    /// Remove every global scope (including the soft delete scope).
    pub fn without_global_scopes(mut self) -> Self {
        self.without_scopes = true;
        self.trashed = TrashedMode::With;
        self
    }

    /// Remove every global scope except the given ones (by name; the soft
    /// delete scope is `"SoftDeletingScope"`).
    pub fn without_global_scopes_except(mut self, scopes: impl IntoRelations) -> Self {
        let keep = scopes.into_relations();
        let names: Vec<String> = EloquentState::resolve()
            .global_scopes::<M>()
            .into_iter()
            .map(|(name, _)| name)
            .chain(self.local_scopes.iter().map(|(name, _)| name.clone()))
            .collect();
        for name in names {
            if !keep.contains(&name) {
                self.removed_scopes.push(name);
            }
        }
        if !keep.iter().any(|name| name == "SoftDeletingScope") {
            self.trashed = TrashedMode::With;
        }
        self
    }

    /// Register a global scope on this query only.
    pub fn with_global_scope(
        mut self,
        name: &str,
        scope: impl Fn(Builder<M>) -> Builder<M> + Send + Sync + 'static,
    ) -> Self {
        self.local_scopes.retain(|(existing, _)| existing != name);
        self.local_scopes.push((name.to_string(), Arc::new(scope)));
        self
    }

    /// The names of the global scopes removed from the query.
    pub fn removed_scopes(&self) -> &[String] {
        &self.removed_scopes
    }

    /// Register a callback run on the models the query returns.
    ///
    /// ```ignore
    /// let users = User::query()
    ///     .after_query(|users| users.filter(|user| user.active))
    ///     .get()
    ///     .await?;
    /// ```
    pub fn after_query(
        mut self,
        callback: impl Fn(Collection<M>) -> Collection<M> + Send + Sync + 'static,
    ) -> Self {
        self.after_query_callbacks.push(Arc::new(callback));
        self
    }

    /// Run the "after query" callbacks on the models.
    pub fn apply_after_query_callbacks(&self, mut models: Collection<M>) -> Collection<M> {
        for callback in &self.after_query_callbacks {
            models = callback(models);
        }
        models
    }

    /// Apply the global scopes and the soft delete scope.
    pub(crate) fn applied(&self) -> Self {
        let mut builder = self.clone();
        if builder.scopes_applied {
            return builder;
        }
        builder.scopes_applied = true;
        let scopes: Vec<_> = if builder.without_scopes {
            Vec::new()
        } else {
            EloquentState::resolve()
                .global_scopes::<M>()
                .into_iter()
                .chain(builder.local_scopes.iter().cloned())
                .filter(|(name, _)| !builder.removed_scopes.contains(name))
                .collect()
        };
        let soft_deletes = M::soft_deletes() && builder.trashed != TrashedMode::With;
        if !scopes.is_empty() || soft_deletes {
            // `a or b` must become `(a or b) and scope`.
            builder.query = nest_wheres_from(builder.query, 0, 0);
        }
        for (_, scope) in scopes {
            let (wheres, bindings) = (
                builder.query.wheres.len(),
                builder.query.bindings.where_.len(),
            );
            builder = scope(builder);
            builder.query = nest_wheres_from(builder.query, wheres, bindings);
        }
        if soft_deletes {
            let column = builder.qualify_column(M::deleted_at_column());
            builder.query = match builder.trashed {
                TrashedMode::Only => builder.query.where_not_null(column),
                _ => builder.query.where_null(column),
            };
        }
        builder
    }

    // ------------------------------------------------------------------
    // Eager loading
    // ------------------------------------------------------------------

    /// Eager load relationships: `with("posts")`, `with(["posts.comments",
    /// "profile"])`, `with("posts:id,user_id,title")`.
    pub fn with(mut self, relations: impl IntoRelations) -> Self {
        for relation in relations.into_relations() {
            relations::add_path(&mut self.eager, &relation, None);
        }
        self
    }

    /// Eager load a relationship, constraining its query.
    ///
    /// ```ignore
    /// let users = User::query()
    ///     .with_constrained("posts", |query| query.where_("published", true))
    ///     .get()
    ///     .await?;
    /// ```
    pub fn with_constrained(
        mut self,
        relation: &str,
        constraint: impl Fn(QueryBuilder) -> QueryBuilder + Send + Sync + 'static,
    ) -> Self {
        let constraint: Constraint = std::sync::Arc::new(constraint);
        relations::add_path(&mut self.eager, relation, Some(constraint));
        self
    }

    /// Stop eager loading the given relationships.
    pub fn without(mut self, relations: impl IntoRelations) -> Self {
        for relation in relations.into_relations() {
            relations::remove_path(&mut self.eager, &relation);
        }
        self
    }

    /// Replace the eager loaded relationships.
    pub fn with_only(mut self, relations: impl IntoRelations) -> Self {
        self.eager.clear();
        self.with(relations)
    }

    /// Stop eager loading the given relationships (an alias of `without`).
    pub fn without_eager_load(self, relations: impl IntoRelations) -> Self {
        self.without(relations)
    }

    /// Stop eager loading every relationship.
    pub fn without_eager_loads(mut self) -> Self {
        self.eager.clear();
        self
    }

    // ------------------------------------------------------------------
    // Relationship aggregates
    // ------------------------------------------------------------------

    /// Add `{relation}_count` columns (use `"posts as total"` to alias).
    pub fn with_count(mut self, relations: impl IntoRelations) -> Self {
        for relation in relations.into_relations() {
            self = self.with_aggregate(&relation, "*", "count", |query| query);
        }
        self
    }

    /// Add a `{relation}_count` column counting related models matching the
    /// constraint.
    pub fn with_count_constrained(
        self,
        relation: &str,
        constraint: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        self.with_aggregate(relation, "*", "count", constraint)
    }

    /// Add `{relation}_exists` columns.
    pub fn with_exists(mut self, relations: impl IntoRelations) -> Self {
        for relation in relations.into_relations() {
            self = self.with_aggregate(&relation, "*", "exists", |query| query);
        }
        self
    }

    /// Add a `{relation}_sum_{column}` column.
    pub fn with_sum(self, relation: &str, column: &str) -> Self {
        self.with_aggregate(relation, column, "sum", |query| query)
    }

    /// Add a `{relation}_avg_{column}` column.
    pub fn with_avg(self, relation: &str, column: &str) -> Self {
        self.with_aggregate(relation, column, "avg", |query| query)
    }

    /// Add a `{relation}_min_{column}` column.
    pub fn with_min(self, relation: &str, column: &str) -> Self {
        self.with_aggregate(relation, column, "min", |query| query)
    }

    /// Add a `{relation}_max_{column}` column.
    pub fn with_max(self, relation: &str, column: &str) -> Self {
        self.with_aggregate(relation, column, "max", |query| query)
    }

    /// Add a sub-select aggregating a relationship's column.
    pub fn with_aggregate(
        mut self,
        relation: &str,
        column: &str,
        function: &str,
        constraint: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        let (name, alias) = match relation.split_once(" as ") {
            Some((name, alias)) => (name.trim(), Some(alias.trim().to_string())),
            None => (relation.trim(), None),
        };
        let Some(dyn_relation) = M::relation(name) else {
            return self.fail(RelationNotFoundException::new(M::class_name(), name).to_string());
        };
        let (query, related) = dyn_relation.existence_query(&self.qualifier);
        let query = constraint(query);
        let grammar = self.query.get_grammar();
        let alias = alias.unwrap_or_else(|| match function {
            "count" | "exists" => format!("{}_{function}", Str::snake(name)),
            _ => format!(
                "{}_{function}_{}",
                Str::snake(name),
                Str::snake(&column.replace('.', "_"))
            ),
        });
        if self.query.columns.is_none() {
            let all = format!("{}.*", self.qualifier);
            self.query = self.query.select(all);
        }
        let expression = match function {
            "count" | "exists" => "count(*)".to_string(),
            _ => {
                let column = if column.contains('.') {
                    column.to_string()
                } else {
                    format!("{related}.{column}")
                };
                format!("{function}({})", grammar.wrap_str(&column))
            }
        };
        if function == "exists" {
            let query = query.select(Expression::new("*"));
            match query.try_to_sql() {
                Ok(sql) => {
                    let bindings = query.get_bindings();
                    let wrapped = grammar.wrap_str(&alias);
                    self.query = self
                        .query
                        .select_raw(&format!("exists({sql}) as {wrapped}"), bindings);
                }
                Err(error) => return self.fail(error.to_string()),
            }
        } else {
            let query = query.select(Expression::new(expression));
            self.query = self.query.select_sub(query, &alias);
        }
        self
    }

    // ------------------------------------------------------------------
    // Relationship existence
    // ------------------------------------------------------------------

    /// Only models that have at least one related model (`"posts.comments"`
    /// checks nested relationships).
    pub fn has(self, relation: &str) -> Self {
        self.add_has(
            relation,
            ">=",
            1,
            "and",
            None::<fn(QueryBuilder) -> QueryBuilder>,
        )
    }

    /// Only models with a number of related models matching the operator
    /// and count: `has_op("posts", ">=", 3)`.
    pub fn has_op(self, relation: &str, operator: &str, count: i64) -> Self {
        self.add_has(
            relation,
            operator,
            count,
            "and",
            None::<fn(QueryBuilder) -> QueryBuilder>,
        )
    }

    /// Add an "or has" clause.
    pub fn or_has(self, relation: &str) -> Self {
        self.add_has(
            relation,
            ">=",
            1,
            "or",
            None::<fn(QueryBuilder) -> QueryBuilder>,
        )
    }

    /// Only models without related models.
    pub fn doesnt_have(self, relation: &str) -> Self {
        self.add_has(
            relation,
            "<",
            1,
            "and",
            None::<fn(QueryBuilder) -> QueryBuilder>,
        )
    }

    /// Add an "or doesn't have" clause.
    pub fn or_doesnt_have(self, relation: &str) -> Self {
        self.add_has(
            relation,
            "<",
            1,
            "or",
            None::<fn(QueryBuilder) -> QueryBuilder>,
        )
    }

    /// Only models with related models matching the constraint.
    ///
    /// ```ignore
    /// let users = User::query()
    ///     .where_has("posts", |query| query.where_like("title", "%Laravel%"))
    ///     .get()
    ///     .await?;
    /// ```
    pub fn where_has(
        self,
        relation: &str,
        constraint: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        self.add_has(relation, ">=", 1, "and", Some(constraint))
    }

    /// Only models with a number of related models matching the constraint.
    pub fn where_has_count(
        self,
        relation: &str,
        constraint: impl FnOnce(QueryBuilder) -> QueryBuilder,
        operator: &str,
        count: i64,
    ) -> Self {
        self.add_has(relation, operator, count, "and", Some(constraint))
    }

    /// Add an "or where has" clause.
    pub fn or_where_has(
        self,
        relation: &str,
        constraint: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        self.add_has(relation, ">=", 1, "or", Some(constraint))
    }

    /// Only models without related models matching the constraint.
    pub fn where_doesnt_have(
        self,
        relation: &str,
        constraint: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        self.add_has(relation, "<", 1, "and", Some(constraint))
    }

    /// Add an "or where doesn't have" clause.
    pub fn or_where_doesnt_have(
        self,
        relation: &str,
        constraint: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        self.add_has(relation, "<", 1, "or", Some(constraint))
    }

    /// Only models with a related model whose column equals the value.
    pub fn where_relation(
        self,
        relation: &str,
        column: &str,
        value: impl Into<crate::Operand>,
    ) -> Self {
        let column = column.to_string();
        self.where_has(relation, move |query| query.where_(column, value))
    }

    /// Only models with a related model whose column matches the operator
    /// and value.
    pub fn where_relation_op(
        self,
        relation: &str,
        column: &str,
        operator: &str,
        value: impl Into<crate::Operand>,
    ) -> Self {
        let column = column.to_string();
        self.where_has(relation, move |query| {
            query.where_op(column, operator, value)
        })
    }

    /// Add an "or where relation" clause.
    pub fn or_where_relation(
        self,
        relation: &str,
        column: &str,
        value: impl Into<crate::Operand>,
    ) -> Self {
        let column = column.to_string();
        self.or_where_has(relation, move |query| query.where_(column, value))
    }

    /// Add an "or where relation" clause with an operator.
    pub fn or_where_relation_op(
        self,
        relation: &str,
        column: &str,
        operator: &str,
        value: impl Into<crate::Operand>,
    ) -> Self {
        let column = column.to_string();
        self.or_where_has(relation, move |query| {
            query.where_op(column, operator, value)
        })
    }

    /// Only models without a related model whose column equals the value.
    pub fn where_doesnt_have_relation(
        self,
        relation: &str,
        column: &str,
        value: impl Into<crate::Operand>,
    ) -> Self {
        let column = column.to_string();
        self.where_doesnt_have(relation, move |query| query.where_(column, value))
    }

    /// Only models without a related model matching the operator and value.
    pub fn where_doesnt_have_relation_op(
        self,
        relation: &str,
        column: &str,
        operator: &str,
        value: impl Into<crate::Operand>,
    ) -> Self {
        let column = column.to_string();
        self.where_doesnt_have(relation, move |query| {
            query.where_op(column, operator, value)
        })
    }

    /// Add an "or where doesn't have relation" clause.
    pub fn or_where_doesnt_have_relation(
        self,
        relation: &str,
        column: &str,
        value: impl Into<crate::Operand>,
    ) -> Self {
        let column = column.to_string();
        self.or_where_doesnt_have(relation, move |query| query.where_(column, value))
    }

    /// Only models whose relationship matches the constraint, eager loading
    /// the matching related models.
    ///
    /// ```ignore
    /// let users = User::query()
    ///     .with_where_has("posts", |query| query.where_("featured", true))
    ///     .get()
    ///     .await?;
    /// ```
    pub fn with_where_has(
        self,
        relation: &str,
        constraint: impl Fn(QueryBuilder) -> QueryBuilder + Send + Sync + 'static,
    ) -> Self {
        let constraint: Constraint = Arc::new(constraint);
        let name = relation.split(':').next().unwrap_or(relation).to_string();
        let existence = constraint.clone();
        self.where_has(&name, move |query| existence(query))
            .with_constrained(relation, move |query| constraint(query))
    }

    /// Only models with a related model whose column equals the value,
    /// eager loading the matching related models.
    pub fn with_where_relation(
        self,
        relation: &str,
        column: &str,
        value: impl Into<crate::Operand> + Clone + Send + Sync + 'static,
    ) -> Self {
        let column = column.to_string();
        let value: crate::Operand = value.into();
        self.with_where_has(relation, move |query| {
            query.where_(column.clone(), value.clone())
        })
    }

    /// The relationship used by `where_belongs_to` / `where_morphed_to`,
    /// checked to be an inverse (belongs-to) relationship.
    fn belongs_to_relation(&self, name: &str) -> Result<Box<dyn DynRelation>> {
        match M::relation(name) {
            Some(relation) if relation.kind() == relations::RelationKind::BelongsTo => Ok(relation),
            _ => Err(RelationNotFoundException::new(M::class_name(), name).into()),
        }
    }

    fn add_where_belongs_to<R: Model>(
        mut self,
        related: impl super::RelatedModels<R>,
        relation: Option<&str>,
        boolean: &str,
    ) -> Self {
        let name = relation
            .map(String::from)
            .unwrap_or_else(|| Str::snake(R::class_name()));
        let relation = match self.belongs_to_relation(&name) {
            Ok(relation) => relation,
            Err(error) => return self.fail(error.to_string()),
        };
        let (models, many) = related.into_related_models();
        let template = relation.attributes_for_parent(&R::template().to_attributes());
        let Some(foreign_key) = template.keys().next().cloned() else {
            return self;
        };
        let column = self.qualify_column(&foreign_key);
        let keys: Vec<Value> = models
            .iter()
            .map(|model| {
                relation
                    .attributes_for_parent(&model.to_attributes())
                    .get(&foreign_key)
                    .cloned()
                    .unwrap_or(Value::Null)
            })
            .collect();
        self.query = if many {
            self.query.add_where_in(column, keys, boolean, false)
        } else {
            let key = keys.into_iter().next().unwrap_or(Value::Null);
            self.query.add_where(column, "=", key, boolean)
        };
        self
    }

    /// Only models belonging to the given model(s), through the
    /// belongs-to relationship named after the related model (`user` for
    /// `User`).
    ///
    /// ```ignore
    /// let posts = Post::query().where_belongs_to(&user).get().await?;
    /// let posts = Post::query().where_belongs_to(users).get().await?;
    /// ```
    pub fn where_belongs_to<R: Model>(self, related: impl super::RelatedModels<R>) -> Self {
        self.add_where_belongs_to(related, None, "and")
    }

    /// Only models belonging to the given model(s) through the named
    /// relationship.
    pub fn where_belongs_to_relation<R: Model>(
        self,
        related: impl super::RelatedModels<R>,
        relation: &str,
    ) -> Self {
        self.add_where_belongs_to(related, Some(relation), "and")
    }

    /// Add an "or where belongs to" clause.
    pub fn or_where_belongs_to<R: Model>(self, related: impl super::RelatedModels<R>) -> Self {
        self.add_where_belongs_to(related, None, "or")
    }

    /// Add an "or where belongs to" clause through the named relationship.
    pub fn or_where_belongs_to_relation<R: Model>(
        self,
        related: impl super::RelatedModels<R>,
        relation: &str,
    ) -> Self {
        self.add_where_belongs_to(related, Some(relation), "or")
    }

    fn add_where_attached_to<R: Model>(
        self,
        related: impl super::RelatedModels<R>,
        relation: Option<&str>,
        boolean: &str,
    ) -> Self {
        let name = relation
            .map(String::from)
            .unwrap_or_else(|| Str::plural(&Str::snake(R::class_name())));
        match M::relation(&name) {
            Some(relation) if relation.kind() == relations::RelationKind::BelongsToMany => {}
            _ => {
                return self
                    .fail(RelationNotFoundException::new(M::class_name(), &name).to_string());
            }
        }
        let (models, _) = related.into_related_models();
        let keys: Vec<Value> = models.iter().map(|model| model.get_key()).collect();
        let column = format!("{}.{}", R::table(), R::primary_key());
        self.add_has(
            &name,
            ">=",
            1,
            boolean,
            Some(move |query: QueryBuilder| query.where_in(column, keys)),
        )
    }

    /// Only models attached to the given model(s) through the many-to-many
    /// relationship named after the related model (`roles` for `Role`).
    ///
    /// ```ignore
    /// let users = User::query().where_attached_to(&admin_role).get().await?;
    /// ```
    pub fn where_attached_to<R: Model>(self, related: impl super::RelatedModels<R>) -> Self {
        self.add_where_attached_to(related, None, "and")
    }

    /// Only models attached to the given model(s) through the named
    /// relationship.
    pub fn where_attached_to_relation<R: Model>(
        self,
        related: impl super::RelatedModels<R>,
        relation: &str,
    ) -> Self {
        self.add_where_attached_to(related, Some(relation), "and")
    }

    /// Add an "or where attached to" clause.
    pub fn or_where_attached_to<R: Model>(self, related: impl super::RelatedModels<R>) -> Self {
        self.add_where_attached_to(related, None, "or")
    }

    /// Add an "or where attached to" clause through the named relationship.
    pub fn or_where_attached_to_relation<R: Model>(
        self,
        related: impl super::RelatedModels<R>,
        relation: &str,
    ) -> Self {
        self.add_where_attached_to(related, Some(relation), "or")
    }

    /// Constrain the query by several polymorphic relationships at once:
    /// a model matches when *any* of them matches. Rust `morph_to`
    /// relationships are typed (`imageable_post`, `imageable_video`), so
    /// each name covers one morph type.
    fn add_has_morph(
        mut self,
        relations: impl IntoRelations,
        operator: &str,
        count: i64,
        boolean: &str,
        constraint: Option<&(dyn Fn(QueryBuilder) -> QueryBuilder + Send + Sync)>,
    ) -> Self {
        let mut nested = Builder::<M>::from_query(self.query.for_nested_where());
        nested.qualifier = self.qualifier.clone();
        for (index, relation) in relations.into_relations().iter().enumerate() {
            let inner = if index == 0 { "and" } else { "or" };
            nested = match constraint {
                Some(constraint) => nested.add_has(
                    relation,
                    operator,
                    count,
                    inner,
                    Some(|query: QueryBuilder| constraint(query)),
                ),
                None => nested.add_has(
                    relation,
                    operator,
                    count,
                    inner,
                    None::<fn(QueryBuilder) -> QueryBuilder>,
                ),
            };
        }
        self.query = self.query.add_nested_where_query(nested.query, boolean);
        self
    }

    /// Only models whose polymorphic relationship (any of the given typed
    /// `morph_to` relations) exists.
    ///
    /// ```ignore
    /// let comments = Comment::query()
    ///     .has_morph(["commentable_post", "commentable_video"])
    ///     .get()
    ///     .await?;
    /// ```
    pub fn has_morph(self, relations: impl IntoRelations) -> Self {
        self.add_has_morph(relations, ">=", 1, "and", None)
    }

    /// Only models with a number of morphed related models matching the
    /// operator and count.
    pub fn has_morph_op(self, relations: impl IntoRelations, operator: &str, count: i64) -> Self {
        self.add_has_morph(relations, operator, count, "and", None)
    }

    /// Add an "or has morph" clause.
    pub fn or_has_morph(self, relations: impl IntoRelations) -> Self {
        self.add_has_morph(relations, ">=", 1, "or", None)
    }

    /// Only models whose polymorphic relationship doesn't exist.
    pub fn doesnt_have_morph(self, relations: impl IntoRelations) -> Self {
        self.add_has_morph(relations, "<", 1, "and", None)
    }

    /// Add an "or doesn't have morph" clause.
    pub fn or_doesnt_have_morph(self, relations: impl IntoRelations) -> Self {
        self.add_has_morph(relations, "<", 1, "or", None)
    }

    /// Only models whose polymorphic relationship matches the constraint.
    ///
    /// ```ignore
    /// let comments = Comment::query()
    ///     .where_has_morph(["commentable_post", "commentable_video"], |query| {
    ///         query.where_like("title", "code%")
    ///     })
    ///     .get()
    ///     .await?;
    /// ```
    pub fn where_has_morph(
        self,
        relations: impl IntoRelations,
        constraint: impl Fn(QueryBuilder) -> QueryBuilder + Send + Sync,
    ) -> Self {
        self.add_has_morph(relations, ">=", 1, "and", Some(&constraint))
    }

    /// Add an "or where has morph" clause.
    pub fn or_where_has_morph(
        self,
        relations: impl IntoRelations,
        constraint: impl Fn(QueryBuilder) -> QueryBuilder + Send + Sync,
    ) -> Self {
        self.add_has_morph(relations, ">=", 1, "or", Some(&constraint))
    }

    /// Only models whose polymorphic relationship doesn't match the
    /// constraint.
    pub fn where_doesnt_have_morph(
        self,
        relations: impl IntoRelations,
        constraint: impl Fn(QueryBuilder) -> QueryBuilder + Send + Sync,
    ) -> Self {
        self.add_has_morph(relations, "<", 1, "and", Some(&constraint))
    }

    /// Add an "or where doesn't have morph" clause.
    pub fn or_where_doesnt_have_morph(
        self,
        relations: impl IntoRelations,
        constraint: impl Fn(QueryBuilder) -> QueryBuilder + Send + Sync,
    ) -> Self {
        self.add_has_morph(relations, "<", 1, "or", Some(&constraint))
    }

    /// Only models whose polymorphic relationship has the column value.
    pub fn where_morph_relation(
        self,
        relations: impl IntoRelations,
        column: &str,
        value: impl Into<crate::Operand>,
    ) -> Self {
        let value: crate::Operand = value.into();
        self.where_has_morph(relations, move |query| query.where_(column, value.clone()))
    }

    /// Add an "or where morph relation" clause.
    pub fn or_where_morph_relation(
        self,
        relations: impl IntoRelations,
        column: &str,
        value: impl Into<crate::Operand>,
    ) -> Self {
        let value: crate::Operand = value.into();
        self.or_where_has_morph(relations, move |query| query.where_(column, value.clone()))
    }

    /// Only models whose polymorphic relationship doesn't have the column
    /// value.
    pub fn where_morph_doesnt_have_relation(
        self,
        relations: impl IntoRelations,
        column: &str,
        value: impl Into<crate::Operand>,
    ) -> Self {
        let value: crate::Operand = value.into();
        self.where_doesnt_have_morph(relations, move |query| query.where_(column, value.clone()))
    }

    /// Add an "or where morph doesn't have relation" clause.
    pub fn or_where_morph_doesnt_have_relation(
        self,
        relations: impl IntoRelations,
        column: &str,
        value: impl Into<crate::Operand>,
    ) -> Self {
        let value: crate::Operand = value.into();
        self.or_where_doesnt_have_morph(relations, move |query| query.where_(column, value.clone()))
    }

    /// The `(type column, foreign key)` of a polymorphic relationship.
    fn morph_columns<R: Model>(
        &self,
        name: &str,
    ) -> Result<(Box<dyn DynRelation>, String, String)> {
        let relation = self.belongs_to_relation(name)?;
        let template = relation.attributes_for_parent(&R::template().to_attributes());
        let mut keys = template.keys().cloned();
        match (keys.next(), keys.next()) {
            (Some(foreign_key), Some(morph_type)) => Ok((relation, morph_type, foreign_key)),
            _ => Err(RelationNotFoundException::new(M::class_name(), name).into()),
        }
    }

    fn add_where_morphed_to<R: Model>(
        mut self,
        relation: &str,
        related: impl super::RelatedModels<R>,
        boolean: &str,
        not: bool,
    ) -> Self {
        let (relation, morph_type, foreign_key) = match self.morph_columns::<R>(relation) {
            Ok(columns) => columns,
            Err(error) => return self.fail(error.to_string()),
        };
        let (models, _) = related.into_related_models();
        let class = relation
            .attributes_for_parent(&R::template().to_attributes())
            .get(&morph_type)
            .cloned()
            .unwrap_or(Value::Null);
        let keys: Vec<Value> = models.iter().map(|model| model.get_key()).collect();
        let (morph_type, foreign_key) = (
            self.qualify_column(&morph_type),
            self.qualify_column(&foreign_key),
        );
        let group = move |query: QueryBuilder| {
            query.or_where_group(|query| {
                let query = if not {
                    query.where_null_safe_equals(morph_type, class)
                } else {
                    query.where_(morph_type, class)
                };
                query.where_in(foreign_key, keys)
            })
        };
        self.query = if not {
            self.query
                .add_nested_where(group, &format!("{boolean} not"))
        } else {
            self.query.add_nested_where(group, boolean)
        };
        self
    }

    /// Only models whose polymorphic relationship points at the given
    /// model(s).
    ///
    /// ```ignore
    /// let comments = Comment::query().where_morphed_to("commentable_post", &post).get().await?;
    /// ```
    pub fn where_morphed_to<R: Model>(
        self,
        relation: &str,
        related: impl super::RelatedModels<R>,
    ) -> Self {
        self.add_where_morphed_to(relation, related, "and", false)
    }

    /// Add an "or where morphed to" clause.
    pub fn or_where_morphed_to<R: Model>(
        self,
        relation: &str,
        related: impl super::RelatedModels<R>,
    ) -> Self {
        self.add_where_morphed_to(relation, related, "or", false)
    }

    /// Only models whose polymorphic relationship doesn't point at the given
    /// model(s).
    pub fn where_not_morphed_to<R: Model>(
        self,
        relation: &str,
        related: impl super::RelatedModels<R>,
    ) -> Self {
        self.add_where_morphed_to(relation, related, "and", true)
    }

    /// Add an "or where not morphed to" clause.
    pub fn or_where_not_morphed_to<R: Model>(
        self,
        relation: &str,
        related: impl super::RelatedModels<R>,
    ) -> Self {
        self.add_where_morphed_to(relation, related, "or", true)
    }

    fn add_has(
        mut self,
        relation: &str,
        operator: &str,
        count: i64,
        boolean: &str,
        constraint: Option<impl FnOnce(QueryBuilder) -> QueryBuilder>,
    ) -> Self {
        let (first, rest) = match relation.split_once('.') {
            Some((first, rest)) => (first, Some(rest)),
            None => (relation, None),
        };
        let Some(dyn_relation) = M::relation(first) else {
            return self.fail(RelationNotFoundException::new(M::class_name(), first).to_string());
        };
        match existence(
            dyn_relation.as_ref(),
            &self.qualifier,
            rest,
            operator,
            count,
            constraint,
        ) {
            Ok((query, operator, count)) => {
                self.query = add_existence_clause(self.query, query, &operator, count, boolean);
                self
            }
            Err(error) => self.fail(error.to_string()),
        }
    }

    fn fail(mut self, message: String) -> Self {
        self.query.error.get_or_insert(message);
        self
    }

    // ------------------------------------------------------------------
    // Retrieving models
    // ------------------------------------------------------------------

    /// Execute the query and get the models.
    pub async fn get(&self) -> Result<Collection<M>> {
        let (rows, eager) = self.get_rows().await?;
        let mut models = hydrate::<M>(rows)?;
        finish(&mut models, &eager).await?;
        Ok(self.apply_after_query_callbacks(models.into()))
    }

    /// Execute the query and get the raw rows (with scopes applied), plus
    /// the relationships to eager load.
    pub(crate) async fn get_rows(
        &self,
    ) -> Result<(Vec<Map<String, Value>>, Vec<(String, EagerSpec)>)> {
        let builder = self.applied();
        let rows = builder.query.get().await?;
        let rows = rows
            .into_iter()
            .filter_map(|row| match row {
                Value::Object(map) => Some(map),
                _ => None,
            })
            .collect();
        Ok((rows, builder.eager))
    }

    /// Get the first model.
    pub async fn first(&self) -> Result<Option<M>> {
        Ok(self
            .clone()
            .take(1)
            .get()
            .await?
            .into_vec()
            .into_iter()
            .next())
    }

    /// Get the first model or fail with a [`ModelNotFoundException`].
    pub async fn first_or_fail(&self) -> Result<M> {
        self.first()
            .await?
            .ok_or_else(|| ModelNotFoundException::new(M::class_name(), Vec::new()).into())
    }

    /// Get the only model matching the query, failing when there are none
    /// ([`ModelNotFoundException`]) or several ([`MultipleRecordsFoundException`]).
    pub async fn sole(&self) -> Result<M> {
        let mut models = self.clone().take(2).get().await?.into_vec();
        match models.len() {
            0 => Err(ModelNotFoundException::new(M::class_name(), Vec::new()).into()),
            1 => Ok(models.remove(0)),
            count => Err(MultipleRecordsFoundException::new(count).into()),
        }
    }

    /// Find a model by its primary key.
    pub async fn find(&self, id: impl Into<Value>) -> Result<Option<M>> {
        self.clone().where_key(id.into()).first().await
    }

    /// Find a model by its primary key, or call the callback when there is
    /// none.
    ///
    /// ```ignore
    /// let user = User::query().find_or(1, || async { abort(404) }).await?;
    /// ```
    pub async fn find_or<F, Fut, E>(&self, id: impl Into<Value>, callback: F) -> Result<M>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = std::result::Result<M, E>>,
        E: Into<illuminate_support::Error>,
    {
        match self.find(id).await? {
            Some(model) => Ok(model),
            None => callback().await.map_err(Into::into),
        }
    }

    /// Find the only model with the primary key, failing when there is
    /// none or several.
    pub async fn find_sole(&self, id: impl Into<Value>) -> Result<M> {
        self.clone().where_key(id.into()).sole().await
    }

    /// Get the first model, or call the callback when there is none.
    pub async fn first_or<F, Fut, E>(&self, callback: F) -> Result<M>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = std::result::Result<M, E>>,
        E: Into<illuminate_support::Error>,
    {
        match self.first().await? {
            Some(model) => Ok(model),
            None => callback().await.map_err(Into::into),
        }
    }

    /// Get a single column's value from the first result, or fail with a
    /// [`ModelNotFoundException`].
    pub async fn value_or_fail(&self, column: impl Into<Ident>) -> Result<Value> {
        let column = column.into();
        let builder = self.applied();
        let mut query = builder.query.limit(1);
        if query.columns.is_none() {
            query = query.select(vec![column.clone()]);
        }
        match query.first().await? {
            Some(Value::Object(row)) => Ok(row
                .into_iter()
                .next()
                .map(|(_, v)| v)
                .unwrap_or(Value::Null)),
            _ => Err(ModelNotFoundException::new(M::class_name(), Vec::new()).into()),
        }
    }

    /// Find a model by its primary key or fail with a [`ModelNotFoundException`].
    pub async fn find_or_fail(&self, id: impl Into<Value>) -> Result<M> {
        let id = id.into();
        self.find(id.clone())
            .await?
            .ok_or_else(|| ModelNotFoundException::new(M::class_name(), vec![id]).into())
    }

    /// Find models by their primary keys.
    pub async fn find_many(&self, ids: impl IntoIds) -> Result<Collection<M>> {
        let ids = ids.into_ids();
        if ids.is_empty() {
            return Ok(Collection::new());
        }
        self.clone().where_key(Value::Array(ids)).get().await
    }

    /// Find a model by its primary key or return a new instance.
    pub async fn find_or_new(&self, id: impl Into<Value>) -> Result<M> {
        match self.find(id).await? {
            Some(model) => Ok(model),
            None => self.new_model_instance(Attributes::new()),
        }
    }

    /// Get the first model matching the attributes, or instantiate a new one
    /// (not saved) with the attributes and values.
    pub async fn first_or_new(
        &self,
        attributes: impl Into<Attributes>,
        values: impl Into<Attributes>,
    ) -> Result<M> {
        let attributes = attributes.into();
        if let Some(model) = self.clone().where_map(attributes.0.clone()).first().await? {
            return Ok(model);
        }
        self.new_model_instance(attributes.merge(values))
    }

    /// Get the first model matching the attributes, or create one with the
    /// attributes and values.
    pub async fn first_or_create(
        &self,
        attributes: impl Into<Attributes>,
        values: impl Into<Attributes>,
    ) -> Result<M> {
        let attributes = attributes.into();
        if let Some(model) = self.clone().where_map(attributes.0.clone()).first().await? {
            return Ok(model);
        }
        let mut model = self.new_model_instance(attributes.merge(values))?;
        model.save().await?;
        Ok(model)
    }

    /// Create the model, or — when a unique constraint stops the insert —
    /// get the existing one matching the attributes. Faster than
    /// `first_or_create` when the model usually doesn't exist yet, and safe
    /// under concurrent requests.
    pub async fn create_or_first(
        &self,
        attributes: impl Into<Attributes>,
        values: impl Into<Attributes>,
    ) -> Result<M> {
        let attributes = attributes.into();
        let connection = self.query.get_connection().clone();
        let candidate = self.new_model_instance(attributes.clone().merge(values));
        let result = connection
            .transaction(|| async move {
                let mut model = candidate?;
                model.save().await?;
                Ok(model)
            })
            .await;
        match result {
            Ok(model) => Ok(model),
            Err(error)
                if error
                    .downcast_ref::<crate::QueryException>()
                    .is_some_and(crate::QueryException::is_unique_constraint_violation) =>
            {
                match self
                    .clone()
                    .use_write_pdo()
                    .where_map(attributes.0)
                    .first()
                    .await?
                {
                    Some(model) => Ok(model),
                    None => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Increment a column of the first model matching the attributes, or
    /// create it with the column set to `default`.
    ///
    /// ```ignore
    /// let counter = PageView::query().increment_or_create(json!({"page": "/"}), "count", 1, 1).await?;
    /// ```
    pub async fn increment_or_create(
        &self,
        attributes: impl Into<Attributes>,
        column: &str,
        default: impl Into<Value>,
        step: impl Into<Value>,
    ) -> Result<M> {
        let attributes = attributes.into();
        if let Some(mut model) = self.clone().where_map(attributes.0.clone()).first().await? {
            model.increment(column, step.into()).await?;
            return Ok(model);
        }
        let mut defaults = Attributes::new();
        defaults.insert(column, default.into());
        let mut model = self.new_model_instance_forced(attributes.merge(defaults))?;
        model.save().await?;
        Ok(model)
    }

    /// Create and save a new model without firing any events.
    pub async fn create_quietly(&self, attributes: impl Into<Attributes>) -> Result<M> {
        super::state::without_events(self.create(attributes)).await
    }

    /// Create and save a new model, ignoring mass assignment protection,
    /// without firing any events.
    pub async fn force_create_quietly(&self, attributes: impl Into<Attributes>) -> Result<M> {
        super::state::without_events(self.force_create(attributes)).await
    }

    /// Update the first model matching the attributes with the values, or
    /// create it.
    pub async fn update_or_create(
        &self,
        attributes: impl Into<Attributes>,
        values: impl Into<Attributes>,
    ) -> Result<M> {
        let attributes = attributes.into();
        let values = values.into();
        let mut model = self.first_or_new(attributes, Attributes::new()).await?;
        model.fill(values)?;
        model.save().await?;
        Ok(model)
    }

    /// Create and save a new model.
    pub async fn create(&self, attributes: impl Into<Attributes>) -> Result<M> {
        let mut model = self.new_model_instance(attributes)?;
        model.save().await?;
        Ok(model)
    }

    /// Create and save a new model, ignoring mass assignment protection.
    pub async fn force_create(&self, attributes: impl Into<Attributes>) -> Result<M> {
        let mut model = self.new_model_instance_forced(attributes)?;
        model.save().await?;
        Ok(model)
    }

    /// A new, unsaved model carrying the builder's pending attributes.
    pub fn make(&self, attributes: impl Into<Attributes>) -> Result<M> {
        self.new_model_instance(attributes)
    }

    /// Get a single column's value from the first result.
    pub async fn value(&self, column: impl Into<Ident>) -> Result<Option<Value>> {
        self.applied().query.value(column).await
    }

    /// Get a column's values.
    pub async fn pluck(&self, column: impl Into<Ident>) -> Result<Collection<Value>> {
        self.applied().query.pluck(column).await
    }

    /// Get a column's values keyed by another column.
    pub async fn pluck_with_key(
        &self,
        column: impl Into<Ident>,
        key: impl Into<Ident>,
    ) -> Result<indexmap::IndexMap<String, Value>> {
        self.applied().query.pluck_with_key(column, key).await
    }

    /// Determine whether any models match the query.
    pub async fn exists(&self) -> Result<bool> {
        self.applied().query.exists().await
    }

    /// Determine whether no models match the query.
    pub async fn doesnt_exist(&self) -> Result<bool> {
        Ok(!self.exists().await?)
    }

    /// Count the matching models.
    pub async fn count(&self) -> Result<i64> {
        self.applied().query.count().await
    }

    /// The minimum value of a column.
    pub async fn min(&self, column: impl Into<Ident>) -> Result<Value> {
        self.applied().query.min(column).await
    }

    /// The maximum value of a column.
    pub async fn max(&self, column: impl Into<Ident>) -> Result<Value> {
        self.applied().query.max(column).await
    }

    /// The sum of a column.
    pub async fn sum(&self, column: impl Into<Ident>) -> Result<Value> {
        self.applied().query.sum(column).await
    }

    /// The average of a column.
    pub async fn avg(&self, column: impl Into<Ident>) -> Result<Value> {
        self.applied().query.avg(column).await
    }

    /// Paginate the models: the page comes from the current request's
    /// `page` parameter, and `None` uses the model's `per_page`.
    pub async fn paginate(
        &self,
        per_page: impl Into<Option<u64>>,
    ) -> Result<LengthAwarePaginator<M>> {
        self.paginate_with(per_page, "page", None).await
    }

    /// Paginate the models with a custom page name or an explicit page.
    pub async fn paginate_with(
        &self,
        per_page: impl Into<Option<u64>>,
        page_name: &str,
        page: Option<u64>,
    ) -> Result<LengthAwarePaginator<M>> {
        let per_page = per_page.into().unwrap_or_else(M::per_page).max(1);
        let page = page.unwrap_or_else(|| current_page(page_name)).max(1);
        let builder = self.applied();
        let total = builder.query.get_count_for_pagination().await?.max(0) as u64;
        let items = if total > 0 {
            builder.for_page(page as i64, per_page as i64).get().await?
        } else {
            Collection::new()
        };
        let options = PaginatorOptions::path(current_path()).page_name(page_name);
        Ok(LengthAwarePaginator::new(
            items, total, per_page, page, options,
        ))
    }

    /// Paginate the models without counting the total ("previous" / "next"
    /// links only).
    pub async fn simple_paginate(&self, per_page: impl Into<Option<u64>>) -> Result<Paginator<M>> {
        self.simple_paginate_with(per_page, "page", None).await
    }

    /// Simple pagination with a custom page name or an explicit page.
    pub async fn simple_paginate_with(
        &self,
        per_page: impl Into<Option<u64>>,
        page_name: &str,
        page: Option<u64>,
    ) -> Result<Paginator<M>> {
        let per_page = per_page.into().unwrap_or_else(M::per_page).max(1);
        let page = page.unwrap_or_else(|| current_page(page_name)).max(1);
        let items = self
            .clone()
            .skip(((page - 1) * per_page) as i64)
            .take(per_page as i64 + 1)
            .get()
            .await?;
        let options = PaginatorOptions::path(current_path()).page_name(page_name);
        Ok(Paginator::new(items, per_page, page, options))
    }

    /// Stream the models one at a time from a single query (Laravel's
    /// `cursor`): only one model is hydrated at a time. Relationships
    /// can't be eager loaded on a cursor.
    ///
    /// ```ignore
    /// use futures::StreamExt;
    ///
    /// let mut users = User::query().cursor();
    /// while let Some(user) = users.next().await {
    ///     let user = user?;
    /// }
    /// ```
    pub fn cursor(&self) -> BoxStream<'static, Result<M>> {
        let builder = self.applied();
        let callbacks = builder.after_query_callbacks.clone();
        builder
            .query
            .cursor()
            .then(move |row| {
                let callbacks = callbacks.clone();
                async move {
                    let row = match row? {
                        Value::Object(row) => row,
                        _ => return Ok(None),
                    };
                    let mut models = hydrate::<M>(vec![row])?;
                    finish(&mut models, &[]).await?;
                    let mut models = Collection::from(models);
                    for callback in &callbacks {
                        models = callback(models);
                    }
                    Ok(models.into_iter().next())
                }
            })
            .filter_map(|model: Result<Option<M>>| futures::future::ready(model.transpose()))
            .boxed()
    }

    /// Paginate the models with a cursor (`None` uses the model's
    /// `per_page`); the cursor comes from the request's `cursor` parameter
    /// unless one is given.
    pub async fn cursor_paginate(
        &self,
        per_page: impl Into<Option<u64>>,
        cursor: Option<Cursor>,
    ) -> Result<CursorPaginator<M>> {
        self.cursor_paginate_with(per_page, "cursor", cursor).await
    }

    /// Cursor pagination with a custom cursor parameter name.
    pub async fn cursor_paginate_with(
        &self,
        per_page: impl Into<Option<u64>>,
        cursor_name: &str,
        cursor: Option<Cursor>,
    ) -> Result<CursorPaginator<M>> {
        let per_page = per_page.into().unwrap_or_else(M::per_page).max(1);
        let cursor = cursor.or_else(|| resolve_current_cursor(cursor_name));
        let mut builder = self.applied();
        let (query, parameters) = builder
            .query
            .clone()
            .prepare_cursor_pagination(per_page, cursor.as_ref())?;
        builder.query = query;
        let items = builder.get().await?;
        Ok(CursorPaginator::new(
            items,
            per_page,
            cursor,
            CursorPaginatorOptions {
                path: current_path(),
                cursor_name: cursor_name.to_string(),
                parameters,
                ..Default::default()
            },
        ))
    }

    /// Stream the models, loading them in chunks (Laravel's `lazy`).
    /// Results are ordered by primary key unless the query is ordered.
    pub fn lazy(&self, chunk_size: i64) -> BoxStream<'static, Result<M>> {
        let mut builder = self.applied();
        if builder.query.orders.is_empty() {
            let key = builder.qualify_column(M::primary_key());
            builder = builder.order_by(key, "asc");
        }
        let chunk_size = chunk_size.max(1);
        futures::stream::try_unfold(Some((builder, 1_i64)), move |state| async move {
            let Some((builder, page)) = state else {
                return Ok::<_, illuminate_support::Error>(None);
            };
            let models = builder
                .clone()
                .for_page(page, chunk_size)
                .get()
                .await?
                .into_vec();
            if models.is_empty() {
                return Ok(None);
            }
            let next = (models.len() as i64 == chunk_size).then_some((builder, page + 1));
            Ok(Some((
                futures::stream::iter(models.into_iter().map(Ok::<M, illuminate_support::Error>)),
                next,
            )))
        })
        .try_flatten()
        .boxed()
    }

    /// Stream the models in chunks paginated by primary key (safe while
    /// updating them).
    pub fn lazy_by_id(&self, chunk_size: i64) -> BoxStream<'static, Result<M>> {
        self.lazy_by_key(chunk_size, false)
    }

    /// Stream the models in chunks paginated by primary key, descending.
    pub fn lazy_by_id_desc(&self, chunk_size: i64) -> BoxStream<'static, Result<M>> {
        self.lazy_by_key(chunk_size, true)
    }

    fn lazy_by_key(&self, chunk_size: i64, descending: bool) -> BoxStream<'static, Result<M>> {
        let builder = self.applied();
        let key = builder.qualify_column(M::primary_key());
        let chunk_size = chunk_size.max(1);
        let initial: Option<(Builder<M>, Option<Value>)> = Some((builder, None));
        futures::stream::try_unfold(initial, move |state| {
            let key = key.clone();
            async move {
                let Some((builder, last)) = state else {
                    return Ok::<_, illuminate_support::Error>(None);
                };
                let mut query = builder
                    .clone()
                    .reorder_by(key.clone(), if descending { "desc" } else { "asc" })
                    .limit(chunk_size);
                if let Some(last) = &last {
                    query = query.where_op(key, if descending { "<" } else { ">" }, last.clone());
                }
                let models = query.get().await?.into_vec();
                if models.is_empty() {
                    return Ok(None);
                }
                let next = (models.len() as i64 == chunk_size)
                    .then(|| models.last().map(|model| (builder, Some(model.get_key()))))
                    .flatten();
                Ok(Some((
                    futures::stream::iter(
                        models.into_iter().map(Ok::<M, illuminate_support::Error>),
                    ),
                    next,
                )))
            }
        })
        .try_flatten()
        .boxed()
    }

    /// Run the callback for each model, loading them in chunks paginated by
    /// primary key; return `Ok(false)` to stop.
    pub async fn each_by_id<F, Fut>(&self, count: i64, mut callback: F) -> Result<bool>
    where
        F: FnMut(M) -> Fut,
        Fut: Future<Output = Result<bool>>,
    {
        let mut models = self.lazy_by_id(count);
        while let Some(model) = models.next().await {
            if !callback(model?).await? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Map every model, loading them in chunks, into a collection.
    pub async fn chunk_map<T, F, Fut>(&self, mut callback: F, count: i64) -> Result<Collection<T>>
    where
        F: FnMut(M) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut models = self.lazy(count);
        let mut mapped = Vec::new();
        while let Some(model) = models.next().await {
            mapped.push(callback(model?).await?);
        }
        Ok(mapped.into())
    }

    /// Process the models in chunks: the callback receives each chunk and
    /// its page number, and returns `Ok(false)` to stop. Results are ordered
    /// by primary key unless the query is already ordered.
    pub async fn chunk<F, Fut>(&self, count: i64, mut callback: F) -> Result<bool>
    where
        F: FnMut(Collection<M>, i64) -> Fut,
        Fut: Future<Output = Result<bool>>,
    {
        let mut builder = self.applied();
        if builder.query.orders.is_empty() {
            let key = builder.qualify_column(M::primary_key());
            builder = builder.order_by(key, "asc");
        }
        let count = count.max(1);
        let mut page = 1;
        loop {
            let results = builder.clone().for_page(page, count).get().await?;
            let found = results.len() as i64;
            if found == 0 {
                break;
            }
            if !callback(results, page).await? {
                return Ok(false);
            }
            if found < count {
                break;
            }
            page += 1;
        }
        Ok(true)
    }

    /// Process the models in chunks paginated by primary key (safe when the
    /// callback updates the models).
    pub async fn chunk_by_id<F, Fut>(&self, count: i64, mut callback: F) -> Result<bool>
    where
        F: FnMut(Collection<M>, i64) -> Fut,
        Fut: Future<Output = Result<bool>>,
    {
        let builder = self.applied();
        let key = builder.qualify_column(M::primary_key());
        let count = count.max(1);
        let mut last: Option<Value> = None;
        let mut page = 1;
        loop {
            let mut query = builder.clone().reorder_by(key.clone(), "asc").limit(count);
            if let Some(last) = &last {
                query = query.where_op(key.clone(), ">", last.clone());
            }
            let results = query.get().await?;
            let found = results.len() as i64;
            if found == 0 {
                break;
            }
            last = results.last().map(|model| model.get_key());
            if !callback(results, page).await? {
                return Ok(false);
            }
            if found < count {
                break;
            }
            page += 1;
        }
        Ok(true)
    }

    /// Run the callback for each model, loading them in chunks.
    pub async fn each<F, Fut>(&self, count: i64, mut callback: F) -> Result<bool>
    where
        F: FnMut(M) -> Fut,
        Fut: Future<Output = Result<bool>>,
    {
        let mut builder = self.applied();
        if builder.query.orders.is_empty() {
            let key = builder.qualify_column(M::primary_key());
            builder = builder.order_by(key, "asc");
        }
        let count = count.max(1);
        let mut page = 1;
        loop {
            let results = builder.clone().for_page(page, count).get().await?;
            let found = results.len() as i64;
            for model in results {
                if !callback(model).await? {
                    return Ok(false);
                }
            }
            if found < count {
                break;
            }
            page += 1;
        }
        Ok(true)
    }

    // ------------------------------------------------------------------
    // Mass updates and deletes
    // ------------------------------------------------------------------

    /// Update the matching rows (touching `updated_at`). No model events
    /// are fired.
    pub async fn update(&self, values: impl Into<Attributes>) -> Result<u64> {
        let mut values = values.into().0;
        if M::uses_timestamps()
            && M::uses_updated_at()
            && !values.contains_key(M::updated_at_column())
        {
            values.insert(M::updated_at_column().to_string(), now());
        }
        self.applied().query.update(values).await
    }

    /// Increment a column on the matching rows (touching `updated_at`).
    pub async fn increment(&self, column: &str, amount: impl Into<Value>) -> Result<u64> {
        self.applied()
            .query
            .increment_with(column, amount, self.touch_record())
            .await
    }

    /// Decrement a column on the matching rows (touching `updated_at`).
    pub async fn decrement(&self, column: &str, amount: impl Into<Value>) -> Result<u64> {
        self.applied()
            .query
            .decrement_with(column, amount, self.touch_record())
            .await
    }

    fn touch_record(&self) -> Map<String, Value> {
        let mut record = Map::new();
        if M::uses_timestamps() && M::uses_updated_at() {
            record.insert(M::updated_at_column().to_string(), now());
        }
        record
    }

    /// Delete the matching rows (soft deleting `#[soft_deletes]` models).
    /// No model events are fired.
    pub async fn delete(&self) -> Result<u64> {
        if M::soft_deletes() {
            let mut record = self.touch_record();
            record.insert(M::deleted_at_column().to_string(), now());
            return self.applied().query.update(record).await;
        }
        self.applied().query.delete().await
    }

    /// Permanently delete the matching rows.
    pub async fn force_delete(&self) -> Result<u64> {
        self.applied().query.delete().await
    }

    /// Restore the matching soft deleted rows.
    pub async fn restore(&self) -> Result<u64> {
        let mut record = self.touch_record();
        record.insert(M::deleted_at_column().to_string(), Value::Null);
        let builder = match self.trashed {
            TrashedMode::Without => self.clone().with_trashed(),
            _ => self.clone(),
        };
        builder.applied().query.update(record).await
    }
}

fn table_alias(table: &str) -> String {
    match table.to_ascii_lowercase().find(" as ") {
        Some(index) => table[index + 4..].trim().to_string(),
        None => table.to_string(),
    }
}

/// Wrap the wheres added since the given position in a nested group, when
/// they contain an "or".
fn nest_wheres_from(mut query: QueryBuilder, wheres: usize, bindings: usize) -> QueryBuilder {
    if query.wheres.len() <= wheres || !query.wheres[wheres..].iter().any(|w| w.boolean == "or") {
        return query;
    }
    let nested_wheres = query.wheres.split_off(wheres);
    let nested_bindings = query
        .bindings
        .where_
        .split_off(bindings.min(query.bindings.where_.len()));
    let mut nested = query.for_nested_where();
    nested.wheres = nested_wheres;
    nested.bindings.where_ = nested_bindings;
    query.add_nested_where_query(nested, "and")
}

/// Hydrate models from rows. Models that track their originals start out
/// clean and existing, remembering the columns the row didn't have.
pub(crate) fn hydrate<M: Model>(rows: Vec<Map<String, Value>>) -> Result<Vec<M>> {
    rows.into_iter()
        .map(|row| {
            let missing: Vec<String> = M::columns()
                .iter()
                .filter(|column| !row.contains_key(**column))
                .map(|column| column.to_string())
                .collect();
            let mut model = M::from_attributes(row)?;
            super::original::hydrated(&mut model);
            if let Some(original) = model.original_state_mut() {
                original.missing = missing;
            }
            Ok(model)
        })
        .collect()
}

/// Fire `retrieved` and eager load relationships on freshly hydrated models.
pub(crate) async fn finish<M: Model>(
    models: &mut [M],
    eager: &[(String, EagerSpec)],
) -> Result<()> {
    for model in models.iter_mut() {
        fire(ModelEvent::Retrieved, model).await?;
    }
    for (name, spec) in eager {
        M::eager_load(models, name, spec.clone()).await?;
    }
    Ok(())
}

/// Build the existence query for a (possibly nested) relationship. Returns
/// the query and the operator / count to compare it with.
fn existence(
    relation: &dyn DynRelation,
    parent: &str,
    rest: Option<&str>,
    operator: &str,
    count: i64,
    constraint: Option<impl FnOnce(QueryBuilder) -> QueryBuilder>,
) -> Result<(QueryBuilder, String, i64)> {
    let (query, related) = relation.existence_query(parent);
    match rest {
        None => {
            let query = match constraint {
                Some(constraint) => constraint(query),
                None => query,
            };
            Ok((query, operator.to_string(), count))
        }
        Some(rest) => {
            let (first, remaining) = match rest.split_once('.') {
                Some((first, remaining)) => (first, Some(remaining)),
                None => (rest, None),
            };
            let nested = relation
                .related_relation(first)
                .ok_or_else(|| RelationNotFoundException::new(relation.related_class(), first))?;
            let (inner, inner_operator, inner_count) = existence(
                nested.as_ref(),
                &related,
                remaining,
                operator,
                count,
                constraint,
            )?;
            let query = add_existence_clause(query, inner, &inner_operator, inner_count, "and");
            Ok((query, ">=".to_string(), 1))
        }
    }
}

/// Constrain a query by a relationship existence query.
pub(crate) fn add_existence_clause(
    query: QueryBuilder,
    existence: QueryBuilder,
    operator: &str,
    count: i64,
    boolean: &str,
) -> QueryBuilder {
    if count == 1 && (operator == ">=" || operator == "<") {
        return query.add_where_exists(existence, boolean, operator == "<");
    }
    let existence = existence.select(Expression::new("count(*)"));
    match existence.try_to_sql() {
        Ok(sql) => {
            let mut bindings = existence.get_bindings();
            bindings.push(Value::from(count));
            let clause = format!("({sql}) {operator} ?");
            if boolean == "or" {
                query.or_where_raw(&clause, bindings)
            } else {
                query.where_raw(&clause, bindings)
            }
        }
        Err(error) => {
            let mut query = query;
            query.error.get_or_insert(error.to_string());
            query
        }
    }
}
