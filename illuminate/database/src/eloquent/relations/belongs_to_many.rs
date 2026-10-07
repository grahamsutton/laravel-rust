//! `belongs_to_many`: many-to-many relationships through a pivot table.

use std::marker::PhantomData;

use illuminate_support::{Collection, Map, Result, Str, Value};

use super::{
    DynRelation, EagerSpec, Relation, RelationKind, SyncChanges, attribute, collect_keys,
    extract_prefixed, group_by_key, next_alias,
};
use crate::eloquent::builder::{Builder, finish, hydrate};
use crate::eloquent::model::{Model, key_is_set, now};
use crate::eloquent::{Attributes, BoxFuture, IntoIds, key_string};
use crate::expression::Operand;
use crate::query::Builder as QueryBuilder;

const PIVOT_PREFIX: &str = "pivot_";

#[derive(Clone)]
enum PivotWhere {
    Basic(String, String, Value),
    In(String, Vec<Value>, bool),
    Null(String, bool),
    Between(String, Value, Value, bool),
}

/// A many-to-many relationship through a pivot table.
///
/// Pivot columns are exposed to the related models through a
/// `#[computed] pivot: Option<Value>` field, when the model has one.
pub struct BelongsToMany<P: Model, R: Model> {
    pub(crate) query: Builder<R>,
    parent: Map<String, Value>,
    table: String,
    foreign_pivot_key: String,
    related_pivot_key: String,
    parent_key: String,
    related_key: String,
    pivot_columns: Vec<String>,
    pivot_wheres: Vec<(String, PivotWhere)>,
    pivot_values: Map<String, Value>,
    timestamps: bool,
    accessor: String,
    _parent: PhantomData<fn() -> P>,
}

/// A polymorphic many-to-many relationship (`self.morph_to_many("taggable")`
/// and its inverse, `self.morphed_by_many("taggable")`): a
/// [`BelongsToMany`] whose pivot table also holds the parent's type.
pub type MorphToMany<P, R> = BelongsToMany<P, R>;

impl<P: Model, R: Model> Clone for BelongsToMany<P, R> {
    fn clone(&self) -> Self {
        Self {
            query: self.query.clone(),
            parent: self.parent.clone(),
            table: self.table.clone(),
            foreign_pivot_key: self.foreign_pivot_key.clone(),
            related_pivot_key: self.related_pivot_key.clone(),
            parent_key: self.parent_key.clone(),
            related_key: self.related_key.clone(),
            pivot_columns: self.pivot_columns.clone(),
            pivot_wheres: self.pivot_wheres.clone(),
            pivot_values: self.pivot_values.clone(),
            timestamps: self.timestamps,
            accessor: self.accessor.clone(),
            _parent: PhantomData,
        }
    }
}

impl<P: Model, R: Model> BelongsToMany<P, R> {
    pub(crate) fn new(parent: &P) -> Self {
        let mut segments = [Str::snake(P::class_name()), Str::snake(R::class_name())];
        segments.sort();
        Self {
            query: Builder::new(),
            parent: parent.to_attributes(),
            table: segments.join("_"),
            foreign_pivot_key: P::get_foreign_key(),
            related_pivot_key: R::get_foreign_key(),
            parent_key: P::primary_key().to_string(),
            related_key: R::primary_key().to_string(),
            pivot_columns: Vec::new(),
            pivot_wheres: Vec::new(),
            pivot_values: Map::new(),
            timestamps: false,
            accessor: "pivot".to_string(),
            _parent: PhantomData,
        }
    }

    /// A polymorphic many-to-many relationship (`morph_to_many`): the
    /// parent's key and type live in the `{name}_id` / `{name}_type` pivot
    /// columns of the `{name}s` table.
    pub(crate) fn morph_to_many(self, name: &str) -> Self {
        let class = crate::eloquent::state::checked_morph_class::<P>();
        let relation = self
            .table(Str::plural(name))
            .foreign_pivot_key(format!("{name}_id"));
        relation.morph_type(name, class)
    }

