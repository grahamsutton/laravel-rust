//! The Eloquent query builder.

use std::fmt;
use std::future::Future;
use std::marker::PhantomData;

use illuminate_pagination::{
    LengthAwarePaginator, Paginator, PaginatorOptions, current_page, current_path,
};
use illuminate_support::{Collection, Conditionable, Map, Result, Str, Tappable, Value};

use super::errors::{ModelNotFoundException, RelationNotFoundException};
use super::events::{ModelEvent, fire};
use super::model::{Model, normalize_key, now};
use super::relations::{self, Constraint, DynRelation, EagerSpec};
use super::state::EloquentState;
use super::{Attributes, IntoIds, IntoRelations};
use crate::error::MultipleRecordsFoundException;
use crate::expression::{Expression, Ident};
use crate::query::Builder as QueryBuilder;
use crate::{Connection, DatabaseManager};

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

    /// Remove every global scope (including the soft delete scope).
    pub fn without_global_scopes(mut self) -> Self {
        self.without_scopes = true;
        self.trashed = TrashedMode::With;
        self
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
        Ok(models.into())
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
        Ok(self.find(id).await?.unwrap_or_else(M::template))
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
        let mut model = M::template();
        model.fill(attributes.merge(values))?;
        Ok(model)
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
        let mut model = M::template();
        model.fill(attributes.merge(values))?;
        model.save().await?;
        Ok(model)
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
        let mut model = M::template();
        model.fill(attributes)?;
        model.save().await?;
        Ok(model)
    }

    /// Create and save a new model, ignoring mass assignment protection.
    pub async fn force_create(&self, attributes: impl Into<Attributes>) -> Result<M> {
        let mut model = M::template();
        model.force_fill(attributes)?;
        model.save().await?;
        Ok(model)
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
        if M::timestamps() && M::uses_updated_at() && !values.contains_key(M::updated_at_column()) {
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
        if M::timestamps() && M::uses_updated_at() {
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

/// Hydrate models from rows.
pub(crate) fn hydrate<M: Model>(rows: Vec<Map<String, Value>>) -> Result<Vec<M>> {
    rows.into_iter().map(M::from_attributes).collect()
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
