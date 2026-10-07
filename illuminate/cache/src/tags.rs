//! Cache tags.

use std::sync::Arc;

use sha1::{Digest, Sha1};

use illuminate_support::{Result, Str, Value, json};

use crate::redis_store::RedisStore;
use crate::store::Store;

/// The key prefix of the items stored under a namespace: the SHA-1 of the
/// tags' ids.
pub(crate) fn namespace_key(namespace: &str) -> String {
    hex::encode(Sha1::digest(namespace.as_bytes()))
}

/// A set of cache tags. Each tag has a random id stored in the cache;
/// flushing a tag gives it a new id, orphaning every item stored under the
/// old one.
///
/// On a Redis store, each tag also records the namespaces it has been part
/// of (in a `tag:{name}:entries` set), so `cache:prune-stale-tags` can
/// delete the items orphaned by a flush — Laravel's tag entries.
#[derive(Clone)]
pub struct TagSet {
    store: Arc<dyn Store>,
    names: Vec<String>,
    redis: Option<RedisStore>,
}

impl std::fmt::Debug for TagSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TagSet")
            .field("names", &self.names)
            .finish()
    }
}

impl TagSet {
    /// Create a new tag set.
    pub fn new(store: Arc<dyn Store>, names: Vec<String>) -> Self {
        let redis = RedisStore::of(&store);
        Self { store, names, redis }
    }

    /// The names of the tags in the set.
    pub fn get_names(&self) -> &[String] {
        &self.names
    }

    /// Reset all tags in the set.
    pub async fn reset(&self) -> Result<()> {
        for name in &self.names {
            self.reset_tag(name).await?;
        }
        Ok(())
    }

    /// Reset the tag, returning its new id.
    pub async fn reset_tag(&self, name: &str) -> Result<String> {
        let id = Str::uuid().simple().to_string();
        self.store.forever(&self.tag_key(name), json!(id)).await?;
        Ok(id)
    }

    /// Flush all the tags in the set.
    pub async fn flush(&self) -> Result<()> {
        for name in &self.names {
            self.flush_tag(name).await?;
        }
        Ok(())
    }

    /// Flush the tag from the cache.
    pub async fn flush_tag(&self, name: &str) -> Result<()> {
        self.store.forget(&self.tag_key(name)).await?;
        Ok(())
    }

    /// A unique namespace that changes when any of the tags are flushed.
    pub async fn get_namespace(&self) -> Result<String> {
        Ok(self.tag_ids().await?.join("|"))
    }

    /// The current id of every tag in the set.
    async fn tag_ids(&self) -> Result<Vec<String>> {
        let mut ids = Vec::with_capacity(self.names.len());
        for name in &self.names {
            ids.push(self.tag_id(name).await?);
        }
        Ok(ids)
    }

    /// The unique tag identifier for a given tag.
    pub async fn tag_id(&self, name: &str) -> Result<String> {
        match self.store.get(&self.tag_key(name)).await? {
            Some(Value::String(id)) if !id.is_empty() => Ok(id),
            _ => self.reset_tag(name).await,
        }
    }

    /// The tag identifier key for a given tag.
    pub fn tag_key(&self, name: &str) -> String {
        format!("tag:{name}:key")
    }

    /// The key a tagged item is stored under.
    pub async fn tagged_item_key(&self, key: &str) -> Result<String> {
        let ids = self.tag_ids().await?;
        if let Some(redis) = &self.redis {
            let tags: Vec<(String, String)> = self.names.iter().cloned().zip(ids.iter().cloned()).collect();
            redis.add_tag_namespace(&tags).await?;
        }
        Ok(format!("{}:{key}", namespace_key(&ids.join("|"))))
    }
}