    /// The inverse of a polymorphic many-to-many relationship
    /// (`morphed_by_many`): the related model's key and type live in the
    /// `{name}_id` / `{name}_type` pivot columns.
    pub(crate) fn morphed_by_many(self, name: &str) -> Self {
        let class = crate::eloquent::state::checked_morph_class::<R>();
        let relation = self
            .table(Str::plural(name))
            .related_pivot_key(format!("{name}_id"));
        relation.morph_type(name, class)
    }

    fn morph_type(mut self, name: &str, class: Result<String>) -> Self {
        let class = match class {
            Ok(class) => class,
            Err(error) => {
                self.query.query.error = Some(error.to_string());
                String::new()
            }
        };
        self.with_pivot_value(&format!("{name}_type"), class)
    }

    /// Use a different pivot table.
    pub fn table(mut self, table: impl Into<String>) -> Self {
        self.table = table.into();
        self
    }

    /// Use a different pivot column for the parent's key (`user_id`).
    pub fn foreign_pivot_key(mut self, key: impl Into<String>) -> Self {
        self.foreign_pivot_key = key.into();
        self
    }

    /// Use a different pivot column for the related model's key (`role_id`).
    pub fn related_pivot_key(mut self, key: impl Into<String>) -> Self {
        self.related_pivot_key = key.into();
        self
    }

    /// Use a different key on the parent model.
    pub fn parent_key(mut self, key: impl Into<String>) -> Self {
        self.parent_key = key.into();
        self
    }

    /// Use a different key on the related model.
    pub fn related_key(mut self, key: impl Into<String>) -> Self {
        self.related_key = key.into();
        self
    }

    /// Retrieve extra pivot columns.
    pub fn with_pivot<C: Into<String>>(mut self, columns: impl IntoIterator<Item = C>) -> Self {
        for column in columns {
            let column = column.into();
            if !self.pivot_columns.contains(&column) {
                self.pivot_columns.push(column);
            }
        }
        self
    }

    /// Maintain `created_at` / `updated_at` on the pivot table.
    pub fn with_timestamps(mut self) -> Self {
        self.timestamps = true;
        self.with_pivot(["created_at", "updated_at"])
    }

    /// Constrain the pivot table: `where_pivot("approved", true)`.
    pub fn where_pivot(self, column: &str, value: impl Into<Value>) -> Self {
        self.where_pivot_op(column, "=", value)
    }

    /// Constrain the pivot table with an operator.
    pub fn where_pivot_op(self, column: &str, operator: &str, value: impl Into<Value>) -> Self {
        self.pivot_where(
            "and",
            PivotWhere::Basic(column.to_string(), operator.to_string(), value.into()),
        )
    }

    fn pivot_where(mut self, boolean: &str, clause: PivotWhere) -> Self {
        self.pivot_wheres.push((boolean.to_string(), clause));
        self
    }

    /// Add an "or" pivot table constraint. The pivot constraints are grouped
    /// together, so an "or" never escapes the parent's constraint.
    pub fn or_where_pivot(self, column: &str, value: impl Into<Value>) -> Self {
        self.or_where_pivot_op(column, "=", value)
    }

    /// Add an "or" pivot table constraint with an operator.
    pub fn or_where_pivot_op(self, column: &str, operator: &str, value: impl Into<Value>) -> Self {
        self.pivot_where(
            "or",
            PivotWhere::Basic(column.to_string(), operator.to_string(), value.into()),
        )
    }

    /// Constrain a pivot column to a list of values.
    pub fn where_pivot_in(self, column: &str, values: impl IntoIds) -> Self {
        self.pivot_where(
            "and",
            PivotWhere::In(column.to_string(), values.into_ids(), false),
        )
    }

    /// Add an "or where pivot in" constraint.
    pub fn or_where_pivot_in(self, column: &str, values: impl IntoIds) -> Self {
        self.pivot_where(
            "or",
            PivotWhere::In(column.to_string(), values.into_ids(), false),
        )
    }

