//! The `Hash` facade and the `bcrypt()` helper.

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, try_app};
use illuminate_support::Result;

use crate::hasher::{HashInfo, HashOptions, Hasher};
use crate::manager::HashManager;

/// The `Hash` facade: secure Bcrypt and Argon2 hashing for passwords.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::Container;
/// use illuminate_hashing::Hash;
/// use illuminate_support::json;
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({"hashing": {"bcrypt": {"rounds": 4}}})));
///
/// let hashed = Hash::make("plain-text").unwrap();
///
/// if Hash::check("plain-text", &hashed) {
///     // The passwords match...
/// }
///
/// if Hash::needs_rehash(&hashed) {
///     // The work factor changed...
/// }
/// # assert!(Hash::check("plain-text", &hashed));
/// # assert!(!Hash::needs_rehash(&hashed));
/// ```
pub struct Hash;

impl Hash {
    /// Get the hash manager behind the facade.
    pub fn manager() -> Arc<HashManager> {
        manager()
    }

    /// Get a hashing driver by name (`bcrypt`, `argon`, `argon2id`, ...).
    pub fn driver(name: &str) -> Result<Arc<dyn Hasher>> {
        manager().driver(name)
    }

    /// Hash the given value.
    pub fn make(value: &str) -> Result<String> {
        manager().make(value)
    }

    /// Hash the given value with specific options (`rounds`, `memory`, `time`, `threads`).
    pub fn make_with(value: &str, options: impl Into<HashOptions>) -> Result<String> {
        manager().make_with(value, &options.into())
    }

    /// Check the given plain value against a hash.
    pub fn check(value: &str, hashed: &str) -> bool {
        manager().check(value, hashed)
    }

    /// Check the given plain value against a hash, reporting a hash that
    /// uses a different algorithm as a `RuntimeException` (when verification
    /// is enabled).
    pub fn try_check(value: &str, hashed: &str) -> Result<bool> {
        manager().try_check(value, hashed)
    }

    /// Determine if the hash was produced with a different work factor.
    pub fn needs_rehash(hashed: &str) -> bool {
        manager().needs_rehash(hashed)
    }

    /// Determine if the hash was produced with different options than the given ones.
    pub fn needs_rehash_with(hashed: &str, options: impl Into<HashOptions>) -> bool {
        manager().needs_rehash_with(hashed, &options.into())
    }

    /// Get information about the given hashed value.
    pub fn info(hashed: &str) -> HashInfo {
        manager().info(hashed)
    }

    /// Determine if the given string is already hashed.
    pub fn is_hashed(value: &str) -> bool {
        manager().is_hashed(value)
    }

    /// Determine if the hash's options are no stronger than the configured ones.
    pub fn verify_configuration(hashed: &str) -> bool {
        manager().verify_configuration(hashed)
    }

    /// Register a custom hashing driver.
    pub fn extend(
        driver: impl Into<String>,
        creator: impl Fn(&HashManager) -> Arc<dyn Hasher> + Send + Sync + 'static,
    ) {
        manager().extend(driver, creator);
    }
}

/// Resolve the hash manager, registering one if the application hasn't.
fn manager() -> Arc<HashManager> {
    if let Some(manager) = try_app::<HashManager>() {
        return manager;
    }
    let container = Container::get_instance();
    container.singleton_if::<HashManager>(|c| {
        let config = c
            .try_make::<Repository>()
            .unwrap_or_else(|_| Arc::new(Repository::empty()));
        Arc::new(HashManager::new(config))
    });
    container.make::<HashManager>()
}

/// Hash the given value using the Bcrypt driver.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_hashing::{bcrypt_with, Hash, HashOptions};
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container);
///
/// let hashed = bcrypt_with("secret", HashOptions::new().rounds(4)).unwrap();
/// assert!(hashed.starts_with("$2y$04$"));
/// assert!(Hash::check("secret", &hashed));
/// ```
pub fn bcrypt(value: &str) -> Result<String> {
    Hash::driver("bcrypt")?.make(value)
}

/// Hash the given value using the Bcrypt driver and the given options.
pub fn bcrypt_with(value: &str, options: impl Into<HashOptions>) -> Result<String> {
    Hash::driver("bcrypt")?.make_with(value, &options.into())
}
