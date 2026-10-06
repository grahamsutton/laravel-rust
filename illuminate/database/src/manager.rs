//! The database manager: resolves connections from configuration.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider, try_app};
use illuminate_support::{Map, Value, ValueExt, json};

use crate::connection::{Connection, Listeners, QueryExecuted};
use crate::migrations::MigrationRegistry;
use crate::seeder::SeederRegistry;

tokio::task_local! {
    /// A task-scoped override of the default connection name.
    static DEFAULT_CONNECTION: String;
}

/// The database manager: creates and caches the configured connections.
///
/// Connections are described in `config/database.php`'s shape:
///
/// ```
/// use illuminate_database::DatabaseManager;
/// use illuminate_support::json;
///
/// let db = DatabaseManager::from_config(json!({
///     "default": "sqlite",
///     "connections": {
///         "sqlite": {"driver": "sqlite", "database": ":memory:", "prefix": ""},
///     },
/// }));
///
/// assert_eq!(db.get_default_connection(), "sqlite");
/// assert_eq!(db.connection("sqlite").get_driver_name(), "sqlite");
/// ```
pub struct DatabaseManager {
    config: Arc<Repository>,
    connections: RwLock<HashMap<String, Connection>>,
    default: RwLock<Option<String>>,
    listeners: Listeners,
}

impl DatabaseManager {
    /// Create a manager reading the `database` configuration from the repository.
    pub fn new(config: Arc<Repository>) -> Self {
        Self {
            config,
            connections: RwLock::new(HashMap::new()),
            default: RwLock::new(None),
            listeners: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Create a manager from a `database` configuration value
    /// (`{"default": ..., "connections": {...}}`).
    pub fn from_config(database: Value) -> Self {
        Self::new(Arc::new(Repository::new(json!({ "database": database }))))
    }

    /// Resolve the manager from the container, registering one built from the
    /// configuration repository when none has been bound yet.
    pub fn resolve() -> Arc<DatabaseManager> {
        if let Some(manager) = try_app::<DatabaseManager>() {
            return manager;
        }
        let container = Container::get_instance();
        container.singleton_if::<DatabaseManager>(|c| {
            let config = c
                .try_make::<Repository>()
                .unwrap_or_else(|_| Arc::new(Repository::empty()));
            Arc::new(DatabaseManager::new(config))
        });
        container.make::<DatabaseManager>()
    }

    /// Get a database connection instance by name.
    ///
    /// Unknown connections still return a handle; every query on it fails
    /// with `Database connection [name] not configured.`.
    pub fn connection(&self, name: &str) -> Connection {
        if let Some(connection) = self.connections.read().unwrap().get(name) {
            return connection.clone();
        }
        let config = self.config.get(&format!("database.connections.{name}"));
        if !config.is_object() {
            return Connection::unconfigured(name, self.listeners.clone());
        }
        let config = Self::configure(name, config);
        let connection =
            Connection::with_listeners(name.to_string(), config, self.listeners.clone());
        self.connections
            .write()
            .unwrap()
            .entry(name.to_string())
            .or_insert(connection)
            .clone()
    }

    /// Get the default connection instance.
    pub fn default_connection(&self) -> Connection {
        self.connection(&self.get_default_connection())
    }

    fn configure(name: &str, config: Value) -> Value {
        let mut map = match config {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        map.insert("name".into(), Value::String(name.to_string()));
        if !map.get("prefix").is_some_and(|p| !p.is_null()) {
            map.insert("prefix".into(), Value::String(String::new()));
        }
        Value::Object(map)
    }

    /// Get the default connection name.
    pub fn get_default_connection(&self) -> String {
        if let Ok(name) = DEFAULT_CONNECTION.try_with(|name| name.clone()) {
            return name;
        }
        if let Some(name) = self.default.read().unwrap().clone() {
            return name;
        }
        self.config.string_or("database.default", "sqlite")
    }

    /// Set the default connection name.
    pub fn set_default_connection(&self, name: &str) {
        *self.default.write().unwrap() = Some(name.to_string());
    }

    /// Run the future with a different default connection, for this task only.
    pub async fn using_connection<F: Future>(&self, name: &str, future: F) -> F::Output {
        DEFAULT_CONNECTION.scope(name.to_string(), future).await
    }

    /// Disconnect from the given database and remove it from the cache.
    pub async fn purge(&self, name: &str) {
        let connection = self.connections.write().unwrap().remove(name);
        if let Some(connection) = connection {
            connection.disconnect().await;
        }
    }

    /// Disconnect from the given database.
    pub async fn disconnect(&self, name: &str) {
        let connection = self.connections.read().unwrap().get(name).cloned();
        if let Some(connection) = connection {
            connection.disconnect().await;
        }
    }

    /// Reconnect to the given database.
    pub async fn reconnect(&self, name: &str) -> illuminate_support::Result<Connection> {
        let connection = self.connection(name);
        connection.reconnect().await?;
        Ok(connection)
    }

    /// Get the names of the connections that have been resolved.
    pub fn get_connections(&self) -> Vec<String> {
        let mut names: Vec<String> = self.connections.read().unwrap().keys().cloned().collect();
        names.sort();
        names
    }

    /// Get the drivers the framework supports.
    pub fn supported_drivers(&self) -> Vec<&'static str> {
        vec!["mysql", "mariadb", "pgsql", "sqlite"]
    }

    /// Register a listener called for every query on every connection.
    pub fn listen(&self, callback: impl Fn(&QueryExecuted) + Send + Sync + 'static) {
        self.listeners.write().unwrap().push(Arc::new(callback));
    }

    /// Get the configuration of a connection.
    pub fn get_config(&self, name: &str) -> Value {
        self.config.get(&format!("database.connections.{name}"))
    }

    /// The name of the migration repository table.
    pub fn migrations_table(&self) -> String {
        match self.config.get("database.migrations") {
            Value::String(table) => table,
            Value::Object(map) => map
                .get("table")
                .map(|t| t.to_string_lossy())
                .unwrap_or_else(|| "migrations".into()),
            _ => "migrations".into(),
        }
    }
}

/// Registers the database services: the [`DatabaseManager`], the default
/// [`Connection`], and the migration / seeder registries.
pub struct DatabaseServiceProvider;

impl ServiceProvider for DatabaseServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<DatabaseManager>(|c| {
            let config = c
                .try_make::<Repository>()
                .unwrap_or_else(|_| Arc::new(Repository::empty()));
            Arc::new(DatabaseManager::new(config))
        });
        app.bind::<Connection>(|c| Arc::new(c.make::<DatabaseManager>().default_connection()));
        app.singleton_if::<MigrationRegistry>(|_| Arc::new(MigrationRegistry::new()));
        app.singleton_if::<SeederRegistry>(|_| Arc::new(SeederRegistry::new()));
    }
}
