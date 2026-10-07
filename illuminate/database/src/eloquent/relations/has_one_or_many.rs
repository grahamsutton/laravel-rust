//! `has_one`, `has_many`, `morph_one` and `morph_many`.

use std::marker::PhantomData;

use illuminate_support::{Collection, Map, Result, Value};

use super::{
    DynRelation, EagerSpec, Relation, RelationKind, attribute, collect_keys, group_by_key,
    next_alias,
};
use crate::eloquent::builder::Builder;
use crate::eloquent::model::{Model, key_is_set};
use crate::eloquent::{Attributes, BoxFuture, key_string};
use crate::query::Builder as QueryBuilder;

/// A one-to-one (`MANY = false`) or one-to-many (`MANY = true`)
/// relationship: the related models hold a foreign key to the parent.
///
/// Use the [`HasOne`], [`HasMany`], [`MorphOne`] and [`MorphMany`] aliases.
pub struct HasOneOrMany<P: Model, R: Model, const MANY: bool> {
    pub(crate) query: Builder<R>,
    parent: Map<String, Value>,
    foreign_key: String,
    local_key: String,
    morph: Option<(String, String)>,
    _parent: PhantomData<fn() -> P>,
}

/// A one-to-one relationship (`self.has_one()`).
pub type HasOne<P, R> = HasOneOrMany<P, R, false>;

/// A one-to-many relationship (`self.has_many()`).
pub type HasMany<P, R> = HasOneOrMany<P, R, true>;

/// A polymorphic one-to-one relationship (`self.morph_one("imageable")`).
pub type MorphOne<P, R> = HasOneOrMany<P, R, false>;

/// A polymorphic one-to-many relationship (`self.morph_many("commentable")`).
pub type MorphMany<P, R> = HasOneOrMany<P, R, true>;

impl<P: Model, R: Model, const MANY: bool> Clone for HasOneOrMany<P, R, MANY> {
    fn clone(&self) -> Self {
        Self {
            query: self.query.clone(),
            parent: self.parent.clone(),
            foreign_key: self.foreign_key.clone(),
            local_key: self.local_key.clone(),
            morph: self.morph.clone(),
            _parent: PhantomData,
        }
    }
}

impl<P: Model, R: Model, const MANY: bool> HasOneOrMany<P, R, MANY> {
    pub(crate) fn new(parent: &P, foreign_key: String, local_key: String) -> Self {
        Self {
            query: Builder::new(),
            parent: parent.to_attributes(),
            foreign_key,
            local_key,
            morph: None,
            _parent: PhantomData,
        }
    }

    pub(crate) fn morph(parent: &P, name: &str) -> Self {
        let mut relation = Self::new(parent, format!("{name}_id"), P::primary_key().to_string());
        let class = match crate::eloquent::state::checked_morph_class::<P>() {
            Ok(class) => class,
            Err(error) => {
                relation.query.query.error = Some(error.to_string());
                P::morph_class()
            }
        };
        relation.morph = Some((format!("{name}_type"), class));
        relation
    }

    /// Use a different foreign key on the related model.
    pub fn foreign_key(mut self, key: impl Into<String>) -> Self {
        self.foreign_key = key.into();
        self
    }

    /// Use a different local key on the parent model.
    pub fn local_key(mut self, key: impl Into<String>) -> Self {
        self.local_key = key.into();
        self
    }

    /// The foreign key on the related model.
    pub fn get_foreign_key_name(&self) -> &str {
        &self.foreign_key
    }

    /// The local key on the parent model.
    pub fn get_local_key_name(&self) -> &str {
        &self.local_key
    }

    /// The parent's local key value.
    pub fn get_parent_key(&self) -> Value {
        attribute(&self.parent, &self.local_key)
    }

    forward_eloquent!(query);

    fn constrain(&self, mut query: Builder<R>) -> Builder<R> {
        if super::constraints_disabled() {
            return query;
        }
        let foreign_key = query.qualify_column(&self.foreign_key);
        query.query = query
            .query
            .where_(foreign_key.clone(), self.get_parent_key())
            .where_not_null(foreign_key);
        if let Some((column, class)) = &self.morph {
            let column = query.qualify_column(column);
            query.query = query.query.where_(column, class.clone());
        }
        query
    }

