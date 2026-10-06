//! The array cache store: values kept in memory for the life of the process.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use indexmap::IndexMap;

use illuminate_support::{Carbon, Result, Value};

use crate::lock::{ArrayLock, ArrayLocks, Lock};
use crate::store::{LockProvider, Store, int_value};

enum Stored {
    Raw(Value),
    Serialized(String),
}

struct Item {
    value: Stored,
    /// Expiration as a UNIX timestamp in milliseconds; `0` never expires.
    expires_at: i64,
}

/// An in-memory cache store — perfect for tests and per-process caching.
///
/// Operations are atomic: `add` and `increment` happen under a single lock.
///
/// ```
/// use illuminate_cache::{ArrayStore, Store};
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let store = ArrayStore::new();
///
/// store.put("name", json!("Taylor"), 60).await.unwrap();
///
/// assert_eq!(store.get("name").await.unwrap(), Some(json!("Taylor")));
/// assert_eq!(store.increment("visits", 1).await.unwrap(), 1);
/// # });
/// ```
pub struct ArrayStore {
    storage: Mutex<IndexMap<String, Item>>,
    locks: ArrayLocks,
    serializes_values: bool,
}

impl Default for ArrayStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ArrayStore {
    /// Create a new array store.
    pub fn new() -> Self {
        Self::with_serialization(false)
    }

    /// Create a new array store that serializes values (as JSON text) when
    /// storing them, like the `serialize` option of the `array` driver.
    pub fn with_serialization(serializes_values: bool) -> Self {
        Self {
            storage: Mutex::new(IndexMap::new()),
            locks: Arc::new(Mutex::new(HashMap::new())),
            serializes_values,
        }
    }

    /// Whether values are serialized within the store.
    pub fn serializes_values(&self) -> bool {
        self.serializes_values
    }

    /// All of the unexpired cached values.
    pub fn all(&self) -> IndexMap<String, Value> {
        let now = now_millis();
        self.storage
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, item)| !is_expired(item, now))
            .map(|(key, item)| (key.clone(), self.unpack(&item.value)))
            .collect()
    }

    fn pack(&self, value: Value) -> Stored {
        if self.serializes_values {
            Stored::Serialized(value.to_string())
        } else {
            Stored::Raw(value)
        }
    }

    fn unpack(&self, stored: &Stored) -> Value {
        match stored {
            Stored::Raw(value) => value.clone(),
            Stored::Serialized(json) => serde_json::from_str(json).unwrap_or(Value::Null),
        }
    }

    fn live<'a>(storage: &'a mut IndexMap<String, Item>, key: &str) -> Option<&'a mut Item> {
        let now = now_millis();
        if storage.get(key).is_some_and(|item| is_expired(item, now)) {
            storage.shift_remove(key);
            return None;
        }
        storage.get_mut(key)
    }
}

fn now_millis() -> i64 {
    Carbon::now().timestamp_millis()
}

fn is_expired(item: &Item, now: i64) -> bool {
    item.expires_at != 0 && now >= item.expires_at
}

/// The expiration timestamp (in milliseconds) for the given number of seconds.
fn expiration(seconds: u64) -> i64 {
    if seconds == 0 {
        0
    } else {
        now_millis().saturating_add((seconds as i64).saturating_mul(1000))
    }
}

#[async_trait]
impl Store for ArrayStore {
    async fn get(&self, key: &str) -> Result<Option<Value>> {
        let mut storage = self.storage.lock().unwrap();
        Ok(Self::live(&mut storage, key).map(|item| self.unpack(&item.value)))
    }

    async fn put(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        let item = Item {
            value: self.pack(value),
            expires_at: expiration(seconds),
        };
        self.storage.lock().unwrap().insert(key.to_string(), item);
        Ok(true)
    }

    async fn add(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        let mut storage = self.storage.lock().unwrap();
        if Self::live(&mut storage, key).is_some() {
            return Ok(false);
        }
        storage.insert(
            key.to_string(),
            Item {
                value: self.pack(value),
                expires_at: expiration(seconds),
            },
        );
        Ok(true)
    }

    async fn increment(&self, key: &str, value: i64) -> Result<i64> {
        let mut storage = self.storage.lock().unwrap();
        if let Some(item) = Self::live(&mut storage, key) {
            let incremented = int_value(&self.unpack(&item.value)) + value;
            item.value = self.pack(Value::from(incremented));
            return Ok(incremented);
        }
        storage.insert(
            key.to_string(),
            Item {
                value: self.pack(Value::from(value)),
                expires_at: 0,
            },
        );
        Ok(value)
    }