    /// Exclude a list of values of a pivot column.
    pub fn where_pivot_not_in(self, column: &str, values: impl IntoIds) -> Self {
        self.pivot_where(
            "and",
            PivotWhere::In(column.to_string(), values.into_ids(), true),
        )
    }

    /// Add an "or where pivot not in" constraint.
    pub fn or_where_pivot_not_in(self, column: &str, values: impl IntoIds) -> Self {
        self.pivot_where(
            "or",
            PivotWhere::In(column.to_string(), values.into_ids(), true),
        )
    }

    /// Only pivot records where the column is null.
    pub fn where_pivot_null(self, column: &str) -> Self {
        self.pivot_where("and", PivotWhere::Null(column.to_string(), false))
    }

    /// Add an "or where pivot null" constraint.
    pub fn or_where_pivot_null(self, column: &str) -> Self {
        self.pivot_where("or", PivotWhere::Null(column.to_string(), false))
    }

    /// Only pivot records where the column is not null.
    pub fn where_pivot_not_null(self, column: &str) -> Self {
        self.pivot_where("and", PivotWhere::Null(column.to_string(), true))
    }

    /// Add an "or where pivot not null" constraint.
    pub fn or_where_pivot_not_null(self, column: &str) -> Self {
        self.pivot_where("or", PivotWhere::Null(column.to_string(), true))
    }

    /// Only pivot records where the column is between the two values.
    pub fn where_pivot_between(
        self,
        column: &str,
        min: impl Into<Value>,
        max: impl Into<Value>,
    ) -> Self {
        self.pivot_where(
            "and",
            PivotWhere::Between(column.to_string(), min.into(), max.into(), false),
        )
    }

    /// Add an "or where pivot between" constraint.
    pub fn or_where_pivot_between(
        self,
        column: &str,
        min: impl Into<Value>,
        max: impl Into<Value>,
    ) -> Self {
        self.pivot_where(
            "or",
            PivotWhere::Between(column.to_string(), min.into(), max.into(), false),
        )
    }

    /// Only pivot records where the column is outside the two values.
    pub fn where_pivot_not_between(
        self,
        column: &str,
        min: impl Into<Value>,
        max: impl Into<Value>,
    ) -> Self {
        self.pivot_where(
            "and",
            PivotWhere::Between(column.to_string(), min.into(), max.into(), true),
        )
    }

    /// Add an "or where pivot not between" constraint.
    pub fn or_where_pivot_not_between(
        self,
        column: &str,
        min: impl Into<Value>,
        max: impl Into<Value>,
    ) -> Self {
        self.pivot_where(
            "or",
            PivotWhere::Between(column.to_string(), min.into(), max.into(), true),
        )
    }

    /// Order the related models by a pivot column.
    pub fn order_by_pivot(mut self, column: &str, direction: &str) -> Self {
        self.query = self
            .query
            .order_by(format!("{}.{column}", self.table), direction);
        self
    }

    /// Order the related models by a pivot column, descending.
    pub fn order_by_pivot_desc(self, column: &str) -> Self {
        self.order_by_pivot(column, "desc")
    }

    /// Expose the pivot columns under a different attribute than `pivot`
    /// (the related model needs a `#[computed]` field of that name).
    pub fn as_(mut self, accessor: &str) -> Self {
        self.accessor = accessor.to_string();
        self
    }

    /// The attribute the pivot columns are exposed under.
    pub fn get_pivot_accessor(&self) -> &str {
        &self.accessor
    }

    /// The keys of every related model attached to the parent.
    pub async fn all_related_ids(&self) -> Result<Collection<Value>> {
        self.new_pivot_query()
            .pluck(self.related_pivot_key.as_str())
            .await
    }

