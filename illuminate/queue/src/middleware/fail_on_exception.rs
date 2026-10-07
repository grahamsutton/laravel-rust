//! The `FailOnException` job middleware.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use illuminate_support::{Error, Result};

use super::{JobMiddleware, Next};
use crate::context::current_job;
use crate::exceptions::error_is;
use crate::job::ShouldQueue;

type ErrorPredicate = Arc<dyn Fn(&Error) -> bool + Send + Sync>;

/// Fail the job right away (no more retries) when it throws certain
/// exceptions, while still retrying the others.
///
/// ```
/// use illuminate_queue::middleware::FailOnException;
/// use illuminate_support::error::InvalidArgumentException;
///
/// let middleware = FailOnException::for_error::<InvalidArgumentException>();
/// let custom = FailOnException::when(|error| error.to_string().contains("revoked"));
/// # let _ = (middleware, custom);
/// ```
#[derive(Clone)]
pub struct FailOnException {
    callback: ErrorPredicate,
}

impl FailOnException {
    /// Fail the job when the exception passes the given test.
    pub fn when(callback: impl Fn(&Error) -> bool + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }

    /// Fail the job when it throws an exception of type `E`.
    pub fn for_error<E>() -> Self
    where
        E: fmt::Display + fmt::Debug + Send + Sync + 'static,
    {
        Self::when(error_is::<E>)
    }
}

impl fmt::Debug for FailOnException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FailOnException").finish_non_exhaustive()
    }
}

#[async_trait]
impl JobMiddleware for FailOnException {
    async fn handle(&self, job: &dyn ShouldQueue, next: Next<'_>) -> Result<()> {
        match next.run(job).await {
            Ok(()) => Ok(()),
            Err(error) => {
                if (self.callback)(&error)
                    && let Some(queued) = current_job()
                {
                    let error = std::sync::Arc::new(error);
                    queued.fail_with(error.clone()).await?;
                    return Err(crate::exceptions::unshare(error));
                }
                Err(error)
            }
        }
    }
}
