//! `belongs_to` and `morph_to`.

use std::marker::PhantomData;

use illuminate_support::{Map, Result, Value, ValueExt};

use super::{
    DynRelation, EagerSpec, Relation, RelationKind, attribute, collect_keys, group_by_key,
    next_alias,
};
use crate::eloquent::builder::Builder;
use crate::eloquent::model::{Model, key_is_set};
use crate::eloquent::{BoxFuture, key_string};
use crate::query::Builder as QueryBuilder;

/// The inverse of a one-to-one or one-to-many relationship: the model `P`
/// holds a foreign key to the related model `R`.
pub struct BelongsTo<P: Model, R: Model> {
    pub(crate) query: Builder<R>,
    child: Map<String, Value>,
    foreign_key: String,
    owner_key: String,
    morph_type: Option<String>,
    _child: PhantomData<fn() -> P>,
}

/// The inverse of a polymorphic relationship (`self.morph_to::<Post>("commentable")`).
///
/// Rust models are statically typed, so a `MorphTo` loads one related type:
/// define one relationship per type (`commentable_post`, `commentable_video`).
pub type MorphTo<P, R> = BelongsTo<P, R>;

impl<P: Model, R: Model> Clone for BelongsTo<P, R> {
    fn clone(&self) -> Self {
        Self {
            query: self.query.clone(),
            child: self.child.clone(),
            foreign_key: self.foreign_key.clone(),
            owner_key: self.owner_key.clone(),
            morph_type: self.morph_type.clone(),
            _child: PhantomData,
        }
    }
}

impl<P: Model, R: Model> BelongsTo<P, R> {
    pub(crate) fn new(child: &P, foreign_key: String, owner_key: String) -> Self {
        Self {
            query: Builder::new(),
            child: child.to_attributes(),
            foreign_key,
            owner_key,
            morph_type: None,
            _child: PhantomData,
        }
    }

    pub(crate) fn morph(child: &P, name: &str) -> Self {
        let mut relation = Self::new(child, format!("{name}_id"), R::primary_key().to_string());
        relation.morph_type = Some(format!("{name}_type"));
        relation
    }

    /// Use a different foreign key on the child model (`author_id`).
    pub fn foreign_key(mut self, key: impl Into<String>) -> Self {
        self.foreign_key = key.into();
        self
    }

    /// Use a different key on the related (owner) model.
    pub fn owner_key(mut self, key: impl Into<String>) -> Self {
        self.owner_key = key.into();
        self
    }

    /// The foreign key on the child model.
    pub fn get_foreign_key_name(&self) -> &str {
        &self.foreign_key
    }

    /// The key on the related model.
    pub fn get_owner_key_name(&self) -> &str {
        &self.owner_key
    }

    /// Whether a child row's morph type points at the related model.
    fn matches_type(&self, attributes: impl Fn(&str) -> Value) -> bool {
        match &self.morph_type {
            Some(column) => attributes(column).to_string_lossy() == R::morph_class(),
            None => true,
        }
    }

    forward_eloquent!(query);

    /// The relationship query, constrained to the child's foreign key.
    pub fn get_query(&self) -> Builder<R> {
        let mut query = self.query.clone();
        let owner_key = query.qualify_column(&self.owner_key);
        query.query = query
            .query
            .where_(owner_key, attribute(&self.child, &self.foreign_key));
        query
    }

    async fn fetch(&self, query: Builder<R>) -> Result<Vec<R>> {
        let key = attribute(&self.child, &self.foreign_key);
        if !key_is_set(&key) || !self.matches_type(|column| attribute(&self.child, column)) {
            return Ok(Vec::new());
        }
        Ok(query.get().await?.into_vec())
    }

    relation_queries!();

    /// Get the related model.
    pub async fn get(&self) -> Result<Option<R>> {
        self.first().await
    }

    /// Associate the child model with a parent model (sets the foreign key;
    /// save the child afterwards).
    ///
    /// ```ignore
    /// post.user().associate(&mut post, &user)?;
    /// post.save().await?;
    /// ```
    pub fn associate(&self, child: &mut P, parent: &R) -> Result<()> {
        child.set_attribute(&self.foreign_key, parent.get_attribute(&self.owner_key))?;
        if let Some(column) = &self.morph_type {
            child.set_attribute(column, Value::String(R::morph_class()))?;
        }
        Ok(())
    }

    /// Dissociate the child model from its parent (clears the foreign key).
    pub fn dissociate(&self, child: &mut P) -> Result<()> {
        child.set_attribute(&self.foreign_key, Value::Null)?;
        if let Some(column) = &self.morph_type {
            child.set_attribute(column, Value::Null)?;
        }
        Ok(())
    }

    /// Update the related model (touching `updated_at`).
    pub async fn update(&self, values: impl Into<crate::eloquent::Attributes>) -> Result<u64> {
        self.get_query().update(values).await
    }
}

impl<P: Model, R: Model> Relation<P> for BelongsTo<P, R> {
    type Related = R;
    type Loaded = Option<R>;

    fn eager_load<'a>(
        self,
        parents: &'a [P],
        spec: EagerSpec,
    ) -> BoxFuture<'a, Result<Vec<Option<R>>>> {
        Box::pin(async move {
            let candidates: Vec<P> = parents
                .iter()
                .filter(|parent| self.matches_type(|column| parent.get_attribute(column)))
                .cloned()
                .collect();
            let keys = collect_keys(&candidates, &self.foreign_key);
            if keys.is_empty() {
                return Ok(vec![None; parents.len()]);
            }
            let mut query = self.query.clone();
            let owner_key = query.qualify_column(&self.owner_key);
            query.query = query.query.where_in(owner_key, keys);
            let related = spec.apply_to(query).get().await?.into_vec();
            let groups = group_by_key(
                related
                    .into_iter()
                    .map(|model| (model.get_attribute(&self.owner_key), model))
                    .collect(),
            );
            Ok(parents
                .iter()
                .map(|parent| {
                    if !self.matches_type(|column| parent.get_attribute(column)) {
                        return None;
                    }
                    key_string(&parent.get_attribute(&self.foreign_key))
                        .and_then(|key| groups.get(&key).and_then(|models| models.first().cloned()))
                })
                .collect())
        })
    }

    fn into_dyn(self) -> Box<dyn DynRelation> {
        Box::new(self)
    }
}

impl<P: Model, R: Model> DynRelation for BelongsTo<P, R> {
    fn kind(&self) -> RelationKind {
        RelationKind::BelongsTo
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
        let owner_key = query.qualify_column(&self.owner_key);
        query.query = query
            .query
            .where_column(owner_key, format!("{parent}.{}", self.foreign_key));
        if let Some(column) = &self.morph_type {
            query.query = query
                .query
                .where_(format!("{parent}.{column}"), R::morph_class());
        }
        let qualifier = query.qualifier.clone();
        (query.to_base(), qualifier)
    }

    fn related_relation(&self, name: &str) -> Option<Box<dyn DynRelation>> {
        R::relation(name)
    }

    fn attributes_for_parent(&self, related: &Map<String, Value>) -> Map<String, Value> {
        let mut attributes = Map::new();
        attributes.insert(
            self.foreign_key.clone(),
            attribute(related, &self.owner_key),
        );
        if let Some(column) = &self.morph_type {
            attributes.insert(column.clone(), Value::String(R::morph_class()));
        }
        attributes
    }
}
