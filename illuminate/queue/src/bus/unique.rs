//! Unique job locks.

use illuminate_cache::{Cache, Repository as CacheRepository};
use illuminate_support::Result;

use crate::job::ShouldQueue;

/// The cache lock that keeps a [unique](ShouldQueue::unique_id) job unique.
///
/// The lock is taken when the job is dispatched and released when it
/// finishes processing (or fails for good) — or as soon as it starts
/// processing, for [`unique_until_processing`](ShouldQueue::unique_until_processing)
/// jobs.
pub struct UniqueLock {
    cache: Option<CacheRepository>,
}

impl UniqueLock {
    /// Create a unique lock using the given cache (the job's `unique_via`
    /// store, or the default store, when `None`).
    pub fn new(cache: Option<CacheRepository>) -> Self {
        Self { cache }
    }

    fn cache_for(&self, job: &dyn ShouldQueue) -> Result<CacheRepository> {
        if let Some(store) = job.unique_via() {
            return Cache::store(&store);
        }
        match &self.cache {
            Some(cache) => Ok(cache.clone()),
            None => Cache::default_store(),
        }
    }

    /// Attempt to acquire the lock for the job, returning the lock's owner
    /// token when acquired.
    pub async fn acquire(&self, job: &dyn ShouldQueue) -> Result<Option<String>> {
        let cache = self.cache_for(job)?;
        let lock = cache.lock(&Self::key(job), job.unique_for());
        if lock.get().await? {
            Ok(Some(lock.owner().to_string()))
        } else {
            Ok(None)
        }
    }

    /// Release the job's lock. With the owner token only that owner's lock
    /// is released; without it the lock is released regardless.
    pub async fn release(&self, job: &dyn ShouldQueue, owner: Option<&str>) -> Result<()> {
        let cache = self.cache_for(job)?;
        let key = Self::key(job);
        match owner.filter(|owner| !owner.is_empty()) {
            Some(owner) => {
                cache.restore_lock(&key, owner).release().await?;
            }
            None => cache.lock(&key, 0).force_release().await?,
        }
        Ok(())
    }

    /// The cache key of the job's unique lock.
    ///
    /// ```
    /// use illuminate_queue::{ShouldQueue, UniqueLock, async_trait};
    /// use illuminate_support::Result;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize)]
    /// struct UpdateSearchIndex {
    ///     product_id: u64,
    /// }
    ///
    /// #[async_trait]
    /// impl ShouldQueue for UpdateSearchIndex {
    ///     async fn handle(&self) -> Result<()> {
    ///         Ok(())
    ///     }
    ///
    ///     fn unique_id(&self) -> Option<String> {
    ///         Some(self.product_id.to_string())
    ///     }
    ///
    ///     fn job_name() -> &'static str {
    ///         "App\\Jobs\\UpdateSearchIndex"
    ///     }
    /// }
    ///
    /// let job = UpdateSearchIndex { product_id: 7 };
    /// assert_eq!(UniqueLock::key(&job), "laravel_unique_job:App\\Jobs\\UpdateSearchIndex:7");
    /// ```
    pub fn key(job: &dyn ShouldQueue) -> String {
        format!(
            "laravel_unique_job:{}:{}",
            job.command_name(),
            job.unique_id().unwrap_or_default()
        )
    }
}
