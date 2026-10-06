//! Atomic locks.
//!
//! ```
//! use illuminate_cache::{ArrayStore, Repository};
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! let cache = Repository::new(ArrayStore::new());
//!
//! let lock = cache.lock("processing", 10);
//! assert!(lock.get().await.unwrap());
//!
//! // Anyone else trying to take the lock is turned away...
//! assert!(!cache.lock("processing", 10).get().await.unwrap());
//!
//! // ...until it is released.
//! lock.release().await.unwrap();
//! assert!(cache.lock("processing", 10).get().await.unwrap());
//! # });
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Result, Str, Value, json};

use crate::store::{BadMethodCallException, Store};

/// Thrown when a lock could not be acquired within the allotted time.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Unable to acquire lock [{name}].")]
pub struct LockTimeoutException {
    pub name: String,
}

/// What a lock driver needs to know about a lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockInfo {
    /// The name of the lock.
    pub name: String,
    /// The scope identifier of this lock.
    pub owner: String,
    /// The number of seconds the lock should be held (`0` for no expiration).
    pub seconds: u64,
}

/// The storage side of a lock (Laravel's `ArrayLock`, `CacheLock`, ...).
#[async_trait]
pub trait LockDriver: Send + Sync + 'static {
    /// Attempt to acquire the lock.
    async fn acquire(&self, lock: &LockInfo) -> Result<bool>;

    /// Release the lock, if it is owned by this lock's owner.
    async fn release(&self, lock: &LockInfo) -> Result<bool>;

    /// Release the lock regardless of ownership.
    async fn force_release(&self, lock: &LockInfo) -> Result<()>;

    /// The owner value currently written into the driver for this lock.
    async fn current_owner(&self, lock: &LockInfo) -> Result<Option<String>>;

    /// Extend the lock's expiration, if it is still owned.
    async fn refresh(&self, _lock: &LockInfo, _seconds: u64) -> Result<bool> {
        Err(RuntimeException::new("This lock driver does not support refreshing locks.").into())
    }

    /// Determine if the lock is currently held by anyone.
    async fn is_locked(&self, lock: &LockInfo) -> Result<bool> {
        Ok(self.current_owner(lock).await?.is_some())
    }
}

/// An atomic lock.
///
/// Locks are cheap handles: clone one to pass it around, or rebuild it
/// elsewhere from its [`owner`](Lock::owner) token with `Cache::restore_lock`.
#[derive(Clone)]
pub struct Lock {
    info: LockInfo,
    sleep_milliseconds: u64,
    driver: Arc<dyn LockDriver>,
}

impl std::fmt::Debug for Lock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lock").field("info", &self.info).finish()
    }
}

impl Lock {
    /// Create a lock. Without an owner, a random owner token is generated.
    pub fn new(driver: Arc<dyn LockDriver>, name: impl Into<String>, seconds: u64, owner: Option<String>) -> Self {
        Self {
            info: LockInfo { name: name.into(), owner: owner.unwrap_or_else(|| Str::random(16)), seconds },
            sleep_milliseconds: 250,
            driver,
        }
    }

    /// The name of the lock.
    pub fn name(&self) -> &str {
        &self.info.name
    }

    /// The current owner token of the lock.
    pub fn owner(&self) -> &str {
        &self.info.owner
    }

    /// The number of seconds the lock is held for.
    pub fn seconds(&self) -> u64 {
        self.info.seconds
    }

    /// Specify the number of milliseconds to sleep between blocked
    /// acquisition attempts (250 by default).
    pub fn between_blocked_attempts_sleep_for(mut self, milliseconds: u64) -> Self {
        self.sleep_milliseconds = milliseconds;
        self
    }

    /// Attempt to acquire the lock.
    pub async fn acquire(&self) -> Result<bool> {
        self.driver.acquire(&self.info).await
    }

    /// Attempt to acquire the lock, returning whether it was acquired.
    pub async fn get(&self) -> Result<bool> {
        self.acquire().await
    }

