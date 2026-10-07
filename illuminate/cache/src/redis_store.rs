//! The Redis cache store and its locks.
//!
//! Items are plain Redis keys — `{redis prefix}{cache prefix}{key}` — so
//! they expire on their own. Numbers are stored as-is, which keeps `INCRBY`
//! and `DECRBY` working on them; everything else is stored as JSON, exactly
//! where Laravel's `RedisStore` would use PHP's `serialize`.

use std::sync::Arc;

use async_trait::async_trait;

use illuminate_config::Repository as Config;
use illuminate_container::{Container, try_app};
use illuminate_redis::{Connection, RedisManager};
use illuminate_support::{Result, Value, ValueExt};

use crate::lock::{Lock, LockDriver, LockInfo};
use crate::store::{LockProvider, Store, separate_lock_store_required};

/// Sets a key only when it doesn't exist yet (Laravel's `LuaScripts::add`).
///
/// KEYS[1] - The name of the key
/// ARGV[1] - The value of the key
/// ARGV[2] - The number of seconds the key should be valid
const ADD_SCRIPT: &str =
    "return redis.call('exists',KEYS[1])<1 and redis.call('setex',KEYS[1],ARGV[2],ARGV[1])";

/// Atomically refreshes a lock's expiration (Laravel's `LuaScripts::refreshLock`).
///
/// KEYS[1] - The name of the lock
/// ARGV[1] - The owner key of the lock instance trying to refresh it
/// ARGV[2] - The number of seconds the lock should be valid
const REFRESH_LOCK_SCRIPT: &str = r#"
if redis.call("get",KEYS[1]) == ARGV[1] then
    if tonumber(ARGV[2]) > 0 then
        return redis.call("expire",KEYS[1],ARGV[2])
    end

    redis.call("persist",KEYS[1])

    return 1
else
    return 0
end
"#;

/// Atomically releases a lock (Laravel's `LuaScripts::releaseLock`).
///
/// KEYS[1] - The name of the lock
/// ARGV[1] - The owner key of the lock instance trying to release it
const RELEASE_LOCK_SCRIPT: &str = r#"
if redis.call("get",KEYS[1]) == ARGV[1] then
    return redis.call("del",KEYS[1])
else
    return 0
end
"#;

/// The Redis manager in the container, registering one built from the
/// given configuration when there is none yet.
pub(crate) fn redis_manager(config: &Arc<Config>) -> Arc<RedisManager> {
    if let Some(redis) = try_app::<RedisManager>() {
        return redis;
    }
    let container = Container::get_instance();
    let config = config.clone();
    container.singleton_if::<RedisManager>(move |_| Arc::new(RedisManager::from_config(&config)));
    container.make::<RedisManager>()
}

/// Encode a value the way Laravel's `RedisStore` serializes it: finite
/// numbers raw, everything else as JSON.
fn serialize(value: &Value) -> String {
    match value {
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

/// Decode a stored value. Anything that isn't JSON (a value written by
/// another client, say) comes back as a string.
fn unserialize(raw: String) -> Value {
    serde_json::from_str(&raw).unwrap_or(Value::String(raw))
}

/// A cache store backed by Redis (`CACHE_STORE=redis`).
///
/// ```no_run
/// use illuminate_cache::{RedisStore, Repository};
/// use illuminate_redis::Redis;
///
/// # async fn example() -> illuminate_support::Result<()> {
/// let store = RedisStore::new(Redis::manager()?, "laravel-cache-", "cache")
///     .with_lock_connection("default");
/// let cache = Repository::new(store);
///
/// cache.put("name", "Taylor", 600).await?;
/// cache.increment("visits").await?;
///
/// let lock = cache.lock("reports", 10);
/// if lock.get().await? {
///     // ...
///     lock.release().await?;
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct RedisStore {
    redis: Arc<RedisManager>,
    prefix: String,
    connection: String,
    lock_connection: String,
}

impl std::fmt::Debug for RedisStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisStore")
            .field("prefix", &self.prefix)
            .field("connection", &self.connection)
            .field("lock_connection", &self.lock_connection)
            .finish()
    }
}

impl RedisStore {
    /// Create a store keeping its items on the given Redis connection, with
    /// keys prefixed by `prefix`. Locks use the same connection unless told
    /// otherwise.
    pub fn new(
        redis: Arc<RedisManager>,
        prefix: impl Into<String>,
        connection: impl Into<String>,
    ) -> Self {
        let connection = connection.into();
        Self {
            redis,
            prefix: prefix.into(),
            lock_connection: connection.clone(),
            connection,
        }
    }

