//! The session manager and the session configuration.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use illuminate_config::Repository;
use illuminate_container::{Container, try_app};
use illuminate_cookie::CookieJar;
use illuminate_encryption::encrypter;
use illuminate_http::SameSite;
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Result, Str, Value, ValueExt};

use illuminate_cache::Cache;
use illuminate_database::DatabaseManager;

use crate::handlers::{
    ArraySessionHandler, CacheBasedSessionHandler, CookieSessionHandler, DatabaseSessionHandler,
    FileSessionHandler, NullSessionHandler, SessionHandler,
};
use crate::store::Store;

/// A custom session driver: builds the handler for a new session.
pub type HandlerFactory = Arc<dyn Fn(&Container) -> Arc<dyn SessionHandler> + Send + Sync>;

/// A snapshot of the `session.*` configuration, with Laravel's defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionConfig {
    /// The session driver (`file`, `cookie`, `database`, `array`, ...).
    /// `None` disables sessions entirely.
    pub driver: Option<String>,
    /// Idle minutes before the session expires.
    pub lifetime: i64,
    /// Expire the session cookie when the browser closes.
    pub expire_on_close: bool,
    /// Encrypt the session payload before it is stored.
    pub encrypt: bool,
    /// The directory used by the `file` driver.
    pub files: PathBuf,
    /// The connection used by the `database` and `redis` drivers.
    pub connection: Option<String>,
    /// The cache store used by the cache-backed drivers (`redis`,
    /// `memcached`, `dynamodb`, `apc`); the driver's name when unset.
    pub store: Option<String>,
    /// The table used by the `database` driver.
    pub table: String,
    /// The garbage collection lottery: `(wins, out_of)`.
    pub lottery: (u32, u32),
    /// The session cookie's name.
    pub cookie: String,
    /// The session cookie's path.
    pub path: String,
    /// The session cookie's domain.
    pub domain: Option<String>,
    /// Only send the cookie over HTTPS (`None` follows the request).
    pub secure: Option<bool>,
    /// Hide the cookie from JavaScript.
    pub http_only: bool,
    /// The cookie's `SameSite` policy.
    pub same_site: Option<SameSite>,
    /// Mark the cookie as partitioned (CHIPS).
    pub partitioned: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self::from_repository(&Repository::empty())
    }
}

impl SessionConfig {
    /// Read the configuration from the repository.
    ///
    /// ```
    /// use illuminate_config::Repository;
    /// use illuminate_session::SessionConfig;
    /// use illuminate_support::json;
    ///
    /// let config = SessionConfig::from_repository(&Repository::new(json!({
    ///     "app": {"name": "My App"},
    ///     "session": {"driver": "file", "lifetime": 60},
    /// })));
    ///
    /// assert_eq!(config.driver.as_deref(), Some("file"));
    /// assert_eq!(config.lifetime, 60);
    /// assert_eq!(config.cookie, "my_app_session");
    /// assert_eq!(config.lottery, (2, 100));
    /// ```
    pub fn from_repository(config: &Repository) -> Self {
        let optional_string = |key: &str| match config.get(key) {
            Value::Null => None,
            value => Some(value.to_string_lossy()).filter(|s| !s.is_empty()),
        };
        let lottery = match config.get("session.lottery") {
            Value::Array(odds) if odds.len() == 2 => (
                odds[0].to_i64_lossy().unwrap_or(2).max(0) as u32,
                odds[1].to_i64_lossy().unwrap_or(100).max(1) as u32,
            ),
            _ => (2, 100),
        };
        let cookie = optional_string("session.cookie").unwrap_or_else(|| {
            format!(
                "{}_session",
                Str::slug_with(&config.string_or("app.name", "laravel"), "_")
            )
        });

        Self {
            driver: optional_string("session.driver"),
            lifetime: config.integer_or("session.lifetime", 120),
            expire_on_close: config.boolean("session.expire_on_close"),
            encrypt: config.boolean("session.encrypt"),
            files: optional_string("session.files")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("storage/framework/sessions")),
            connection: optional_string("session.connection"),
            store: optional_string("session.store"),
            table: config.string_or("session.table", "sessions"),
            lottery,
            cookie,
            path: config.string_or("session.path", "/"),
            domain: optional_string("session.domain"),
            secure: match config.get("session.secure") {
                Value::Null => None,
                _ => Some(config.boolean("session.secure")),
            },
            http_only: match config.get("session.http_only") {
                Value::Null => true,
                _ => config.boolean("session.http_only"),
            },
            same_site: SameSite::parse(&config.string("session.same_site")),
            partitioned: config.boolean("session.partitioned"),
        }
    }

    /// The session lifetime in seconds.
    pub fn lifetime_seconds(&self) -> u64 {
        self.lifetime.max(0) as u64 * 60
    }
}

