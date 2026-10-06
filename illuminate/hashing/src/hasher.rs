//! The hasher contract, hashing options, and `password_get_info` / `password_verify`.

use serde::Serialize;

use illuminate_support::{Map, Result, Value, ValueExt, json};

/// Options that tune the work factor of a single hashing operation.
///
/// ```
/// use illuminate_hashing::HashOptions;
/// use illuminate_support::json;
///
/// let options = HashOptions::new().rounds(12);
/// assert_eq!(options.rounds, Some(12));
///
/// let options = HashOptions::from(json!({"memory": 1024, "time": 2, "threads": 2}));
/// assert_eq!(options.memory, Some(1024));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HashOptions {
    /// The bcrypt cost factor.
    pub rounds: Option<u32>,
    /// The Argon2 memory cost, in KiB.
    pub memory: Option<u32>,
    /// The Argon2 time cost (iterations).
    pub time: Option<u32>,
    /// The Argon2 degree of parallelism.
    pub threads: Option<u32>,
}

impl HashOptions {
    /// Create an empty set of options (use the hasher's defaults).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the bcrypt cost factor.
    pub fn rounds(mut self, rounds: u32) -> Self {
        self.rounds = Some(rounds);
        self
    }

    /// Set the Argon2 memory cost (KiB).
    pub fn memory(mut self, memory: u32) -> Self {
        self.memory = Some(memory);
        self
    }

    /// Set the Argon2 time cost.
    pub fn time(mut self, time: u32) -> Self {
        self.time = Some(time);
        self
    }

    /// Set the Argon2 degree of parallelism.
    pub fn threads(mut self, threads: u32) -> Self {
        self.threads = Some(threads);
        self
    }
}

impl From<Value> for HashOptions {
    fn from(value: Value) -> Self {
        let get = |key: &str| {
            value
                .get(key)
                .and_then(ValueExt::to_i64_lossy)
                .and_then(|n| u32::try_from(n).ok())
        };
        Self {
            rounds: get("rounds"),
            memory: get("memory"),
            time: get("time"),
            threads: get("threads"),
        }
    }
}

impl From<&HashOptions> for HashOptions {
    fn from(options: &HashOptions) -> Self {
        *options
    }
}

/// Information about a hashed value, like PHP's `password_get_info()`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HashInfo {
    /// The algorithm identifier (`2y`, `argon2i`, `argon2id`), or `None`
    /// when the value isn't a recognized hash.
    pub algo: Option<String>,
    /// The algorithm's name: `bcrypt`, `argon2i`, `argon2id`, or `unknown`.
    #[serde(rename = "algoName")]
    pub algo_name: String,
    /// The options used to produce the hash (`cost`, or `memory_cost`,
    /// `time_cost` and `threads`).
    pub options: Value,
}

impl HashInfo {
    fn unknown() -> Self {
        Self {
            algo: None,
            algo_name: "unknown".to_string(),
            options: Value::Object(Map::new()),
        }
    }

    /// Read an integer option (`cost`, `memory_cost`, ...).
    pub fn option(&self, key: &str) -> Option<u32> {
        self.options
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
    }
}

/// Get information about the given hash, exactly like PHP's `password_get_info()`.
///
/// ```
/// use illuminate_hashing::password_get_info;
///
/// let info = password_get_info("$2y$10$92IXUNpkjO0rOQ5byMi.Ye4oKoEa3Ro9llC/.og/at2.uheWG/igi");
/// assert_eq!(info.algo_name, "bcrypt");
/// assert_eq!(info.option("cost"), Some(10));
///
/// assert_eq!(password_get_info("plain-text").algo, None);
/// ```
pub fn password_get_info(hashed: &str) -> HashInfo {
    if let Some(info) = bcrypt_info(hashed) {
        return info;
    }
    if let Some(info) = argon_info(hashed) {
        return info;
    }
    HashInfo::unknown()
}

fn bcrypt_info(hashed: &str) -> Option<HashInfo> {
    let bytes = hashed.as_bytes();
    if bytes.len() != 60 || bytes[0] != b'$' || bytes[1] != b'2' || bytes[3] != b'$' || bytes[6] != b'$' {
        return None;
    }
    // PHP only recognizes `$2y$`; the other revisions verify identically, so we accept them too.
    let version = &hashed[1..3];
    if !matches!(version, "2y" | "2a" | "2b") {
        return None;
    }
    let cost: u32 = hashed[4..6].parse().ok()?;
    Some(HashInfo {
        algo: Some(version.to_string()),
        algo_name: "bcrypt".to_string(),
        options: json!({ "cost": cost }),
    })
}