    /// Constrain a pivot column and set it on attached records.
    pub fn with_pivot_value(mut self, column: &str, value: impl Into<Value>) -> Self {
        let value = value.into();
        self.pivot_values.insert(column.to_string(), value.clone());
        self.where_pivot(column, value)
    }

    /// The pivot table.
    pub fn get_table(&self) -> &str {
        &self.table
    }

    /// The parent's key value.
    pub fn get_parent_key(&self) -> Value {
        attribute(&self.parent, &self.parent_key)
    }

    forward_eloquent!(query);

    fn apply_pivot_wheres(&self, query: QueryBuilder, qualify: bool) -> QueryBuilder {
        let has_or = self.pivot_wheres.iter().any(|(boolean, _)| boolean == "or");
        if has_or {
            return query.where_group(|nested| self.add_pivot_wheres(nested, qualify));
        }
        self.add_pivot_wheres(query, qualify)
    }

    fn add_pivot_wheres(&self, mut query: QueryBuilder, qualify: bool) -> QueryBuilder {
        let column = |name: &str| {
            if qualify {
                format!("{}.{name}", self.table)
            } else {
                name.to_string()
            }
        };
        for (boolean, clause) in &self.pivot_wheres {
            let boolean = boolean.as_str();
            query = match clause {
                PivotWhere::Basic(name, operator, value) => {
                    query.add_where(column(name), operator, value.clone(), boolean)
                }
                PivotWhere::In(name, values, not) => {
                    query.add_where_in(column(name), values.clone(), boolean, *not)
                }
                PivotWhere::Null(name, false) if boolean == "or" => {
                    query.or_where_null(column(name))
                }
                PivotWhere::Null(name, false) => query.where_null(column(name)),
                PivotWhere::Null(name, true) if boolean == "or" => {
                    query.or_where_not_null(column(name))
                }
                PivotWhere::Null(name, true) => query.where_not_null(column(name)),
                PivotWhere::Between(name, min, max, not) => match (boolean, *not) {
                    ("or", false) => {
                        query.or_where_between(column(name), [min.clone(), max.clone()])
                    }
                    ("or", true) => {
                        query.or_where_not_between(column(name), [min.clone(), max.clone()])
                    }
                    (_, false) => query.where_between(column(name), [min.clone(), max.clone()]),
                    (_, true) => query.where_not_between(column(name), [min.clone(), max.clone()]),
                },
            };
        }
        query
    }

    /// Join the pivot table onto a related query.
    fn join_pivot(&self, mut query: Builder<R>) -> Builder<R> {
        let related_key = query.qualify_column(&self.related_key);
        query.query = query.query.join(
            self.table.as_str(),
            related_key,
            "=",
            format!("{}.{}", self.table, self.related_pivot_key),
        );
        query.query = self.apply_pivot_wheres(query.query, true);
        query
    }

    /// Select the related columns and the pivot columns.
    fn select_columns(&self, mut query: Builder<R>) -> Builder<R> {
        if query.query.columns.is_none() {
            let all = format!("{}.*", query.qualifier);
            query.query = query.query.select(all);
        }
        let mut columns = vec![
            self.foreign_pivot_key.clone(),
            self.related_pivot_key.clone(),
        ];
        columns.extend(self.pivot_columns.iter().cloned());
        let aliased: Vec<String> = columns
            .iter()
            .map(|column| format!("{}.{column} as {PIVOT_PREFIX}{column}", self.table))
            .collect();
        query.query = query.query.add_select(aliased);
        query
    }

    /// The relationship query, constrained to the parent.
    pub fn get_query(&self) -> Builder<R> {
        let mut query = self.join_pivot(self.query.clone());
        query.query = query.query.where_(
            format!("{}.{}", self.table, self.foreign_pivot_key),
            self.get_parent_key(),
        );
        query
    }

