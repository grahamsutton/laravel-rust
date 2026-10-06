//! The `Cache` facade and the `cache()` helper.

use std::future::Future;
use std::sync::Arc;

use indexmap::IndexMap;
use serde::Serialize;
use serde::de::DeserializeOwned;

use illuminate_container::{Container, try_app};
use illuminate_support::{Result, Value};

use crate::lock::Lock;
use crate::manager::CacheManager;
use crate::repository::Repository;
use crate::store::Store;
use crate::ttl::Ttl;

pub(crate) fn manager() -> Result<Arc<CacheManager>> {
    if let Some(manager) = try_app::<CacheManager>() {
        return Ok(manager);
    }
    let container = Container::get_instance();
    container.singleton_if::<CacheManager>(crate::provider::make_manager);
    Ok(container.try_make::<CacheManager>()?)
}

/// The `Cache` facade.
///
/// Calls go to the default cache store; use [`Cache::store`] to pick another.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_cache::{Cache, CacheServiceProvider};
/// use illuminate_config::Repository;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({
///     "cache": {"default": "array", "stores": {"array": {"driver": "array"}}},
/// })));
/// CacheServiceProvider.register(&container);
///
/// Cache::put("key", "value", 600).await.unwrap();
///
/// assert_eq!(Cache::get("key").await.unwrap(), Some(json!("value")));
/// assert_eq!(Cache::increment("visits").await.unwrap(), 1);
/// # });
/// ```
pub struct Cache;

impl Cache {
    /// Get the cache manager.
    pub fn manager() -> Result<Arc<CacheManager>> {
        manager()
    }

    /// Get a cache store instance by name.
    pub fn store(name: &str) -> Result<Repository> {
        manager()?.store(name)
    }

    /// Get a cache driver instance — the default one when `None`.
    pub fn driver(name: Option<&str>) -> Result<Repository> {
        manager()?.driver(name)
    }

    /// Get the default cache store.
    pub fn default_store() -> Result<Repository> {
        manager()?.default_store()
    }

    /// Build a cache repository with the given configuration.
    pub fn build(config: Value) -> Result<Repository> {
        manager()?.build(config)
    }

    /// Wrap a store in a repository.
    pub fn repository(store: impl Store) -> Repository {
        Repository::new(store)
    }

    /// Register a custom cache driver.
    pub fn extend(
        driver: &str,
        creator: impl Fn(&Container, &Value) -> Result<Repository> + Send + Sync + 'static,
    ) -> Result<()> {
        manager()?.extend(driver, creator);
        Ok(())
    }

    pub async fn has(key: &str) -> Result<bool> {
        Self::default_store()?.has(key).await
    }

    pub async fn missing(key: &str) -> Result<bool> {
        Self::default_store()?.missing(key).await
    }

    pub async fn get(key: &str) -> Result<Option<Value>> {
        Self::default_store()?.get(key).await
    }

    pub async fn get_as<T: DeserializeOwned>(key: &str) -> Result<Option<T>> {
        Self::default_store()?.get_as(key).await
    }

    pub async fn get_or(key: &str, default: impl Into<Value>) -> Result<Value> {
        Self::default_store()?.get_or(key, default).await
    }

    pub async fn many<K: AsRef<str>>(
        keys: impl IntoIterator<Item = K>,
    ) -> Result<IndexMap<String, Option<Value>>> {
        Self::default_store()?.many(keys).await
    }

    pub async fn pull(key: &str) -> Result<Option<Value>> {
        Self::default_store()?.pull(key).await
    }

    pub async fn string(key: &str) -> Result<String> {
        Self::default_store()?.string(key).await
    }

    pub async fn integer(key: &str) -> Result<i64> {
        Self::default_store()?.integer(key).await
    }

    pub async fn boolean(key: &str) -> Result<bool> {
        Self::default_store()?.boolean(key).await
    }

