//! The `sync` driver: jobs run immediately, in the current process.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;

use illuminate_support::Result;

use crate::contracts::Queue;
use crate::envelope::Envelope;
use crate::events::{self, JobAttempted, JobExceptionOccurred, JobProcessed, JobProcessing};
use crate::exceptions::unshare;
use crate::payload::create_payload;
use crate::queued_job::QueuedJob;

/// Runs jobs immediately, in the current process.
///
/// Great for local development and tests. A job that throws is failed
/// right away (its `failed` hook runs) and the error is returned to the
/// code that dispatched it; it is not stored with the failed jobs.
#[derive(Debug, Clone)]
pub struct SyncQueue {
    name: String,
    after_commit: bool,
}

impl SyncQueue {
    /// Create a sync queue connection with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            after_commit: false,
        }
    }

    /// Dispatch jobs after open database transactions commit.
    pub fn with_after_commit(mut self, after_commit: bool) -> Self {
        self.after_commit = after_commit;
        self
    }

    /// Run a payload right now.
    pub async fn execute(&self, payload: String, queue: &str) -> Result<()> {
        let job = QueuedJob::sync(payload, &self.name, queue)?;
        events::dispatch(JobProcessing {
            connection_name: self.name.clone(),
            job: job.clone(),
        });

        let started = Instant::now();
        match job.fire().await {
            Ok(()) => {
                events::dispatch(JobProcessed {
                    connection_name: self.name.clone(),
                    job: job.clone(),
                    duration: Some(started.elapsed()),
                });
                events::dispatch(JobAttempted {
                    connection_name: self.name.clone(),
                    job,
                    exception: None,
                });
                Ok(())
            }
            Err(error) => {
                let error = Arc::new(error);
                events::dispatch(JobExceptionOccurred {
                    connection_name: self.name.clone(),
                    job: job.clone(),
                    exception: error.clone(),
                });
                if let Err(failure) = job.fail_with(error.clone()).await {
                    crate::report(&failure);
                }
                events::dispatch(JobAttempted {
                    connection_name: self.name.clone(),
                    job,
                    exception: Some(error.clone()),
                });
                Err(unshare(error))
            }
        }
    }
}

#[async_trait]
impl Queue for SyncQueue {
    fn connection_name(&self) -> &str {
        &self.name
    }

    fn dispatches_after_commit(&self) -> bool {
        self.after_commit
    }

    async fn size(&self, _queue: Option<&str>) -> Result<u64> {
        Ok(0)
    }

    async fn push(&self, job: &Envelope, queue: Option<&str>) -> Result<Option<String>> {
        let queue = queue.unwrap_or(self.default_queue());
        let payload = create_payload(job, &self.name, queue, None)?;
        self.execute(payload, queue).await?;
        Ok(None)
    }

    async fn later(
        &self,
        _delay: Duration,
        job: &Envelope,
        queue: Option<&str>,
    ) -> Result<Option<String>> {
        self.push(job, queue).await
    }

    async fn push_raw(
        &self,
        payload: String,
        queue: Option<&str>,
        _delay: Option<Duration>,
    ) -> Result<Option<String>> {
        self.execute(payload, queue.unwrap_or(self.default_queue()))
            .await?;
        Ok(None)
    }

    async fn pop(&self, _queue: Option<&str>) -> Result<Option<QueuedJob>> {
        Ok(None)
    }
}