    /// Run a related query, exposing the pivot columns on each model.
    async fn fetch_with_pivot(&self, query: Builder<R>) -> Result<Vec<(R, Map<String, Value>)>> {
        let (rows, eager) = self.select_columns(query).get_rows().await?;
        let mut pivots = Vec::with_capacity(rows.len());
        let mut cleaned = Vec::with_capacity(rows.len());
        for mut row in rows {
            pivots.push(extract_prefixed(&mut row, PIVOT_PREFIX));
            cleaned.push(row);
        }
        let mut models = hydrate::<R>(cleaned)?;
        for (model, pivot) in models.iter_mut().zip(&pivots) {
            model.set_attribute(&self.accessor, Value::Object(pivot.clone()))?;
        }
        finish(&mut models, &eager).await?;
        Ok(models.into_iter().zip(pivots).collect())
    }

    async fn fetch(&self, query: Builder<R>) -> Result<Vec<R>> {
        if !key_is_set(&self.get_parent_key()) {
            return Ok(Vec::new());
        }
        Ok(self
            .fetch_with_pivot(query)
            .await?
            .into_iter()
            .map(|(model, _)| model)
            .collect())
    }

    relation_queries!();

    /// Get the related models.
    pub async fn get(&self) -> Result<Collection<R>> {
        Ok(self.fetch(self.get_query()).await?.into())
    }

    // ------------------------------------------------------------------
    // Pivot operations
    // ------------------------------------------------------------------

    /// A query on the pivot table, constrained to the parent.
    pub fn new_pivot_query(&self) -> QueryBuilder {
        let query = self
            .query
            .get_query()
            .new_query()
            .from(self.table.as_str())
            .where_(self.foreign_pivot_key.as_str(), self.get_parent_key());
        self.apply_pivot_wheres(query, false)
    }

    fn pivot_record(&self, id: Value, extra: &Map<String, Value>) -> Map<String, Value> {
        let mut record = Map::new();
        record.insert(self.foreign_pivot_key.clone(), self.get_parent_key());
        record.insert(self.related_pivot_key.clone(), id);
        for (key, value) in &self.pivot_values {
            record.insert(key.clone(), value.clone());
        }
        if self.timestamps {
            let time = now();
            record.insert("created_at".to_string(), time.clone());
            record.insert("updated_at".to_string(), time);
        }
        for (key, value) in extra {
            record.insert(key.clone(), value.clone());
        }
        record
    }

    /// Attach related models by key.
    ///
    /// ```ignore
    /// user.roles().attach([1, 2]).await?;
    /// ```
    pub async fn attach(&self, ids: impl IntoIds) -> Result<()> {
        self.attach_with(ids, Attributes::new()).await
    }

    /// Attach related models by key, with extra pivot columns.
    pub async fn attach_with(
        &self,
        ids: impl IntoIds,
        attributes: impl Into<Attributes>,
    ) -> Result<()> {
        let extra = attributes.into().0;
        let records: Vec<Value> = ids
            .into_ids()
            .into_iter()
            .map(|id| Value::Object(self.pivot_record(id, &extra)))
            .collect();
        if records.is_empty() {
            return Ok(());
        }
        let query = self.query.get_query().new_query().from(self.table.as_str());
        query.insert(Value::Array(records)).await?;
        Ok(())
    }

    /// Detach related models by key, returning the number of pivot records
    /// deleted.
    pub async fn detach(&self, ids: impl IntoIds) -> Result<u64> {
        let ids = ids.into_ids();
        if ids.is_empty() {
            return Ok(0);
        }
        self.new_pivot_query()
            .where_in(self.related_pivot_key.as_str(), ids)
            .delete()
            .await
    }

    /// Detach every related model.
    pub async fn detach_all(&self) -> Result<u64> {
        self.new_pivot_query().delete().await
    }

    /// Update the pivot record of an attached model.
    pub async fn update_existing_pivot(
        &self,
        id: impl Into<Value>,
        attributes: impl Into<Attributes>,
    ) -> Result<u64> {
        let mut values = attributes.into().0;
        if self.timestamps && !values.contains_key("updated_at") {
            values.insert("updated_at".to_string(), now());
        }
        self.new_pivot_query()
            .where_(self.related_pivot_key.as_str(), id.into())
            .update(values)
            .await
    }

