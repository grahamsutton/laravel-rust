//! The Argon2 hashers (`argon` / Argon2i and `argon2id`).

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;

use illuminate_support::error::RuntimeException;
use illuminate_support::{Result, Value};

use crate::bcrypt_hasher::{config_bool, config_u32};
use crate::hasher::{HashOptions, Hasher, password_get_info, password_verify};

/// Hashes passwords with Argon2i (the `argon` driver) or Argon2id (the
/// `argon2id` driver), producing the same PHC strings as PHP's `password_hash()`.
///
/// ```
/// use illuminate_hashing::{ArgonHasher, Hasher};
///
/// let hasher = ArgonHasher::argon2id().memory(1024).time(1).threads(1);
/// let hashed = hasher.make("password").unwrap();
///
/// assert!(hashed.starts_with("$argon2id$v=19$m=1024,t=1,p=1$"));
/// assert!(hasher.check("password", &hashed));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgonHasher {
    algorithm: Algorithm,
    memory: u32,
    time: u32,
    threads: u32,
    verify_algorithm: bool,
}

impl Default for ArgonHasher {
    fn default() -> Self {
        Self {
            algorithm: Algorithm::Argon2i,
            memory: 1024,
            time: 2,
            threads: 2,
            verify_algorithm: false,
        }
    }
}

impl ArgonHasher {
    /// Create an Argon2i hasher with Laravel's defaults (Laravel's `ArgonHasher`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an Argon2id hasher with Laravel's defaults (Laravel's `Argon2IdHasher`).
    pub fn argon2id() -> Self {
        Self {
            algorithm: Algorithm::Argon2id,
            ..Self::default()
        }
    }

    /// Configure the hasher from the `hashing.argon` configuration array
    /// (`memory`, `time`, `threads`, and `verify`).
    pub fn with_config(mut self, options: &Value) -> Self {
        if let Some(memory) = config_u32(options, "memory") {
            self.memory = memory;
        }
        if let Some(time) = config_u32(options, "time") {
            self.time = time;
        }
        if let Some(threads) = config_u32(options, "threads") {
            self.threads = threads;
        }
        if let Some(verify) = options.get("verify").and_then(config_bool) {
            self.verify_algorithm = verify;
        }
        self
    }

    /// Set the default memory cost (KiB).
    pub fn memory(mut self, memory: u32) -> Self {
        self.memory = memory;
        self
    }

    /// Set the default time cost.
    pub fn time(mut self, time: u32) -> Self {
        self.time = time;
        self
    }

    /// Set the default degree of parallelism.
    pub fn threads(mut self, threads: u32) -> Self {
        self.threads = threads;
        self
    }

    /// Enable or disable verifying that checked hashes use this algorithm.
    pub fn verify(mut self, verify: bool) -> Self {
        self.verify_algorithm = verify;
        self
    }

    /// The algorithm's name (`argon2i` or `argon2id`).
    pub fn algorithm_name(&self) -> &'static str {
        self.algorithm.as_str()
    }

    fn display_name(&self) -> &'static str {
        match self.algorithm {
            Algorithm::Argon2id => "Argon2id",
            Algorithm::Argon2d => "Argon2d",
            Algorithm::Argon2i => "Argon2i",
        }
    }

    fn costs(&self, options: &HashOptions) -> (u32, u32, u32) {
        (
            options.memory.unwrap_or(self.memory),
            options.time.unwrap_or(self.time),
            options.threads.unwrap_or(self.threads),
        )
    }

    fn is_using_correct_algorithm(&self, hashed: &str) -> bool {
        password_get_info(hashed).algo_name == self.algorithm_name()
    }
}

impl Hasher for ArgonHasher {
    fn make_with(&self, value: &str, options: &HashOptions) -> Result<String> {
        let not_supported = || RuntimeException::new("Argon2 hashing not supported.");
        let (memory, time, threads) = self.costs(options);
        let params = Params::new(memory, time, threads, None).map_err(|_| not_supported())?;

        let mut salt = [0u8; 16];
        rand::rng().fill_bytes(&mut salt);
        let salt = SaltString::encode_b64(&salt).map_err(|_| not_supported())?;

        let hash = Argon2::new(self.algorithm, Version::V0x13, params)
            .hash_password(value.as_bytes(), &salt)
            .map_err(|_| not_supported())?;
        Ok(hash.to_string())
    }

    fn try_check(&self, value: &str, hashed: &str) -> Result<bool> {
        if hashed.is_empty() {
            return Ok(false);
        }
        if self.verify_algorithm && !self.is_using_correct_algorithm(hashed) {
            return Err(RuntimeException::new(format!(
                "This password does not use the {} algorithm.",
                self.display_name()
            ))
            .into());
        }
        Ok(password_verify(value, hashed))
    }

