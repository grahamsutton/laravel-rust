//! Eloquent collections: extra methods for collections of models.

use std::future::Future;

use illuminate_support::{Collection, Result, Value};

use super::builder::Builder;
use super::model::Model;
use super::relations::{EagerSpec, collect_keys};
use super::{IntoAttributeNames, IntoIds, IntoRelations, key_string};

/// Methods for collections of models (Laravel's `Eloquent\Collection`).
///
/// ```ignore
/// use illuminate_database::eloquent::EloquentCollection;
///
/// let mut users = User::all().await?;
/// users.load("posts").await?;
/// let taylor = users.find(1);
/// ```
pub trait EloquentCollection<M: Model> {
    /// The models' primary keys.
    fn model_keys(&self) -> Vec<Value>;

    /// Find a model by its primary key.
    fn find(&self, key: impl Into<Value>) -> Option<&M>;

    /// Whether a model with the primary key is present.
    fn contains_key(&self, key: impl Into<Value>) -> bool;

    /// The models whose keys are not in the list.
    fn except_keys(self, keys: impl IntoIds) -> Collection<M>;

    /// Only the models whose keys are in the list.
    fn only_keys(self, keys: impl IntoIds) -> Collection<M>;

    /// Eager load relationships onto every model with one query per
    /// relationship.
    fn load(&mut self, relations: impl IntoRelations) -> impl Future<Output = Result<()>> + Send;