    /// The relationship query, constrained to the parent.
    pub fn get_query(&self) -> Builder<R> {
        self.constrain(self.query.clone())
    }

    async fn fetch(&self, query: Builder<R>) -> Result<Vec<R>> {
        if !key_is_set(&self.get_parent_key()) && !super::constraints_disabled() {
            return Ok(Vec::new());
        }
        Ok(query.get().await?.into_vec())
    }

    relation_queries!();

    /// Set the foreign key (and morph type) on a related model.
    pub fn set_foreign_attributes(&self, model: &mut R) -> Result<()> {
        if self.morph.is_some() {
            crate::eloquent::state::checked_morph_class::<P>()?;
        }
        model.set_attribute(&self.foreign_key, self.get_parent_key())?;
        if let Some((column, class)) = &self.morph {
            model.set_attribute(column, Value::String(class.clone()))?;
        }
        Ok(())
    }

    fn foreign_attributes(&self) -> Map<String, Value> {
        let mut attributes = Map::new();
        attributes.insert(self.foreign_key.clone(), self.get_parent_key());
        if let Some((column, class)) = &self.morph {
            attributes.insert(column.clone(), Value::String(class.clone()));
        }
        attributes
    }

    /// A new related model (not saved) belonging to the parent.
    pub fn make(&self, attributes: impl Into<Attributes>) -> Result<R> {
        let mut model = R::template();
        model.fill(attributes)?;
        self.set_foreign_attributes(&mut model)?;
        Ok(model)
    }

    /// Create and save a related model belonging to the parent.
    pub async fn create(&self, attributes: impl Into<Attributes>) -> Result<R> {
        let mut model = self.make(attributes)?;
        model.save().await?;
        Ok(model)
    }

    /// Create a related model, ignoring mass assignment protection.
    pub async fn force_create(&self, attributes: impl Into<Attributes>) -> Result<R> {
        let mut model = R::template();
        model.force_fill(attributes)?;
        self.set_foreign_attributes(&mut model)?;
        model.save().await?;
        Ok(model)
    }

    /// Create several related models.
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

    /// New related models (not saved) belonging to the parent.
    pub fn make_many<A: Into<Attributes>>(
        &self,
        records: impl IntoIterator<Item = A>,
    ) -> Result<Collection<R>> {
        records
            .into_iter()
            .map(|attributes| self.make(attributes))
            .collect::<Result<Vec<R>>>()
            .map(Collection::from)
    }

    /// Create several related models, ignoring mass assignment protection.
    pub async fn force_create_many<A: Into<Attributes>>(
        &self,
        records: impl IntoIterator<Item = A>,
    ) -> Result<Collection<R>> {
        let mut models = Vec::new();
        for attributes in records {
            models.push(self.force_create(attributes).await?);
        }
        Ok(models.into())
    }

    /// Create a related model without firing any events.
    pub async fn create_quietly(&self, attributes: impl Into<Attributes>) -> Result<R> {
        crate::eloquent::without_events(self.create(attributes)).await
    }

    /// Create a related model, ignoring mass assignment protection, without
    /// firing any events.
    pub async fn force_create_quietly(&self, attributes: impl Into<Attributes>) -> Result<R> {
        crate::eloquent::without_events(self.force_create(attributes)).await
    }

    /// Create several related models without firing any events.
    pub async fn create_many_quietly<A: Into<Attributes>>(
        &self,
        records: impl IntoIterator<Item = A>,
    ) -> Result<Collection<R>> {
        crate::eloquent::without_events(self.create_many(records)).await
    }

    /// Create several related models, ignoring mass assignment protection,
    /// without firing any events.
    pub async fn force_create_many_quietly<A: Into<Attributes>>(
        &self,
        records: impl IntoIterator<Item = A>,
    ) -> Result<Collection<R>> {
        crate::eloquent::without_events(self.force_create_many(records)).await
    }

