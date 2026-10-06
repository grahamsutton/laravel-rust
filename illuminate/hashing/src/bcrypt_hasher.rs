//! The Bcrypt hasher.

use illuminate_support::error::{InvalidArgumentException, RuntimeException};
use illuminate_support::{Result, Value, ValueExt};

use crate::hasher::{HashOptions, Hasher, password_get_info, password_verify};

/// Hashes passwords with Bcrypt, producing PHP-compatible `$2y$` hashes.
///
/// ```
/// use illuminate_hashing::{BcryptHasher, HashOptions, Hasher};
///
/// let hasher = BcryptHasher::new().rounds(5);
/// let hashed = hasher.make("password").unwrap();
///
/// assert!(hasher.check("password", &hashed));
/// assert!(!hasher.needs_rehash(&hashed));
/// assert!(hasher.needs_rehash_with(&hashed, &HashOptions::new().rounds(6)));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BcryptHasher {
    rounds: u32,
    verify_algorithm: bool,
    limit: Option<usize>,
}

impl Default for BcryptHasher {
    fn default() -> Self {
        Self {
            rounds: 12,
            verify_algorithm: false,
            limit: None,
        }
    }
}

impl BcryptHasher {
    /// Create a hasher with Laravel's defaults (12 rounds).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a hasher from the `hashing.bcrypt` configuration array
    /// (`rounds`, `verify`, and `limit`).
    pub fn from_config(options: &Value) -> Self {
        let mut hasher = Self::default();
        if let Some(rounds) = config_u32(options, "rounds") {
            hasher.rounds = rounds;
        }
        if let Some(verify) = options.get("verify").and_then(config_bool) {
            hasher.verify_algorithm = verify;
        }
        hasher.limit = options
            .get("limit")
            .and_then(ValueExt::to_i64_lossy)
            .and_then(|limit| usize::try_from(limit).ok())
            .filter(|limit| *limit > 0);
        hasher
    }

    /// Set the default work factor.
    pub fn rounds(mut self, rounds: u32) -> Self {
        self.rounds = rounds;
        self
    }

    /// Set the default work factor in place (Laravel's `setRounds`).
    pub fn set_rounds(&mut self, rounds: u32) -> &mut Self {
        self.rounds = rounds;
        self
    }

    /// Enable or disable verifying that checked hashes use Bcrypt.
    pub fn verify(mut self, verify: bool) -> Self {
        self.verify_algorithm = verify;
        self
    }

    /// Limit the length (in bytes) of values that may be hashed.
    pub fn limit(mut self, limit: Option<usize>) -> Self {
        self.limit = limit;
        self
    }

    /// The default work factor.
    pub fn cost(&self) -> u32 {
        self.rounds
    }

    fn is_using_correct_algorithm(&self, hashed: &str) -> bool {
        password_get_info(hashed).algo_name == "bcrypt"
    }
}

impl Hasher for BcryptHasher {
    fn make_with(&self, value: &str, options: &HashOptions) -> Result<String> {
        if let Some(limit) = self.limit.filter(|limit| value.len() > *limit) {
            return Err(InvalidArgumentException::new(format!(
                "Value is too long to hash. Value must be less than {limit} bytes."
            ))
            .into());
        }

        let cost = options.rounds.unwrap_or(self.rounds);
        let parts = ::bcrypt::hash_with_result(value, cost)
            .map_err(|_| RuntimeException::new("Bcrypt hashing not supported."))?;
        Ok(parts.format_for_version(::bcrypt::Version::TwoY))
    }

    fn try_check(&self, value: &str, hashed: &str) -> Result<bool> {
        if hashed.is_empty() {
            return Ok(false);
        }
        if self.verify_algorithm && !self.is_using_correct_algorithm(hashed) {
            return Err(
                RuntimeException::new("This password does not use the Bcrypt algorithm.").into(),
            );
        }
        Ok(password_verify(value, hashed))
    }

    fn needs_rehash_with(&self, hashed: &str, options: &HashOptions) -> bool {
        let info = password_get_info(hashed);
        info.algo_name != "bcrypt"
            || info.option("cost") != Some(options.rounds.unwrap_or(self.rounds))
    }

    fn verify_configuration(&self, hashed: &str) -> bool {
        let info = password_get_info(hashed);
        info.algo_name == "bcrypt" && info.option("cost").is_some_and(|cost| cost <= self.rounds)
    }
}

/// Read a positive integer option from a configuration array.
pub(crate) fn config_u32(options: &Value, key: &str) -> Option<u32> {
    options
        .get(key)
        .and_then(ValueExt::to_i64_lossy)
        .and_then(|n| u32::try_from(n).ok())
}

