//! Eloquent collections: extra methods for collections of models.

use std::future::Future;

use illuminate_support::{Collection, Result, Value};

use super::builder::Builder;
use super::model::Model;
use super::relations::{EagerSpec, collect_keys};
use super::{IntoIds, IntoRelations, key_string};

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
    let keys = collect_keys(models, M::primary_key());
    if keys.is_empty() || relations.is_empty() {
        return Ok(());
    }
    let query = M::query().without_global_scopes();
    let key = query.qualify_column(M::primary_key());
    let rows = query
        .select(key)
        .with_count(relations)
        .where_key(Value::Array(keys))
        .to_base()
        .get()
        .await?;
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
