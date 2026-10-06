//! The cache repository: the friendly API in front of every cache store.

use std::future::Future;
use std::sync::{Arc, RwLock};

use indexmap::IndexMap;
use serde::Serialize;
use serde::de::DeserializeOwned;

use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Carbon, Result, Value, ValueExt, cast};

use crate::lock::{Lock, UnsupportedLock};
use crate::store::{BadMethodCallException, Store};
use crate::tags::TagSet;
use crate::ttl::Ttl;

/// The cache key prefix used to track when a flexible cache value was last refreshed.
pub const FLEXIBLE_CREATED_KEY_PREFIX: &str = "illuminate:cache:flexible:created:";

/// A cache repository.
///
/// Repositories are cheap to clone: clones share the same store.
///
/// ```
/// use illuminate_cache::{ArrayStore, Repository};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let cache = Repository::new(ArrayStore::new());
///
/// cache.put("name", "Taylor", 600).await.unwrap();
///
/// assert_eq!(cache.get_as::<String>("name").await.unwrap().as_deref(), Some("Taylor"));
/// assert!(cache.has("name").await.unwrap());
///
/// let users = cache.remember("users", 60, || async { Ok(vec!["Taylor", "Abigail"]) }).await.unwrap();
/// assert_eq!(users, vec!["Taylor", "Abigail"]);
/// # });
/// ```
#[derive(Clone)]
pub struct Repository {
    store: Arc<dyn Store>,
    name: Option<String>,
    default: Arc<RwLock<Option<u64>>>,
    tags: Option<TagSet>,
}

impl std::fmt::Debug for Repository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repository").field("name", &self.name).field("tags", &self.tags).finish()
    }
}

/// Convert a value into the cache's [`Value`] representation.
fn to_cache_value<T: Serialize + ?Sized>(value: &T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

/// The PHP type name of a value, for error messages.
fn php_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "NULL",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_f64() => "double",
        Value::Number(_) => "integer",
        Value::String(_) => "string",
        Value::Array(_) | Value::Object(_) => "array",
    }
}

impl Repository {
    /// Create a new cache repository around the given store.
    pub fn new(store: impl Store) -> Self {
        Self::from_arc(Arc::new(store))
    }

    /// Create a new cache repository around a shared store.
    pub fn from_arc(store: Arc<dyn Store>) -> Self {
        Self { store, name: None, default: Arc::new(RwLock::new(Some(3600))), tags: None }
    }

    /// Name the repository (the name of its store in the configuration).
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The name of the cache store.
    pub fn get_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The cache store implementation.
    pub fn get_store(&self) -> Arc<dyn Store> {
        self.store.clone()
    }

    /// The default cache time, in seconds.
    pub fn get_default_cache_time(&self) -> Option<u64> {
        *self.default.read().unwrap()
    }

    /// Set the default cache time, in seconds.
    pub fn set_default_cache_time(&self, seconds: Option<u64>) -> &Self {
        *self.default.write().unwrap() = seconds;
        self
    }

    /// The key an item is stored under (tagged caches namespace their keys).
    async fn item_key(&self, key: &str) -> Result<String> {
        match &self.tags {
            Some(tags) => tags.tagged_item_key(key).await,
            None => Ok(key.to_string()),
        }
    }

    // ------------------------------------------------------------------
    // Retrieving items
    // ------------------------------------------------------------------

    /// Determine if an item exists in the cache.
    pub async fn has(&self, key: &str) -> Result<bool> {
        Ok(self.get(key).await?.is_some())
    }

    /// Determine if an item doesn't exist in the cache.
    pub async fn missing(&self, key: &str) -> Result<bool> {
        Ok(!self.has(key).await?)
    }

    /// Retrieve an item from the cache by key.
    pub async fn get(&self, key: &str) -> Result<Option<Value>> {
        let value = self.store.get(&self.item_key(key).await?).await?;
        Ok(value.filter(|value| !value.is_null()))
    }

