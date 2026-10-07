//! The Memcached cache store and its locks.
//!
//! Items are stored under `{prefix}{key}` the way PHP's `Memcached`
//! extension stores them: strings as they are, integers and floats as
//! numbers (so `incr` and `decr` work on them), booleans as `1` or an empty
//! string, and everything else as JSON — each with the extension's type
//! flag, so PHP applications sharing the servers read the same values.
//!
//! ```json
//! "memcached": {
//!     "driver": "memcached",
//!     "persistent_id": null,
//!     "sasl": [null, null],
//!     "options": {},
//!     "servers": [{"host": "127.0.0.1", "port": 11211, "weight": 100}]
//! }
//! ```

use std::sync::Arc;

use async_trait::async_trait;

use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Result, Value};

use crate::lock::{Lock, LockDriver, LockInfo};
use crate::memcached::{Memcached, MemcachedItem};
use crate::store::{LockProvider, Store};

/// The type flags of PHP's `Memcached` extension.
mod flags {
    pub const STRING: u32 = 0;
    pub const LONG: u32 = 1;
    pub const DOUBLE: u32 = 2;
    pub const BOOL: u32 = 3;
    pub const JSON: u32 = 6;
    /// The bits of the flags holding the type.
    pub const TYPE_MASK: u32 = 0xf;
}

/// Locks held for longer than this get an absolute expiration timestamp
/// (Laravel's `MemcachedLock::calculateExpiration`).
const LOCK_TIMESTAMP_THRESHOLD: u64 = 60 * 60 * 24 * 10;

/// The current UNIX timestamp (honouring `Carbon::set_test_now`).
fn current_time() -> i64 {
    Carbon::now().timestamp()
}

/// The UNIX timestamp `seconds` from now.
fn available_at(seconds: u64) -> i64 {
    current_time().saturating_add(i64::try_from(seconds).unwrap_or(i64::MAX))
}

/// Encode a value and its type flag.
fn serialize(value: &Value) -> (Vec<u8>, u32) {
    match value {
        Value::String(string) => (string.clone().into_bytes(), flags::STRING),
        Value::Number(number) if number.is_f64() => {
            (number.to_string().into_bytes(), flags::DOUBLE)
        }
        Value::Number(number) => (number.to_string().into_bytes(), flags::LONG),
        Value::Bool(true) => (b"1".to_vec(), flags::BOOL),
        Value::Bool(false) => (Vec::new(), flags::BOOL),
        other => (other.to_string().into_bytes(), flags::JSON),
    }
}

/// Decode a stored item according to its type flag.
fn unserialize(item: &MemcachedItem) -> Value {
    let text = String::from_utf8_lossy(&item.value).into_owned();
    let number = |text: &str| -> Option<Value> {
        let text = text.trim();
        text.parse::<i64>()
            .map(Value::from)
            .ok()
            .or_else(|| text.parse::<u64>().map(Value::from).ok())
            .or_else(|| {
                text.parse::<f64>()
                    .ok()
                    .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
            })
    };
    match item.flags & flags::TYPE_MASK {
        flags::STRING => Value::String(text),
        flags::LONG | flags::DOUBLE => number(&text).unwrap_or(Value::String(text)),
        flags::BOOL => Value::Bool(text == "1"),
        _ => serde_json::from_str(&text).unwrap_or(Value::String(text)),
    }
}

/// A cache store backed by Memcached (`CACHE_STORE=memcached`).
///
/// ```no_run
/// use illuminate_cache::{MemcachedStore, Repository};
/// use illuminate_cache::memcached::{Memcached, MemcachedServer};
///
/// # async fn example() -> illuminate_support::Result<()> {
/// let memcached = Memcached::new(vec![MemcachedServer::new("127.0.0.1", 11211, 100)]);
/// let cache = Repository::new(MemcachedStore::new(memcached, "laravel-cache-"));
///
/// cache.put("name", "Taylor", 600).await?;
/// cache.increment("visits").await?;
///
/// let lock = cache.lock("reports", 10);
/// if lock.get().await? {
///     // ...
///     lock.release().await?;
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct MemcachedStore {
    memcached: Memcached,
    prefix: String,
}

impl MemcachedStore {
    /// Create a store on the given client, prefixing every key with `prefix`.
    pub fn new(memcached: Memcached, prefix: impl Into<String>) -> Self {
        Self {
            memcached,
            prefix: prefix.into(),
        }
    }

