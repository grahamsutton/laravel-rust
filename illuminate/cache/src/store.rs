//! The cache store contract.

use async_trait::async_trait;

use illuminate_support::error::RuntimeException;
use illuminate_support::{Result, Value, ValueExt};

use crate::lock::Lock;

/// A cache store: where cached values actually live.
///
/// Values are [`Value`]s (the repository serializes your types with serde
/// before they reach the store). `seconds` is always positive, except for
/// `0`, which means "forever".
///
/// The trait is object safe, so stores can be swapped at runtime — write
/// your own (Redis, DynamoDB, a database table, ...) and register it with
/// `Cache::extend`.
#[async_trait]
pub trait Store: Send + Sync + 'static {
    /// Retrieve an item from the cache by key.
    async fn get(&self, key: &str) -> Result<Option<Value>>;

    /// Retrieve multiple items from the cache by key.
    async fn many(&self, keys: &[String]) -> Result<Vec<(String, Option<Value>)>> {
        let mut values = Vec::with_capacity(keys.len());
        for key in keys {
            values.push((key.clone(), self.get(key).await?));
        }
        Ok(values)
    }

    /// Store an item in the cache for a given number of seconds.
    async fn put(&self, key: &str, value: Value, seconds: u64) -> Result<bool>;

    /// Store multiple items in the cache for a given number of seconds.
    async fn put_many(&self, values: Vec<(String, Value)>, seconds: u64) -> Result<bool> {
        let mut result = true;
        for (key, value) in values {
            result = self.put(&key, value, seconds).await? && result;
        }
        Ok(result)
    }

    /// Store an item in the cache if the key doesn't exist.
    ///
    /// Stores should make this atomic when they can; the default
    /// implementation is a plain check-then-put.
    async fn add(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        if self.get(key).await?.is_some() {
            return Ok(false);
        }
        self.put(key, value, seconds).await
    }

    /// Increment the value of an item, returning the new value. A missing
    /// item starts at zero and is stored forever.
    async fn increment(&self, key: &str, value: i64) -> Result<i64>;

    /// Decrement the value of an item, returning the new value.
    async fn decrement(&self, key: &str, value: i64) -> Result<i64> {
        self.increment(key, -value).await
    }

    /// Store an item in the cache indefinitely.
    async fn forever(&self, key: &str, value: Value) -> Result<bool>;

    /// Adjust the expiration time of an existing item.
    async fn touch(&self, key: &str, seconds: u64) -> Result<bool>;

    /// Remove an item from the cache.
    async fn forget(&self, key: &str) -> Result<bool>;

    /// Remove all items from the cache.
    async fn flush(&self) -> Result<bool>;

    /// The cache key prefix.
    fn get_prefix(&self) -> String {
        String::new()
    }

    /// The store's lock provider, if it supports atomic locks.
    fn lock_provider(&self) -> Option<&dyn LockProvider> {
        None
    }

    /// Whether the store supports cache tags.
    fn supports_tags(&self) -> bool {
        false
    }

    /// Remove all locks from the store.
    async fn flush_locks(&self) -> Result<bool> {
        Err(BadMethodCallException::new("This cache store does not support flushing locks.").into())
    }
}

/// A store that can hand out atomic locks.
pub trait LockProvider: Send + Sync {
    /// Get a lock instance.
    fn lock(&self, name: &str, seconds: u64, owner: Option<String>) -> Lock;

    /// Restore a lock instance using the owner identifier.
    fn restore_lock(&self, name: &str, owner: &str) -> Lock {
        self.lock(name, 0, Some(owner.to_string()))
    }
}

/// Thrown when calling a method the cache store doesn't support.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct BadMethodCallException {
    pub message: String,
}

impl BadMethodCallException {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// The error returned when flushing locks that share the cache's storage.
pub(crate) fn separate_lock_store_required() -> illuminate_support::Error {
    RuntimeException::new(
        "Flushing locks is only supported when the lock store is separate from the cache store.",
    )
    .into()
}

/// PHP's `(int)` cast for a cached value.
pub(crate) fn int_value(value: &Value) -> i64 {
    value.to_i64_lossy().unwrap_or(0)
}
