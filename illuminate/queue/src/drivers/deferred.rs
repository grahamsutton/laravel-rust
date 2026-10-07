//! The `deferred` and `background` drivers: jobs run in the current
//! process, without slowing the response down.

use std::time::Duration;

use async_trait::async_trait;

use illuminate_support::Result;

use super::sync::SyncQueue;
use crate::contracts::Queue;
use crate::deferred::{DeferredCallbacks, spawn_with_container};
use crate::envelope::Envelope;
use crate::queued_job::QueuedJob;

/// Runs jobs synchronously, but only after the HTTP response has been
/// sent to the browser.
#[derive(Debug, Clone)]
pub struct DeferredQueue {
    sync: SyncQueue,
}

impl DeferredQueue {
    /// Create a deferred queue connection with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            sync: SyncQueue::new(name),
        }
    }
}

#[async_trait]
impl Queue for DeferredQueue {
    fn connection_name(&self) -> &str {
        self.sync.connection_name()
    }

    async fn size(&self, _queue: Option<&str>) -> Result<u64> {
        Ok(0)
    }

    async fn push(&self, job: &Envelope, queue: Option<&str>) -> Result<Option<String>> {
        let (sync, job, queue) = (self.sync.clone(), job.clone(), queue.map(String::from));
        DeferredCallbacks::current().defer(move || async move {
            if let Err(error) = sync.push(&job, queue.as_deref()).await {
                crate::report(&error);
            }
        });
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
        let (sync, queue) = (self.sync.clone(), queue.map(String::from));
        DeferredCallbacks::current().defer(move || async move {
            if let Err(error) = sync.push_raw(payload, queue.as_deref(), None).await {
                crate::report(&error);
            }
        });
        Ok(None)
    }

    async fn pop(&self, _queue: Option<&str>) -> Result<Option<QueuedJob>> {
        Ok(None)
    }
}

/// Runs jobs in the background: each job is spawned onto the Tokio
/// runtime, so the request carries on while the job runs.
#[derive(Debug, Clone)]
pub struct BackgroundQueue {
    sync: SyncQueue,
}

impl BackgroundQueue {
    /// Create a background queue connection with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            sync: SyncQueue::new(name),
        }
    }
}

#[async_trait]
impl Queue for BackgroundQueue {
    fn connection_name(&self) -> &str {
        self.sync.connection_name()
    }

    async fn size(&self, _queue: Option<&str>) -> Result<u64> {
        Ok(0)
    }

    async fn push(&self, job: &Envelope, queue: Option<&str>) -> Result<Option<String>> {
        let (sync, job, queue) = (self.sync.clone(), job.clone(), queue.map(String::from));
        spawn_with_container(async move {
            if let Err(error) = sync.push(&job, queue.as_deref()).await {
                crate::report(&error);
            }
        });
        Ok(None)
    }

    async fn later(
        &self,
        delay: Duration,
        job: &Envelope,
        queue: Option<&str>,
    ) -> Result<Option<String>> {
        let (sync, job, queue) = (self.sync.clone(), job.clone(), queue.map(String::from));
        spawn_with_container(async move {
            tokio::time::sleep(delay).await;
            if let Err(error) = sync.push(&job, queue.as_deref()).await {
                crate::report(&error);
            }
        });
        Ok(None)
    }

    async fn push_raw(
        &self,
        payload: String,
        queue: Option<&str>,
        delay: Option<Duration>,
    ) -> Result<Option<String>> {
        let (sync, queue) = (self.sync.clone(), queue.map(String::from));
        spawn_with_container(async move {
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            if let Err(error) = sync.push_raw(payload, queue.as_deref(), None).await {
                crate::report(&error);
            }
        });
        Ok(None)
    }

    async fn pop(&self, _queue: Option<&str>) -> Result<Option<QueuedJob>> {
        Ok(None)
    }
}