    async fn current_keys(&self) -> Result<Vec<Value>> {
        Ok(self
            .new_pivot_query()
            .pluck(self.related_pivot_key.as_str())
            .await?
            .into_vec())
    }

    async fn sync_records(
        &self,
        records: Vec<(Value, Map<String, Value>)>,
        detaching: bool,
    ) -> Result<SyncChanges> {
        let current = self.current_keys().await?;
        let current_keys: Vec<Option<String>> = current.iter().map(key_string).collect();
        let wanted: Vec<Option<String>> = records.iter().map(|(id, _)| key_string(id)).collect();
        let mut changes = SyncChanges::default();

        if detaching {
            let detach: Vec<Value> = current
                .iter()
                .zip(&current_keys)
                .filter(|(_, key)| !wanted.contains(key))
                .map(|(id, _)| id.clone())
                .collect();
            if !detach.is_empty() {
                self.detach(detach.clone()).await?;
                changes.detached = detach;
            }
        }

        for (id, extra) in records {
            if current_keys.contains(&key_string(&id)) {
                if !extra.is_empty() && self.update_existing_pivot(id.clone(), extra).await? > 0 {
                    changes.updated.push(id);
                }
            } else {
                self.attach_with(vec![id.clone()], extra).await?;
                changes.attached.push(id);
            }
        }
        Ok(changes)
    }

    /// Sync the attached models with the given keys: models not in the list
    /// are detached, missing ones are attached.
    pub async fn sync(&self, ids: impl IntoIds) -> Result<SyncChanges> {
        let records = ids
            .into_ids()
            .into_iter()
            .map(|id| (id, Map::new()))
            .collect();
        self.sync_records(records, true).await
    }

    /// Sync with extra pivot columns per key.
    ///
    /// ```ignore
    /// user.roles().sync_with([(1, json!({"expires": true})), (2, json!({}))]).await?;
    /// ```
    pub async fn sync_with<K: Into<Value>, A: Into<Attributes>>(
        &self,
        records: impl IntoIterator<Item = (K, A)>,
    ) -> Result<SyncChanges> {
        let records = records
            .into_iter()
            .map(|(id, extra)| (id.into(), extra.into().0))
            .collect();
        self.sync_records(records, true).await
    }

    /// Sync with the same pivot values for every key.
    pub async fn sync_with_pivot_values(
        &self,
        ids: impl IntoIds,
        values: impl Into<Attributes>,
    ) -> Result<SyncChanges> {
        let values = values.into().0;
        let records = ids
            .into_ids()
            .into_iter()
            .map(|id| (id, values.clone()))
            .collect();
        self.sync_records(records, true).await
    }

    /// Attach the missing keys without detaching anything.
    pub async fn sync_without_detaching(&self, ids: impl IntoIds) -> Result<SyncChanges> {
        let records = ids
            .into_ids()
            .into_iter()
            .map(|id| (id, Map::new()))
            .collect();
        self.sync_records(records, false).await
    }

    /// Attach the keys that aren't attached and detach the ones that are.
    pub async fn toggle(&self, ids: impl IntoIds) -> Result<SyncChanges> {
        let current: Vec<Option<String>> =
            self.current_keys().await?.iter().map(key_string).collect();
        let mut changes = SyncChanges::default();
        for id in ids.into_ids() {
            if current.contains(&key_string(&id)) {
                changes.detached.push(id);
            } else {
                changes.attached.push(id);
            }
        }
        if !changes.detached.is_empty() {
            self.detach(changes.detached.clone()).await?;
        }
        if !changes.attached.is_empty() {
            self.attach(changes.attached.clone()).await?;
        }
        Ok(changes)
    }

    /// Create a related model and attach it, without firing events.
    pub async fn create_quietly(&self, attributes: impl Into<Attributes>) -> Result<R> {
        crate::eloquent::without_events(self.create(attributes)).await
    }