    /// Attach a model to the parent and save it without firing events.
    pub async fn save_quietly(&self, model: &mut R) -> Result<bool> {
        crate::eloquent::without_events(self.save(model)).await
    }

    /// Attach several models to the parent and save them without firing
    /// events.
    pub async fn save_many_quietly(&self, models: &mut [R]) -> Result<()> {
        crate::eloquent::without_events(self.save_many(models)).await
    }

    /// Create a related model, or get the existing one matching the
    /// attributes when a unique constraint stops the insert.
    pub async fn create_or_first(
        &self,
        attributes: impl Into<Attributes>,
        values: impl Into<Attributes>,
    ) -> Result<R> {
        let attributes = attributes.into();
        let candidate = self.make(attributes.clone().merge(values))?;
        let connection = R::get_connection();
        let result = connection
            .transaction(|| async move {
                let mut model = candidate;
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
                    .get_query()
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

    /// Increment a column of the first related model matching the
    /// attributes, or create it with the column set to `default`.
    pub async fn increment_or_create(
        &self,
        attributes: impl Into<Attributes>,
        column: &str,
        default: impl Into<Value>,
        step: impl Into<Value>,
    ) -> Result<R> {
        let attributes = attributes.into();
        if let Some(mut model) = self
            .get_query()
            .where_map(attributes.0.clone())
            .first()
            .await?
        {
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

    /// Attach a model to the parent (setting its foreign key) and save it.
    pub async fn save(&self, model: &mut R) -> Result<bool> {
        self.set_foreign_attributes(model)?;
        model.save().await
    }

    /// Attach several models to the parent and save them.
    pub async fn save_many(&self, models: &mut [R]) -> Result<()> {
        for model in models {
            self.save(model).await?;
        }
        Ok(())
    }

    /// The first related model matching the attributes, or a new one (not
    /// saved) belonging to the parent.
    pub async fn first_or_new(
        &self,
        attributes: impl Into<Attributes>,
        values: impl Into<Attributes>,
    ) -> Result<R> {
        let attributes = attributes.into();
        if let Some(model) = self
            .get_query()
            .where_map(attributes.0.clone())
            .first()
            .await?
        {
            return Ok(model);
        }
        self.make(attributes.merge(values))
    }

    /// The first related model matching the attributes, or a newly created one.
    pub async fn first_or_create(
        &self,
        attributes: impl Into<Attributes>,
        values: impl Into<Attributes>,
    ) -> Result<R> {
        let attributes = attributes.into();
        if let Some(model) = self
            .get_query()
            .where_map(attributes.0.clone())
            .first()
            .await?
        {
            return Ok(model);
        }
        self.create(attributes.merge(values)).await
    }

    /// Update the first related model matching the attributes, or create it.
    pub async fn update_or_create(
        &self,
        attributes: impl Into<Attributes>,
        values: impl Into<Attributes>,
    ) -> Result<R> {
        let mut model = self.first_or_new(attributes, Attributes::new()).await?;
        model.fill(values)?;
        self.save(&mut model).await?;
        Ok(model)
    }

    /// Update every related model (touching `updated_at`).
    pub async fn update(&self, values: impl Into<Attributes>) -> Result<u64> {
        self.get_query().update(values).await
    }

    /// Delete every related model (soft deleting when supported).
    pub async fn delete(&self) -> Result<u64> {
        self.get_query().delete().await
    }

    async fn eager_groups(
        self,
        parents: &[P],
        spec: EagerSpec,
    ) -> Result<std::collections::HashMap<String, Vec<R>>> {
        let keys = collect_keys(parents, &self.local_key);
        if keys.is_empty() {
            return Ok(Default::default());
        }
        let mut query = self.query.clone();
        let foreign_key = query.qualify_column(&self.foreign_key);
        query.query = query.query.where_in(foreign_key, keys);
        if let Some((column, class)) = &self.morph {
            let column = query.qualify_column(column);
            query.query = query.query.where_(column, class.clone());
        }
        let query = spec.apply_to(query);
        let related = query.get().await?.into_vec();
        Ok(group_by_key(
            related
                .into_iter()
                .map(|model| (model.get_attribute(&self.foreign_key), model))
                .collect(),
        ))
    }

    fn dyn_existence(&self, parent: &str) -> (QueryBuilder, String) {
        let mut query = self.query.clone();
        if R::table() == parent {
            query = query.aliased(&next_alias());
        }
        let foreign_key = query.qualify_column(&self.foreign_key);
        query.query = query
            .query
            .where_column(foreign_key, format!("{parent}.{}", self.local_key));
        if let Some((column, class)) = &self.morph {
            let column = query.qualify_column(column);
            query.query = query.query.where_(column, class.clone());
        }
        let qualifier = query.qualifier.clone();
        (query.to_base(), qualifier)
    }
}

impl<P: Model, R: Model> HasOneOrMany<P, R, true> {
    /// Get the related models.
    pub async fn get(&self) -> Result<Collection<R>> {
        Ok(self.fetch(self.get_query()).await?.into())
    }

    /// The same relationship as a one-to-one relationship (Laravel's
    /// `HasMany::one()`), usually narrowed with `latest()`, `oldest()` or a
    /// constraint.
    ///
    /// ```ignore
    /// pub fn latest_post(&self) -> HasOne<Self, Post> {
    ///     self.posts().one().latest()
    /// }
    /// ```
    pub fn one(self) -> HasOneOrMany<P, R, false> {
        HasOneOrMany {
            query: self.query,
            parent: self.parent,
            foreign_key: self.foreign_key,
            local_key: self.local_key,
            morph: self.morph,
            _parent: PhantomData,
        }
    }
}

impl<P: Model, R: Model> HasOneOrMany<P, R, false> {
    /// Get the related model.
    pub async fn get(&self) -> Result<Option<R>> {
        self.first().await
    }
}

impl<P: Model, R: Model> Relation<P> for HasOneOrMany<P, R, true> {
    type Related = R;
    type Loaded = Vec<R>;

    fn eager_load<'a>(
        self,
        parents: &'a [P],
        spec: EagerSpec,
    ) -> BoxFuture<'a, Result<Vec<Vec<R>>>> {
        let local_key = self.local_key.clone();
        Box::pin(async move {
            let groups = self.eager_groups(parents, spec).await?;
            Ok(parents
                .iter()
                .map(|parent| {
                    key_string(&parent.get_attribute(&local_key))
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

impl<P: Model, R: Model> Relation<P> for HasOneOrMany<P, R, false> {
    type Related = R;
    type Loaded = Option<R>;

    fn eager_load<'a>(
        self,
        parents: &'a [P],
        spec: EagerSpec,
    ) -> BoxFuture<'a, Result<Vec<Option<R>>>> {
        let local_key = self.local_key.clone();
        Box::pin(async move {
            let groups = self.eager_groups(parents, spec).await?;
            Ok(parents
                .iter()
                .map(|parent| {
                    key_string(&parent.get_attribute(&local_key))
                        .and_then(|key| groups.get(&key).and_then(|models| models.first().cloned()))
                })
                .collect())
        })
    }

    fn into_dyn(self) -> Box<dyn DynRelation> {
        Box::new(self)
    }
}

impl<P: Model, R: Model, const MANY: bool> DynRelation for HasOneOrMany<P, R, MANY> {
    fn kind(&self) -> RelationKind {
        if MANY {
            RelationKind::HasMany
        } else {
            RelationKind::HasOne
        }
    }

    fn related_class(&self) -> &'static str {
        R::class_name()
    }

    fn related_table(&self) -> String {
        R::table()
    }

    fn existence_query(&self, parent: &str) -> (QueryBuilder, String) {
        self.dyn_existence(parent)
    }

    fn related_relation(&self, name: &str) -> Option<Box<dyn DynRelation>> {
        R::relation(name)
    }

    fn attributes_for_related(&self, parent: &Map<String, Value>) -> Map<String, Value> {
        let mut relation = self.clone();
        relation.parent = parent.clone();
        relation.foreign_attributes()
    }
}