    fn needs_rehash_with(&self, hashed: &str, options: &HashOptions) -> bool {
        let info = password_get_info(hashed);
        let (memory, time, threads) = self.costs(options);
        info.algo_name != self.algorithm_name()
            || info.option("memory_cost") != Some(memory)
            || info.option("time_cost") != Some(time)
            || info.option("threads") != Some(threads)
    }

    fn verify_configuration(&self, hashed: &str) -> bool {
        let info = password_get_info(hashed);
        let within = |key: &str, limit: u32| info.option(key).is_some_and(|value| value <= limit);
        self.is_using_correct_algorithm(hashed)
            && within("memory_cost", self.memory)
            && within("time_cost", self.time)
            && within("threads", self.threads)
    }
}

/// Verify a value against an Argon2 PHC string.
pub(crate) fn verify(value: &str, hashed: &str) -> bool {
    PasswordHash::new(hashed).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(value.as_bytes(), &parsed)
            .is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    // Produced by PHP 8.3's password_hash('password', ..., ['memory_cost' => 1024, 'time_cost' => 2, 'threads' => 1]).
    const PHP_ARGON2I: &str = "$argon2i$v=19$m=1024,t=2,p=1$QkFnc1pyemZFNW1uWWlrdA$6uFpZ3HvEUHsdQLPL+StMyb+IOOE4KdujHG+YiSKgBA";
    const PHP_ARGON2ID: &str = "$argon2id$v=19$m=1024,t=2,p=1$Wjgvc2lONklPc0xJNHl5eQ$b+IfhGVVX8rJ7IOy2gdlesaHbzvwjvjwpci0nHyAwME";

    fn argon() -> ArgonHasher {
        ArgonHasher::new().memory(1024).time(2).threads(1)
    }

    #[test]
    fn argon2i_roundtrips() {
        let hashed = argon().make("password").unwrap();
        assert!(hashed.starts_with("$argon2i$v=19$m=1024,t=2,p=1$"));
        assert!(argon().check("password", &hashed));
        assert!(!argon().check("secret", &hashed));
        assert!(!argon().needs_rehash(&hashed));
    }

    #[test]
    fn argon2id_roundtrips() {
        let hasher = ArgonHasher::argon2id().memory(1024).time(1).threads(1);
        let hashed = hasher
            .make_with("password", &HashOptions::new().time(2))
            .unwrap();
        assert!(hashed.starts_with("$argon2id$v=19$m=1024,t=2,p=1$"));
        assert!(hasher.check("password", &hashed));
        assert!(hasher.needs_rehash(&hashed));
        assert!(!hasher.needs_rehash_with(&hashed, &HashOptions::new().time(2)));
        assert_eq!(hasher.algorithm_name(), "argon2id");
    }

    #[test]
    fn it_verifies_hashes_produced_by_php() {
        assert!(argon().check("password", PHP_ARGON2I));
        assert!(!argon().check("Password", PHP_ARGON2I));
        assert!(ArgonHasher::argon2id().check("password", PHP_ARGON2ID));
        assert!(!argon().needs_rehash(PHP_ARGON2I));
        assert!(argon().needs_rehash(PHP_ARGON2ID));
    }

    #[test]
    fn the_algorithm_can_be_verified() {
        let strict = argon().verify(true);
        assert_eq!(
            strict
                .try_check("password", PHP_ARGON2ID)
                .unwrap_err()
                .to_string(),
            "This password does not use the Argon2i algorithm."
        );
        assert!(strict.try_check("password", PHP_ARGON2I).unwrap());

        let strict = ArgonHasher::argon2id().verify(true);
        assert_eq!(
            strict
                .try_check("password", PHP_ARGON2I)
                .unwrap_err()
                .to_string(),
            "This password does not use the Argon2id algorithm."
        );
        assert!(!strict.check(
            "password",
            "$2y$04$UtsTbDj3S8JnxjQ9frsVDuGWdvYreKAjxMFDN7b8aabNihodod5Zy"
        ));
    }

    #[test]
    fn it_verifies_its_configuration() {
        assert!(argon().verify_configuration(PHP_ARGON2I));
        assert!(!argon().memory(512).verify_configuration(PHP_ARGON2I));
        assert!(!argon().verify_configuration(PHP_ARGON2ID));
    }

    #[test]
    fn invalid_parameters_are_runtime_exceptions() {
        let error = argon().threads(0).make("password").unwrap_err();
        assert_eq!(error.to_string(), "Argon2 hashing not supported.");
    }

    #[test]
    fn it_reads_configuration() {
        let hasher = ArgonHasher::new()
            .with_config(&json!({"memory": 65536, "threads": "1", "time": 4, "verify": true}));
        assert_eq!(
            hasher,
            ArgonHasher::new()
                .memory(65536)
                .threads(1)
                .time(4)
                .verify(true)
        );
    }
}
