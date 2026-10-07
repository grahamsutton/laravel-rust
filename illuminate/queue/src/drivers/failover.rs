//! The `failover` driver: push to the first connection that works.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use illuminate_support::{Error, Result};

use crate::contracts::Queue;
use crate::envelope::Envelope;
use crate::events::{self, QueueFailedOver};
use crate::exceptions::UnsupportedOperationException;
use crate::manager::queue_manager;
use crate::queued_job::QueuedJob;

/// Pushes jobs onto the first of several connections that accepts them,
/// firing a [`QueueFailedOver`] event for every connection that fails.
///
/// Workers pop from (and sizes are read from) the first connection.
#[derive(Debug, Clone)]
pub struct FailoverQueue {
    name: String,
    connections: Vec<String>,
}

impl FailoverQueue {
    /// Create a failover connection trying the given connections in order.
    pub fn new(name: impl Into<String>, connections: Vec<String>) -> Self {
        let name = name.into();
        let connections = connections
            .into_iter()
            .filter(|connection| *connection != name)
            .collect();
        Self { name, connections }
    }

    /// The connections, in the order they are tried.
    pub fn connections(&self) -> &[String] {
        &self.connections
    }

    fn primary(&self) -> Result<Arc<dyn Queue>> {
        let Some(first) = self.connections.first() else {
            return Err(UnsupportedOperationException::new(format!(
                "The [{}] failover connection has no connections configured.",
                self.name
            ))
            .into());
        };
        queue_manager().connection(Some(first))
    }

    async fn attempt<F, Fut>(&self, operation: F) -> Result<Option<String>>
    where
        F: Fn(Arc<dyn Queue>) -> Fut,
        Fut: std::future::Future<Output = Result<Option<String>>>,
    {
        let manager = queue_manager();
        let mut last_error: Option<Error> = None;

        for connection in &self.connections {
            let result = match manager.connection(Some(connection)) {
                Ok(queue) => operation(queue).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(id) => return Ok(id),
                Err(error) => {
                    let error = Arc::new(error);
                    events::dispatch(QueueFailedOver {
                        connection_name: self.name.clone(),
                        failed_connection: connection.clone(),
                        exception: error.clone(),
                    });
                    last_error = Some(crate::exceptions::unshare(error));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            UnsupportedOperationException::new(format!(
                "The [{}] failover connection has no connections configured.",
                self.name
            ))
            .into()
        }))
    }
}

#[async_trait]
impl Queue for FailoverQueue {
    fn connection_name(&self) -> &str {
        &self.name
    }

    async fn size(&self, queue: Option<&str>) -> Result<u64> {
        self.primary()?.size(queue).await
    }

    async fn push(&self, job: &Envelope, queue: Option<&str>) -> Result<Option<String>> {
        self.attempt(|connection| async move { connection.push(job, queue).await })
            .await
    }

    async fn later(
        &self,
        delay: Duration,
        job: &Envelope,
        queue: Option<&str>,
    ) -> Result<Option<String>> {
        self.attempt(|connection| async move { connection.later(delay, job, queue).await })
            .await
    }

    async fn push_raw(
        &self,
        payload: String,
        queue: Option<&str>,
        delay: Option<Duration>,
    ) -> Result<Option<String>> {
        self.attempt(|connection| {
            let payload = payload.clone();
            async move { connection.push_raw(payload, queue, delay).await }
        })
        .await
    }

    async fn pop(&self, queue: Option<&str>) -> Result<Option<QueuedJob>> {
        self.primary()?.pop(queue).await
    }

    async fn clear(&self, queue: Option<&str>) -> Result<u64> {
        self.primary()?.clear(queue).await
    }
}
