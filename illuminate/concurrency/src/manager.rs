//! The concurrency manager: resolves drivers by name.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use illuminate_config::Repository;
use illuminate_container::try_app;
use illuminate_support::Result;
use illuminate_support::error::InvalidArgumentException;

use crate::driver::{Driver, SyncDriver, TokioDriver};

/// The driver used when `concurrency.default` isn't configured.
pub const DEFAULT_DRIVER: &str = "tokio";

type Creator = Arc<dyn Fn() -> Arc<dyn Driver> + Send + Sync>;

/// Resolves concurrency drivers by name, creating each one once.
///
/// The default driver is read from the `concurrency.default` configuration
/// value (falling back to `"tokio"`). The built-in drivers are:
///
/// - `tokio`: every task runs on its own Tokio task.
/// - `sync`: tasks run one after another, which is handy in tests.
///
/// Laravel's `process` and `fork` drivers exist because PHP can't run two
/// closures at once within a single process, so it serializes each closure
/// and runs it in a child PHP process. Rust runs futures concurrently (and
/// in parallel) natively, and a Rust closure can't be serialized and
/// shipped to another process anyway, so those drivers aren't provided.
///
/// ```
/// use illuminate_concurrency::ConcurrencyManager;
///
/// let manager = ConcurrencyManager::new();
///
/// assert_eq!(manager.get_default_instance(), "tokio");
/// assert!(manager.driver("sync").is_ok());
/// assert!(manager.driver("process").is_err());
/// ```
pub struct ConcurrencyManager {
    instances: RwLock<HashMap<String, Arc<dyn Driver>>>,
    creators: RwLock<HashMap<String, Creator>>,
    default: RwLock<Option<String>>,
}

impl Default for ConcurrencyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ConcurrencyManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut resolved: Vec<String> = self.instances.read().unwrap().keys().cloned().collect();
        resolved.sort();
        f.debug_struct("ConcurrencyManager")
            .field("default", &self.get_default_instance())
            .field("resolved", &resolved)
            .finish()
    }
}

impl ConcurrencyManager {
    /// Create a new concurrency manager.
    pub fn new() -> Self {
        Self {
            instances: RwLock::new(HashMap::new()),
            creators: RwLock::new(HashMap::new()),
            default: RwLock::new(None),
        }
    }

    /// Get a driver instance by name (`None` for the default driver).
    pub fn driver<'a>(&self, name: impl Into<Option<&'a str>>) -> Result<Arc<dyn Driver>> {
        let name = name
            .into()
            .map(str::to_string)
            .unwrap_or_else(|| self.get_default_instance());

        if let Some(driver) = self.instances.read().unwrap().get(&name) {
            return Ok(driver.clone());
        }

        let driver = self.resolve(&name)?;
        Ok(self
            .instances
            .write()
            .unwrap()
            .entry(name)
            .or_insert(driver)
            .clone())
    }

    fn resolve(&self, name: &str) -> Result<Arc<dyn Driver>> {
        if let Some(creator) = self.creators.read().unwrap().get(name).cloned() {
            return Ok(creator());
        }

        match name {
            "tokio" => Ok(Arc::new(TokioDriver)),
            "sync" => Ok(Arc::new(SyncDriver)),
            "process" | "fork" => Err(InvalidArgumentException::new(format!(
                "The [{name}] concurrency driver is not supported: closures can't be serialized \
                 and sent to another process. Use the [tokio] driver, which runs tasks \
                 concurrently within this process."
            ))
            .into()),
            _ => Err(InvalidArgumentException::new(format!(
                "Concurrency driver [{name}] is not supported."
            ))
            .into()),
        }
    }

    /// Register a custom driver creator.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_concurrency::{ConcurrencyManager, SyncDriver};
    ///
    /// let manager = ConcurrencyManager::new();
    /// manager.extend("sequential", || Arc::new(SyncDriver));
    ///
    /// assert!(manager.driver("sequential").is_ok());
    /// ```
    pub fn extend<F>(&self, name: impl Into<String>, creator: F) -> &Self
    where
        F: Fn() -> Arc<dyn Driver> + Send + Sync + 'static,
    {
        let name = name.into();
        self.instances.write().unwrap().remove(&name);
        self.creators
            .write()
            .unwrap()
            .insert(name, Arc::new(creator));
        self
    }

    /// Get the default driver name: `concurrency.default`, then
    /// `concurrency.driver`, then `"tokio"`.
    pub fn get_default_instance(&self) -> String {
        if let Some(config) = try_app::<Repository>() {
            for key in ["concurrency.default", "concurrency.driver"] {
                if let Some(name) = config.get(key).as_str()
                    && !name.is_empty()
                {
                    return name.to_string();
                }
            }
        }

        self.default
            .read()
            .unwrap()
            .clone()
            .unwrap_or_else(|| DEFAULT_DRIVER.to_string())
    }

    /// Set the default driver name.
    pub fn set_default_instance(&self, name: impl Into<String>) {
        let name = name.into();
        if let Some(config) = try_app::<Repository>() {
            config.set("concurrency.default", name.clone());
        }
        *self.default.write().unwrap() = Some(name);
    }

    /// Forget a resolved driver instance, so it is created again.
    pub fn forget_instance(&self, name: &str) -> &Self {
        self.instances.write().unwrap().remove(name);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_container::Container;
    use illuminate_support::json;

    #[test]
    fn the_default_driver_comes_from_configuration() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container.clone());
        let manager = ConcurrencyManager::new();

        assert_eq!(manager.get_default_instance(), "tokio");

        container.instance(Repository::new(json!({"concurrency": {"default": "sync"}})));
        assert_eq!(manager.get_default_instance(), "sync");

        manager.set_default_instance("tokio");
        assert_eq!(manager.get_default_instance(), "tokio");
        assert_eq!(
            container.make::<Repository>().string("concurrency.default"),
            "tokio"
        );
    }

    #[test]
    fn the_legacy_driver_key_is_respected() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container.clone());
        container.instance(Repository::new(json!({"concurrency": {"driver": "sync"}})));

        assert_eq!(ConcurrencyManager::new().get_default_instance(), "sync");
    }

    #[test]
    fn drivers_are_created_once() {
        let manager = ConcurrencyManager::new();
        let first = manager.driver("sync").unwrap();
        let second = manager.driver("sync").unwrap();
        assert!(Arc::ptr_eq(&first, &second));

        manager.forget_instance("sync");
        let third = manager.driver("sync").unwrap();
        assert!(!Arc::ptr_eq(&first, &third));
    }

    #[test]
    fn unsupported_drivers_explain_themselves() {
        let manager = ConcurrencyManager::new();
        let error = manager.driver("process").err().unwrap().to_string();
        assert!(error.contains("closures can't be serialized"));
        assert!(manager.driver("fork").is_err());
        assert_eq!(
            manager.driver("redis").err().unwrap().to_string(),
            "Concurrency driver [redis] is not supported."
        );
    }

    #[test]
    fn custom_drivers_can_be_registered() {
        let manager = ConcurrencyManager::new();
        manager.extend("tokio", || Arc::new(SyncDriver));
        assert!(manager.driver(None).is_ok());
        assert!(format!("{manager:?}").contains("tokio"));
    }
}