/// Builds session stores for the configured driver.
///
/// Every call to [`SessionManager::driver`] creates a fresh [`Store`] (one per
/// request); handlers that keep state between requests — like the `array`
/// driver — are shared by the manager.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_session::SessionManager;
/// use illuminate_support::json;
///
/// let manager = SessionManager::new(Arc::new(Repository::new(json!({
///     "session": {"driver": "array", "cookie": "laravel_session"},
/// }))));
///
/// let session = manager.driver().unwrap();
/// assert_eq!(session.name(), "laravel_session");
/// ```
pub struct SessionManager {
    config: Arc<Repository>,
    custom_creators: RwLock<HashMap<String, HandlerFactory>>,
    shared_handlers: RwLock<HashMap<String, Arc<dyn SessionHandler>>>,
}

impl SessionManager {
    /// Create a new manager reading the `session.*` configuration.
    pub fn new(config: Arc<Repository>) -> Self {
        Self {
            config,
            custom_creators: RwLock::new(HashMap::new()),
            shared_handlers: RwLock::new(HashMap::new()),
        }
    }

    /// The configuration repository the manager reads from.
    pub fn repository(&self) -> &Repository {
        &self.config
    }

    /// The session configuration.
    pub fn get_session_config(&self) -> SessionConfig {
        SessionConfig::from_repository(&self.config)
    }

    /// The default session driver name.
    pub fn get_default_driver(&self) -> Option<String> {
        self.get_session_config().driver
    }

    /// Set the default session driver name.
    pub fn set_default_driver(&self, name: &str) {
        self.config.set("session.driver", name);
    }

    /// Determine if a session driver has been configured.
    pub fn session_configured(&self) -> bool {
        self.get_default_driver().is_some()
    }

    /// The name of the session cookie.
    pub fn cookie_name(&self) -> String {
        self.get_session_config().cookie
    }

    /// Build a new session store for the default driver.
    pub fn driver(&self) -> Result<Arc<Store>> {
        let driver = self.get_default_driver().ok_or_else(|| {
            InvalidArgumentException::new("No session driver has been configured.")
        })?;
        self.driver_named(&driver)
    }

    /// Build a new session store for the given driver.
    pub fn driver_named(&self, name: &str) -> Result<Arc<Store>> {
        let handler = self.handler(name)?;
        self.build_session(handler)
    }

    /// Create the handler for the given driver.
    pub fn handler(&self, name: &str) -> Result<Arc<dyn SessionHandler>> {
        let creator = self.custom_creators.read().unwrap().get(name).cloned();
        if let Some(creator) = creator {
            return Ok(creator(&Container::get_instance()));
        }

        let config = self.get_session_config();
        match name {
            "array" => {
                Ok(self
                    .shared_handler(name, || Arc::new(ArraySessionHandler::new(config.lifetime))))
            }
            "file" | "native" => Ok(Arc::new(FileSessionHandler::new(
                &config.files,
                config.lifetime,
            ))),
            "cookie" => {
                let jar = try_app::<CookieJar>()
                    .unwrap_or_else(|| Arc::new(CookieJar::from_config(&self.config)));
                Ok(Arc::new(CookieSessionHandler::new(
                    jar,
                    config.lifetime,
                    config.expire_on_close,
                )))
            }
            "database" => Ok(Arc::new(self.create_database_handler(&config))),
            "null" => Ok(Arc::new(NullSessionHandler)),
            "redis" => Ok(Arc::new(self.create_redis_handler(&config)?)),
            "memcached" | "dynamodb" | "apc" => {
                Ok(Arc::new(self.create_cache_handler(name, &config)?))
            }
            _ => {
                Err(InvalidArgumentException::new(format!("Driver [{name}] not supported.")).into())
            }
        }
    }

    /// Create a handler for the `database` driver: sessions live in
    /// `session.table` on the `session.connection` connection (the default
    /// connection when unset). Each session gets its own handler, since the
    /// handler tracks whether its row exists.
    fn create_database_handler(&self, config: &SessionConfig) -> DatabaseSessionHandler {
        let db = DatabaseManager::resolve();
        let connection = match &config.connection {
            Some(name) => db.connection(name),
            None => db.default_connection(),
        };
        DatabaseSessionHandler::new(connection, config.table.clone(), config.lifetime)
    }

