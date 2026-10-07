//! The cache manager: resolves the configured cache stores.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use illuminate_config::Repository as Config;
use illuminate_container::Container;
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Result, Str, Value, ValueExt, json};

use illuminate_database::{Connection, DatabaseManager};

use crate::array_store::ArrayStore;
use crate::database_store::{DEFAULT_LOCK_LOTTERY, DEFAULT_LOCK_TIMEOUT, DatabaseStore};
use crate::file_store::FileStore;
use crate::null_store::NullStore;
use crate::redis_store::{RedisStore, redis_manager};
use crate::repository::Repository;
use crate::store::Store;

/// Creates a cache repository for a custom driver from the store's configuration.
pub type StoreCreator = Arc<dyn Fn(&Container, &Value) -> Result<Repository> + Send + Sync>;

/// Resolves and caches the stores configured under `cache.stores`.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_cache::CacheManager;
/// use illuminate_config::Repository;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let manager = CacheManager::new(Arc::new(Repository::new(json!({
///     "cache": {"default": "array", "stores": {"array": {"driver": "array"}}},
/// }))));
///
/// manager.store("array").unwrap().put("name", "Taylor", 60).await.unwrap();
///
/// let cache = manager.default_store().unwrap();
/// assert_eq!(cache.string("name").await.unwrap(), "Taylor");
/// # });
/// ```
pub struct CacheManager {
    config: Arc<Config>,
    stores: RwLock<HashMap<String, Repository>>,
    custom_creators: RwLock<HashMap<String, StoreCreator>>,
}

