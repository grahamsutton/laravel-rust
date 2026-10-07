//! Debounced jobs: when the same job is dispatched many times in a short
//! window, only the latest dispatch runs.

use illuminate_cache::{Cache, Repository as CacheRepository};
use illuminate_support::{Carbon, Result, Str, ValueExt};

use crate::job::ShouldQueue;

/// How long a [debounced](ShouldQueue::debounce_for) job waits for newer
/// dispatches (Laravel's `#[DebounceFor]` attribute).
///
/// ```
/// use illuminate_queue::DebounceFor;
///
/// let debounce = DebounceFor::new(30).max_wait(120);
///
/// assert_eq!(debounce.seconds, 30);
/// assert_eq!(debounce.max_wait, Some(120));
/// assert_eq!(DebounceFor::from(30), DebounceFor::new(30));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DebounceFor {
    /// Seconds to debounce the job for.
    pub seconds: u64,
    /// The maximum number of seconds the job can be deferred before it is
    /// forced to run.
    pub max_wait: Option<u64>,
}

impl DebounceFor {
    /// Debounce the job for the given number of seconds.
    pub fn new(seconds: u64) -> Self {
        Self {
            seconds,
            max_wait: None,
        }
    }

    /// Run the job anyway once it has been deferred this many seconds.
    pub fn max_wait(mut self, seconds: u64) -> Self {
        self.max_wait = Some(seconds);
        self
    }
}

impl From<u64> for DebounceFor {
    fn from(seconds: u64) -> Self {
        Self::new(seconds)
    }
}

/// The result of [acquiring](DebounceLock::acquire) a debounce lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcquiredDebounce {
    /// The token identifying the latest dispatch.
    pub owner: String,
    /// Whether the job has waited longer than its maximum wait.
    pub max_wait_exceeded: bool,
}

/// Tracks the latest dispatch of each debounced job in the cache.
///
/// Every dispatch stores a fresh owner token under the job's key, so the
/// last writer wins: when a worker picks up a job whose token is no longer
/// the current one, a newer dispatch superseded it and it is deleted.
pub struct DebounceLock {
    cache: Option<CacheRepository>,
}

impl DebounceLock {
    /// Create a debounce lock using the given cache (the job's
    /// `debounce_via` store, or the default store, when `None`).
    pub fn new(cache: Option<CacheRepository>) -> Self {
        Self { cache }
    }

    fn cache_for(&self, job: &dyn ShouldQueue) -> Result<CacheRepository> {
        if let Some(store) = job.debounce_via() {
            return Cache::store(&store);
        }
        match &self.cache {
            Some(cache) => Ok(cache.clone()),
            None => Cache::default_store(),
        }
    }

    /// Store a new owner token for the job, replacing any existing one.
    ///
    /// `debounce_for` and `max_wait` default to the job's
    /// [`debounce_for`](ShouldQueue::debounce_for) setting.
    pub async fn acquire(
        &self,
        job: &dyn ShouldQueue,
        debounce_for: Option<u64>,
        max_wait: Option<u64>,
    ) -> Result<AcquiredDebounce> {
        let cache = self.cache_for(job)?;
        let setting = job.debounce_for();
        let debounce_for = debounce_for
            .or(setting.map(|setting| setting.seconds))
            .unwrap_or(0);
        let max_wait = max_wait.or(setting.and_then(|setting| setting.max_wait));

        let ttl = (debounce_for * 10).max(300);
        let key = Self::key(job);
        let owner = Str::random(40);
        cache.put(&key, &owner, ttl).await?;

        Ok(AcquiredDebounce {
            max_wait_exceeded: Self::max_wait_exceeded(&cache, &key, ttl, max_wait).await?,
            owner,
        })
    }

    async fn max_wait_exceeded(
        cache: &CacheRepository,
        key: &str,
        ttl: u64,
        max_wait: Option<u64>,
    ) -> Result<bool> {
        let Some(max_wait) = max_wait else {
            return Ok(false);
        };
        let timestamp_key = format!("{key}:first_dispatched_at");
        let now = Carbon::now().timestamp();

        let Some(first_dispatched_at) = cache
            .get(&timestamp_key)
            .await?
            .and_then(|value| value.to_i64_lossy())
        else {
            cache.put(&timestamp_key, now, ttl).await?;
            return Ok(false);
        };

        if now - first_dispatched_at >= i64::try_from(max_wait).unwrap_or(i64::MAX) {
            cache.forget(&timestamp_key).await?;
            return Ok(true);
        }
        Ok(false)
    }

    /// The owner token of the job's latest dispatch.
    pub async fn current_owner(&self, job: &dyn ShouldQueue) -> Result<Option<String>> {
        let value = self.cache_for(job)?.get(&Self::key(job)).await?;
        Ok(value
            .filter(|value| !value.is_null())
            .map(|value| value.to_string_lossy()))
    }

    /// Remove the job's token. With an owner, only that owner's token is
    /// removed.
    pub async fn release(&self, job: &dyn ShouldQueue, owner: Option<&str>) -> Result<()> {
        let cache = self.cache_for(job)?;
        let key = Self::key(job);
        if let Some(owner) = owner.filter(|owner| !owner.is_empty())
            && self.current_owner(job).await?.as_deref() != Some(owner)
        {
            return Ok(());
        }
        cache.forget(&key).await?;
        cache.forget(&format!("{key}:first_dispatched_at")).await?;
        Ok(())
    }

    /// Restart the job's maximum wait: called when the job starts running.
    pub async fn release_max_wait(&self, job: &dyn ShouldQueue) -> Result<()> {
        self.cache_for(job)?
            .forget(&format!("{}:first_dispatched_at", Self::key(job)))
            .await?;
        Ok(())
    }

    /// The cache key of the job's debounce token.
    ///
    /// ```
    /// use illuminate_queue::{DebounceFor, DebounceLock, ShouldQueue, async_trait};
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
    ///     fn debounce_for(&self) -> Option<DebounceFor> {
    ///         Some(DebounceFor::new(30))
    ///     }
    ///
    ///     fn debounce_id(&self) -> String {
    ///         self.product_id.to_string()
    ///     }
    ///
    ///     fn job_name() -> &'static str {
    ///         "App\\Jobs\\UpdateSearchIndex"
    ///     }
    /// }
    ///
    /// let job = UpdateSearchIndex { product_id: 7 };
    /// assert_eq!(DebounceLock::key(&job), "laravel_debounced_job:App\\Jobs\\UpdateSearchIndex:7");
    /// ```
    pub fn key(job: &dyn ShouldQueue) -> String {
        format!(
            "laravel_debounced_job:{}:{}",
            job.command_name(),
            job.debounce_id()
        )
    }
}

/// Release the job's debounce token, reporting (rather than throwing)
/// errors.
pub(crate) async fn release_debounce_lock(job: &dyn ShouldQueue, owner: Option<&str>) {
    if let Err(error) = DebounceLock::new(None).release(job, owner).await {
        crate::report(&error);
    }
}
