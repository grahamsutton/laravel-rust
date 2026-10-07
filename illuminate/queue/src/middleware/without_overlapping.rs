//! The `WithoutOverlapping` job middleware.

use std::fmt::Display;

use async_trait::async_trait;

use illuminate_cache::Cache;
use illuminate_support::Result;

use super::{JobMiddleware, Next};
use crate::context::InteractsWithQueue;
use crate::delay::{IntoDelay, ceil_seconds};
use crate::job::ShouldQueue;

/// Prevent jobs of the same type (and key) from running at the same time,
/// using an atomic cache lock. Overlapping jobs are released back onto the
/// queue.
///
/// ```
/// use illuminate_queue::middleware::WithoutOverlapping;
///
/// let user_id = 42;
/// let middleware = WithoutOverlapping::new(user_id)
///     .release_after(60)
///     .expire_after(180);
///
/// assert_eq!(middleware.key(), "42");
/// ```
#[derive(Debug, Clone)]
pub struct WithoutOverlapping {
    key: String,
    release_after: Option<u64>,
    expires_after: u64,
    prefix: String,
    share_key: bool,
}

impl WithoutOverlapping {
    /// Prevent overlaps of jobs sharing the given key.
    pub fn new(key: impl Display) -> Self {
        Self {
            key: key.to_string(),
            release_after: Some(0),
            expires_after: 0,
            prefix: "laravel-queue-overlap:".to_string(),
            share_key: false,
        }
    }

    /// The overlap key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Release overlapping jobs for the given number of seconds.
    pub fn release_after(mut self, seconds: u64) -> Self {
        self.release_after = Some(seconds);
        self
    }

    /// Delete overlapping jobs instead of releasing them.
    pub fn dont_release(mut self) -> Self {
        self.release_after = None;
        self
    }

    /// Release the lock after the given time, even if the job never
    /// finishes (a safety net for crashed workers).
    pub fn expire_after(mut self, expires_after: impl IntoDelay) -> Self {
        self.expires_after = ceil_seconds(expires_after.into_delay());
        self
    }

    /// Set the prefix of the lock key.
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Share the key across job types.
    pub fn shared(mut self) -> Self {
        self.share_key = true;
        self
    }

    /// The lock key for the given job.
    pub fn get_lock_key(&self, job: &dyn ShouldQueue) -> String {
        if self.share_key {
            format!("{}{}", self.prefix, self.key)
        } else {
            format!("{}{}:{}", self.prefix, job.command_name(), self.key)
        }
    }
}

#[async_trait]
impl JobMiddleware for WithoutOverlapping {
    async fn handle(&self, job: &dyn ShouldQueue, next: Next<'_>) -> Result<()> {
        let lock = Cache::default_store()?.lock(&self.get_lock_key(job), self.expires_after);

        if lock.get().await? {
            let result = next.run(job).await;
            lock.release().await?;
            return result;
        }

        match self.release_after {
            Some(delay) => job.release(delay).await,
            None => Ok(()),
        }
    }
}