impl CacheManager {
    /// Create a new cache manager reading the given configuration.
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            stores: RwLock::new(HashMap::new()),
            custom_creators: RwLock::new(HashMap::new()),
        }
    }

    /// Get a cache store instance by name.
    pub fn store(&self, name: &str) -> Result<Repository> {
        if let Some(store) = self.stores.read().unwrap().get(name) {
            return Ok(store.clone());
        }
        let store = self.resolve(name)?;
        Ok(self
            .stores
            .write()
            .unwrap()
            .entry(name.to_string())
            .or_insert(store)
            .clone())
    }

    /// Get the default cache store (`cache.default`).
    pub fn default_store(&self) -> Result<Repository> {
        self.store(&self.get_default_driver())
    }

    /// Get a cache driver instance — the default one when `None`.
    pub fn driver(&self, name: Option<&str>) -> Result<Repository> {
        match name {
            Some(name) => self.store(name),
            None => self.default_store(),
        }
    }

    /// Resolve the given store, bypassing the resolved-store cache.
    pub fn resolve(&self, name: &str) -> Result<Repository> {
        let mut config = self.get_config(name);
        if !config.is_object() {
            return Err(InvalidArgumentException::new(format!(
                "Cache store [{name}] is not defined."
            ))
            .into());
        }
        if config.get("store").is_none() {
            config["store"] = json!(name);
        }
        self.build(config)
    }

    /// Build a cache repository with the given configuration.
    pub fn build(&self, mut config: Value) -> Result<Repository> {
        if !config.is_object() {
            return Err(InvalidArgumentException::new(
                "Cache store configuration must be an object.",
            )
            .into());
        }
        if config.get("store").is_none() {
            let name = config
                .get("name")
                .cloned()
                .unwrap_or_else(|| json!("ondemand"));
            config["store"] = name;
        }
        let driver = config
            .get("driver")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let creator = self.custom_creators.read().unwrap().get(&driver).cloned();
        if let Some(creator) = creator {
            return creator(&Container::get_instance(), &config);
        }

        let name = config
            .get("store")
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let store: Arc<dyn Store> = match driver.as_str() {
            "array" => Arc::new(ArrayStore::with_serialization(
                config.get("serialize").is_some_and(ValueExt::truthy),
            )),
            "database" => Arc::new(self.create_database_store(&config)),
            "file" => Arc::new(self.create_file_store(&config)?),
            "null" => Arc::new(NullStore),
            "redis" => Arc::new(self.create_redis_store(&config)),
            other => {
                return Err(InvalidArgumentException::new(format!(
                    "Driver [{other}] is not supported."
                ))
                .into());
            }
        };
        Ok(Repository::from_arc(store).with_name(name))
    }

    fn create_file_store(&self, config: &Value) -> Result<FileStore> {
        let path = config
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| {
                InvalidArgumentException::new("The file cache driver requires a [path].")
            })?;
        let mut store = FileStore::new(PathBuf::from(path));
        if let Some(lock_path) = config.get("lock_path").and_then(Value::as_str) {
            store = store.with_lock_directory(lock_path);
        }
        if let Some(permission) = config.get("permission").and_then(Value::to_i64_lossy) {
            store = store.with_file_permission(permission as u32);
        }
        Ok(store)
    }

    /// Create the `database` store: items in `table` (default `cache`) and
    /// locks in `lock_table` (default `cache_locks`) of the configured
    /// `connection` / `lock_connection` (the default connection when unset).
    fn create_database_store(&self, config: &Value) -> DatabaseStore {
        let option = |key: &str| {
            config
                .get(key)
                .filter(|value| !value.is_null())
                .map(|value| value.to_string_lossy())
                .filter(|value| !value.is_empty())
        };
        let db = DatabaseManager::resolve();
        let connection_named = |name: Option<&String>| -> Connection {
            match name {
                Some(name) => db.connection(name),
                None => db.default_connection(),
            }
        };

        let connection_name = option("connection");
        let lock_connection_name = option("lock_connection").or_else(|| connection_name.clone());
        let lottery = match config.get("lock_lottery") {
            Some(Value::Array(odds)) if odds.len() == 2 => {
                let odd = |value: &Value| {
                    value.to_i64_lossy().unwrap_or(0).clamp(0, u32::MAX as i64) as u32
                };
                Some([odd(&odds[0]), odd(&odds[1])])
            }
            Some(Value::Array(_)) => None,
            _ => Some(DEFAULT_LOCK_LOTTERY),
        };
        let lock_timeout = config
            .get("lock_timeout")
            .and_then(ValueExt::to_i64_lossy)
            .map(|seconds| seconds.max(0) as u64)
            .unwrap_or(DEFAULT_LOCK_TIMEOUT);

        DatabaseStore::new(
            connection_named(connection_name.as_ref()),
            option("table").unwrap_or_else(|| "cache".to_string()),
        )
        .with_prefix(self.get_prefix(config))
        .with_lock_table(option("lock_table").unwrap_or_else(|| "cache_locks".to_string()))
        .with_lock_connection(connection_named(lock_connection_name.as_ref()))
        .with_lock_lottery(lottery)
        .with_default_lock_timeout(lock_timeout)
    }

    /// Create the `redis` store: items on the `connection` Redis connection
    /// (default `cache`) and locks on the `lock_connection` one (default
    /// `default`), both from `database.redis`.
    fn create_redis_store(&self, config: &Value) -> RedisStore {
        let option = |key: &str, default: &str| {
            config
                .get(key)
                .filter(|value| !value.is_null())
                .map(|value| value.to_string_lossy())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| default.to_string())
        };
        RedisStore::new(
            redis_manager(&self.config),
            self.get_prefix(config),
            option("connection", "cache"),
        )
        .with_lock_connection(option("lock_connection", "default"))
    }

    /// The store the rate limiter uses: `cache.limiter`, or the default store.
    pub fn limiter_store(&self) -> Result<Repository> {
        match self.config.get("cache.limiter") {
            Value::String(name) if !name.is_empty() => self.store(&name),
            _ => self.default_store(),
        }
    }

    /// Wrap a store in a repository (handy inside `extend` callbacks).
    pub fn repository(&self, store: impl Store) -> Repository {
        Repository::new(store)
    }

    /// The cache key prefix for a store: its own `prefix`, or `cache.prefix`.
    ///
    /// Stores that share their backend with other applications (database,
    /// Redis, ...) should prefix their keys with it.
    pub fn get_prefix(&self, config: &Value) -> String {
        if let Some(prefix) = config.get("prefix").and_then(Value::as_str) {
            return prefix.to_string();
        }
        match self.config.get("cache.prefix") {
            Value::Null => format!(
                "{}-cache-",
                Str::slug(&self.config.string_or("app.name", "laravel"))
            ),
            prefix => prefix.to_string_lossy(),
        }
    }

    fn get_config(&self, name: &str) -> Value {
        if name == "null" {
            return json!({"driver": "null"});
        }
        self.config.get(&format!("cache.stores.{name}"))
    }

    /// The default cache driver's name (`cache.default`, or `"null"`).
    pub fn get_default_driver(&self) -> String {
        self.config.string_or("cache.default", "null")
    }

    /// Set the default cache driver's name.
    pub fn set_default_driver(&self, name: &str) {
        self.config.set("cache.default", name);
    }

    /// Unset the given resolved stores, so they are resolved fresh next time.
    pub fn forget_driver(&self, names: &[&str]) -> &Self {
        let mut stores = self.stores.write().unwrap();
        for name in names {
            stores.remove(*name);
        }
        self
    }

    /// Disconnect the given store (the default store when `None`).
    pub fn purge(&self, name: Option<&str>) {
        let name = name
            .map(str::to_string)
            .unwrap_or_else(|| self.get_default_driver());
        self.stores.write().unwrap().remove(&name);
    }

    /// Register a custom driver creator.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_cache::{ArrayStore, CacheManager, Repository};
    /// use illuminate_config::Repository as Config;
    /// use illuminate_support::json;
    ///
    /// let manager = CacheManager::new(Arc::new(Config::new(json!({
    ///     "cache": {"stores": {"mongo": {"driver": "mongo"}}},
    /// }))));
    ///
    /// manager.extend("mongo", |_app, _config| Ok(Repository::new(ArrayStore::new())));
    ///
    /// assert!(manager.store("mongo").is_ok());
    /// ```
    pub fn extend(
        &self,
        driver: &str,
        creator: impl Fn(&Container, &Value) -> Result<Repository> + Send + Sync + 'static,
    ) -> &Self {
        self.custom_creators
            .write()
            .unwrap()
            .insert(driver.to_string(), Arc::new(creator));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::freeze_time;
    use illuminate_support::Carbon;

    fn manager(config: Value) -> CacheManager {
        CacheManager::new(Arc::new(Config::new(config)))
    }

    #[tokio::test]
    async fn it_resolves_configured_stores() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let directory = tempfile::tempdir().unwrap();
        let manager = manager(json!({
            "cache": {
                "default": "file",
                "stores": {
                    "array": {"driver": "array", "serialize": true},
                    "file": {"driver": "file", "path": directory.path().join("data").to_string_lossy(), "lock_path": directory.path().join("locks").to_string_lossy()},
                    "broken": {"driver": "mongo"},
                },
            },
        }));

        let file = manager.default_store().unwrap();
        assert_eq!(file.get_name(), Some("file"));
        file.put("a", "b", 60).await.unwrap();
        assert!(directory.path().join("data").is_dir());
        assert!(file.lock("x", 10).get().await.unwrap());
        assert!(directory.path().join("locks").is_dir());

        let array = manager.store("array").unwrap();
        array.put("a", 1, 60).await.unwrap();
        assert!(Arc::ptr_eq(
            &array.get_store(),
            &manager.store("array").unwrap().get_store()
        ));
        assert_eq!(
            manager.store("array").unwrap().integer("a").await.unwrap(),
            1
        );

        let null = manager.store("null").unwrap();
        assert_eq!(null.get_name(), Some("null"));

        assert_eq!(
            manager.store("missing").unwrap_err().to_string(),
            "Cache store [missing] is not defined."
        );
        assert_eq!(
            manager.store("broken").unwrap_err().to_string(),
            "Driver [mongo] is not supported."
        );
    }

    #[tokio::test]
    async fn stores_can_be_forgotten_and_purged() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let manager = manager(
            json!({"cache": {"default": "array", "stores": {"array": {"driver": "array"}}}}),
        );
        manager
            .default_store()
            .unwrap()
            .put("a", 1, 60)
            .await
            .unwrap();
        manager.purge(None);
        assert!(manager.default_store().unwrap().missing("a").await.unwrap());
        manager
            .store("array")
            .unwrap()
            .put("a", 1, 60)
            .await
            .unwrap();
        manager.forget_driver(&["array"]);
        assert!(manager.store("array").unwrap().missing("a").await.unwrap());

        manager.set_default_driver("null");
        assert_eq!(manager.get_default_driver(), "null");
    }

    #[test]
    fn on_demand_stores_and_custom_drivers() {
        let manager =
            manager(json!({"cache": {"stores": {"custom": {"driver": "custom", "answer": 42}}}}));
        let repository = manager.build(json!({"driver": "array"})).unwrap();
        assert_eq!(repository.get_name(), Some("ondemand"));
        let repository = manager
            .build(json!({"driver": "array", "name": "adhoc"}))
            .unwrap();
        assert_eq!(repository.get_name(), Some("adhoc"));

        manager.extend("custom", |_app, config| {
            assert_eq!(config["answer"], 42);
            assert_eq!(config["store"], "custom");
            Ok(Repository::new(NullStore).with_name("custom"))
        });
        assert_eq!(manager.store("custom").unwrap().get_name(), Some("custom"));

        assert!(manager.build(json!({"driver": "file"})).is_err());
        assert_eq!(manager.get_default_driver(), "null");
    }

    #[test]
    fn prefixes_follow_laravels_defaults() {
        let manager = manager(json!({"app": {"name": "My App"}}));
        assert_eq!(manager.get_prefix(&json!({})), "my-app-cache-");
        assert_eq!(manager.get_prefix(&json!({"prefix": "custom_"})), "custom_");

        let manager = manager_with_prefix();
        assert_eq!(manager.get_prefix(&json!({})), "laravel_cache_");
    }

    fn manager_with_prefix() -> CacheManager {
        manager(json!({"cache": {"prefix": "laravel_cache_"}}))
    }
}
