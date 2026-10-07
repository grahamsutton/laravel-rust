//! The `RateLimited` job middleware.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use illuminate_cache::Limit;
use illuminate_container::{Container, try_app};
use illuminate_support::Result;

use super::{JobMiddleware, Next, md5};
use crate::context::InteractsWithQueue;
use crate::job::ShouldQueue;

/// The limits a job rate limiter returns: one [`Limit`], several, or
/// [`Limit::none()`].
#[derive(Debug, Clone, Default)]
pub struct JobLimits(pub Vec<Limit>);

impl From<Limit> for JobLimits {
    fn from(limit: Limit) -> Self {
        JobLimits(vec![limit])
    }
}

impl From<Vec<Limit>> for JobLimits {
    fn from(limits: Vec<Limit>) -> Self {
        JobLimits(limits)
    }
}

impl<const N: usize> From<[Limit; N]> for JobLimits {
    fn from(limits: [Limit; N]) -> Self {
        JobLimits(limits.into())
    }
}

type JobLimiter = Arc<dyn Fn(&dyn ShouldQueue) -> Option<JobLimits> + Send + Sync>;

/// The named rate limiters jobs can be limited by.
///
/// Define them with [`RateLimitsJobs::for_job`] on the `RateLimiter`
/// facade, typically in a service provider's `boot` method.
#[derive(Default)]
pub struct JobRateLimiters {
    limiters: RwLock<HashMap<String, JobLimiter>>,
}

impl JobRateLimiters {
    /// Create an empty set of limiters.
    pub fn new() -> Self {
        Self::default()
    }

    /// The limiters bound in the container (registered on first use).
    pub fn instance() -> Arc<JobRateLimiters> {
        if let Some(limiters) = try_app::<JobRateLimiters>() {
            return limiters;
        }
        let container = Container::get_instance();
        container.singleton_if::<JobRateLimiters>(|_| Arc::new(JobRateLimiters::new()));
        container.make::<JobRateLimiters>()
    }

    /// Register a named limiter for jobs of type `T`. Jobs of other types
    /// limited by this name are not limited.
    pub fn for_<T, R, F>(&self, name: &str, callback: F) -> &Self
    where
        T: ShouldQueue,
        R: Into<JobLimits>,
        F: Fn(&T) -> R + Send + Sync + 'static,
    {
        let limiter: JobLimiter = Arc::new(move |job: &dyn ShouldQueue| {
            job.downcast_ref::<T>().map(|job| callback(job).into())
        });
        self.limiters
            .write()
            .unwrap()
            .insert(name.to_string(), limiter);
        self
    }

    /// Register a named limiter for any job.
    pub fn for_any<R, F>(&self, name: &str, callback: F) -> &Self
    where
        R: Into<JobLimits>,
        F: Fn(&dyn ShouldQueue) -> R + Send + Sync + 'static,
    {
        let limiter: JobLimiter = Arc::new(move |job: &dyn ShouldQueue| Some(callback(job).into()));
        self.limiters
            .write()
            .unwrap()
            .insert(name.to_string(), limiter);
        self
    }

    /// Get the limits of the named limiter for the given job.
    pub fn limits(&self, name: &str, job: &dyn ShouldQueue) -> Option<JobLimits> {
        let limiter = self.limiters.read().unwrap().get(name).cloned()?;
        limiter(job)
    }

    /// Determine if a limiter with the given name exists.
    pub fn has(&self, name: &str) -> bool {
        self.limiters.read().unwrap().contains_key(name)
    }
}

/// Define job rate limiters on the `RateLimiter` facade.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_cache::Limit;
/// use illuminate_cache::facades::RateLimiter;
/// use illuminate_container::Container;
/// use illuminate_queue::{ShouldQueue, async_trait};
/// use illuminate_queue::middleware::RateLimitsJobs;
/// use illuminate_support::Result;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct BackupUserData {
///     user_id: u64,
///     vip: bool,
/// }
///
/// #[async_trait]
/// impl ShouldQueue for BackupUserData {
///     async fn handle(&self) -> Result<()> {
///         Ok(())
///     }
/// }
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container);
///
/// RateLimiter::for_job("backups", |job: &BackupUserData| {
///     if job.vip { Limit::none() } else { Limit::per_hour(1).by(job.user_id) }
/// });
/// ```
pub trait RateLimitsJobs {
    /// Register a named rate limiter for jobs of type `T`.
    fn for_job<T, R, F>(name: &str, callback: F)
    where
        T: ShouldQueue,
        R: Into<JobLimits>,
        F: Fn(&T) -> R + Send + Sync + 'static;
}

impl RateLimitsJobs for illuminate_cache::facades::RateLimiter {
    fn for_job<T, R, F>(name: &str, callback: F)
    where
        T: ShouldQueue,
        R: Into<JobLimits>,
        F: Fn(&T) -> R + Send + Sync + 'static,
    {
        JobRateLimiters::instance().for_(name, callback);
    }
}

/// Limit how often a job may run, using a named rate limiter. Jobs over
/// the limit are released back onto the queue until the limit resets.
///
/// ```
/// use illuminate_queue::middleware::RateLimited;
///
/// let middleware = RateLimited::new("backups").release_after(60);
/// let strict = RateLimited::new("backups").dont_release();
/// # let _ = (middleware, strict);
/// ```
#[derive(Debug, Clone)]
pub struct RateLimited {
    limiter_name: String,
    release_after: Option<u64>,
    should_release: bool,
}

impl RateLimited {
    /// Rate limit the job with the named limiter.
    pub fn new(limiter_name: impl Into<String>) -> Self {
        Self {
            limiter_name: limiter_name.into(),
            release_after: None,
            should_release: true,
        }
    }

    /// Release limited jobs for the given number of seconds (by default,
    /// until the limit resets).
    pub fn release_after(mut self, seconds: u64) -> Self {
        self.release_after = Some(seconds);
        self
    }

    /// Delete limited jobs instead of releasing them.
    pub fn dont_release(mut self) -> Self {
        self.should_release = false;
        self
    }
}

#[async_trait]
impl JobMiddleware for RateLimited {
    async fn handle(&self, job: &dyn ShouldQueue, next: Next<'_>) -> Result<()> {
        let Some(limits) = JobRateLimiters::instance().limits(&self.limiter_name, job) else {
            return next.run(job).await;
        };

        let limits: Vec<Limit> = limits
            .0
            .into_iter()
            .filter(|limit| !limit.is_unlimited())
            .collect();
        if limits.is_empty() {
            return next.run(job).await;
        }

        let limiter = illuminate_cache::facades::RateLimiter::instance()?;
        let keys: Vec<String> = limits
            .iter()
            .map(|limit| md5(&format!("{}{}", self.limiter_name, limit.key)))
            .collect();

        for (limit, key) in limits.iter().zip(&keys) {
            if limiter.too_many_attempts(key, limit.max_attempts).await? {
                if !self.should_release {
                    return Ok(());
                }
                let delay = match self.release_after {
                    Some(seconds) => seconds,
                    None => (limiter.available_in(key).await? + 3) as u64,
                };
                return job.release(delay).await;
            }
        }

        for (limit, key) in limits.iter().zip(&keys) {
            limiter.hit(key, limit.decay_seconds).await?;
        }

        next.run(job).await
    }
}