    /// Create a handler keeping sessions in the cache store named by
    /// `session.store` (the driver's own name when unset).
    fn create_cache_handler(
        &self,
        driver: &str,
        config: &SessionConfig,
    ) -> Result<CacheBasedSessionHandler> {
        let store = config.store.as_deref().unwrap_or(driver);
        let cache = Cache::manager()?.store(store)?;
        Ok(CacheBasedSessionHandler::new(cache, config.lifetime))
    }

    /// Create a handler for the `redis` driver: sessions live in the cache
    /// store named by `session.store` (default `redis`), on the Redis
    /// connection named by `session.connection` (the `default` connection
    /// when unset).
    fn create_redis_handler(&self, config: &SessionConfig) -> Result<CacheBasedSessionHandler> {
        let store = config.store.as_deref().unwrap_or("redis");
        let mut store_config = self.config.get(&format!("cache.stores.{store}"));
        if !store_config.is_object() {
            return Err(InvalidArgumentException::new(format!(
                "Cache store [{store}] is not defined."
            ))
            .into());
        }
        store_config["store"] = Value::from(store);
        if store_config.get("driver").and_then(Value::as_str) == Some("redis") {
            store_config["connection"] =
                Value::from(config.connection.as_deref().unwrap_or("default"));
        }
        let cache = Cache::manager()?.build(store_config)?;
        Ok(CacheBasedSessionHandler::new(cache, config.lifetime))
    }

    fn shared_handler(
        &self,
        name: &str,
        create: impl FnOnce() -> Arc<dyn SessionHandler>,
    ) -> Arc<dyn SessionHandler> {
        if let Some(handler) = self.shared_handlers.read().unwrap().get(name) {
            return handler.clone();
        }
        self.shared_handlers
            .write()
            .unwrap()
            .entry(name.to_string())
            .or_insert_with(create)
            .clone()
    }

    /// Wrap a handler in a session store, encrypting it when `session.encrypt` is on.
    pub fn build_session(&self, handler: Arc<dyn SessionHandler>) -> Result<Arc<Store>> {
        let config = self.get_session_config();
        let store = Store::new(config.cookie, handler, None);
        if config.encrypt {
            return Ok(Arc::new(store.with_encrypter(encrypter()?)));
        }
        Ok(Arc::new(store))
    }