    /// Eager load relationships that aren't loaded on every model yet.
    fn load_missing(
        &mut self,
        relations: impl IntoRelations,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Load `{relation}_count` attributes onto every model.
    fn load_count(
        &mut self,
        relations: impl IntoRelations,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Load `{relation}_{function}_{column}` aggregate attributes (`sum`,
    /// `avg`, `min`, `max`, `count`, `exists`) onto every model with one
    /// query.
    fn load_aggregate(
        &mut self,
        relations: impl IntoRelations,
        column: &str,
        function: &str,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Load `{relation}_sum_{column}` attributes onto every model.
    fn load_sum(
        &mut self,
        relations: impl IntoRelations,
        column: &str,
    ) -> impl Future<Output = Result<()>> + Send {
        self.load_aggregate(relations, column, "sum")
    }

    /// Load `{relation}_avg_{column}` attributes onto every model.
    fn load_avg(
        &mut self,
        relations: impl IntoRelations,
        column: &str,
    ) -> impl Future<Output = Result<()>> + Send {
        self.load_aggregate(relations, column, "avg")
    }

    /// Load `{relation}_min_{column}` attributes onto every model.
    fn load_min(
        &mut self,
        relations: impl IntoRelations,
        column: &str,
    ) -> impl Future<Output = Result<()>> + Send {
        self.load_aggregate(relations, column, "min")
    }

    /// Load `{relation}_max_{column}` attributes onto every model.
    fn load_max(
        &mut self,
        relations: impl IntoRelations,
        column: &str,
    ) -> impl Future<Output = Result<()>> + Send {
        self.load_aggregate(relations, column, "max")
    }

    /// Load `{relation}_exists` attributes onto every model.
    fn load_exists(
        &mut self,
        relations: impl IntoRelations,
    ) -> impl Future<Output = Result<()>> + Send {
        self.load_aggregate(relations, "*", "exists")
    }

    /// Eager load a polymorphic (`morph_to`) relationship and the given
    /// relationships of the models it points to.
    fn load_morph(
        &mut self,
        relation: &str,
        relations: impl IntoRelations,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Eager load a polymorphic relationship with relationship counts on
    /// the models it points to.
    fn load_morph_count(
        &mut self,
        relation: &str,
        relations: impl IntoRelations,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Hide the given attributes on every model (models with an
    /// [`Original`](super::Original) field).
    fn make_hidden(&mut self, attributes: impl IntoAttributeNames) -> &mut Self;

    /// Make the given attributes visible on every model (models with an
    /// [`Original`](super::Original) field).
    fn make_visible(&mut self, attributes: impl IntoAttributeNames) -> &mut Self;

    /// Every model without its loaded relationships.
    fn without_relations(&self) -> Collection<M>;

    /// Reload every model from the database (models that no longer exist
    /// are dropped).
    fn fresh(&self) -> impl Future<Output = Result<Collection<M>>> + Send;

    /// A query for the models in the collection.
    fn to_query(&self) -> Builder<M>;
}

impl<M: Model> EloquentCollection<M> for Collection<M> {
    fn model_keys(&self) -> Vec<Value> {
        self.iter().map(|model| model.get_key()).collect()
    }

    fn find(&self, key: impl Into<Value>) -> Option<&M> {
        let key = key_string(&key.into());
        self.iter()
            .find(|model| key.is_some() && key_string(&model.get_key()) == key)
    }

    fn contains_key(&self, key: impl Into<Value>) -> bool {
        EloquentCollection::find(self, key).is_some()
    }

    fn except_keys(self, keys: impl IntoIds) -> Collection<M> {
        let keys: Vec<Option<String>> = keys.into_ids().iter().map(key_string).collect();
        self.into_iter()
            .filter(|model| !keys.contains(&key_string(&model.get_key())))
            .collect()
    }

    fn only_keys(self, keys: impl IntoIds) -> Collection<M> {
        let keys: Vec<Option<String>> = keys.into_ids().iter().map(key_string).collect();
        self.into_iter()
            .filter(|model| keys.contains(&key_string(&model.get_key())))
            .collect()
    }

    fn load(&mut self, relations: impl IntoRelations) -> impl Future<Output = Result<()>> + Send {
        let tree = EagerSpec::tree(relations.into_relations());
        async move {
            for (name, spec) in tree {
                M::eager_load(self, &name, spec).await?;
            }
            Ok(())
        }
    }

    fn load_missing(
        &mut self,
        relations: impl IntoRelations,
    ) -> impl Future<Output = Result<()>> + Send {
        let tree: Vec<(String, EagerSpec)> = EagerSpec::tree(relations.into_relations())
            .into_iter()
            .filter(|(name, _)| {
                self.iter()
                    .any(|model| !model.loaded_relations().contains(&name.as_str()))
            })
            .collect();
        async move {
            for (name, spec) in tree {
                M::eager_load(self, &name, spec).await?;
            }
            Ok(())
        }
    }

    fn load_count(
        &mut self,
        relations: impl IntoRelations,
    ) -> impl Future<Output = Result<()>> + Send {
        let relations = relations.into_relations();
        async move { load_counts(self, relations).await }
    }

    fn load_aggregate(
        &mut self,
        relations: impl IntoRelations,
        column: &str,
        function: &str,
    ) -> impl Future<Output = Result<()>> + Send {
        let relations = relations.into_relations();
        let (column, function) = (column.to_string(), function.to_string());
        async move { load_aggregates(self, relations, &column, &function).await }
    }

    fn load_morph(
        &mut self,
        relation: &str,
        relations: impl IntoRelations,
    ) -> impl Future<Output = Result<()>> + Send {
        let tree = EagerSpec::morph_tree(relation, relations.into_relations());
        async move {
            for (name, spec) in tree {
                M::eager_load(self, &name, spec).await?;
            }
            Ok(())
        }
    }

    fn load_morph_count(
        &mut self,
        relation: &str,
        relations: impl IntoRelations,
    ) -> impl Future<Output = Result<()>> + Send {
        let tree =
            EagerSpec::morph_aggregate_tree(relation, relations.into_relations(), "*", "count");
        async move {
            for (name, spec) in tree {
                M::eager_load(self, &name, spec).await?;
            }
            Ok(())
        }
    }

    fn make_hidden(&mut self, attributes: impl IntoAttributeNames) -> &mut Self {
        let attributes = attributes.into_attribute_names();
        for model in self.iter_mut() {
            model.make_hidden(attributes.clone());
        }
        self
    }

    fn make_visible(&mut self, attributes: impl IntoAttributeNames) -> &mut Self {
        let attributes = attributes.into_attribute_names();
        for model in self.iter_mut() {
            model.make_visible(attributes.clone());
        }
        self
    }

    fn without_relations(&self) -> Collection<M> {
        self.iter().map(|model| model.without_relations()).collect()
    }

    fn fresh(&self) -> impl Future<Output = Result<Collection<M>>> + Send {
        let keys = self.model_keys();
        async move {
            let fresh = M::query()
                .without_global_scopes()
                .find_many(keys.clone())
                .await?;
            Ok(keys
                .iter()
                .filter_map(|key| EloquentCollection::find(&fresh, key.clone()).cloned())
                .collect())
        }
    }

    fn to_query(&self) -> Builder<M> {
        M::query().where_key(Value::Array(self.model_keys()))
    }
}

/// Load `{relation}_count` (or aliased) attributes onto the models with one
/// query.
pub(crate) async fn load_counts<M: Model>(models: &mut [M], relations: Vec<String>) -> Result<()> {
    load_aggregates(models, relations, "*", "count").await
}

/// Load relationship aggregate attributes (`{relation}_{function}_{column}`)
/// onto the models with one query.
pub(crate) async fn load_aggregates<M: Model>(
    models: &mut [M],
    relations: Vec<String>,
    column: &str,
    function: &str,
) -> Result<()> {
    let keys = collect_keys(models, M::primary_key());
    if keys.is_empty() || relations.is_empty() {
        return Ok(());
    }
    let query = M::query().without_global_scopes();
    let key = query.qualify_column(M::primary_key());
    let mut query = query.select(key);
    for relation in &relations {
        query = query.with_aggregate(relation, column, function, |query| query);
    }
    let rows = query.where_key(Value::Array(keys)).to_base().get().await?;
    for row in rows {
        let Value::Object(row) = row else { continue };
        let row_key = key_string(row.get(M::primary_key()).unwrap_or(&Value::Null));
        for model in models
            .iter_mut()
            .filter(|model| key_string(&model.get_key()) == row_key)
        {
            for (column, value) in &row {
                if column != M::primary_key() {
                    model.set_attribute(column, value.clone())?;
                }
            }
        }
    }
    Ok(())
}
