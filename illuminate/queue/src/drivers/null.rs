//! The `null` driver: jobs are discarded.

use std::time::Duration;

use async_trait::async_trait;

use illuminate_support::Result;

use crate::contracts::Queue;
use crate::envelope::Envelope;
use crate::queued_job::QueuedJob;

/// Discards every job pushed onto it.
#[derive(Debug, Clone)]
pub struct NullQueue {
    name: String,
}

impl NullQueue {
    /// Create a null queue connection with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
impl Queue for NullQueue {
    fn connection_name(&self) -> &str {
        &self.name
    }

    async fn size(&self, _queue: Option<&str>) -> Result<u64> {
        Ok(0)
    }

    async fn push(&self, _job: &Envelope, _queue: Option<&str>) -> Result<Option<String>> {
        Ok(None)
    }

    async fn later(
        &self,
        _delay: Duration,
        _job: &Envelope,
        _queue: Option<&str>,
    ) -> Result<Option<String>> {
        Ok(None)
    }

    async fn push_raw(
        &self,
        _payload: String,
        _queue: Option<&str>,
        _delay: Option<Duration>,
    ) -> Result<Option<String>> {
        Ok(None)
    }

    async fn pop(&self, _queue: Option<&str>) -> Result<Option<QueuedJob>> {
        Ok(None)
    }

    async fn clear(&self, _queue: Option<&str>) -> Result<u64> {
        Ok(0)
    }
}
