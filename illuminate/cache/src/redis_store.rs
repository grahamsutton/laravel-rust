//! The Redis cache store and its locks.
//!
//! Items are plain Redis keys — `{redis prefix}{cache prefix}{key}` — so
//! they expire on their own. Numbers are stored as-is, which keeps `INCRBY`
//! and `DECRBY` working on them; everything else is stored as JSON, exactly
//! where Laravel's `RedisStore` would use PHP's `serialize`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;

use illuminate_config::Repository as Config;
use illuminate_container::{Container, try_app};
use illuminate_redis::{Connection, RedisManager};
use illuminate_support::{Result, Value, ValueExt};

use crate::lock::{Lock, LockDriver, LockInfo};
use crate::manager::CacheManager;
use crate::store::{LockProvider, Store, separate_lock_store_required};
use crate::tags::namespace_key;

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

    /// The Redis store behind a cache store resolved by the cache manager,
    /// if it is one — the Rust spelling of Laravel's
    /// `$cache->getStore() instanceof RedisStore`.
    ///
    /// ```no_run
    /// use illuminate_cache::{Cache, RedisStore};
    ///
    /// # async fn example() -> illuminate_support::Result<()> {
    /// if let Some(redis) = RedisStore::of(&Cache::store("redis")?.get_store()) {
    ///     redis.flush_stale_tags().await?;
    /// }
    /// # Ok(()) }
    /// ```
    pub fn of(store: &Arc<dyn Store>) -> Option<RedisStore> {
        let manager = try_app::<CacheManager>()?;
        let config = try_app::<Config>()?;
        let Value::Object(stores) = config.get("cache.stores") else {
            return None;
        };
        stores
            .iter()
            .filter(|(_, options)| options.get("driver").and_then(Value::as_str) == Some("redis"))
            // Only resolve the stores that could be this one.
            .filter(|(_, options)| store.get_prefix() == manager.get_prefix(options))
            .find_map(|(name, options)| {
                let resolved = manager.store(name).ok()?.get_store();
                if !Arc::ptr_eq(&resolved, store) {
                    return None;
                }
                let option = |key: &str, default: &str| {
                    options
                        .get(key)
                        .filter(|value| !value.is_blank())
                        .map(ValueExt::to_string_lossy)
                        .unwrap_or_else(|| default.to_string())
                };
                Some(
                    RedisStore::new(redis_manager(&config), resolved.get_prefix(), option("connection", "cache"))
                        .with_lock_connection(option("lock_connection", "default")),
                )
            })
    }

    // ------------------------------------------------------------------
    // Tags
    // ------------------------------------------------------------------

    /// The key of the set recording the namespaces a tag has been part of.
    fn tag_entries_key(name: &str) -> String {
        format!("tag:{name}:entries")
    }

    /// Record that the given tags (and their current ids) form a namespace
    /// tagged items are stored under, so pruning can find them later.
    pub(crate) async fn add_tag_namespace(&self, tags: &[(String, String)]) -> Result<()> {
        if tags.is_empty() {
            return Ok(());
        }
        let member = serde_json::to_string(tags)?;
        self.connection()?
            .pipeline(|redis| {
                for (name, _) in tags {
                    redis.sadd(self.prefixed(&Self::tag_entries_key(name)), (member.as_str(),));
                }
            })
            .await?;
        Ok(())
    }

    /// The names of every tag currently in use (Laravel's `currentTags`).
    pub async fn current_tags(&self) -> Result<Vec<String>> {
        let prefix = self.prefixed("tag:");
        let mut tags: Vec<String> = self
            .scan(&format!("{prefix}*:entries"))
            .await?
            .into_iter()
            .filter_map(|key| {
                key.strip_prefix(&prefix)
                    .and_then(|rest| rest.strip_suffix(":entries"))
                    .map(str::to_string)
            })
            .collect();
        tags.sort();
        tags.dedup();
        Ok(tags)
    }

    /// Remove the stale entries of every tag — Laravel's `flushStaleTags`,
    /// run by `cargo artisan cache:prune-stale-tags`.
    ///
    /// Flushing a tag gives it a new id, orphaning the items stored under
    /// the old one; items stored forever would otherwise stay in Redis for
    /// good. Pruning deletes those items, and forgets the namespaces that no
    /// longer have any items, so each tag's record of its entries doesn't
    /// grow without bound.
    pub async fn flush_stale_tags(&self) -> Result<()> {
        let connection = self.connection()?;
        let mut ids: HashMap<String, Option<String>> = HashMap::new();
        let mut stale: HashSet<String> = HashSet::new();
        let mut records: Vec<(String, String, String, bool)> = Vec::new();

        for tag in self.current_tags().await? {
            let key = self.prefixed(&Self::tag_entries_key(&tag));
            for member in connection.smembers(&key).await? {
                let Ok(tags) = serde_json::from_str::<Vec<(String, String)>>(&member) else {
                    connection.srem(&key, (member.as_str(),)).await?;
                    continue;
                };
                let mut live = true;
                for (name, id) in &tags {
                    if !ids.contains_key(name) {
                        let current = match self.get(&format!("tag:{name}:key")).await? {
                            Some(Value::String(current)) => Some(current),
                            _ => None,
                        };
                        ids.insert(name.clone(), current);
                    }
                    live &= ids[name].as_deref() == Some(id.as_str());
                }
                let namespace = tags.iter().map(|(_, id)| id.as_str()).collect::<Vec<_>>().join("|");
                let hash = namespace_key(&namespace);
                if !live {
                    stale.insert(hash.clone());
                }
                records.push((key.clone(), member, hash, live));
            }
        }
        if records.is_empty() {
            return Ok(());
        }

        // A single pass over the store's keys deletes the items of stale
        // namespaces, and notes which namespaces still hold items.
        let mut present: HashSet<String> = HashSet::new();
        let mut orphans: Vec<String> = Vec::new();
        for key in self.scan(&format!("{}*", self.prefix)).await? {
            let Some(hash) = key.strip_prefix(&self.prefix).and_then(tagged_namespace) else {
                continue;
            };
            if stale.contains(hash) {
                orphans.push(key.clone());
            } else {
                present.insert(hash.to_string());
            }
        }
        for chunk in orphans.chunks(1000) {
            connection.del(chunk.to_vec()).await?;
        }

        // Redis removes a set once its last member is gone.
        for (key, member, hash, live) in records {
            if !live || !present.contains(&hash) {
                connection.srem(&key, (member.as_str(),)).await?;
            }
        }
        Ok(())
    }

    /// Every key matching the pattern (relative to the connection's own
    /// prefix), found with `SCAN` so Redis is never blocked.
    async fn scan(&self, pattern: &str) -> Result<Vec<String>> {
        let connection = self.connection()?;
        let connection_prefix = connection.prefix().to_string();
        let pattern = format!("{connection_prefix}{pattern}");
        let mut cursor = "0".to_string();
        let mut keys = Vec::new();
        loop {
            let (next, chunk): (String, Vec<String>) = connection
                .query("scan", (cursor.as_str(), "match", pattern.as_str(), "count", 1000))
                .await?;
            keys.extend(chunk.into_iter().map(|key| match key.strip_prefix(&connection_prefix) {
                Some(key) if !connection_prefix.is_empty() => key.to_string(),
                _ => key,
            }));
            if next == "0" {
                break;
            }
            cursor = next;
        }
        keys.sort();
        keys.dedup();
        Ok(keys)
    }
}

