//! The Redis manager: resolves the connections configured under
//! `database.redis`.

use std::collections::HashMap;
use std::sync::RwLock;

use illuminate_config::Repository;
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Result, Str, Value, json};

use crate::config::ConnectionConfig;
use crate::connection::Connection;

/// Keys of `database.redis` that aren't connections.
const RESERVED_KEYS: &[&str] = &["client", "options", "clusters"];

/// Resolves and caches the Redis connections (Laravel's `RedisManager`).
///
/// ```
/// use illuminate_redis::RedisManager;
/// use illuminate_support::json;
///
/// let redis = RedisManager::new("phpredis", json!({
///     "options": {"prefix": "laravel-database-"},
///     "default": {"host": "127.0.0.1", "port": 6379, "database": 0},
///     "cache": {"url": "redis://127.0.0.1:6379/1"},
/// }));
///
/// let cache = redis.connection("cache").unwrap();
/// assert_eq!(cache.config().database, 1);
/// assert_eq!(cache.prefix(), "laravel-database-");
///
/// // Connections are created once and shared...
/// assert!(cache.same_as(&redis.connection("cache").unwrap()));
/// assert_eq!(redis.connection(None).unwrap().name(), "default");
/// ```
pub struct RedisManager {
    client: String,
    config: Value,
    connections: RwLock<HashMap<String, Connection>>,
}

impl std::fmt::Debug for RedisManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisManager")
            .field("client", &self.client)
            .field("connections", &self.connections())
            .finish()
    }
}

impl RedisManager {
    /// Create a manager for the given `database.redis` configuration (without
    /// its `client` key).
    pub fn new(client: impl Into<String>, config: Value) -> Self {
        Self {
            client: client.into(),
            config,
            connections: RwLock::new(HashMap::new()),
        }
    }

    /// Create a manager from the application's configuration, the way
    /// Laravel's `config/database.php` sets it up: `database.redis.client`,
    /// the shared `options` — whose `prefix` defaults to the slug of
    /// `app.name` followed by `-database-` — and the named connections.
    ///
    /// When `database.redis` isn't configured at all, Laravel's defaults are
    /// used: a `default` connection to database 0 and a `cache` connection
    /// to database 1 of the server at `127.0.0.1:6379`.
    pub fn from_config(config: &Repository) -> Self {
        let mut redis = match config.get("database.redis") {
            Value::Object(redis) => Value::Object(redis),
            _ => json!({
                "client": "phpredis",
                "options": {"cluster": "redis"},
                "default": {"host": "127.0.0.1", "port": 6379, "database": 0},
                "cache": {"host": "127.0.0.1", "port": 6379, "database": 1},
            }),
        };

        if !redis.get("options").is_some_and(Value::is_object) {
            redis["options"] = json!({});
        }
        if redis["options"].get("prefix").is_none_or(Value::is_null) {
            redis["options"]["prefix"] = Value::from(format!(
                "{}-database-",
                Str::slug(&config.string_or("app.name", "laravel"))
            ));
        }

        let client = match redis
            .as_object_mut()
            .and_then(|redis| redis.remove("client"))
        {
            Some(Value::String(client)) if !client.is_empty() => client,
            _ => "phpredis".to_string(),
        };
        Self::new(client, redis)
    }

    /// Get a Redis connection by name (`None` for the `default` connection).
    pub fn connection<'a>(&self, name: impl Into<Option<&'a str>>) -> Result<Connection> {
        let name = connection_name(name.into());
        if let Some(connection) = self.connections.read().unwrap().get(name) {
            return Ok(connection.clone());
        }
        let connection = self.resolve(Some(name))?;
        Ok(self
            .connections
            .write()
            .unwrap()
            .entry(name.to_string())
            .or_insert(connection)
            .clone())
    }

    /// Build the given connection, bypassing the connection cache.
    pub fn resolve(&self, name: Option<&str>) -> Result<Connection> {
        let name = connection_name(name);
        let options = self.config.get("options").cloned().unwrap_or(Value::Null);

        if let Some(config) = self
            .config
            .get(name)
            .filter(|_| !RESERVED_KEYS.contains(&name))
            .filter(|config| config.is_object())
        {
            return Connection::new(name, ConnectionConfig::parse(config, &options)?);
        }

        if self
            .config
            .get("clusters")
            .and_then(|clusters| clusters.get(name))
            .is_some()
        {
            return Err(InvalidArgumentException::new(format!(
                "Redis cluster [{name}] is configured, but Redis clusters are not supported yet."
            ))
            .into());
        }

        Err(
            InvalidArgumentException::new(format!("Redis connection [{name}] not configured."))
                .into(),
        )
    }

    /// The names of the connections that have been created.
    pub fn connections(&self) -> Vec<String> {
        let mut names: Vec<String> = self.connections.read().unwrap().keys().cloned().collect();
        names.sort();
        names
    }

    /// Disconnect the given connection (the default one when `None`) and
    /// forget it, so the next use connects again.
    pub fn purge(&self, name: Option<&str>) {
        if let Some(connection) = self
            .connections
            .write()
            .unwrap()
            .remove(connection_name(name))
        {
            connection.disconnect();
        }
    }

    /// The configured client (`phpredis` or `predis` in Laravel; every client
    /// uses the same native driver here).
    pub fn client(&self) -> &str {
        &self.client
    }

    /// The Redis configuration.
    pub fn config(&self) -> &Value {
        &self.config
    }
}