    /// The Memcached client.
    pub fn get_memcached(&self) -> &Memcached {
        &self.memcached
    }

    /// Set the cache key prefix.
    pub fn set_prefix(&mut self, prefix: impl Into<String>) {
        self.prefix = prefix.into();
    }

    fn prefixed(&self, key: &str) -> String {
        format!("{}{key}", self.prefix)
    }

    /// The expiration Memcached gets for an item stored for `seconds`: like
    /// Laravel, always a UNIX timestamp (Memcached reads anything past 30
    /// days as one), and `0` — never — for forever.
    pub fn calculate_expiration(seconds: u64) -> i64 {
        if seconds > 0 {
            available_at(seconds)
        } else {
            0
        }
    }

    /// Store a counter that doesn't exist yet, returning whether it was stored.
    async fn add_counter(&self, key: &str, value: u64) -> Result<bool> {
        self.memcached
            .add(key, value.to_string().as_bytes(), flags::LONG, 0)
            .await
    }
}

#[async_trait]
impl Store for MemcachedStore {
    async fn get(&self, key: &str) -> Result<Option<Value>> {
        Ok(self
            .memcached
            .get(&self.prefixed(key))
            .await?
            .map(|item| unserialize(&item)))
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<(String, Option<Value>)>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let prefixed: Vec<String> = keys.iter().map(|key| self.prefixed(key)).collect();
        let items = self.memcached.get_multi(&prefixed).await?;
        Ok(keys
            .iter()
            .zip(prefixed)
            .map(|(key, prefixed)| (key.clone(), items.get(&prefixed).map(unserialize)))
            .collect())
    }

    async fn put(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        let (bytes, flags) = serialize(&value);
        self.memcached
            .set(
                &self.prefixed(key),
                &bytes,
                flags,
                Self::calculate_expiration(seconds),
            )
            .await
    }

    async fn add(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        let (bytes, flags) = serialize(&value);
        self.memcached
            .add(
                &self.prefixed(key),
                &bytes,
                flags,
                Self::calculate_expiration(seconds),
            )
            .await
    }

    /// Increment the item with `incr`. Where Laravel returns `false` for a
    /// missing item, the item starts at zero and is stored forever, like
    /// every other store.
    async fn increment(&self, key: &str, value: i64) -> Result<i64> {
        if value < 0 {
            return self.decrement(key, value.saturating_neg()).await;
        }
        let key = self.prefixed(key);
        for _ in 0..2 {
            if let Some(updated) = self.memcached.increment(&key, value as u64).await? {
                return Ok(updated.min(i64::MAX as u64) as i64);
            }
            if self.add_counter(&key, value as u64).await? {
                return Ok(value);
            }
        }
        Err(RuntimeException::new(format!("Unable to increment the cache item [{key}].")).into())
    }

    /// Decrement the item with `decr`. Memcached never goes below zero, so
    /// neither does this store (a missing item stays at zero).
    async fn decrement(&self, key: &str, value: i64) -> Result<i64> {
        if value < 0 {
            return self.increment(key, value.saturating_neg()).await;
        }
        let key = self.prefixed(key);
        for _ in 0..2 {
            if let Some(updated) = self.memcached.decrement(&key, value as u64).await? {
                return Ok(updated.min(i64::MAX as u64) as i64);
            }
            if self.add_counter(&key, 0).await? {
                return Ok(0);
            }
        }
        Err(RuntimeException::new(format!("Unable to decrement the cache item [{key}].")).into())
    }

    async fn forever(&self, key: &str, value: Value) -> Result<bool> {
        self.put(key, value, 0).await
    }

    async fn touch(&self, key: &str, seconds: u64) -> Result<bool> {
        self.memcached
            .touch(&self.prefixed(key), Self::calculate_expiration(seconds))
            .await
    }

    async fn forget(&self, key: &str) -> Result<bool> {
        self.memcached.delete(&self.prefixed(key)).await
    }

    async fn flush(&self) -> Result<bool> {
        self.memcached.flush().await
    }

    fn get_prefix(&self) -> String {
        self.prefix.clone()
    }

    fn lock_provider(&self) -> Option<&dyn LockProvider> {
        Some(self)
    }

    fn supports_tags(&self) -> bool {
        true
    }
}

impl LockProvider for MemcachedStore {
    fn lock(&self, name: &str, seconds: u64, owner: Option<String>) -> Lock {
        Lock::new(
            Arc::new(MemcachedLock::new(self.memcached.clone())),
            self.prefixed(name),
            seconds,
            owner,
        )
    }
}

