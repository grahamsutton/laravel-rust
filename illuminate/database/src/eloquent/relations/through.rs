//! `has_one_through` and `has_many_through`.

use std::marker::PhantomData;

use illuminate_support::{Collection, Map, Result, Value};

use super::{
    DynRelation, EagerSpec, Relation, RelationKind, attribute, collect_keys, group_by_key,
    next_alias,
};
use crate::eloquent::builder::{Builder, finish, hydrate};
use crate::eloquent::model::{Model, key_is_set};
use crate::eloquent::{BoxFuture, key_string};
use crate::query::Builder as QueryBuilder;

const THROUGH_KEY: &str = "laravel_through_key";

/// A relationship to distant models through an intermediate model: a
/// `Country` has many `Post`s through its `User`s.
///
/// `P` is the parent, `R` the related model and `T` the intermediate
/// ("through") model. Use the [`HasOneThrough`] and [`HasManyThrough`]
/// aliases.
pub struct HasOneOrManyThrough<P: Model, R: Model, T: Model, const MANY: bool> {
    pub(crate) query: Builder<R>,
    parent: Map<String, Value>,
    first_key: String,
    second_key: String,
    local_key: String,
    second_local_key: String,
    with_trashed_parents: bool,
    _models: PhantomData<fn() -> (P, T)>,
}

/// A has-one-through relationship (`self.has_one_through::<R, T>()`).
pub type HasOneThrough<P, R, T> = HasOneOrManyThrough<P, R, T, false>;

/// A has-many-through relationship (`self.has_many_through::<R, T>()`).
pub type HasManyThrough<P, R, T> = HasOneOrManyThrough<P, R, T, true>;

impl<P: Model, R: Model, T: Model, const MANY: bool> Clone for HasOneOrManyThrough<P, R, T, MANY> {
    fn clone(&self) -> Self {
        Self {
            query: self.query.clone(),
            parent: self.parent.clone(),
            first_key: self.first_key.clone(),
            second_key: self.second_key.clone(),
            local_key: self.local_key.clone(),
            second_local_key: self.second_local_key.clone(),
            with_trashed_parents: self.with_trashed_parents,
            _models: PhantomData,
        }
    }
}

impl<P: Model, R: Model, T: Model, const MANY: bool> HasOneOrManyThrough<P, R, T, MANY> {
    pub(crate) fn new(parent: &P) -> Self {
        Self {
            query: Builder::new(),
            parent: parent.to_attributes(),
            first_key: P::get_foreign_key(),
            second_key: T::get_foreign_key(),
            local_key: P::primary_key().to_string(),
            second_local_key: T::primary_key().to_string(),
            with_trashed_parents: false,
            _models: PhantomData,
        }
    }

    /// Include related models whose intermediate (soft deleted) model is
    /// trashed.
    pub fn with_trashed_parents(mut self) -> Self {
        self.with_trashed_parents = true;
        self
    }

    /// Whether the intermediate model is soft deletable (Laravel's
    /// `throughParentSoftDeletes`).
    pub fn through_parent_soft_deletes(&self) -> bool {
        T::soft_deletes()
    }

    /// The foreign key on the intermediate model (`country_id` on `users`).
    pub fn first_key(mut self, key: impl Into<String>) -> Self {
        self.first_key = key.into();
        self
    }

    /// The foreign key on the related model (`user_id` on `posts`).
    pub fn second_key(mut self, key: impl Into<String>) -> Self {
        self.second_key = key.into();
        self
    }

    /// The local key on the parent model.
    pub fn local_key(mut self, key: impl Into<String>) -> Self {
        self.local_key = key.into();
        self
    }

    /// The local key on the intermediate model.
    pub fn second_local_key(mut self, key: impl Into<String>) -> Self {
        self.second_local_key = key.into();
        self
    }

    /// The parent's key value.
    pub fn get_parent_key(&self) -> Value {
        attribute(&self.parent, &self.local_key)
    }

    forward_eloquent!(query);

    fn through_table() -> String {
        T::table()
    }

    fn join_through(&self, mut query: Builder<R>) -> Builder<R> {
        let through = Self::through_table();
        let second_key = query.qualify_column(&self.second_key);
        query.query = query.query.join(
            through.as_str(),
            format!("{through}.{}", self.second_local_key),
            "=",
            second_key,
        );
        if T::soft_deletes() && !self.with_trashed_parents {
            query.query = query
                .query
                .where_null(format!("{through}.{}", T::deleted_at_column()));
        }
        query
    }

