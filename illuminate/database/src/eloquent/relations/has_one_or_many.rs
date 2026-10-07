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
        relation.morph = Some((format!("{name}_type"), P::morph_class()));
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
        if !key_is_set(&self.get_parent_key()) {
            return Ok(Vec::new());
        }
        Ok(query.get().await?.into_vec())
    }

    relation_queries!();

    /// Set the foreign key (and morph type) on a related model.
    pub fn set_foreign_attributes(&self, model: &mut R) -> Result<()> {
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