    pub async fn put(key: &str, value: impl Serialize, ttl: impl Into<Ttl>) -> Result<bool> {
        Self::default_store()?.put(key, value, ttl).await
    }

    pub async fn put_many<K: AsRef<str>, V: Serialize>(
        values: impl IntoIterator<Item = (K, V)>,
        ttl: impl Into<Ttl>,
    ) -> Result<bool> {
        Self::default_store()?.put_many(values, ttl).await
    }

    pub async fn add(key: &str, value: impl Serialize, ttl: impl Into<Ttl>) -> Result<bool> {
        Self::default_store()?.add(key, value, ttl).await
    }

    pub async fn increment(key: &str) -> Result<i64> {
        Self::default_store()?.increment(key).await
    }

    pub async fn increment_by(key: &str, amount: i64) -> Result<i64> {
        Self::default_store()?.increment_by(key, amount).await
    }

    pub async fn decrement(key: &str) -> Result<i64> {
        Self::default_store()?.decrement(key).await
    }

    pub async fn decrement_by(key: &str, amount: i64) -> Result<i64> {
        Self::default_store()?.decrement_by(key, amount).await
    }

    pub async fn forever(key: &str, value: impl Serialize) -> Result<bool> {
        Self::default_store()?.forever(key, value).await
    }

    pub async fn remember<T, F, Fut>(key: &str, ttl: impl Into<Ttl>, callback: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        Self::default_store()?.remember(key, ttl, callback).await
    }

    pub async fn remember_forever<T, F, Fut>(key: &str, callback: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        Self::default_store()?.remember_forever(key, callback).await
    }

    pub async fn sear<T, F, Fut>(key: &str, callback: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        Self::default_store()?.sear(key, callback).await
    }

    pub async fn flexible<T, F, Fut>(
        key: &str,
        ttl: (impl Into<Ttl>, impl Into<Ttl>),
        callback: F,
    ) -> Result<T>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        Self::default_store()?.flexible(key, ttl, callback).await
    }

    pub async fn touch(key: &str, ttl: impl Into<Ttl>) -> Result<bool> {
        Self::default_store()?.touch(key, ttl).await
    }

    pub async fn forget(key: &str) -> Result<bool> {
        Self::default_store()?.forget(key).await
    }

    pub async fn flush() -> Result<bool> {
        Self::default_store()?.flush().await
    }

    pub async fn flush_locks() -> Result<bool> {
        Self::default_store()?.flush_locks().await
    }

    /// Get a lock instance from the default store.
    pub fn lock(name: &str, seconds: u64) -> Result<Lock> {
        Ok(Self::default_store()?.lock(name, seconds))
    }

    /// Restore a lock instance using its owner token.
    pub fn restore_lock(name: &str, owner: &str) -> Result<Lock> {
        Ok(Self::default_store()?.restore_lock(name, owner))
    }

    pub async fn without_overlapping<T, F, Fut>(
        key: &str,
        callback: F,
        lock_for: u64,
        wait_for: u64,
    ) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        Self::default_store()?
            .without_overlapping(key, callback, lock_for, wait_for)
            .await
    }

    /// Begin a tagged cache operation on the default store.
    pub fn tags<S: Into<String>>(names: impl IntoIterator<Item = S>) -> Result<Repository> {
        Self::default_store()?.tags(names)
    }
}

/// Get the default cache store.
///
/// # Panics
///
/// Panics when the default cache store is not configured correctly, the
/// same way resolving an unbound service from the container does.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_cache::cache;
/// use illuminate_config::Repository;
/// use illuminate_container::Container;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({
///     "cache": {"default": "array", "stores": {"array": {"driver": "array"}}},
/// })));
///
/// let count: u64 = cache().remember("users.count", 60, || async { Ok(42) }).await.unwrap();
/// assert_eq!(count, 42);
/// # });
/// ```
pub fn cache() -> Repository {
    match Cache::default_store() {
        Ok(repository) => repository,
        Err(error) => panic!("{error}"),
    }
}
