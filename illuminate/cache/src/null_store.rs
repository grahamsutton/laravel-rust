//! The null cache store: caching disabled.

use std::sync::Arc;

use async_trait::async_trait;

use illuminate_support::{Result, Value};

use crate::lock::{Lock, NoLock};
use crate::store::{LockProvider, Store};

/// A store that never stores anything. Handy for disabling caching in an
/// environment, while every lock it hands out is always available.
///
/// ```
/// use illuminate_cache::{NullStore, Repository};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let cache = Repository::new(NullStore);
///
/// cache.put("name", "Taylor", 60).await.unwrap();
///
/// assert_eq!(cache.get("name").await.unwrap(), None);
/// # });
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct NullStore;

#[async_trait]
impl Store for NullStore {
    async fn get(&self, _key: &str) -> Result<Option<Value>> {
        Ok(None)
    }

    async fn put(&self, _key: &str, _value: Value, _seconds: u64) -> Result<bool> {
        Ok(false)
    }

    async fn add(&self, _key: &str, _value: Value, _seconds: u64) -> Result<bool> {
        Ok(false)
    }

    async fn increment(&self, _key: &str, _value: i64) -> Result<i64> {
        Ok(0)
    }

    async fn forever(&self, _key: &str, _value: Value) -> Result<bool> {
        Ok(false)
    }

    async fn touch(&self, _key: &str, _seconds: u64) -> Result<bool> {
        Ok(false)
    }

    async fn forget(&self, _key: &str) -> Result<bool> {
        Ok(true)
    }

    async fn flush(&self) -> Result<bool> {
        Ok(true)
    }

    fn lock_provider(&self) -> Option<&dyn LockProvider> {
        Some(self)
    }

    fn supports_tags(&self) -> bool {
        true
    }

    async fn flush_locks(&self) -> Result<bool> {
        Ok(true)
    }
}

impl LockProvider for NullStore {
    fn lock(&self, name: &str, seconds: u64, owner: Option<String>) -> Lock {
        Lock::new(Arc::new(NoLock), name, seconds, owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[tokio::test]
    async fn nothing_is_ever_stored() {
        let store = NullStore;
        assert!(!store.put("a", json!(1), 10).await.unwrap());
        assert!(!store.forever("a", json!(1)).await.unwrap());
        assert!(!store.add("a", json!(1), 10).await.unwrap());
        assert_eq!(store.get("a").await.unwrap(), None);
        assert_eq!(store.increment("a", 1).await.unwrap(), 0);
        assert!(!store.touch("a", 10).await.unwrap());
        assert!(store.forget("a").await.unwrap());
        assert!(store.flush().await.unwrap());
        assert!(store.flush_locks().await.unwrap());

        let lock = store.lock("a", 10, Some("me".into()));
        assert!(lock.get().await.unwrap());
        assert!(store.lock("a", 10, None).get().await.unwrap());
        assert!(!lock.is_locked().await.unwrap());
        assert!(lock.is_owned_by("me").await.unwrap());
        assert!(lock.refresh(None).await.unwrap());
        assert!(lock.release().await.unwrap());
    }
}