/// Read a boolean option the way `env()` values are interpreted.
pub(crate) fn config_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Null => None,
        Value::Bool(b) => Some(*b),
        Value::String(s) => Some(matches!(
            s.to_ascii_lowercase().trim(),
            "1" | "true" | "on" | "yes"
        )),
        other => Some(other.truthy()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    // Produced by PHP 8.3: password_hash('password', PASSWORD_BCRYPT, ['cost' => 4]).
    const PHP_BCRYPT: &str = "$2y$04$UtsTbDj3S8JnxjQ9frsVDuGWdvYreKAjxMFDN7b8aabNihodod5Zy";
    // The hash of "password" shipped in Laravel's user factory for years.
    const LARAVEL_FACTORY_HASH: &str =
        "$2y$10$92IXUNpkjO0rOQ5byMi.Ye4oKoEa3Ro9llC/.og/at2.uheWG/igi";

    fn hasher() -> BcryptHasher {
        BcryptHasher::new().rounds(4)
    }

    #[test]
    fn it_produces_php_compatible_hashes() {
        let hashed = hasher().make("password").unwrap();
        assert_eq!(hashed.len(), 60);
        assert!(hashed.starts_with("$2y$04$"));
        assert_ne!(hashed, hasher().make("password").unwrap());
        assert!(hasher().check("password", &hashed));
        assert!(password_verify("password", &hashed));
    }

    #[test]
    fn it_verifies_hashes_produced_by_php() {
        assert!(hasher().check("password", PHP_BCRYPT));
        assert!(!hasher().check("Password", PHP_BCRYPT));
        assert!(hasher().check("password", LARAVEL_FACTORY_HASH));
        assert!(!hasher().check("secret", LARAVEL_FACTORY_HASH));
    }

    #[test]
    fn work_factors_are_configurable() {
        let hashed = hasher()
            .make_with("password", &HashOptions::new().rounds(5))
            .unwrap();
        assert!(hashed.starts_with("$2y$05$"));
        assert!(hasher().needs_rehash(&hashed));
        assert!(!hasher().needs_rehash_with(&hashed, &HashOptions::new().rounds(5)));
        assert!(
            !BcryptHasher::new()
                .rounds(10)
                .needs_rehash(LARAVEL_FACTORY_HASH)
        );
        assert!(BcryptHasher::new().needs_rehash(LARAVEL_FACTORY_HASH));
        assert!(hasher().needs_rehash("plain"));
        assert_eq!(BcryptHasher::new().cost(), 12);

        let mut hasher = BcryptHasher::new();
        hasher.set_rounds(4);
        assert_eq!(hasher.cost(), 4);
    }

    #[test]
    fn invalid_costs_are_runtime_exceptions() {
        let error = BcryptHasher::new().rounds(2).make("password").unwrap_err();
        assert_eq!(error.to_string(), "Bcrypt hashing not supported.");
    }

    #[test]
    fn values_longer_than_the_limit_are_rejected() {
        let hasher = hasher().limit(Some(8));
        let error = hasher.make("much-too-long").unwrap_err();
        assert!(error.downcast_ref::<InvalidArgumentException>().is_some());
        assert_eq!(
            error.to_string(),
            "Value is too long to hash. Value must be less than 8 bytes."
        );
        assert!(hasher.make("short").is_ok());
    }

    #[test]
    fn the_algorithm_can_be_verified() {
        let argon = "$argon2id$v=19$m=1024,t=2,p=1$Wjgvc2lONklPc0xJNHl5eQ$b+IfhGVVX8rJ7IOy2gdlesaHbzvwjvjwpci0nHyAwME";

        // Without verification, any algorithm PHP understands is accepted.
        assert!(hasher().check("password", argon));

        let strict = hasher().verify(true);
        let error = strict.try_check("password", argon).unwrap_err();
        assert_eq!(
            error.to_string(),
            "This password does not use the Bcrypt algorithm."
        );
        assert!(!strict.check("password", argon));
        assert!(strict.try_check("password", PHP_BCRYPT).unwrap());
        assert!(!strict.try_check("password", "").unwrap());
    }

    #[test]
    fn it_verifies_its_configuration() {
        assert!(
            BcryptHasher::new()
                .rounds(10)
                .verify_configuration(LARAVEL_FACTORY_HASH)
        );
        assert!(!hasher().verify_configuration(LARAVEL_FACTORY_HASH));
        assert!(!hasher().verify_configuration("plain"));
    }

    #[test]
    fn it_reads_configuration() {
        let hasher =
            BcryptHasher::from_config(&json!({"rounds": "4", "verify": "true", "limit": null}));
        assert_eq!(hasher, BcryptHasher::new().rounds(4).verify(true));
        let hasher = BcryptHasher::from_config(&json!({"limit": 72}));
        assert_eq!(hasher, BcryptHasher::new().limit(Some(72)));
        assert_eq!(BcryptHasher::from_config(&json!(null)), BcryptHasher::new());
    }
}