    /// The relationship query, constrained to the parent.
    pub fn get_query(&self) -> Builder<R> {
        let mut query = self.join_through(self.query.clone());
        if super::constraints_disabled() {
            return query;
        }
        let first_key = format!("{}.{}", Self::through_table(), self.first_key);
        query.query = query.query.where_(first_key, self.get_parent_key());
        query
    }

    async fn fetch_with_keys(&self, mut query: Builder<R>) -> Result<Vec<(R, Value)>> {
        if query.query.columns.is_none() {
            let all = format!("{}.*", query.qualifier);
            query.query = query.query.select(all);
        }
        let through_key = format!(
            "{}.{} as {THROUGH_KEY}",
            Self::through_table(),
            self.first_key
        );
        query.query = query.query.add_select(through_key);
        let (rows, eager) = query.get_rows().await?;
        let mut keys = Vec::with_capacity(rows.len());
        let mut cleaned = Vec::with_capacity(rows.len());
        for mut row in rows {
            keys.push(row.remove(THROUGH_KEY).unwrap_or(Value::Null));
            cleaned.push(row);
        }
        let mut models = hydrate::<R>(cleaned)?;
        finish(&mut models, &eager).await?;
        Ok(models.into_iter().zip(keys).collect())
    }

    async fn fetch(&self, query: Builder<R>) -> Result<Vec<R>> {
        if !key_is_set(&self.get_parent_key()) && !super::constraints_disabled() {
            return Ok(Vec::new());
        }
        Ok(self
            .fetch_with_keys(query)
            .await?
            .into_iter()
            .map(|(model, _)| model)
            .collect())
    }

    relation_queries!();

    async fn eager_groups(
        self,
        parents: &[P],
        spec: EagerSpec,
    ) -> Result<std::collections::HashMap<String, Vec<R>>> {
        let keys = collect_keys(parents, &self.local_key);
        if keys.is_empty() {
            return Ok(Default::default());
        }
        let mut query = self.join_through(self.query.clone());
        query.query = query.query.where_in(
            format!("{}.{}", Self::through_table(), self.first_key),
            keys,
        );
        let query = spec.apply_to(query);
        let related = self.fetch_with_keys(query).await?;
        Ok(group_by_key(
            related
                .into_iter()
                .map(|(model, key)| (key, model))
                .collect(),
        ))
    }
}

impl<P: Model, R: Model, T: Model> HasOneOrManyThrough<P, R, T, true> {
    /// Get the related models.
    pub async fn get(&self) -> Result<Collection<R>> {
        Ok(self.fetch(self.get_query()).await?.into())
    }

    /// The same relationship as a has-one-through relationship.
    pub fn one(self) -> HasOneOrManyThrough<P, R, T, false> {
        HasOneOrManyThrough {
            query: self.query,
            parent: self.parent,
            first_key: self.first_key,
            second_key: self.second_key,
            local_key: self.local_key,
            second_local_key: self.second_local_key,
            with_trashed_parents: self.with_trashed_parents,
            _models: PhantomData,
        }
    }
}

impl<P: Model, R: Model, T: Model> HasOneOrManyThrough<P, R, T, false> {
    /// Get the related model.
    pub async fn get(&self) -> Result<Option<R>> {
        self.first().await
    }
}

impl<P: Model, R: Model, T: Model> Relation<P> for HasOneOrManyThrough<P, R, T, true> {
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

impl<P: Model, R: Model, T: Model> Relation<P> for HasOneOrManyThrough<P, R, T, false> {
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

impl<P: Model, R: Model, T: Model, const MANY: bool> DynRelation
    for HasOneOrManyThrough<P, R, T, MANY>
{
    fn kind(&self) -> RelationKind {
        if MANY {
            RelationKind::HasManyThrough
        } else {
            RelationKind::HasOneThrough
        }
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
        let mut query = self.join_through(query);
        query.query = query.query.where_column(
            format!("{}.{}", Self::through_table(), self.first_key),
            format!("{parent}.{}", self.local_key),
        );
        let qualifier = query.qualifier.clone();
        (query.to_base(), qualifier)
    }

    fn related_relation(&self, name: &str) -> Option<Box<dyn DynRelation>> {
        R::relation(name)
    }
}
