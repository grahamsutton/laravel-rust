//! The `Skip`, `Release`, and `SkipIfBatchCancelled` job middleware.

use async_trait::async_trait;

use illuminate_support::Result;

use super::{JobMiddleware, Next};
use crate::context::InteractsWithQueue;
use crate::job::ShouldQueue;

/// Skip (delete) the job without running it.
///
/// ```
/// use illuminate_queue::middleware::Skip;
///
/// let order_cancelled = true;
/// let middleware = Skip::when(order_cancelled);
/// assert!(middleware.skips());
/// assert!(!Skip::unless(true).skips());
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct Skip {
    skip: bool,
}

impl Skip {
    /// Skip the job when the condition is true.
    pub fn when(condition: bool) -> Self {
        Self { skip: condition }
    }

    /// Skip the job unless the condition is true.
    pub fn unless(condition: bool) -> Self {
        Self { skip: !condition }
    }

    /// Whether the job will be skipped.
    pub fn skips(&self) -> bool {
        self.skip
    }
}

#[async_trait]
impl JobMiddleware for Skip {
    async fn handle(&self, job: &dyn ShouldQueue, next: Next<'_>) -> Result<()> {
        if self.skip {
            return Ok(());
        }
        next.run(job).await
    }
}

/// Release the job back onto the queue without running it.
///
/// ```
/// use illuminate_queue::middleware::Release;
///
/// let order_paid = false;
/// let middleware = Release::unless(order_paid, 60);
/// assert!(middleware.releases());
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct Release {
    release: bool,
    release_after: u64,
}

impl Release {
    /// Release the job (for `release_after` seconds) when the condition is
    /// true.
    pub fn when(condition: bool, release_after: u64) -> Self {
        Self {
            release: condition,
            release_after,
        }
    }

    /// Release the job (for `release_after` seconds) unless the condition
    /// is true.
    pub fn unless(condition: bool, release_after: u64) -> Self {
        Self::when(!condition, release_after)
    }

    /// Whether the job will be released.
    pub fn releases(&self) -> bool {
        self.release
    }
}

#[async_trait]
impl JobMiddleware for Release {
    async fn handle(&self, job: &dyn ShouldQueue, next: Next<'_>) -> Result<()> {
        if self.release {
            return job.release(self.release_after).await;
        }
        next.run(job).await
    }
}

/// Don't run batched jobs whose batch has been cancelled.
#[derive(Debug, Clone, Copy, Default)]
pub struct SkipIfBatchCancelled;

#[async_trait]
impl JobMiddleware for SkipIfBatchCancelled {
    async fn handle(&self, job: &dyn ShouldQueue, next: Next<'_>) -> Result<()> {
        if let Some(batch) = job.batch().await?
            && batch.cancelled()
        {
            return Ok(());
        }
        next.run(job).await
    }
}