    /// Attempt to acquire the lock and, if acquired, run the callback and
    /// release the lock. Returns `None` when the lock was not acquired.
    ///
    /// ```
    /// use illuminate_cache::{ArrayStore, Repository};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let cache = Repository::new(ArrayStore::new());
    ///
    /// let result = cache.lock("foo", 10).get_with(|| async { "Lock acquired!" }).await.unwrap();
    ///
    /// assert_eq!(result, Some("Lock acquired!"));
    /// # });
    /// ```
    pub async fn get_with<T, F, Fut>(&self, callback: F) -> Result<Option<T>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        if !self.acquire().await? {
            return Ok(None);
        }
        let result = callback().await;
        self.release().await?;
        Ok(Some(result))
    }

    /// Wait up to `seconds` for the lock, retrying every
    /// [`between_blocked_attempts_sleep_for`](Lock::between_blocked_attempts_sleep_for)
    /// milliseconds. Fails with a [`LockTimeoutException`].
    pub async fn block(&self, seconds: u64) -> Result<bool> {
        let started = Instant::now();
        let timeout = Duration::from_secs(seconds);
        let sleep = Duration::from_millis(self.sleep_milliseconds);
        while !self.acquire().await? {
            if started.elapsed() + sleep >= timeout {
                return Err(LockTimeoutException { name: self.info.name.clone() }.into());
            }
            tokio::time::sleep(sleep).await;
        }
        Ok(true)
    }

    /// Wait up to `seconds` for the lock, then run the callback and release it.
    ///
    /// ```
    /// use illuminate_cache::{ArrayStore, Repository};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let cache = Repository::new(ArrayStore::new());
    ///
    /// let value = cache.lock("foo", 10).block_with(5, || async { 42 }).await.unwrap();
    ///
    /// assert_eq!(value, 42);
    /// # });
    /// ```
    pub async fn block_with<T, F, Fut>(&self, seconds: u64, callback: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        self.block(seconds).await?;
        let result = callback().await;
        self.release().await?;
        Ok(result)
    }

    /// Release the lock, if it is owned by this lock's owner.
    pub async fn release(&self) -> Result<bool> {
        self.driver.release(&self.info).await
    }

    /// Release the lock regardless of who owns it.
    pub async fn force_release(&self) -> Result<()> {
        self.driver.force_release(&self.info).await
    }

    /// Extend the lock by the given number of seconds (or its original duration).
    pub async fn refresh(&self, seconds: Option<u64>) -> Result<bool> {
        self.driver.refresh(&self.info, seconds.unwrap_or(self.info.seconds)).await
    }

    /// Determine if the lock is currently held by anyone.
    pub async fn is_locked(&self) -> Result<bool> {
        self.driver.is_locked(&self.info).await
    }

    /// Determine whether this lock is owned by the current process.
    pub async fn is_owned_by_current_process(&self) -> Result<bool> {
        self.is_owned_by(&self.info.owner).await
    }

    /// Determine whether this lock is owned by the given owner.
    pub async fn is_owned_by(&self, owner: &str) -> Result<bool> {
        Ok(self.driver.current_owner(&self.info).await?.as_deref() == Some(owner))
    }
}

// ----------------------------------------------------------------------
// Array locks
// ----------------------------------------------------------------------

/// A lock held in memory by the array store.
#[derive(Debug, Clone)]
pub(crate) struct ArrayLockEntry {
    owner: String,
    expires_at: Option<Carbon>,
}

pub(crate) type ArrayLocks = Arc<Mutex<HashMap<String, ArrayLockEntry>>>;

/// Locks for the array store.
pub struct ArrayLock {
    locks: ArrayLocks,
}

impl ArrayLock {
    pub(crate) fn new(locks: ArrayLocks) -> Self {
        Self { locks }
    }
}

#[async_trait]
impl LockDriver for ArrayLock {
    async fn acquire(&self, lock: &LockInfo) -> Result<bool> {
        let mut locks = self.locks.lock().unwrap();
        if let Some(existing) = locks.get(&lock.name) {
            let expiration = existing.expires_at.unwrap_or_else(|| Carbon::now().add_second());
            if expiration.is_future() {
                return Ok(false);
            }
        }
        let expires_at = (lock.seconds > 0).then(|| Carbon::now().add_seconds(lock.seconds as i64));
        locks.insert(lock.name.clone(), ArrayLockEntry { owner: lock.owner.clone(), expires_at });
        Ok(true)
    }