    /// Register a custom session driver.
    ///
    /// The factory runs for every new session (once per request), so
    /// request-specific handlers stay isolated; return a shared `Arc` for
    /// handlers that should live as long as the application.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_config::Repository;
    /// use illuminate_session::{NullSessionHandler, SessionManager};
    /// use illuminate_support::json;
    ///
    /// let manager = SessionManager::new(Arc::new(Repository::new(json!({"session": {"driver": "mongo"}}))));
    /// manager.extend("mongo", |_app| Arc::new(NullSessionHandler));
    ///
    /// assert!(manager.driver().is_ok());
    /// ```
    pub fn extend(
        &self,
        driver: impl Into<String>,
        factory: impl Fn(&Container) -> Arc<dyn SessionHandler> + Send + Sync + 'static,
    ) -> &Self {
        let driver = driver.into();
        self.shared_handlers.write().unwrap().remove(&driver);
        self.custom_creators
            .write()
            .unwrap()
            .insert(driver, Arc::new(factory));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_container::ServiceProvider;
    use illuminate_encryption::{EncryptionServiceProvider, MissingAppKeyException};
    use illuminate_support::json;

    fn manager(config: Value) -> SessionManager {
        SessionManager::new(Arc::new(Repository::new(config)))
    }

    #[test]
    fn it_reads_laravels_defaults() {
        let config = SessionConfig::default();
        assert_eq!(config.driver, None);
        assert_eq!(config.lifetime, 120);
        assert_eq!(config.cookie, "laravel_session");
        assert_eq!(config.path, "/");
        assert_eq!(config.secure, None);
        assert!(config.http_only);
        assert_eq!(config.same_site, None);
        assert_eq!(config.lottery, (2, 100));
        assert_eq!(config.table, "sessions");
        assert_eq!(config.lifetime_seconds(), 7200);
    }

    #[test]
    fn it_reads_configuration() {
        let config = SessionConfig::from_repository(&Repository::new(json!({
            "session": {
                "driver": "cookie", "lifetime": "30", "expire_on_close": true, "encrypt": "true",
                "files": "/tmp/sessions", "cookie": "my_session", "path": "/app", "domain": "laravel.com",
                "secure": true, "http_only": false, "same_site": "strict", "partitioned": true, "lottery": [1, 50],
                "connection": "sqlite",
            },
        })));
        assert_eq!(config.driver.as_deref(), Some("cookie"));
        assert_eq!(config.lifetime, 30);
        assert!(config.expire_on_close && config.encrypt && config.partitioned);
        assert_eq!(config.files, PathBuf::from("/tmp/sessions"));
        assert_eq!(config.cookie, "my_session");
        assert_eq!(config.domain.as_deref(), Some("laravel.com"));
        assert_eq!(config.secure, Some(true));
        assert!(!config.http_only);
        assert_eq!(config.same_site, Some(SameSite::Strict));
        assert_eq!(config.lottery, (1, 50));
        assert_eq!(config.connection.as_deref(), Some("sqlite"));
    }

    #[test]
    fn it_builds_stores_for_each_driver() {
        let manager = manager(json!({"session": {"driver": "array", "cookie": "sess"}}));
        assert!(manager.session_configured());
        assert_eq!(manager.cookie_name(), "sess");

        let a = manager.driver().unwrap();
        let b = manager.driver().unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
        assert!(
            Arc::ptr_eq(&a.get_handler(), &b.get_handler()),
            "the array handler is shared"
        );

        for driver in ["file", "cookie", "null"] {
            assert!(manager.driver_named(driver).is_ok(), "{driver}");
        }
        assert!(
            manager
                .driver_named("cookie")
                .unwrap()
                .handler_needs_request()
        );

        assert_eq!(
            manager.driver_named("redis").unwrap_err().to_string(),
            "Cache store [redis] is not defined."
        );
        assert_eq!(
            manager.driver_named("nope").unwrap_err().to_string(),
            "Driver [nope] not supported."
        );

        manager.set_default_driver("null");
        assert_eq!(manager.get_default_driver().as_deref(), Some("null"));
    }

    #[test]
    fn a_missing_driver_means_no_sessions() {
        let manager = manager(json!({"session": {"driver": null}}));
        assert!(!manager.session_configured());
        assert!(manager.driver().is_err());
    }

    #[test]
    fn custom_drivers_can_be_registered() {
        let manager = manager(json!({"session": {"driver": "database"}}));
        manager.extend("database", |_| Arc::new(ArraySessionHandler::new(5)));
        assert!(manager.driver().is_ok());
    }

    #[tokio::test]
    async fn cache_backed_drivers_use_a_cache_store() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container.clone());
        let config = Arc::new(Repository::new(json!({
            "session": {"driver": "memcached", "store": "sessions", "lifetime": 30},
            "cache": {"stores": {"sessions": {"driver": "array"}}},
        })));
        container.instance_arc(config.clone());
        let manager = SessionManager::new(config.clone());
        assert_eq!(
            manager.get_session_config().store.as_deref(),
            Some("sessions")
        );

        let handler = manager.handler("memcached").unwrap();
        handler.write("abc", "payload").await.unwrap();
        assert_eq!(handler.read("abc").await.unwrap(), "payload");
        // Every handler shares the cache store.
        assert_eq!(
            manager.handler("apc").unwrap().read("abc").await.unwrap(),
            "payload"
        );
        handler.destroy("abc").await.unwrap();
        assert_eq!(handler.read("abc").await.unwrap(), "");
        assert_eq!(handler.gc(60).await.unwrap(), 0);

        // The redis driver needs a redis store; others are used as they are.
        config.set("session.store", "missing");
        assert_eq!(
            manager.handler("dynamodb").err().unwrap().to_string(),
            "Cache store [missing] is not defined."
        );
        config.set("session.store", "sessions");
        let handler = manager.handler("redis").unwrap();
        handler.write("xyz", "data").await.unwrap();
        assert_eq!(handler.read("xyz").await.unwrap(), "data");
    }

    #[test]
    fn encrypted_sessions_need_a_key() {
        let container = Arc::new(Container::new());
        container.instance(Repository::new(json!({"app": {"key": null}})));
        EncryptionServiceProvider.register(&container);
        let _guard = Container::set_local_instance(container.clone());

        let manager = manager(json!({"session": {"driver": "array", "encrypt": true}}));
        let error = manager.driver().unwrap_err();
        assert!(error.downcast_ref::<MissingAppKeyException>().is_some());

        container.instance(Repository::new(json!({"app": {"key": "a".repeat(32)}})));
        container.forget_instance::<illuminate_encryption::Encrypter>();
        assert!(manager.driver().unwrap().is_encrypted());
    }
}
