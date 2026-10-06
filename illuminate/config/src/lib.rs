//! # Illuminate Config
//!
//! All of the configuration for your application lives in one repository,
//! organized by file (`app`, `database`, `cache`, ...) and accessed with
//! "dot" notation:
//!
//! ```
//! use illuminate_config::Repository;
//! use illuminate_support::json;
//!
//! let config = Repository::new(json!({
//!     "app": {"name": "Laravel", "debug": true},
//! }));
//!
//! assert_eq!(config.string("app.name"), "Laravel");
//! assert!(config.boolean("app.debug"));
//! assert_eq!(config.get_or("app.timezone", "UTC"), json!("UTC"));
//! ```

use std::sync::{Arc, RwLock};

use serde::de::DeserializeOwned;

use illuminate_container::{Container, try_app};
use illuminate_support::{Arr, Map, Value, ValueExt, cast};

/// The configuration repository.
#[derive(Default)]
pub struct Repository {
    items: RwLock<Value>,
}

impl Repository {
    /// Create a new configuration repository.
    pub fn new(items: Value) -> Self {
        let items = match items {
            Value::Object(_) => items,
            _ => Value::Object(Map::new()),
        };
        Self {
            items: RwLock::new(items),
        }
    }

    /// Create an empty repository.
    pub fn empty() -> Self {
        Self::new(Value::Object(Map::new()))
    }

    /// Determine if the given configuration value exists.
    pub fn has(&self, key: &str) -> bool {
        self.items.read().unwrap().dot(key).is_some()
    }

    /// Get the specified configuration value (null when missing).
    pub fn get(&self, key: &str) -> Value {
        self.items.read().unwrap().dot_or_null(key)
    }

    /// Get the specified configuration value, or a default when it is missing or null.
    pub fn get_or(&self, key: &str, default: impl Into<Value>) -> Value {
        match self.items.read().unwrap().dot(key) {
            Some(Value::Null) | None => default.into(),
            Some(value) => value.clone(),
        }
    }

    /// Get the specified configuration value, deserialized into a type.
    pub fn get_as<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        match self.get(key) {
            Value::Null => None,
            value => cast(value).ok(),
        }
    }

    /// Get many configuration values at once.
    pub fn get_many(&self, keys: &[&str]) -> Map<String, Value> {
        keys.iter()
            .map(|key| (key.to_string(), self.get(key)))
            .collect()
    }

    /// Get the specified configuration value as a string.
    pub fn string(&self, key: &str) -> String {
        self.get(key).to_string_lossy()
    }

    /// Get the specified configuration value as a string, or a default.
    pub fn string_or(&self, key: &str, default: &str) -> String {
        match self.get(key) {
            Value::Null => default.to_string(),
            value => value.to_string_lossy(),
        }
    }

    /// Get the specified configuration value as an integer.
    pub fn integer(&self, key: &str) -> i64 {
        self.get(key).to_i64_lossy().unwrap_or(0)
    }

    /// Get the specified configuration value as an integer, or a default.
    pub fn integer_or(&self, key: &str, default: i64) -> i64 {
        self.get(key).to_i64_lossy().unwrap_or(default)
    }

    /// Get the specified configuration value as a float.
    pub fn float(&self, key: &str) -> f64 {
        self.get(key).to_f64_lossy().unwrap_or(0.0)
    }

    /// Get the specified configuration value as a boolean.
    pub fn boolean(&self, key: &str) -> bool {
        match self.get(key) {
            Value::String(s) => matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"),
            other => other.truthy(),
        }
    }

    /// Get the specified configuration value as a list of strings.
    pub fn strings(&self, key: &str) -> Vec<String> {
        match self.get(key) {
            Value::Array(items) => items.iter().map(|v| v.to_string_lossy()).collect(),
            Value::Null => Vec::new(),
            Value::String(s) => vec![s],
            other => vec![other.to_string_lossy()],
        }
    }

    /// Set a given configuration value.
    pub fn set(&self, key: &str, value: impl Into<Value>) {
        Arr::set(&mut self.items.write().unwrap(), key, value);
    }

    /// Set many configuration values at once.
    pub fn set_many(&self, values: impl IntoIterator<Item = (String, Value)>) {
        let mut items = self.items.write().unwrap();
        for (key, value) in values {
            Arr::set(&mut items, &key, value);
        }
    }

    /// Prepend a value onto an array configuration value.
    pub fn prepend(&self, key: &str, value: impl Into<Value>) {
        let mut list = match self.get(key) {
            Value::Array(items) => items,
            _ => Vec::new(),
        };
        list.insert(0, value.into());
        self.set(key, Value::Array(list));
    }

    /// Push a value onto an array configuration value.
    pub fn push(&self, key: &str, value: impl Into<Value>) {
        let mut list = match self.get(key) {
            Value::Array(items) => items,
            _ => Vec::new(),
        };
        list.push(value.into());
        self.set(key, Value::Array(list));
    }

    /// Get all of the configuration items.
    pub fn all(&self) -> Value {
        self.items.read().unwrap().clone()
    }

    /// Remove a configuration value.
    pub fn forget(&self, key: &str) {
        Arr::forget(&mut self.items.write().unwrap(), key);
    }
}