fn argon_info(hashed: &str) -> Option<HashInfo> {
    let mut parts = hashed.strip_prefix('$')?.split('$');
    let algo = parts.next()?;
    if algo != "argon2i" && algo != "argon2id" {
        return None;
    }
    let mut params = parts.next()?;
    if params.starts_with("v=") {
        params = parts.next()?;
    }
    let mut options = Map::new();
    for pair in params.split(',') {
        let (key, value) = pair.split_once('=')?;
        let name = match key {
            "m" => "memory_cost",
            "t" => "time_cost",
            "p" => "threads",
            _ => continue,
        };
        options.insert(name.to_string(), json!(value.parse::<u32>().ok()?));
    }
    Some(HashInfo {
        algo: Some(algo.to_string()),
        algo_name: algo.to_string(),
        options: Value::Object(options),
    })
}

/// Verify that a password matches a hash, like PHP's `password_verify()`.
///
/// Bcrypt (`$2y$`, `$2a$`, `$2b$`) and Argon2 (`$argon2i$`, `$argon2id$`)
/// hashes are supported; anything else never matches.
///
/// ```
/// use illuminate_hashing::password_verify;
///
/// // The famous hash of "password" from Laravel's old user factory.
/// let hash = "$2y$10$92IXUNpkjO0rOQ5byMi.Ye4oKoEa3Ro9llC/.og/at2.uheWG/igi";
///
/// assert!(password_verify("password", hash));
/// assert!(!password_verify("secret", hash));
/// ```
pub fn password_verify(value: &str, hashed: &str) -> bool {
    match password_get_info(hashed).algo_name.as_str() {
        "bcrypt" => ::bcrypt::verify(value, hashed).unwrap_or(false),
        "argon2i" | "argon2id" => crate::argon_hasher::verify(value, hashed),
        _ => false,
    }
}

/// The hasher contract.
///
/// Implemented by [`BcryptHasher`](crate::BcryptHasher),
/// [`ArgonHasher`](crate::ArgonHasher) and the [`HashManager`](crate::HashManager);
/// implement it yourself to register a custom driver with `HashManager::extend`.
pub trait Hasher: Send + Sync {
    /// Get information about the given hashed value.
    fn info(&self, hashed: &str) -> HashInfo {
        password_get_info(hashed)
    }

    /// Hash the given value using the hasher's default work factor.
    fn make(&self, value: &str) -> Result<String> {
        self.make_with(value, &HashOptions::default())
    }

    /// Hash the given value using the given options.
    fn make_with(&self, value: &str, options: &HashOptions) -> Result<String>;

    /// Check the given plain value against a hash.
    ///
    /// Any failure — including a hash produced by a different algorithm when
    /// algorithm verification is enabled — is simply a mismatch. Use
    /// [`Hasher::try_check`] to receive that case as Laravel's `RuntimeException`.
    fn check(&self, value: &str, hashed: &str) -> bool {
        self.try_check(value, hashed).unwrap_or(false)
    }

    /// Check the given plain value against a hash, failing with a
    /// `RuntimeException` when algorithm verification is enabled and the hash
    /// uses a different algorithm.
    fn try_check(&self, value: &str, hashed: &str) -> Result<bool>;

    /// Determine if the hash was produced with different options than the
    /// hasher's current ones.
    fn needs_rehash(&self, hashed: &str) -> bool {
        self.needs_rehash_with(hashed, &HashOptions::default())
    }

    /// Determine if the hash was produced with different options than the given ones.
    fn needs_rehash_with(&self, hashed: &str, options: &HashOptions) -> bool;

    /// Determine if the hash uses this algorithm with options no stronger
    /// than the configured ones.
    fn verify_configuration(&self, _hashed: &str) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHP_ARGON2ID: &str = "$argon2id$v=19$m=1024,t=2,p=1$Wjgvc2lONklPc0xJNHl5eQ$b+IfhGVVX8rJ7IOy2gdlesaHbzvwjvjwpci0nHyAwME";

    #[test]
    fn it_reads_hash_information_like_php() {
        let info = password_get_info(PHP_ARGON2ID);
        assert_eq!(info.algo.as_deref(), Some("argon2id"));
        assert_eq!(info.algo_name, "argon2id");
        assert_eq!(info.options, json!({"memory_cost": 1024, "time_cost": 2, "threads": 1}));

        let info = password_get_info("$2y$04$UtsTbDj3S8JnxjQ9frsVDuGWdvYreKAjxMFDN7b8aabNihodod5Zy");
        assert_eq!(info.algo.as_deref(), Some("2y"));
        assert_eq!(info.option("cost"), Some(4));
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            json!({"algo": "2y", "algoName": "bcrypt", "options": {"cost": 4}})
        );

        for value in ["", "password", "$2y$10$short", "$argon2i$v=19$m=x,t=2,p=1$a$b", "$md5$abc"] {
            assert_eq!(password_get_info(value), HashInfo::unknown(), "{value}");
        }
    }

    #[test]
    fn options_can_be_built_from_values() {
        let options = HashOptions::from(json!({"rounds": "10", "memory": 2048}));
        assert_eq!(options, HashOptions::new().rounds(10).memory(2048));
        assert_eq!(HashOptions::from(&options), options);
        assert_eq!(HashOptions::from(json!(null)), HashOptions::default());
    }
}