    async fn release(&self, lock: &LockInfo) -> Result<bool> {
        let mut locks = self.locks.lock().unwrap();
        match locks.get(&lock.name) {
            Some(existing) if existing.owner == lock.owner => {
                locks.remove(&lock.name);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn force_release(&self, lock: &LockInfo) -> Result<()> {
        self.locks.lock().unwrap().remove(&lock.name);
        Ok(())
    }

    async fn current_owner(&self, lock: &LockInfo) -> Result<Option<String>> {
        Ok(self.locks.lock().unwrap().get(&lock.name).map(|entry| entry.owner.clone()))
    }

    async fn refresh(&self, lock: &LockInfo, seconds: u64) -> Result<bool> {
        let mut locks = self.locks.lock().unwrap();
        let Some(entry) = locks.get_mut(&lock.name) else {
            return Ok(false);
        };
        if entry.owner != lock.owner || entry.expires_at.is_some_and(|at| !at.is_future()) {
            return Ok(false);
        }
        entry.expires_at = (seconds > 0).then(|| Carbon::now().add_seconds(seconds as i64));
        Ok(true)
    }
}

// ----------------------------------------------------------------------
// Cache locks
// ----------------------------------------------------------------------

/// A lock stored as a regular cache item, for stores with an atomic `add`.
pub struct CacheLock {
    store: Arc<dyn Store>,
}

impl CacheLock {
    /// Create a lock driver storing its locks in the given store.
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl LockDriver for CacheLock {
    async fn acquire(&self, lock: &LockInfo) -> Result<bool> {
        self.store.add(&lock.name, json!(lock.owner), lock.seconds).await
    }

    async fn release(&self, lock: &LockInfo) -> Result<bool> {
        if self.current_owner(lock).await?.as_deref() == Some(lock.owner.as_str()) {
            return self.store.forget(&lock.name).await;
        }
        Ok(false)
    }

    async fn force_release(&self, lock: &LockInfo) -> Result<()> {
        self.store.forget(&lock.name).await?;
        Ok(())
    }

    async fn current_owner(&self, lock: &LockInfo) -> Result<Option<String>> {
        Ok(self.store.get(&lock.name).await?.map(|owner| match owner {
            Value::String(owner) => owner,
            other => other.to_string(),
        }))
    }
}

// ----------------------------------------------------------------------
// No-op & unsupported locks
// ----------------------------------------------------------------------

/// A lock that is always available (used by the null store).
pub struct NoLock;

#[async_trait]
impl LockDriver for NoLock {
    async fn acquire(&self, _lock: &LockInfo) -> Result<bool> {
        Ok(true)
    }

    async fn release(&self, _lock: &LockInfo) -> Result<bool> {
        Ok(true)
    }

    async fn force_release(&self, _lock: &LockInfo) -> Result<()> {
        Ok(())
    }

    async fn current_owner(&self, lock: &LockInfo) -> Result<Option<String>> {
        Ok(Some(lock.owner.clone()))
    }

    async fn refresh(&self, _lock: &LockInfo, _seconds: u64) -> Result<bool> {
        Ok(true)
    }

    async fn is_locked(&self, _lock: &LockInfo) -> Result<bool> {
        Ok(false)
    }
}

/// The lock handed out by stores without lock support: every operation
/// fails with a [`BadMethodCallException`].
pub(crate) struct UnsupportedLock;

impl UnsupportedLock {
    fn error() -> illuminate_support::Error {
        BadMethodCallException::new("This cache store does not support locks.").into()
    }
}

#[async_trait]
impl LockDriver for UnsupportedLock {
    async fn acquire(&self, _lock: &LockInfo) -> Result<bool> {
        Err(Self::error())
    }

    async fn release(&self, _lock: &LockInfo) -> Result<bool> {
        Err(Self::error())
    }

    async fn force_release(&self, _lock: &LockInfo) -> Result<()> {
        Err(Self::error())
    }

    async fn current_owner(&self, _lock: &LockInfo) -> Result<Option<String>> {
        Err(Self::error())
    }

    async fn refresh(&self, _lock: &LockInfo, _seconds: u64) -> Result<bool> {
        Err(Self::error())
    }
}