    async fn forever(&self, key: &str, value: Value) -> Result<bool> {
        self.put(key, value, 0).await
    }

    async fn touch(&self, key: &str, seconds: u64) -> Result<bool> {
        let mut storage = self.storage.lock().unwrap();
        match Self::live(&mut storage, key) {
            Some(item) => {
                item.expires_at = expiration(seconds);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn forget(&self, key: &str) -> Result<bool> {
        Ok(self.storage.lock().unwrap().shift_remove(key).is_some())
    }

    async fn flush(&self) -> Result<bool> {
        self.storage.lock().unwrap().clear();
        Ok(true)
    }

    fn lock_provider(&self) -> Option<&dyn LockProvider> {
        Some(self)
    }

    fn supports_tags(&self) -> bool {
        true
    }

    async fn flush_locks(&self) -> Result<bool> {
        self.locks.lock().unwrap().clear();
        Ok(true)
    }
}

impl LockProvider for ArrayStore {
    fn lock(&self, name: &str, seconds: u64, owner: Option<String>) -> Lock {
        Lock::new(
            Arc::new(ArrayLock::new(self.locks.clone())),
            name,
            seconds,
            owner,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::freeze_time;
    use illuminate_support::json;

    #[tokio::test]
    async fn items_expire() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let store = ArrayStore::new();
        store.put("foo", json!("bar"), 10).await.unwrap();
        store.forever("baz", json!(1)).await.unwrap();

        time.travel_seconds(9);
        assert_eq!(store.get("foo").await.unwrap(), Some(json!("bar")));
        time.travel_seconds(1);
        assert_eq!(store.get("foo").await.unwrap(), None);
        time.travel_seconds(1_000_000);
        assert_eq!(store.get("baz").await.unwrap(), Some(json!(1)));
        assert_eq!(store.all().len(), 1);
    }

    #[tokio::test]
    async fn add_is_atomic_and_respects_expiration() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let store = Arc::new(ArrayStore::new());

        let mut handles = Vec::new();
        for i in 0..20 {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                store.add("winner", json!(i), 10).await.unwrap()
            }));
        }
        let mut winners = 0;
        for handle in handles {
            if handle.await.unwrap() {
                winners += 1;
            }
        }
        assert_eq!(winners, 1);

        time.travel_seconds(10);
        assert!(store.add("winner", json!("late"), 10).await.unwrap());
        assert_eq!(store.get("winner").await.unwrap(), Some(json!("late")));
    }

    #[tokio::test]
    async fn increment_keeps_expiration() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let store = ArrayStore::new();
        assert_eq!(store.increment("hits", 1).await.unwrap(), 1);
        assert_eq!(store.decrement("hits", 3).await.unwrap(), -2);

        store.put("counter", json!("5"), 10).await.unwrap();
        assert_eq!(store.increment("counter", 2).await.unwrap(), 7);
        time.travel_seconds(10);
        assert_eq!(store.get("counter").await.unwrap(), None);
    }

    #[tokio::test]
    async fn touch_forget_and_flush() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let store = ArrayStore::new();
        store.put("a", json!(1), 10).await.unwrap();
        assert!(store.touch("a", 100).await.unwrap());
        assert!(!store.touch("missing", 100).await.unwrap());
        time.travel_seconds(50);
        assert!(store.get("a").await.unwrap().is_some());

        assert!(store.forget("a").await.unwrap());
        assert!(!store.forget("a").await.unwrap());
        store.put("b", json!(2), 10).await.unwrap();
        assert!(store.flush().await.unwrap());
        assert!(store.all().is_empty());
    }

    #[tokio::test]
    async fn values_can_be_serialized() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let store = ArrayStore::with_serialization(true);
        assert!(store.serializes_values());
        store
            .put("user", json!({"name": "Taylor", "roles": ["admin"]}), 10)
            .await
            .unwrap();
        assert_eq!(
            store.get("user").await.unwrap().unwrap()["roles"][0],
            "admin"
        );
        assert_eq!(store.increment("n", 5).await.unwrap(), 5);
        assert_eq!(
            store.many(&["user".into(), "nope".into()]).await.unwrap()[1],
            ("nope".into(), None)
        );
    }
}
