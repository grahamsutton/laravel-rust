//! The hash manager: resolves and caches the configured hashing drivers.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use illuminate_config::Repository;
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Result, Value};

use crate::argon_hasher::ArgonHasher;
use crate::bcrypt_hasher::BcryptHasher;
use crate::hasher::{HashInfo, HashOptions, Hasher, password_get_info};

type Creator = Arc<dyn Fn(&HashManager) -> Arc<dyn Hasher> + Send + Sync>;

/// Creates and caches the hashing drivers (`bcrypt`, `argon`, `argon2id`, and
/// any custom ones), and hashes with the default driver from `hashing.driver`.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_hashing::{HashManager, Hasher};
/// use illuminate_support::json;
///
/// let manager = HashManager::new(Arc::new(Repository::new(json!({
///     "hashing": {"driver": "bcrypt", "bcrypt": {"rounds": 4}},
/// }))));
///
/// let hashed = manager.make("secret").unwrap();
/// assert!(hashed.starts_with("$2y$04$"));
/// assert!(manager.check("secret", &hashed));
/// assert!(manager.is_hashed(&hashed));
/// ```
pub struct HashManager {
    config: Arc<Repository>,
    drivers: RwLock<HashMap<String, Arc<dyn Hasher>>>,
    custom_creators: RwLock<HashMap<String, Creator>>,
}

impl HashManager {
    /// Create a new manager reading the `hashing.*` configuration.
    pub fn new(config: Arc<Repository>) -> Self {
        Self {
            config,
            drivers: RwLock::new(HashMap::new()),
            custom_creators: RwLock::new(HashMap::new()),
        }
    }

    /// The configuration repository the manager reads from.
    pub fn config(&self) -> &Repository {
        &self.config
    }

    /// The default driver name (`hashing.driver`, falling back to `bcrypt`).
    pub fn get_default_driver(&self) -> String {
        self.config.string_or("hashing.driver", "bcrypt")
    }

    /// Get the default hashing driver.
    pub fn hasher(&self) -> Result<Arc<dyn Hasher>> {
        self.driver(&self.get_default_driver())
    }

    /// Get a hashing driver by name.
    ///
    /// Fails with an `InvalidArgumentException` (`Driver [name] not supported.`)
    /// for unknown drivers.
    pub fn driver(&self, name: &str) -> Result<Arc<dyn Hasher>> {
        if let Some(driver) = self.drivers.read().unwrap().get(name) {
            return Ok(driver.clone());
        }
        let driver = self.create_driver(name)?;
        let mut drivers = self.drivers.write().unwrap();
        Ok(drivers.entry(name.to_string()).or_insert(driver).clone())
    }

    fn create_driver(&self, name: &str) -> Result<Arc<dyn Hasher>> {
        let creator = self.custom_creators.read().unwrap().get(name).cloned();
        if let Some(creator) = creator {
            return Ok(creator(self));
        }
        match name {
            "bcrypt" => Ok(Arc::new(self.create_bcrypt_driver())),
            "argon" => Ok(Arc::new(self.create_argon_driver())),
            "argon2id" => Ok(Arc::new(self.create_argon2id_driver())),
            _ => {
                Err(InvalidArgumentException::new(format!("Driver [{name}] not supported.")).into())
            }
        }
    }

    /// Create an instance of the Bcrypt hash driver.
    pub fn create_bcrypt_driver(&self) -> BcryptHasher {
        BcryptHasher::from_config(&self.options("hashing.bcrypt"))
    }

    /// Create an instance of the Argon2i hash driver.
    pub fn create_argon_driver(&self) -> ArgonHasher {
        ArgonHasher::new().with_config(&self.options("hashing.argon"))
    }

    /// Create an instance of the Argon2id hash driver.
    pub fn create_argon2id_driver(&self) -> ArgonHasher {
        ArgonHasher::argon2id().with_config(&self.options("hashing.argon"))
    }

    fn options(&self, key: &str) -> Value {
        self.config.get(key)
    }