/// The `Config` facade.
///
/// ```ignore
/// use laravel::facades::Config;
///
/// let name = Config::string("app.name");
/// Config::set("app.timezone", "America/Chicago");
/// ```
pub struct Config;

impl Config {
    /// Get the configuration repository from the container.
    pub fn repository() -> Arc<Repository> {
        resolve_repository()
    }

    pub fn has(key: &str) -> bool {
        resolve_repository().has(key)
    }

    pub fn get(key: &str) -> Value {
        resolve_repository().get(key)
    }

    pub fn get_or(key: &str, default: impl Into<Value>) -> Value {
        resolve_repository().get_or(key, default)
    }

    pub fn get_as<T: DeserializeOwned>(key: &str) -> Option<T> {
        resolve_repository().get_as(key)
    }

    pub fn string(key: &str) -> String {
        resolve_repository().string(key)
    }

    pub fn string_or(key: &str, default: &str) -> String {
        resolve_repository().string_or(key, default)
    }

    pub fn integer(key: &str) -> i64 {
        resolve_repository().integer(key)
    }

    pub fn integer_or(key: &str, default: i64) -> i64 {
        resolve_repository().integer_or(key, default)
    }

    pub fn float(key: &str) -> f64 {
        resolve_repository().float(key)
    }

    pub fn boolean(key: &str) -> bool {
        resolve_repository().boolean(key)
    }

    pub fn strings(key: &str) -> Vec<String> {
        resolve_repository().strings(key)
    }

    pub fn set(key: &str, value: impl Into<Value>) {
        resolve_repository().set(key, value)
    }

    pub fn push(key: &str, value: impl Into<Value>) {
        resolve_repository().push(key, value)
    }

    pub fn all() -> Value {
        resolve_repository().all()
    }
}

/// Resolve the configuration repository, registering an empty one if the
/// application hasn't bootstrapped configuration yet.
fn resolve_repository() -> Arc<Repository> {
    if let Some(repository) = try_app::<Repository>() {
        return repository;
    }
    let container = Container::get_instance();
    container.singleton_if::<Repository>(|_| Arc::new(Repository::empty()));
    container.make::<Repository>()
}

/// Get the specified configuration value.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::{config, Repository};
/// use illuminate_container::Container;
/// use illuminate_support::json;
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({"app": {"name": "Laravel"}})));
///
/// assert_eq!(config("app.name"), json!("Laravel"));
/// ```
pub fn config(key: &str) -> Value {
    Config::get(key)
}

/// Get the specified configuration value, or a default.
pub fn config_or(key: &str, default: impl Into<Value>) -> Value {
    Config::get_or(key, default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn it_gets_and_sets_values() {
        let config = Repository::new(json!({"app": {"name": "Laravel", "providers": ["a"]}}));
        config.set("app.locale", "en");
        config.push("app.providers", "b");
        assert_eq!(config.get("app.locale"), json!("en"));
        assert_eq!(config.strings("app.providers"), vec!["a", "b"]);
        assert_eq!(config.get_as::<String>("app.name"), Some("Laravel".to_string()));
        assert_eq!(config.get_or("app.missing", 5), json!(5));
        config.forget("app.locale");
        assert!(!config.has("app.locale"));
    }

    #[test]
    fn booleans_understand_strings() {
        let config = Repository::new(json!({"a": "true", "b": "0", "c": 1}));
        assert!(config.boolean("a"));
        assert!(!config.boolean("b"));
        assert!(config.boolean("c"));
    }
}