/// The namespace of a tagged item's key (`{sha1}:{key}`), if it is one.
fn tagged_namespace(key: &str) -> Option<&str> {
    let (hash, rest) = key.split_at_checked(40)?;
    (rest.starts_with(':') && hash.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(hash)
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

    #[test]
    fn tagged_item_keys_are_recognized() {
        let hash = namespace_key("a|b");
        assert_eq!(tagged_namespace(&format!("{hash}:name")), Some(hash.as_str()));
        assert_eq!(tagged_namespace("tag:people:key"), None);
        assert_eq!(tagged_namespace(&hash), None);
    }

    #[tokio::test]
    async fn stale_tags_are_pruned() {
        use crate::CacheServiceProvider;
        use illuminate_container::ServiceProvider;
        use illuminate_redis::RedisServiceProvider;
        use illuminate_redis::testing::RedisServer;

        let server = RedisServer::shared();
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container.clone());
        container.instance(Config::new(json!({
            "app": {"name": "Laravel"},
            "database": {"redis": server.config()},
            "cache": {
                "default": "redis",
                "stores": {"redis": {"driver": "redis", "connection": "cache"}},
                "prefix": "app-",
            },
        })));
        RedisServiceProvider.register(&container);
        CacheServiceProvider.register(&container);

        let manager = container.make::<CacheManager>();
        let cache = manager.store("redis").unwrap();
        let redis = RedisStore::of(&cache.get_store()).expect("the store is a Redis store");
        assert_eq!(redis.get_prefix(), "app-");
        assert!(RedisStore::of(&manager.repository(crate::ArrayStore::new()).get_store()).is_none());

        cache.tags(["people", "artists"]).unwrap().forever("John", "Lennon").await.unwrap();
        cache.tags(["people", "authors"]).unwrap().forever("Anne", "Rice").await.unwrap();
        cache.tags(["people", "authors"]).unwrap().put("Mary", "Shelley", 1).await.unwrap();
        assert_eq!(redis.current_tags().await.unwrap(), ["artists", "authors", "people"]);

        // Flushing `authors` orphans Anne, who was stored forever.
        cache.tags(["authors"]).unwrap().flush().await.unwrap();
        let connection = redis.connection().unwrap();
        let before = connection.keys("app-*").await.unwrap();
        assert_eq!(before.iter().filter(|key| tagged_namespace(&key[4..]).is_some()).count(), 3);

        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        redis.flush_stale_tags().await.unwrap();

        let after = connection.keys("app-*").await.unwrap();
        let items: Vec<&String> = after.iter().filter(|key| tagged_namespace(&key[4..]).is_some()).collect();
        assert_eq!(items.len(), 1, "{after:?}");
        assert_eq!(
            cache.tags(["people", "artists"]).unwrap().get("John").await.unwrap(),
            Some(json!("Lennon"))
        );
        // Only the live namespace is still recorded; `authors` has none left.
        assert_eq!(connection.scard("app-tag:people:entries").await.unwrap(), 1);
        assert_eq!(connection.scard("app-tag:authors:entries").await.unwrap(), 0);
        assert_eq!(redis.current_tags().await.unwrap(), ["artists", "people"]);

        // Pruning again changes nothing.
        redis.flush_stale_tags().await.unwrap();
        assert_eq!(connection.keys("app-*").await.unwrap().len(), after.len());
    }
}