    /// Register a custom driver creator.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_config::Repository;
    /// use illuminate_hashing::{BcryptHasher, HashManager, Hasher};
    ///
    /// let manager = HashManager::new(Arc::new(Repository::empty()));
    /// manager.extend("fast", |_| Arc::new(BcryptHasher::new().rounds(4)));
    ///
    /// let hashed = manager.driver("fast").unwrap().make("secret").unwrap();
    /// assert!(hashed.starts_with("$2y$04$"));
    /// ```
    pub fn extend(
        &self,
        driver: impl Into<String>,
        creator: impl Fn(&HashManager) -> Arc<dyn Hasher> + Send + Sync + 'static,
    ) -> &Self {
        let driver = driver.into();
        self.drivers.write().unwrap().remove(&driver);
        self.custom_creators
            .write()
            .unwrap()
            .insert(driver, Arc::new(creator));
        self
    }

    /// Forget every resolved driver instance.
    pub fn forget_drivers(&self) -> &Self {
        self.drivers.write().unwrap().clear();
        self
    }

    /// Determine if the given string is already hashed.
    pub fn is_hashed(&self, value: &str) -> bool {
        self.info(value).algo.is_some()
    }
}

impl Hasher for HashManager {
    fn info(&self, hashed: &str) -> HashInfo {
        match self.hasher() {
            Ok(driver) => driver.info(hashed),
            Err(_) => password_get_info(hashed),
        }
    }

    fn make_with(&self, value: &str, options: &HashOptions) -> Result<String> {
        self.hasher()?.make_with(value, options)
    }

    fn try_check(&self, value: &str, hashed: &str) -> Result<bool> {
        self.hasher()?.try_check(value, hashed)
    }

    fn needs_rehash_with(&self, hashed: &str, options: &HashOptions) -> bool {
        self.hasher()
            .map(|driver| driver.needs_rehash_with(hashed, options))
            .unwrap_or(true)
    }

    fn verify_configuration(&self, hashed: &str) -> bool {
        self.hasher()
            .map(|driver| driver.verify_configuration(hashed))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn manager(hashing: Value) -> HashManager {
        HashManager::new(Arc::new(Repository::new(json!({ "hashing": hashing }))))
    }

    #[test]
    fn it_uses_the_configured_default_driver() {
        let manager = manager(json!({
            "driver": "argon2id",
            "argon": {"memory": 1024, "time": 1, "threads": 1},
        }));
        assert_eq!(manager.get_default_driver(), "argon2id");
        let hashed = manager.make("password").unwrap();
        assert!(hashed.starts_with("$argon2id$v=19$m=1024,t=1,p=1$"));
        assert!(manager.check("password", &hashed));
        assert!(!manager.needs_rehash(&hashed));
        assert_eq!(manager.info(&hashed).algo_name, "argon2id");
        assert!(manager.verify_configuration(&hashed));
    }

    #[test]
    fn bcrypt_is_the_default() {
        let manager = manager(json!({"bcrypt": {"rounds": 4}}));
        assert_eq!(manager.get_default_driver(), "bcrypt");
        assert!(manager.make("password").unwrap().starts_with("$2y$04$"));
        assert!(manager.is_hashed("$2y$10$92IXUNpkjO0rOQ5byMi.Ye4oKoEa3Ro9llC/.og/at2.uheWG/igi"));
        assert!(!manager.is_hashed("password"));
    }

    #[test]
    fn drivers_are_cached() {
        let manager = manager(json!({}));
        let a = manager.driver("argon").unwrap();
        let b = manager.driver("argon").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        manager.forget_drivers();
        assert!(!Arc::ptr_eq(&a, &manager.driver("argon").unwrap()));
    }

    #[test]
    fn unknown_drivers_are_rejected() {
        let manager = manager(json!({"driver": "md5"}));
        let error = manager.make("password").err().unwrap();
        assert!(error.downcast_ref::<InvalidArgumentException>().is_some());
        assert_eq!(error.to_string(), "Driver [md5] not supported.");
        assert!(!manager.check(
            "password",
            "$2y$10$92IXUNpkjO0rOQ5byMi.Ye4oKoEa3Ro9llC/.og/at2.uheWG/igi"
        ));
        assert!(manager.needs_rehash("x"));
        assert!(!manager.verify_configuration("x"));
        assert_eq!(manager.info("x").algo_name, "unknown");
    }

    #[test]
    fn verification_follows_configuration() {
        let manager = manager(json!({"bcrypt": {"rounds": 4, "verify": true}}));
        let argon = "$argon2id$v=19$m=1024,t=2,p=1$Wjgvc2lONklPc0xJNHl5eQ$b+IfhGVVX8rJ7IOy2gdlesaHbzvwjvjwpci0nHyAwME";
        assert!(manager.try_check("password", argon).is_err());
        assert!(!manager.check("password", argon));
    }
}
