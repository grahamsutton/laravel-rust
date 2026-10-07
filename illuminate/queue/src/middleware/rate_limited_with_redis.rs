//! The `RateLimitedWithRedis` job middleware.

use async_trait::async_trait;

use illuminate_cache::Limit;
use illuminate_redis::{Connection, DurationLimiter, Redis};
use illuminate_support::{Carbon, Result};

use super::rate_limited::JobRateLimiters;
use super::{JobMiddleware, Next, md5};
use crate::context::InteractsWithQueue;
use crate::job::ShouldQueue;

/// Limit how often a job may run with a named rate limiter, counted by
/// Redis' atomic [`DurationLimiter`] — fine-tuned for Redis and more
/// efficient than [`RateLimited`](super::RateLimited). Jobs over the limit
/// are released back onto the queue until the limit resets.
///
/// ```
/// use illuminate_queue::middleware::RateLimitedWithRedis;
///
/// let middleware = RateLimitedWithRedis::new("backups").connection("limiter");
/// let strict = RateLimitedWithRedis::new("backups").dont_release();
/// # let _ = (middleware, strict);
/// ```
#[derive(Debug, Clone)]
pub struct RateLimitedWithRedis {
    limiter_name: String,
    release_after: Option<u64>,
    should_release: bool,
    connection: Option<String>,
}

impl RateLimitedWithRedis {
    /// Rate limit the job with the named limiter.
    pub fn new(limiter_name: impl Into<String>) -> Self {
        Self {
            limiter_name: limiter_name.into(),
            release_after: None,
            should_release: true,
            connection: None,
        }
    }

    /// Count attempts on the given Redis connection (the default one
    /// otherwise).
    pub fn connection(mut self, name: impl Into<String>) -> Self {
        self.connection = Some(name.into());
        self
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

    fn redis(&self) -> Result<Connection> {
        Redis::connection(self.connection.as_deref())
    }
}

#[async_trait]
impl JobMiddleware for RateLimitedWithRedis {
    async fn handle(&self, job: &dyn ShouldQueue, next: Next<'_>) -> Result<()> {
        let Some(limits) = JobRateLimiters::instance().limits(&self.limiter_name, job) else {
            return next.run(job).await;
        };
        let limits: Vec<Limit> = limits
            .0
            .into_iter()
            .filter(|limit| !limit.is_unlimited())
            .collect();

        for limit in &limits {
            let key = md5(&format!("{}{}", self.limiter_name, limit.key));
            let mut limiter = DurationLimiter::new(
                self.redis()?,
                key,
                limit.max_attempts.max(0) as u64,
                limit.decay_seconds,
            );
            if !limiter.acquire().await? {
                if !self.should_release {
                    return Ok(());
                }
                let delay = match self.release_after {
                    Some(seconds) => seconds,
                    None => (limiter.decays_at() - Carbon::now().timestamp() + 3).max(0) as u64,
                };
                return job.release(delay).await;
            }
        }

        next.run(job).await
    }
}