/// Locks kept as Memcached items holding their owner (Laravel's
/// `MemcachedLock`): acquired with `add`, refreshed with `cas`, and
/// released only by their owner.
#[derive(Debug, Clone)]
pub struct MemcachedLock {
    memcached: Memcached,
}

impl MemcachedLock {
    /// Create a lock driver keeping its locks on the given client.
    pub fn new(memcached: Memcached) -> Self {
        Self { memcached }
    }

    /// The expiration Memcached gets for a lock held for `seconds`: locks
    /// longer than ten days get a UNIX timestamp, shorter ones the seconds.
    pub fn calculate_expiration(seconds: u64) -> i64 {
        if seconds > LOCK_TIMESTAMP_THRESHOLD {
            available_at(seconds)
        } else {
            seconds as i64
        }
    }
}

#[async_trait]
impl LockDriver for MemcachedLock {
    async fn acquire(&self, lock: &LockInfo) -> Result<bool> {
        self.memcached
            .add(
                &lock.name,
                lock.owner.as_bytes(),
                flags::STRING,
                Self::calculate_expiration(lock.seconds),
            )
            .await
    }

    async fn release(&self, lock: &LockInfo) -> Result<bool> {
        if self.current_owner(lock).await?.as_deref() == Some(lock.owner.as_str()) {
            return self.memcached.delete(&lock.name).await;
        }
        Ok(false)
    }

    async fn force_release(&self, lock: &LockInfo) -> Result<()> {
        self.memcached.delete(&lock.name).await?;
        Ok(())
    }

    async fn current_owner(&self, lock: &LockInfo) -> Result<Option<String>> {
        Ok(self
            .memcached
            .get(&lock.name)
            .await?
            .map(|item| match unserialize(&item) {
                Value::String(owner) => owner,
                other => other.to_string(),
            }))
    }

    async fn refresh(&self, lock: &LockInfo, seconds: u64) -> Result<bool> {
        let Some(item) = self.memcached.gets(&lock.name).await? else {
            return Ok(false);
        };
        let (Some(cas), Value::String(owner)) = (item.cas, unserialize(&item)) else {
            return Ok(false);
        };
        if owner != lock.owner {
            return Ok(false);
        }
        self.memcached
            .cas(
                cas,
                &lock.name,
                lock.owner.as_bytes(),
                flags::STRING,
                Self::calculate_expiration(seconds),
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::freeze_time;
    use illuminate_support::json;

    fn item(value: &[u8], flags: u32) -> MemcachedItem {
        MemcachedItem {
            value: value.to_vec(),
            flags,
            cas: None,
        }
    }

    #[test]
    fn values_are_stored_like_the_php_extension_stores_them() {
        assert_eq!(serialize(&json!("Taylor")), (b"Taylor".to_vec(), 0));
        assert_eq!(serialize(&json!(42)), (b"42".to_vec(), 1));
        assert_eq!(serialize(&json!(1.5)), (b"1.5".to_vec(), 2));
        assert_eq!(serialize(&json!(true)), (b"1".to_vec(), 3));
        assert_eq!(serialize(&json!(false)), (Vec::new(), 3));
        assert_eq!(serialize(&json!({"a": [1]})), (br#"{"a":[1]}"#.to_vec(), 6));

        for value in [
            json!("Taylor"),
            json!("42"),
            json!(42),
            json!(-3),
            json!(1.5),
            json!(true),
            json!(false),
            json!(null),
            json!([1, "two"]),
            json!({"a": {"b": null}}),
        ] {
            let (bytes, flags) = serialize(&value);
            assert_eq!(unserialize(&item(&bytes, flags)), value);
        }
        assert_eq!(unserialize(&item(b"7 ", 1)), json!(7));
        assert_eq!(unserialize(&item(b"oops", 1)), json!("oops"));
        assert_eq!(unserialize(&item(b"plain", 9)), json!("plain"));
    }

    #[test]
    fn expirations_follow_laravel() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        assert_eq!(MemcachedStore::calculate_expiration(0), 0);
        assert_eq!(MemcachedStore::calculate_expiration(60), 1_700_000_060);
        assert_eq!(MemcachedLock::calculate_expiration(0), 0);
        assert_eq!(MemcachedLock::calculate_expiration(60), 60);
        assert_eq!(MemcachedLock::calculate_expiration(864_000), 864_000);
        assert_eq!(MemcachedLock::calculate_expiration(864_001), 1_700_864_001);
    }
}
