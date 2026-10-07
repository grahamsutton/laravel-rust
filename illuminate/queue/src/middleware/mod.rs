//! Job middleware: logic wrapped around the execution of queued jobs.
//!
//! Like route middleware, job middleware receive the job being processed
//! and a [`Next`] to continue processing it. Attach them by returning them
//! from your job's [`middleware`](crate::ShouldQueue::middleware) method:
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_queue::middleware::{JobMiddleware, Next, WithoutOverlapping};
//! use illuminate_queue::{InteractsWithQueue, ShouldQueue, async_trait};
//! use illuminate_support::Result;
//! use serde::{Deserialize, Serialize};
//!
//! /// Only let the job run during business hours.
//! struct BusinessHours;
//!
//! #[async_trait]
//! impl JobMiddleware for BusinessHours {
//!     async fn handle(&self, job: &dyn ShouldQueue, next: Next<'_>) -> Result<()> {
//!         let hour = illuminate_support::now().hour();
//!         if !(9..17).contains(&hour) {
//!             return job.release(3600).await;
//!         }
//!         next.run(job).await
//!     }
//! }
//!
//! #[derive(Serialize, Deserialize)]
//! struct UpdateCreditScore {
//!     user_id: u64,
//! }
//!
//! #[async_trait]
//! impl ShouldQueue for UpdateCreditScore {
//!     async fn handle(&self) -> Result<()> {
//!         Ok(())
//!     }
//!
//!     fn middleware(&self) -> Vec<Arc<dyn JobMiddleware>> {
//!         vec![
//!             Arc::new(BusinessHours),
//!             Arc::new(WithoutOverlapping::new(self.user_id).release_after(60)),
//!         ]
//!     }
//! }
//! ```

mod fail_on_exception;
mod rate_limited;
mod skip;
mod throttles_exceptions;
mod without_overlapping;

use std::sync::Arc;

use async_trait::async_trait;

use illuminate_http::BoxFuture;
use illuminate_support::Result;

use crate::job::ShouldQueue;

pub use fail_on_exception::FailOnException;
pub use rate_limited::{JobLimits, JobRateLimiters, RateLimited, RateLimitsJobs};
pub use skip::{Release, Skip, SkipIfBatchCancelled};
pub use throttles_exceptions::ThrottlesExceptions;
pub use without_overlapping::WithoutOverlapping;

/// Middleware wrapped around a queued job.
#[async_trait]
pub trait JobMiddleware: Send + Sync + 'static {
    /// Process the queued job. Call `next.run(job)` to continue; release,
    /// delete or fail the job (or simply return) to stop.
    async fn handle(&self, job: &dyn ShouldQueue, next: Next<'_>) -> Result<()>;
}

/// The end of the middleware pipeline: actually running the job.
pub(crate) type Destination<'a> =
    &'a (dyn for<'j> Fn(&'j dyn ShouldQueue) -> BoxFuture<'j, Result<()>> + Send + Sync);

/// Help the compiler infer a destination closure's signature.
pub(crate) fn destination<F>(callback: F) -> F
where
    F: for<'j> Fn(&'j dyn ShouldQueue) -> BoxFuture<'j, Result<()>> + Send + Sync,
{
    callback
}

/// The rest of the job middleware pipeline.
pub struct Next<'a> {
    middleware: &'a [Arc<dyn JobMiddleware>],
    destination: Destination<'a>,
}

impl<'a> Next<'a> {
    pub(crate) fn new(
        middleware: &'a [Arc<dyn JobMiddleware>],
        destination: Destination<'a>,
    ) -> Self {
        Self {
            middleware,
            destination,
        }
    }

    /// Pass the job to the next middleware (and, finally, its `handle`).
    pub fn run<'j>(self, job: &'j dyn ShouldQueue) -> BoxFuture<'j, Result<()>>
    where
        'a: 'j,
    {
        Box::pin(async move {
            match self.middleware.split_first() {
                Some((first, rest)) => {
                    first
                        .handle(
                            job,
                            Next {
                                middleware: rest,
                                destination: self.destination,
                            },
                        )
                        .await
                }
                None => (self.destination)(job).await,
            }
        })
    }
}

/// An md5 hex digest.
pub(crate) fn md5(value: &str) -> String {
    use md5::{Digest, Md5};
    hex::encode(Md5::digest(value.as_bytes()))
}