    /// Retrieve an item from the cache, deserialized into a type.
    pub async fn get_as<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        match self.get(key).await? {
            Some(value) => Ok(Some(cast(value)?)),
            None => Ok(None),
        }
    }

    /// Retrieve an item from the cache, or the given default.
    pub async fn get_or(&self, key: &str, default: impl Into<Value>) -> Result<Value> {
        Ok(self.get(key).await?.unwrap_or_else(|| default.into()))
    }

    /// Retrieve an item from the cache, or compute a default with a closure.
    pub async fn get_or_else<V: Into<Value>>(&self, key: &str, default: impl FnOnce() -> V) -> Result<Value> {
        Ok(self.get(key).await?.unwrap_or_else(|| default().into()))
    }

    /// Retrieve multiple items from the cache by key. Missing items are `None`.
    pub async fn many<K: AsRef<str>>(&self, keys: impl IntoIterator<Item = K>) -> Result<IndexMap<String, Option<Value>>> {
        let keys: Vec<String> = keys.into_iter().map(|k| k.as_ref().to_string()).collect();
        let mut item_keys = Vec::with_capacity(keys.len());
        for key in &keys {
            item_keys.push(self.item_key(key).await?);
        }
        let values = self.store.many(&item_keys).await?;
        Ok(keys
            .into_iter()
            .zip(values)
            .map(|(key, (_, value))| (key, value.filter(|value| !value.is_null())))
            .collect())
    }

    /// Retrieve an item from the cache and delete it.
    pub async fn pull(&self, key: &str) -> Result<Option<Value>> {
        let value = self.get(key).await?;
        self.forget(key).await?;
        Ok(value)
    }

    /// Retrieve an item, deserialized into a type, and delete it.
    pub async fn pull_as<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let value = self.get_as(key).await?;
        self.forget(key).await?;
        Ok(value)
    }

    /// Retrieve a string item from the cache.
    pub async fn string(&self, key: &str) -> Result<String> {
        match self.get(key).await?.unwrap_or(Value::Null) {
            Value::String(value) => Ok(value),
            other => Err(type_error(key, "a string", &other)),
        }
    }

    /// Retrieve an integer item from the cache.
    pub async fn integer(&self, key: &str) -> Result<i64> {
        match self.get(key).await?.unwrap_or(Value::Null) {
            Value::Number(n) if n.is_i64() || n.is_u64() => n.as_i64().ok_or_else(|| type_error(key, "an integer", &Value::Number(n))),
            Value::String(s) if s.trim().parse::<i64>().is_ok() => Ok(s.trim().parse::<i64>()?),
            other => Err(type_error(key, "an integer", &other)),
        }
    }

    /// Retrieve a float item from the cache.
    pub async fn float(&self, key: &str) -> Result<f64> {
        match self.get(key).await?.unwrap_or(Value::Null) {
            Value::Number(n) => n.as_f64().ok_or_else(|| type_error(key, "a float", &Value::Number(n))),
            Value::String(s) if s.trim().parse::<f64>().is_ok() => Ok(s.trim().parse::<f64>()?),
            other => Err(type_error(key, "a float", &other)),
        }
    }

    /// Retrieve a boolean item from the cache.
    pub async fn boolean(&self, key: &str) -> Result<bool> {
        match self.get(key).await?.unwrap_or(Value::Null) {
            Value::Bool(value) => Ok(value),
            other => Err(type_error(key, "a boolean", &other)),
        }
    }

    /// Retrieve an array item from the cache.
    pub async fn array(&self, key: &str) -> Result<Value> {
        match self.get(key).await?.unwrap_or(Value::Null) {
            value @ (Value::Array(_) | Value::Object(_)) => Ok(value),
            other => Err(type_error(key, "an array", &other)),
        }
    }

    // ------------------------------------------------------------------
    // Storing items
    // ------------------------------------------------------------------

    /// Store an item in the cache.
    ///
    /// The TTL may be seconds, a `Duration`, a `CarbonInterval`, an absolute
    /// `Carbon`, or `None` (forever). A TTL that has already passed removes
    /// the item instead.
    pub async fn put(&self, key: &str, value: impl Serialize, ttl: impl Into<Ttl>) -> Result<bool> {
        let value = to_cache_value(&value)?;
        match ttl.into().to_seconds() {
            None => self.forever_value(key, value).await,
            Some(seconds) if seconds <= 0 => self.forget(key).await,
            Some(seconds) => self.store.put(&self.item_key(key).await?, value, seconds as u64).await,
        }
    }

    /// Alias of [`Repository::put`].
    pub async fn set(&self, key: &str, value: impl Serialize, ttl: impl Into<Ttl>) -> Result<bool> {
        self.put(key, value, ttl).await
    }

    /// Store multiple items in the cache.
    pub async fn put_many<K, V>(&self, values: impl IntoIterator<Item = (K, V)>, ttl: impl Into<Ttl>) -> Result<bool>
    where
        K: AsRef<str>,
        V: Serialize,
    {
        let mut items = Vec::new();
        for (key, value) in values {
            items.push((key.as_ref().to_string(), to_cache_value(&value)?));
        }
        match ttl.into().to_seconds() {
            None => {
                let mut result = true;
                for (key, value) in items {
                    result = self.forever_value(&key, value).await? && result;
                }
                Ok(result)
            }
            Some(seconds) if seconds <= 0 => {
                let mut result = true;
                for (key, _) in items {
                    result = self.forget(&key).await? && result;
                }
                Ok(result)
            }
            Some(seconds) => {
                let mut keyed = Vec::with_capacity(items.len());
                for (key, value) in items {
                    keyed.push((self.item_key(&key).await?, value));
                }
                self.store.put_many(keyed, seconds as u64).await
            }
        }
    }

    /// Store an item in the cache if the key does not exist. Atomic when
    /// the store supports it (the array and file stores do).
    pub async fn add(&self, key: &str, value: impl Serialize, ttl: impl Into<Ttl>) -> Result<bool> {
        let seconds = match ttl.into().to_seconds() {
            Some(seconds) if seconds <= 0 => return Ok(false),
            Some(seconds) => seconds as u64,
            None => 0,
        };
        self.store.add(&self.item_key(key).await?, to_cache_value(&value)?, seconds).await
    }

    /// Increment the value of an item in the cache by one.
    pub async fn increment(&self, key: &str) -> Result<i64> {
        self.increment_by(key, 1).await
    }

    /// Increment the value of an item in the cache by the given amount.
    pub async fn increment_by(&self, key: &str, amount: i64) -> Result<i64> {
        self.store.increment(&self.item_key(key).await?, amount).await
    }

    /// Decrement the value of an item in the cache by one.
    pub async fn decrement(&self, key: &str) -> Result<i64> {
        self.decrement_by(key, 1).await
    }

    /// Decrement the value of an item in the cache by the given amount.
    pub async fn decrement_by(&self, key: &str, amount: i64) -> Result<i64> {
        self.store.decrement(&self.item_key(key).await?, amount).await
    }

    /// Store an item in the cache indefinitely.
    pub async fn forever(&self, key: &str, value: impl Serialize) -> Result<bool> {
        self.forever_value(key, to_cache_value(&value)?).await
    }

    async fn forever_value(&self, key: &str, value: Value) -> Result<bool> {
        self.store.forever(&self.item_key(key).await?, value).await
    }

    /// Get an item from the cache, or execute the callback and store its result.
    pub async fn remember<T, F, Fut>(&self, key: &str, ttl: impl Into<Ttl>, callback: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        Ok(self.remember_with_warmth(key, ttl, callback).await?.0)
    }

    /// Like [`Repository::remember`], also returning whether the value was
    /// already in the cache ("warm").
    pub async fn remember_with_warmth<T, F, Fut>(&self, key: &str, ttl: impl Into<Ttl>, callback: F) -> Result<(T, bool)>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        if let Some(value) = self.get(key).await? {
            return Ok((cast(value)?, true));
        }
        let value = callback().await?;
        self.put(key, &value, ttl).await?;
        Ok((value, false))
    }

    /// Get an item from the cache, or execute the callback and store its result forever.
    pub async fn remember_forever<T, F, Fut>(&self, key: &str, callback: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        if let Some(value) = self.get(key).await? {
            return Ok(cast(value)?);
        }
        let value = callback().await?;
        self.forever(key, &value).await?;
        Ok(value)
    }

    /// Alias of [`Repository::remember_forever`].
    pub async fn sear<T, F, Fut>(&self, key: &str, callback: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.remember_forever(key, callback).await
    }

    /// Retrieve an item, refreshing it in the background once it is stale
    /// ("stale while revalidate").
    ///
    /// Within the `fresh` period the cached value is returned as-is. Between
    /// `fresh` and `stale`, the stale value is returned immediately while a
    /// background task recomputes it. After `stale`, the value is recomputed
    /// before returning.
    ///
    /// ```
    /// use illuminate_cache::{ArrayStore, Repository};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let cache = Repository::new(ArrayStore::new());
    ///
    /// let users = cache.flexible("users", (5, 10), || async { Ok(vec!["Taylor"]) }).await.unwrap();
    ///
    /// assert_eq!(users, vec!["Taylor"]);
    /// # });
    /// ```
    pub async fn flexible<T, F, Fut>(&self, key: &str, ttl: (impl Into<Ttl>, impl Into<Ttl>), callback: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        let (fresh, stale) = (ttl.0.into(), ttl.1.into());
        let created_key = format!("{FLEXIBLE_CREATED_KEY_PREFIX}{key}");
        let mut values = self.many([key, created_key.as_str()]).await?;
        let value = values.shift_remove(key).flatten();
        let created = values.shift_remove(&created_key).flatten().and_then(|c| c.as_i64());

        let (Some(value), Some(created)) = (value, created) else {
            let value = callback().await?;
            let entries = [(key.to_string(), to_cache_value(&value)?), (created_key, Value::from(Carbon::now().timestamp()))];
            self.put_many(entries, stale).await?;
            return Ok(value);
        };

        let fresh_seconds = fresh.to_seconds().unwrap_or(i64::MAX);
        if created.saturating_add(fresh_seconds) > Carbon::now().timestamp() {
            return Ok(cast(value)?);
        }

        let repository = self.clone();
        let key = key.to_string();
        let lock_name = format!("illuminate:cache:flexible:lock:{}", self.item_key(&key).await?);
        let refresh = async move {
            let lock = repository.store.lock_provider().map(|provider| provider.lock(&lock_name, 0, None));
            let refresh = async {
                let current = repository.get(&created_key).await?.and_then(|c| c.as_i64());
                if current != Some(created) {
                    return Ok::<(), illuminate_support::Error>(());
                }
                let value = callback().await?;
                let entries = [(key.clone(), to_cache_value(&value)?), (created_key.clone(), Value::from(Carbon::now().timestamp()))];
                repository.put_many(entries, stale).await?;
                Ok(())
            };
            match lock {
                Some(lock) => lock.get_with(|| refresh).await?.unwrap_or(Ok(())),
                None => refresh.await,
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(refresh);
            }
            Err(_) => refresh.await?,
        }

        Ok(cast(value)?)
    }

    /// Set the expiration of a cached item.
    pub async fn touch(&self, key: &str, ttl: impl Into<Ttl>) -> Result<bool> {
        match ttl.into().to_seconds() {
            None => Ok(false),
            Some(seconds) if seconds <= 0 => self.forget(key).await,
            Some(seconds) => self.store.touch(&self.item_key(key).await?, seconds as u64).await,
        }
    }

    // ------------------------------------------------------------------
    // Removing items
    // ------------------------------------------------------------------

    /// Remove an item from the cache.
    pub async fn forget(&self, key: &str) -> Result<bool> {
        self.store.forget(&self.item_key(key).await?).await
    }

    /// Alias of [`Repository::forget`].
    pub async fn delete(&self, key: &str) -> Result<bool> {
        self.forget(key).await
    }

    /// Remove all items from the cache. On a tagged cache, only the items
    /// stored under the tags are removed.
    pub async fn flush(&self) -> Result<bool> {
        match &self.tags {
            Some(tags) => {
                tags.reset().await?;
                Ok(true)
            }
            None => self.store.flush().await,
        }
    }

    /// Alias of [`Repository::flush`].
    pub async fn clear(&self) -> Result<bool> {
        self.flush().await
    }

    /// Remove all locks from the cache store.
    pub async fn flush_locks(&self) -> Result<bool> {
        self.store.flush_locks().await
    }

    // ------------------------------------------------------------------
    // Locks
    // ------------------------------------------------------------------

    /// Get a lock instance.
    ///
    /// Stores without lock support hand out a lock whose every operation
    /// fails with a [`BadMethodCallException`].
    pub fn lock(&self, name: &str, seconds: u64) -> Lock {
        self.lock_with_owner(name, seconds, None)
    }

    /// Get a lock instance with a specific owner token.
    pub fn lock_with_owner(&self, name: &str, seconds: u64, owner: Option<String>) -> Lock {
        match self.store.lock_provider() {
            Some(provider) => provider.lock(name, seconds, owner),
            None => Lock::new(Arc::new(UnsupportedLock), name, seconds, owner),
        }
    }

    /// Restore a lock instance using the owner identifier.
    pub fn restore_lock(&self, name: &str, owner: &str) -> Lock {
        match self.store.lock_provider() {
            Some(provider) => provider.restore_lock(name, owner),
            None => Lock::new(Arc::new(UnsupportedLock), name, 0, Some(owner.to_string())),
        }
    }

    /// Run the callback while holding a lock, waiting up to `wait_for`
    /// seconds for it, so that calls never overlap.
    pub async fn without_overlapping<T, F, Fut>(&self, key: &str, callback: F, lock_for: u64, wait_for: u64) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        self.lock(key, lock_for).block_with(wait_for, callback).await
    }

    // ------------------------------------------------------------------
    // Tags
    // ------------------------------------------------------------------

    /// Determine if the store supports tags.
    pub fn supports_tags(&self) -> bool {
        self.store.supports_tags()
    }

    /// Begin a tagged cache operation.
    ///
    /// ```
    /// use illuminate_cache::{ArrayStore, Repository};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let cache = Repository::new(ArrayStore::new());
    ///
    /// cache.tags(["people", "artists"]).unwrap().put("John", "Lennon", 60).await.unwrap();
    /// cache.tags(["people", "authors"]).unwrap().put("Anne", "Rice", 60).await.unwrap();
    ///
    /// cache.tags(["authors"]).unwrap().flush().await.unwrap();
    ///
    /// assert!(cache.tags(["people", "artists"]).unwrap().has("John").await.unwrap());
    /// assert!(cache.tags(["people", "authors"]).unwrap().missing("Anne").await.unwrap());
    /// # });
    /// ```
    pub fn tags<S: Into<String>>(&self, names: impl IntoIterator<Item = S>) -> Result<Repository> {
        if !self.supports_tags() {
            return Err(BadMethodCallException::new("This cache store does not support tagging.").into());
        }
        let mut tagged = self.clone();
        tagged.tags = Some(TagSet::new(self.store.clone(), names.into_iter().map(Into::into).collect()));
        Ok(tagged)
    }

    /// The tag set of a tagged cache.
    pub fn get_tags(&self) -> Option<&TagSet> {
        self.tags.as_ref()
    }
}