    /// Manage locks on a different Redis connection (the `lock_connection`
    /// option).
    pub fn with_lock_connection(mut self, connection: impl Into<String>) -> Self {
        self.lock_connection = connection.into();
        self
    }

    /// Use a different Redis connection for the items.
    pub fn with_connection(mut self, connection: impl Into<String>) -> Self {
        self.connection = connection.into();
        self
    }

    /// Use a different cache key prefix.
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// The Redis connection the items live on.
    pub fn connection(&self) -> Result<Connection> {
        self.redis.connection(self.connection.as_str())
    }

    /// The Redis connection used to manage locks.
    pub fn lock_connection(&self) -> Result<Connection> {
        self.redis.connection(self.lock_connection.as_str())
    }

    /// The name of the Redis connection the items live on.
    pub fn get_connection_name(&self) -> &str {
        &self.connection
    }

    /// The name of the Redis connection used to manage locks.
    pub fn get_lock_connection_name(&self) -> &str {
        &self.lock_connection
    }

    /// The Redis manager.
    pub fn get_redis(&self) -> &Arc<RedisManager> {
        &self.redis
    }

    /// Determine if locks live on a different connection than the items.
    pub fn has_separate_lock_store(&self) -> bool {
        self.lock_connection != self.connection
    }

    fn prefixed(&self, key: &str) -> String {
        format!("{}{key}", self.prefix)
    }
}

#[async_trait]
impl Store for RedisStore {
    async fn get(&self, key: &str) -> Result<Option<Value>> {
        Ok(self
            .connection()?
            .get(self.prefixed(key))
            .await?
            .map(unserialize))
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<(String, Option<Value>)>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let prefixed: Vec<String> = keys.iter().map(|key| self.prefixed(key)).collect();
        let values = self.connection()?.mget(prefixed).await?;
        Ok(keys
            .iter()
            .cloned()
            .zip(values.into_iter().map(|value| value.map(unserialize)))
            .collect())
    }

    async fn put(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        self.connection()?
            .set_ex(self.prefixed(key), serialize(&value), seconds.max(1))
            .await
    }

    async fn put_many(&self, values: Vec<(String, Value)>, seconds: u64) -> Result<bool> {
        if values.is_empty() {
            return Ok(true);
        }
        let replies = self
            .connection()?
            .transaction(|redis| {
                for (key, value) in &values {
                    redis.set_ex(self.prefixed(key), serialize(value), seconds.max(1));
                }
            })
            .await?;
        Ok(replies.iter().all(ValueExt::truthy))
    }

    async fn add(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        let connection = self.connection()?;
        let key = self.prefixed(key);
        if seconds == 0 {
            let added = connection
                .command("set", (key, serialize(&value), "NX"))
                .await?;
            return Ok(added.truthy());
        }
        let added = connection
            .eval(ADD_SCRIPT, (key,), (serialize(&value), seconds))
            .await?;
        Ok(added.truthy())
    }

    async fn increment(&self, key: &str, value: i64) -> Result<i64> {
        self.connection()?.incrby(self.prefixed(key), value).await
    }

    async fn decrement(&self, key: &str, value: i64) -> Result<i64> {
        self.connection()?.decrby(self.prefixed(key), value).await
    }

    async fn forever(&self, key: &str, value: Value) -> Result<bool> {
        self.connection()?
            .set(self.prefixed(key), serialize(&value))
            .await
    }

    async fn touch(&self, key: &str, seconds: u64) -> Result<bool> {
        let seconds = i64::try_from(seconds.max(1)).unwrap_or(i64::MAX);
        self.connection()?.expire(self.prefixed(key), seconds).await
    }

    async fn forget(&self, key: &str) -> Result<bool> {
        Ok(self.connection()?.del(self.prefixed(key)).await? > 0)
    }

    /// Flush the store's whole Redis database, like Laravel does.
    async fn flush(&self) -> Result<bool> {
        self.connection()?.flushdb().await?;
        Ok(true)
    }

    fn get_prefix(&self) -> String {
        self.prefix.clone()
    }

    fn lock_provider(&self) -> Option<&dyn LockProvider> {
        Some(self)
    }

    fn supports_tags(&self) -> bool {
        true
    }

    async fn flush_locks(&self) -> Result<bool> {
        if !self.has_separate_lock_store() {
            return Err(separate_lock_store_required());
        }
        self.lock_connection()?.flushdb().await?;
        Ok(true)
    }
}

impl LockProvider for RedisStore {
    fn lock(&self, name: &str, seconds: u64, owner: Option<String>) -> Lock {
        Lock::new(
            Arc::new(RedisLock::new(
                self.redis.clone(),
                self.lock_connection.clone(),
            )),
            self.prefixed(name),
            seconds,
            owner,
        )
    }
}

