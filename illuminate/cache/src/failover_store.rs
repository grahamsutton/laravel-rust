//! The `failover` store: tries each configured store in turn.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use illuminate_support::error::RuntimeException;
use illuminate_support::{Error, Result, Value};

use crate::lock::Lock;
use crate::repository::Repository;
use crate::store::{LockProvider, Store};

/// A store failed and the next one took over.
#[derive(Clone, Debug)]
pub struct CacheFailedOver {
    /// The name of the store that failed.
    pub store_name: String,
    /// Why it failed.
    pub exception: Arc<Error>,
}

/// Resolves a configured cache store by name.
pub type StoreResolver = Arc<dyn Fn(&str) -> Result<Repository> + Send + Sync>;

/// A store that uses the first of several stores that works: when one
/// fails (its server is down, say), the next one is used.
///
/// ```json
/// "failover": {"driver": "failover", "stores": ["redis", "database", "array"]}
/// ```
///
/// [`CacheFailedOver`] is dispatched the first time a store fails, and
/// again only after it has recovered and failed again.
pub struct FailoverStore {
    stores: Vec<String>,
    resolve: StoreResolver,
    failing: Mutex<Vec<String>>,
}

impl FailoverStore {
    /// Create a store trying the named stores in order.
    pub fn new(stores: Vec<String>, resolve: StoreResolver) -> Self {
        Self {
            stores,
            resolve,
            failing: Mutex::new(Vec::new()),
        }
    }

    /// The names of the stores, in the order they're tried.
    pub fn stores(&self) -> &[String] {
        &self.stores
    }

    fn store(&self, name: &str) -> Result<Arc<dyn Store>> {
        Ok((self.resolve)(name)?.get_store())
    }

    /// Record that the store failed, dispatching `CacheFailedOver` unless it
    /// was already failing.
    async fn failed(&self, name: &str, error: Error, attempt: &mut Attempt) {
        let error = Arc::new(error);
        let already_failing = self.failing.lock().unwrap().iter().any(|store| store == name);
        if !already_failing {
            let event = CacheFailedOver {
                store_name: name.to_string(),
                exception: error.clone(),
            };
            let _ = illuminate_events::Event::dispatch(event).await;
        }
        attempt.failed.push(name.to_string());
        attempt.last_error = Some(error);
    }

    /// Finish an attempt: remember which stores failed, and return the
    /// result or the last error.
    fn finish<T>(&self, outcome: Option<T>, attempt: Attempt) -> Result<T> {
        *self.failing.lock().unwrap() = attempt.failed;
        match (outcome, attempt.last_error) {
            (Some(value), _) => Ok(value),
            (None, Some(error)) => Err(Arc::try_unwrap(error)
                .unwrap_or_else(|error| RuntimeException::new(error.to_string()).into())),
            (None, None) => Err(RuntimeException::new("All failover cache stores failed.").into()),
        }
    }
}

/// The stores that failed during one operation.
#[derive(Default)]
struct Attempt {
    failed: Vec<String>,
    last_error: Option<Arc<Error>>,
}

/// Run the call on each store until one succeeds.
macro_rules! attempt_on_all_stores {
    ($this:ident, $store:ident => $call:expr) => {{
        let mut attempt = Attempt::default();
        let mut outcome = None;
        for name in &$this.stores {
            let result = match $this.store(name) {
                Ok($store) => $call.await,
                Err(error) => Err(error),
            };
            match result {
                Ok(value) => {
                    outcome = Some(value);
                    break;
                }
                Err(error) => $this.failed(name, error, &mut attempt).await,
            }
        }
        $this.finish(outcome, attempt)
    }};
}

#[async_trait]
impl Store for FailoverStore {
    async fn get(&self, key: &str) -> Result<Option<Value>> {
        attempt_on_all_stores!(self, store => store.get(key))
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<(String, Option<Value>)>> {
        attempt_on_all_stores!(self, store => store.many(keys))
    }

    async fn put(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        attempt_on_all_stores!(self, store => store.put(key, value.clone(), seconds))
    }

    async fn put_many(&self, values: Vec<(String, Value)>, seconds: u64) -> Result<bool> {
        attempt_on_all_stores!(self, store => store.put_many(values.clone(), seconds))
    }

    async fn add(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        attempt_on_all_stores!(self, store => store.add(key, value.clone(), seconds))
    }

    async fn increment(&self, key: &str, value: i64) -> Result<i64> {
        attempt_on_all_stores!(self, store => store.increment(key, value))
    }

    async fn decrement(&self, key: &str, value: i64) -> Result<i64> {
        attempt_on_all_stores!(self, store => store.decrement(key, value))
    }

    async fn forever(&self, key: &str, value: Value) -> Result<bool> {
        attempt_on_all_stores!(self, store => store.forever(key, value.clone()))
    }

    async fn touch(&self, key: &str, seconds: u64) -> Result<bool> {
        attempt_on_all_stores!(self, store => store.touch(key, seconds))
    }

    async fn forget(&self, key: &str) -> Result<bool> {
        attempt_on_all_stores!(self, store => store.forget(key))
    }

    async fn flush(&self) -> Result<bool> {
        attempt_on_all_stores!(self, store => store.flush())
    }

    fn lock_provider(&self) -> Option<&dyn LockProvider> {
        Some(self)
    }

    async fn flush_locks(&self) -> Result<bool> {
        let mut flushed = true;
        for name in &self.stores {
            let store = self.store(name)?;
            if let Ok(result) = store.flush_locks().await {
                flushed &= result;
            }
        }
        Ok(flushed)
    }
}

impl LockProvider for FailoverStore {
    /// Locks come from the first store that can be resolved and supports
    /// locks (a lock that is always acquired, when none does).
    fn lock(&self, name: &str, seconds: u64, owner: Option<String>) -> Lock {
        for store in &self.stores {
            if let Ok(store) = self.store(store)
                && let Some(provider) = store.lock_provider()
            {
                return provider.lock(name, seconds, owner);
            }
        }
        Lock::new(Arc::new(crate::lock::NoLock), name, seconds, owner)
    }
}