fn type_error(key: &str, expected: &str, value: &Value) -> illuminate_support::Error {
    InvalidArgumentException::new(format!(
        "Cache value for key [{key}] must be {expected}, {} given.",
        php_type(value)
    ))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array_store::ArrayStore;
    use crate::null_store::NullStore;
    use crate::testing::freeze_time;
    use illuminate_support::{CarbonInterval, json};
    use serde::Deserialize;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn cache() -> Repository {
        Repository::new(ArrayStore::new())
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct User {
        id: u64,
        name: String,
    }

    #[tokio::test]
    async fn it_stores_and_retrieves_typed_values() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();
        let user = User { id: 1, name: "Taylor".into() };

        assert!(cache.put("user", &user, 60).await.unwrap());
        assert_eq!(cache.get_as::<User>("user").await.unwrap(), Some(user.clone()));
        assert_eq!(cache.get("user").await.unwrap(), Some(json!({"id": 1, "name": "Taylor"})));
        assert_eq!(cache.get_or("missing", "default").await.unwrap(), json!("default"));
        assert_eq!(cache.get_or_else("missing", || 5).await.unwrap(), json!(5));
        assert!(cache.missing("missing").await.unwrap());

        cache.put("a", &1, 60).await.unwrap();
        let many = cache.many(["a", "nope"]).await.unwrap();
        assert_eq!(many["a"], Some(json!(1)));
        assert_eq!(many["nope"], None);

        assert_eq!(cache.pull_as::<User>("user").await.unwrap(), Some(user));
        assert!(cache.missing("user").await.unwrap());
    }

    #[tokio::test]
    async fn ttls_are_respected() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();

        cache.put("seconds", "x", 10).await.unwrap();
        cache.put("duration", "x", std::time::Duration::from_secs(20)).await.unwrap();
        cache.put("interval", "x", CarbonInterval::minutes(1)).await.unwrap();
        cache.put("carbon", "x", Carbon::now().add_seconds(30)).await.unwrap();
        cache.put("forever", "x", None::<u64>).await.unwrap();

        time.travel_seconds(10);
        assert!(cache.missing("seconds").await.unwrap());
        assert!(cache.has("duration").await.unwrap());
        time.travel_seconds(10);
        assert!(cache.missing("duration").await.unwrap());
        assert!(cache.has("carbon").await.unwrap());
        time.travel_seconds(10);
        assert!(cache.missing("carbon").await.unwrap());
        assert!(cache.has("interval").await.unwrap());
        time.travel_seconds(30);
        assert!(cache.missing("interval").await.unwrap());
        assert!(cache.has("forever").await.unwrap());
    }

    #[tokio::test]
    async fn non_positive_ttls_forget_the_item() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();
        cache.forever("key", "value").await.unwrap();
        cache.put("key", "value", 0).await.unwrap();
        assert!(cache.missing("key").await.unwrap());

        cache.forever("key", "value").await.unwrap();
        cache.put("key", "value", Carbon::now().sub_minutes(5)).await.unwrap();
        assert!(cache.missing("key").await.unwrap());

        assert!(!cache.add("key", "value", -1).await.unwrap());
        assert!(cache.missing("key").await.unwrap());
    }

    #[tokio::test]
    async fn add_only_stores_missing_items() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();
        assert!(cache.add("key", "first", 60).await.unwrap());
        assert!(!cache.add("key", "second", 60).await.unwrap());
        assert_eq!(cache.string("key").await.unwrap(), "first");
        assert!(cache.add("forever", "value", None::<u64>).await.unwrap());
    }

    #[tokio::test]
    async fn increments_and_decrements() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();
        assert_eq!(cache.increment("count").await.unwrap(), 1);
        assert_eq!(cache.increment_by("count", 10).await.unwrap(), 11);
        assert_eq!(cache.decrement("count").await.unwrap(), 10);
        assert_eq!(cache.decrement_by("count", 5).await.unwrap(), 5);
        assert_eq!(cache.integer("count").await.unwrap(), 5);
    }

    #[tokio::test]
    async fn typed_getters_validate_types() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();
        cache.put("s", "text", 60).await.unwrap();
        cache.put("i", "42", 60).await.unwrap();
        cache.put("f", &1.5, 60).await.unwrap();
        cache.put("b", &true, 60).await.unwrap();
        cache.put("a", &vec![1, 2], 60).await.unwrap();

        assert_eq!(cache.string("s").await.unwrap(), "text");
        assert_eq!(cache.integer("i").await.unwrap(), 42);
        assert_eq!(cache.float("f").await.unwrap(), 1.5);
        assert!(cache.boolean("b").await.unwrap());
        assert_eq!(cache.array("a").await.unwrap(), json!([1, 2]));

        assert_eq!(
            cache.string("b").await.unwrap_err().to_string(),
            "Cache value for key [b] must be a string, boolean given."
        );
        assert_eq!(
            cache.integer("missing").await.unwrap_err().to_string(),
            "Cache value for key [missing] must be an integer, NULL given."
        );
        assert!(cache.boolean("s").await.is_err());
        assert!(cache.array("s").await.is_err());
    }

    #[tokio::test]
    async fn remember_only_calls_the_callback_on_a_miss() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();
        let calls = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            let calls = calls.clone();
            let value: u64 = cache
                .remember("answer", 60, || async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(42)
                })
                .await
                .unwrap();
            assert_eq!(value, 42);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let (_, warm) = cache.remember_with_warmth("answer", 60, || async { Ok(0) }).await.unwrap();
        assert!(warm);
        time.travel_seconds(60);
        let (value, warm) = cache.remember_with_warmth("answer", 60, || async { Ok(7) }).await.unwrap();
        assert_eq!((value, warm), (7, false));

        let forever: String = cache.remember_forever("name", || async { Ok("Taylor".to_string()) }).await.unwrap();
        assert_eq!(forever, "Taylor");
        let seared: String = cache.sear("name", || async { Ok("Other".to_string()) }).await.unwrap();
        assert_eq!(seared, "Taylor");

        let error = cache.remember::<u64, _, _>("failing", 60, || async { Err(illuminate_support::error::error!("boom")) }).await;
        assert!(error.is_err());
        assert!(cache.missing("failing").await.unwrap());
    }

    #[tokio::test]
    async fn flexible_serves_stale_values_while_revalidating() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();
        let calls = Arc::new(AtomicUsize::new(0));
        let callback = {
            let calls = calls.clone();
            move || {
                let calls = calls.clone();
                async move { Ok(calls.fetch_add(1, Ordering::SeqCst) + 1) }
            }
        };

        // A miss computes the value immediately.
        assert_eq!(cache.flexible("value", (10, 20), callback.clone()).await.unwrap(), 1);

        // Fresh: served from the cache.
        time.travel_seconds(5);
        assert_eq!(cache.flexible("value", (10, 20), callback.clone()).await.unwrap(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Stale: the old value is served while it refreshes in the background.
        time.travel_seconds(10);
        assert_eq!(cache.flexible("value", (10, 20), callback.clone()).await.unwrap(), 1);
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(cache.flexible("value", (10, 20), callback.clone()).await.unwrap(), 2);

        // Expired: recomputed before returning.
        time.travel_seconds(25);
        assert_eq!(cache.flexible("value", (10, 20), callback.clone()).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn touch_extends_the_lifetime() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();
        cache.put("key", "value", 10).await.unwrap();
        assert!(cache.touch("key", 100).await.unwrap());
        time.travel_seconds(50);
        assert!(cache.has("key").await.unwrap());
        assert!(cache.touch("key", 0).await.unwrap());
        assert!(cache.missing("key").await.unwrap());
    }

    #[tokio::test]
    async fn tags_namespace_and_flush_items() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();
        let artists = cache.tags(["people", "artists"]).unwrap();
        let authors = cache.tags(["people", "authors"]).unwrap();
        artists.put("John", "Lennon", 60).await.unwrap();
        authors.put("Anne", "Rice", 60).await.unwrap();
        assert_eq!(artists.get_tags().unwrap().get_names(), ["people", "artists"]);

        assert!(cache.missing("John").await.unwrap());
        assert_eq!(artists.increment("plays").await.unwrap(), 1);

        cache.tags(["authors"]).unwrap().flush().await.unwrap();
        assert!(cache.tags(["people", "artists"]).unwrap().has("John").await.unwrap());
        assert!(cache.tags(["people", "authors"]).unwrap().missing("Anne").await.unwrap());

        cache.tags(["people"]).unwrap().flush().await.unwrap();
        assert!(cache.tags(["people", "artists"]).unwrap().missing("John").await.unwrap());
    }

    #[tokio::test]
    async fn file_stores_do_not_support_tags() {
        let directory = tempfile::tempdir().unwrap();
        let cache = Repository::new(crate::FileStore::new(directory.path()));
        assert_eq!(
            cache.tags(["a"]).unwrap_err().to_string(),
            "This cache store does not support tagging."
        );
    }

    #[tokio::test]
    async fn locks_can_be_acquired_released_and_restored() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();

        let lock = cache.lock("foo", 10);
        assert!(lock.get().await.unwrap());
        assert!(lock.is_locked().await.unwrap());
        assert!(!cache.lock("foo", 10).get().await.unwrap());
        assert!(!cache.lock("foo", 10).release().await.unwrap());

        let restored = cache.restore_lock("foo", lock.owner());
        assert!(restored.is_owned_by_current_process().await.unwrap());
        assert!(restored.release().await.unwrap());
        assert!(!lock.is_locked().await.unwrap());

        // Expired locks may be taken over.
        assert!(lock.get().await.unwrap());
        time.travel_seconds(10);
        assert!(cache.lock("foo", 10).get().await.unwrap());

        cache.lock("bar", 0).force_release().await.unwrap();
        assert!(cache.lock("bar", 0).get().await.unwrap());
        time.travel_seconds(1_000_000);
        assert!(!cache.lock("bar", 0).get().await.unwrap());
        cache.lock("bar", 0).force_release().await.unwrap();
        assert!(cache.lock("bar", 0).get().await.unwrap());

        assert!(cache.flush_locks().await.unwrap());
    }

    #[tokio::test]
    async fn lock_callbacks_release_automatically() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();
        let value = cache.lock("foo", 10).get_with(|| async { "done" }).await.unwrap();
        assert_eq!(value, Some("done"));
        assert!(!cache.lock("foo", 10).is_locked().await.unwrap());

        let held = cache.lock("foo", 10);
        held.get().await.unwrap();
        assert_eq!(cache.lock("foo", 10).get_with(|| async { "never" }).await.unwrap(), None);
    }

    #[tokio::test]
    async fn blocking_waits_for_contended_locks() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();
        let held = cache.lock("contended", 10);
        assert!(held.get().await.unwrap());

        let releaser = {
            let held = held.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                held.release().await.unwrap();
            })
        };
        let waiter = cache.lock("contended", 10).between_blocked_attempts_sleep_for(10);
        let value = waiter.block_with(2, || async { "acquired" }).await.unwrap();
        assert_eq!(value, "acquired");
        releaser.await.unwrap();
        assert!(!waiter.is_locked().await.unwrap());

        let held = cache.lock("busy", 10);
        held.get().await.unwrap();
        let error = cache.lock("busy", 10).between_blocked_attempts_sleep_for(10).block(0).await.unwrap_err();
        let timeout = error.downcast_ref::<crate::LockTimeoutException>().unwrap();
        assert_eq!(timeout.name, "busy");
        assert_eq!(error.to_string(), "Unable to acquire lock [busy].");

        let result = cache.without_overlapping("job", || async { 7 }, 10, 1).await.unwrap();
        assert_eq!(result, 7);
    }

    #[tokio::test]
    async fn lock_refreshes_extend_ownership() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let cache = cache();
        let lock = cache.lock("long", 10);
        lock.get().await.unwrap();
        time.travel_seconds(8);
        assert!(lock.refresh(None).await.unwrap());
        time.travel_seconds(8);
        assert!(!cache.lock("long", 10).get().await.unwrap());
        assert!(!cache.lock("long", 10).refresh(None).await.unwrap());
    }

    #[tokio::test]
    async fn stores_without_locks_report_it() {
        struct Plain;
        #[async_trait::async_trait]
        impl Store for Plain {
            async fn get(&self, _: &str) -> Result<Option<Value>> {
                Ok(None)
            }
            async fn put(&self, _: &str, _: Value, _: u64) -> Result<bool> {
                Ok(true)
            }
            async fn increment(&self, _: &str, value: i64) -> Result<i64> {
                Ok(value)
            }
            async fn forever(&self, _: &str, _: Value) -> Result<bool> {
                Ok(true)
            }
            async fn touch(&self, _: &str, _: u64) -> Result<bool> {
                Ok(true)
            }
            async fn forget(&self, _: &str) -> Result<bool> {
                Ok(true)
            }
            async fn flush(&self) -> Result<bool> {
                Ok(true)
            }
        }
        let cache = Repository::new(Plain);
        let error = cache.lock("foo", 1).get().await.unwrap_err();
        assert_eq!(error.to_string(), "This cache store does not support locks.");
        assert!(cache.flush_locks().await.is_err());
        // The default `add` checks before writing.
        assert!(cache.add("key", "value", 10).await.unwrap());
    }

    #[tokio::test]
    async fn the_null_store_never_hits() {
        let cache = Repository::new(NullStore);
        cache.put("a", "b", 10).await.unwrap();
        assert!(cache.missing("a").await.unwrap());
        let value: u64 = cache.remember("a", 10, || async { Ok(1) }).await.unwrap();
        assert_eq!(value, 1);
        assert_eq!(cache.get_default_cache_time(), Some(3600));
        cache.set_default_cache_time(Some(60));
        assert_eq!(cache.get_default_cache_time(), Some(60));
    }
}