/// Locks kept as Redis keys holding their owner (Laravel's `RedisLock`):
/// acquired with `SET ... EX ... NX`, released only by their owner.
#[derive(Clone)]
pub struct RedisLock {
    redis: Arc<RedisManager>,
    connection: String,
}

impl std::fmt::Debug for RedisLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisLock")
            .field("connection", &self.connection)
            .finish()
    }
}

impl RedisLock {
    /// Create a lock driver keeping its locks on the given Redis connection.
    pub fn new(redis: Arc<RedisManager>, connection: impl Into<String>) -> Self {
        Self {
            redis,
            connection: connection.into(),
        }
    }

    /// The name of the Redis connection managing the locks.
    pub fn get_connection_name(&self) -> &str {
        &self.connection
    }

    fn connection(&self) -> Result<Connection> {
        self.redis.connection(self.connection.as_str())
    }
}

#[async_trait]
impl LockDriver for RedisLock {
    async fn acquire(&self, lock: &LockInfo) -> Result<bool> {
        let connection = self.connection()?;
        if lock.seconds > 0 {
            let acquired = connection
                .command(
                    "set",
                    (
                        lock.name.as_str(),
                        lock.owner.as_str(),
                        "EX",
                        lock.seconds,
                        "NX",
                    ),
                )
                .await?;
            return Ok(acquired.truthy());
        }
        connection
            .setnx(lock.name.as_str(), lock.owner.as_str())
            .await
    }

    async fn release(&self, lock: &LockInfo) -> Result<bool> {
        let released = self
            .connection()?
            .eval(
                RELEASE_LOCK_SCRIPT,
                (lock.name.as_str(),),
                (lock.owner.as_str(),),
            )
            .await?;
        Ok(released.truthy())
    }

    async fn force_release(&self, lock: &LockInfo) -> Result<()> {
        self.connection()?.del(lock.name.as_str()).await?;
        Ok(())
    }

    async fn current_owner(&self, lock: &LockInfo) -> Result<Option<String>> {
        self.connection()?.get(lock.name.as_str()).await
    }

    async fn refresh(&self, lock: &LockInfo, seconds: u64) -> Result<bool> {
        let refreshed = self
            .connection()?
            .eval(
                REFRESH_LOCK_SCRIPT,
                (lock.name.as_str(),),
                (lock.owner.as_str(), seconds),
            )
            .await?;
        Ok(refreshed.truthy())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn numbers_are_stored_raw_and_everything_else_as_json() {
        assert_eq!(serialize(&json!(42)), "42");
        assert_eq!(serialize(&json!(-1.5)), "-1.5");
        assert_eq!(serialize(&json!("42")), r#""42""#);
        assert_eq!(serialize(&json!("Taylor")), r#""Taylor""#);
        assert_eq!(serialize(&json!(true)), "true");
        assert_eq!(serialize(&json!(null)), "null");
        assert_eq!(serialize(&json!({"a": [1]})), r#"{"a":[1]}"#);

        for value in [
            json!(42),
            json!(-1.5),
            json!("42"),
            json!("Taylor"),
            json!(true),
            json!(null),
            json!({"a": [1]}),
        ] {
            assert_eq!(unserialize(serialize(&value)), value);
        }
        assert_eq!(unserialize("not json".into()), json!("not json"));
        assert_eq!(unserialize("007".into()), json!("007"));
    }

    #[test]
    fn stores_describe_their_connections() {
        let redis = Arc::new(RedisManager::new(
            "phpredis",
            json!({"default": {}, "cache": {}}),
        ));
        let store = RedisStore::new(redis.clone(), "laravel-cache-", "cache");
        assert_eq!(store.get_connection_name(), "cache");
        assert_eq!(store.get_lock_connection_name(), "cache");
        assert!(!store.has_separate_lock_store());
        assert_eq!(store.get_prefix(), "laravel-cache-");

        let store = store
            .with_lock_connection("default")
            .with_prefix("app-")
            .with_connection("cache");
        assert!(store.has_separate_lock_store());
        assert_eq!(store.connection().unwrap().name(), "cache");
        assert_eq!(store.lock_connection().unwrap().name(), "default");
        assert!(Arc::ptr_eq(store.get_redis(), &redis));
        assert!(store.supports_tags());
        assert!(format!("{store:?}").contains("app-"));

        let lock = store.lock("reports", 10, Some("me".into()));
        assert_eq!(lock.name(), "app-reports");
        assert_eq!(lock.owner(), "me");
        let driver = RedisLock::new(redis, "default");
        assert_eq!(driver.get_connection_name(), "default");
        assert!(format!("{driver:?}").contains("default"));
    }
}