fn connection_name(name: Option<&str>) -> &str {
    match name {
        Some(name) if !name.is_empty() => name,
        _ => "default",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connections_are_resolved_and_cached() {
        let manager = RedisManager::new(
            "phpredis",
            json!({
                "options": {"prefix": "app:"},
                "default": {"host": "127.0.0.1"},
                "cache": {"host": "127.0.0.1", "database": 1, "prefix": "cache:"},
            }),
        );
        assert!(manager.connections().is_empty());

        let default = manager.connection(None).unwrap();
        assert_eq!(default.name(), "default");
        assert_eq!(default.prefix(), "app:");
        assert!(default.same_as(&manager.connection("default").unwrap()));
        assert!(default.same_as(&manager.connection(Some("")).unwrap()));

        let cache = manager.connection("cache").unwrap();
        assert_eq!(cache.prefix(), "cache:");
        assert_eq!(cache.config().database, 1);
        assert_eq!(manager.connections(), ["cache", "default"]);

        // Resolving bypasses the cache...
        assert!(!manager.resolve(Some("cache")).unwrap().same_as(&cache));

        manager.purge(Some("cache"));
        assert_eq!(manager.connections(), ["default"]);
        assert!(!manager.connection("cache").unwrap().same_as(&cache));
        manager.purge(None);
        assert_eq!(manager.connections(), ["cache"]);
        assert_eq!(manager.client(), "phpredis");
        assert!(manager.config().is_object());
        assert!(format!("{manager:?}").contains("phpredis"));
    }

    #[test]
    fn unknown_connections_are_reported() {
        let manager = RedisManager::new(
            "phpredis",
            json!({"options": {}, "clusters": {"default": [{"host": "127.0.0.1"}]}}),
        );
        assert_eq!(
            manager.connection("missing").unwrap_err().to_string(),
            "Redis connection [missing] not configured."
        );
        assert_eq!(
            manager.connection("options").unwrap_err().to_string(),
            "Redis connection [options] not configured."
        );
        assert!(
            manager
                .connection(None)
                .unwrap_err()
                .to_string()
                .contains("clusters are not supported")
        );
    }

    #[test]
    fn it_reads_the_application_configuration() {
        let manager = RedisManager::from_config(&Repository::new(json!({
            "app": {"name": "My App"},
            "database": {"redis": {
                "client": "predis",
                "options": {"cluster": "redis"},
                "default": {"url": null, "host": "127.0.0.1", "port": "6379", "database": "0"},
            }},
        })));
        assert_eq!(manager.client(), "predis");
        assert!(manager.config().get("client").is_none());
        assert_eq!(
            manager.connection(None).unwrap().prefix(),
            "my-app-database-"
        );

        let manager = RedisManager::from_config(&Repository::new(json!({
            "database": {"redis": {"options": {"prefix": ""}, "default": {}}},
        })));
        assert_eq!(manager.connection(None).unwrap().prefix(), "");
        assert_eq!(manager.client(), "phpredis");
    }

    #[test]
    fn laravels_defaults_apply_without_configuration() {
        let manager = RedisManager::from_config(&Repository::empty());
        let default = manager.connection(None).unwrap();
        assert_eq!(default.config().host, "127.0.0.1");
        assert_eq!(default.config().database, 0);
        assert_eq!(default.prefix(), "laravel-database-");
        assert_eq!(manager.connection("cache").unwrap().config().database, 1);
    }
}