    /// Create and attach several related models, without firing events.
    pub async fn create_many_quietly<A: Into<Attributes>>(
        &self,
        records: impl IntoIterator<Item = A>,
    ) -> Result<Collection<R>> {
        crate::eloquent::without_events(self.create_many(records)).await
    }

    /// Save and attach a related model, without firing events.
    pub async fn save_quietly(&self, model: &mut R) -> Result<bool> {
        crate::eloquent::without_events(self.save(model)).await
    }

    /// Save and attach several related models, without firing events.
    pub async fn save_many_quietly(&self, models: &mut [R]) -> Result<()> {
        crate::eloquent::without_events(self.save_many(models)).await
    }

    /// Create a related model and attach it, or — when a unique constraint
    /// stops the insert — attach the existing one matching the attributes.
    pub async fn create_or_first(
        &self,
        attributes: impl Into<Attributes>,
        values: impl Into<Attributes>,
    ) -> Result<R> {
        let attributes = attributes.into();
        let mut candidate = R::template();
        candidate.fill(attributes.clone().merge(values))?;
        let connection = R::get_connection();
        let result = connection
            .transaction(|| async move {
                candidate.save().await?;
                Ok(candidate)
            })
            .await;
        let model = match result {
            Ok(model) => model,
            Err(error)
                if error
                    .downcast_ref::<crate::QueryException>()
                    .is_some_and(crate::QueryException::is_unique_constraint_violation) =>
            {
                let existing = R::query()
                    .use_write_pdo()
                    .where_map(attributes.0)
                    .first()
                    .await?;
                match existing {
                    Some(model) => {
                        let key = model.get_attribute(&self.related_key);
                        if self
                            .new_pivot_query()
                            .where_(self.related_pivot_key.as_str(), key.clone())
                            .exists()
                            .await?
                        {
                            return Ok(model);
                        }
                        model
                    }
                    None => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        self.attach(vec![model.get_attribute(&self.related_key)])
            .await?;
        Ok(model)
    }

    /// Increment a column of the first related model matching the
    /// attributes, or create (and attach) it with the column set to
    /// `default`.
    pub async fn increment_or_create(
        &self,
        attributes: impl Into<Attributes>,
        column: &str,
        default: impl Into<Value>,
        step: impl Into<Value>,
    ) -> Result<R> {
        let attributes = attributes.into();
        let mut query = self.get_query();
        for (name, value) in &attributes.0 {
            let name = query.qualify_column(name);
            query.query = query.query.where_(name, Operand::from(value.clone()));
        }
        if let Some(mut model) = self.fetch(query.take(1)).await?.into_iter().next() {
            model.increment(column, step.into()).await?;
            return Ok(model);
        }
        let mut defaults = Attributes::new();
        defaults.insert(column, default.into());
        let mut model = R::template();
        model.force_fill(attributes.merge(defaults))?;
        self.save(&mut model).await?;
        Ok(model)
    }

    /// Save a related model and attach it.
    pub async fn save(&self, model: &mut R) -> Result<bool> {
        self.save_with_pivot(model, Attributes::new()).await
    }

    /// Save a related model and attach it with extra pivot columns.
    pub async fn save_with_pivot(
        &self,
        model: &mut R,
        pivot: impl Into<Attributes>,
    ) -> Result<bool> {
        let saved = model.save().await?;
        self.attach_with(vec![model.get_attribute(&self.related_key)], pivot)
            .await?;
        Ok(saved)
    }

    /// Save and attach several related models.
    pub async fn save_many(&self, models: &mut [R]) -> Result<()> {
        for model in models {
            self.save(model).await?;
        }
        Ok(())
    }

    /// Create a related model and attach it.
    pub async fn create(&self, attributes: impl Into<Attributes>) -> Result<R> {
        self.create_with_pivot(attributes, Attributes::new()).await
    }

    /// Create a related model and attach it with extra pivot columns.
    pub async fn create_with_pivot(
        &self,
        attributes: impl Into<Attributes>,
        pivot: impl Into<Attributes>,
    ) -> Result<R> {
        let mut model = R::template();
        model.fill(attributes)?;
        self.save_with_pivot(&mut model, pivot).await?;
        Ok(model)
    }

    /// Create and attach several related models.
    pub async fn create_many<A: Into<Attributes>>(
        &self,
        records: impl IntoIterator<Item = A>,
    ) -> Result<Collection<R>> {
        let mut models = Vec::new();
        for attributes in records {
            models.push(self.create(attributes).await?);
        }
        Ok(models.into())
    }

    /// The first related model matching the attributes, or a newly created
    /// (and attached) one.
    pub async fn first_or_create(
        &self,
        attributes: impl Into<Attributes>,
        values: impl Into<Attributes>,
    ) -> Result<R> {
        let attributes = attributes.into();
        let mut query = self.get_query();
        for (column, value) in &attributes.0 {
            let column = query.qualify_column(column);
            query.query = query.query.where_(column, Operand::from(value.clone()));
        }
        if let Some(model) = self.fetch(query.take(1)).await?.into_iter().next() {
            return Ok(model);
        }
        self.create(attributes.merge(values)).await
    }
}

impl<P: Model, R: Model> Relation<P> for BelongsToMany<P, R> {
    type Related = R;
    type Loaded = Vec<R>;

    fn eager_load<'a>(
        self,
        parents: &'a [P],
        spec: EagerSpec,
    ) -> BoxFuture<'a, Result<Vec<Vec<R>>>> {
        Box::pin(async move {
            let keys = collect_keys(parents, &self.parent_key);
            if keys.is_empty() {
                return Ok(vec![Vec::new(); parents.len()]);
            }
            let mut query = self.join_pivot(self.query.clone());
            query.query = query
                .query
                .where_in(format!("{}.{}", self.table, self.foreign_pivot_key), keys);
            let query = spec.apply_to(query);
            let related = self.fetch_with_pivot(query).await?;
            let groups = group_by_key(
                related
                    .into_iter()
                    .map(|(model, pivot)| (attribute(&pivot, &self.foreign_pivot_key), model))
                    .collect(),
            );
            Ok(parents
                .iter()
                .map(|parent| {
                    key_string(&parent.get_attribute(&self.parent_key))
                        .and_then(|key| groups.get(&key).cloned())
                        .unwrap_or_default()
                })
                .collect())
        })
    }

    fn into_dyn(self) -> Box<dyn DynRelation> {
        Box::new(self)
    }
}

impl<P: Model, R: Model> DynRelation for BelongsToMany<P, R> {
    fn kind(&self) -> RelationKind {
        RelationKind::BelongsToMany
    }

    fn related_class(&self) -> &'static str {
        R::class_name()
    }

    fn related_table(&self) -> String {
        R::table()
    }

    fn existence_query(&self, parent: &str) -> (QueryBuilder, String) {
        let mut query = self.query.clone();
        if R::table() == parent {
            query = query.aliased(&next_alias());
        }
        let mut query = self.join_pivot(query);
        query.query = query.query.where_column(
            format!("{}.{}", self.table, self.foreign_pivot_key),
            format!("{parent}.{}", self.parent_key),
        );
        let qualifier = query.qualifier.clone();
        (query.to_base(), qualifier)
    }

    fn related_relation(&self, name: &str) -> Option<Box<dyn DynRelation>> {
        R::relation(name)
    }

    fn attach_models<'a>(
        &'a self,
        parent: &'a Map<String, Value>,
        related: &'a [Map<String, Value>],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut relation = self.clone();
            relation.parent = parent.clone();
            let ids: Vec<Value> = related
                .iter()
                .map(|model| attribute(model, &self.related_key))
                .collect();
            relation.attach(ids).await
        })
    }
}
