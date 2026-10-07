//! The contracts queue drivers (and their integrations) implement.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use illuminate_http::BoxFuture;
use illuminate_support::{Result, Value};

use crate::envelope::Envelope;
use crate::events::{self, JobQueued, JobQueueing};
use crate::exceptions::UnsupportedOperationException;
use crate::payload::create_payload;
use crate::queued_job::QueuedJob;

/// A queue connection (Laravel's `Illuminate\Contracts\Queue\Queue`).
///
/// A driver needs to store payloads ([`push_raw`](Queue::push_raw)) and
/// hand them back ([`pop`](Queue::pop)); creating payloads, delays and
/// bulk pushes are taken care of by the provided methods.
///
/// Drivers are created by a [`QueueConnector`] registered with
/// [`QueueManager::extend`](crate::QueueManager::extend).
#[async_trait]
pub trait Queue: Send + Sync + 'static {
    /// The name of the connection.
    fn connection_name(&self) -> &str;

    /// The queue jobs are pushed onto when none is given.
    fn default_queue(&self) -> &str {
        "default"
    }

    /// Whether jobs should be dispatched after open database transactions
    /// commit (the connection's `after_commit` option).
    fn dispatches_after_commit(&self) -> bool {
        false
    }

    /// The number of jobs on the queue.
    async fn size(&self, queue: Option<&str>) -> Result<u64>;

    /// The number of jobs ready to be processed.
    async fn pending_size(&self, queue: Option<&str>) -> Result<u64> {
        self.size(queue).await
    }

    /// The number of delayed jobs.
    async fn delayed_size(&self, _queue: Option<&str>) -> Result<u64> {
        Ok(0)
    }

    /// The number of reserved (processing) jobs.
    async fn reserved_size(&self, _queue: Option<&str>) -> Result<u64> {
        Ok(0)
    }

    /// Push a new job onto the queue, returning the driver's job id.
    async fn push(&self, job: &Envelope, queue: Option<&str>) -> Result<Option<String>> {
        enqueue(self, job, queue, None).await
    }

    /// Push a new job onto the queue after a delay.
    async fn later(
        &self,
        delay: Duration,
        job: &Envelope,
        queue: Option<&str>,
    ) -> Result<Option<String>> {
        enqueue(self, job, queue, Some(delay)).await
    }

    /// Push several jobs onto the queue.
    async fn bulk(&self, jobs: &[Envelope], queue: Option<&str>) -> Result<()> {
        for job in jobs {
            let queue = queue.or(job.queue_name());
            match job.get_delay().filter(|delay| !delay.is_zero()) {
                Some(delay) => self.later(delay, job, queue).await?,
                None => self.push(job, queue).await?,
            };
        }
        Ok(())
    }

    /// Push a raw payload onto the queue.
    async fn push_raw(
        &self,
        payload: String,
        queue: Option<&str>,
        delay: Option<Duration>,
    ) -> Result<Option<String>>;

    /// Pop the next job off of the queue.
    async fn pop(&self, queue: Option<&str>) -> Result<Option<QueuedJob>>;

    /// Delete all of the jobs from the queue, returning how many were
    /// deleted.
    async fn clear(&self, _queue: Option<&str>) -> Result<u64> {
        Err(UnsupportedOperationException::new(format!(
            "The [{}] queue connection does not support clearing queues.",
            self.connection_name()
        ))
        .into())
    }
}

/// Create the job's payload and push it, firing the `JobQueueing` and
/// `JobQueued` events around it.
///
/// This is what [`Queue::push`] and [`Queue::later`] do by default; custom
/// drivers overriding them can call it too.
pub async fn enqueue<Q: Queue + ?Sized>(
    queue: &Q,
    job: &Envelope,
    queue_name: Option<&str>,
    delay: Option<Duration>,
) -> Result<Option<String>> {
    let queue_name = queue_name.unwrap_or(queue.default_queue()).to_string();
    let payload = create_payload(job, queue.connection_name(), &queue_name, delay)?;

    events::dispatch(JobQueueing {
        connection_name: queue.connection_name().to_string(),
        queue: queue_name.clone(),
        job: job.clone(),
        payload: payload.clone(),
        delay,
    });

    let id = queue
        .push_raw(payload.clone(), Some(&queue_name), delay)
        .await?;

    events::dispatch(JobQueued {
        connection_name: queue.connection_name().to_string(),
        queue: queue_name,
        id: id.clone(),
        job: job.clone(),
        payload,
        delay,
    });

    Ok(id)
}

/// Creates a queue connection from its configuration (Laravel's
/// `ConnectorInterface`).
///
/// Any `Fn(&Value, &str) -> Result<Arc<dyn Queue>>` closure is a connector:
/// it receives the connection's configuration and name.
pub trait QueueConnector: Send + Sync + 'static {
    /// Establish a queue connection.
    fn connect(&self, config: &Value, connection_name: &str) -> Result<Arc<dyn Queue>>;
}

impl<F> QueueConnector for F
where
    F: Fn(&Value, &str) -> Result<Arc<dyn Queue>> + Send + Sync + 'static,
{
    fn connect(&self, config: &Value, connection_name: &str) -> Result<Arc<dyn Queue>> {
        self(config, connection_name)
    }
}

/// A callback to run when a database transaction commits (or rolls back).
pub type TransactionCallback = Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>;

/// The database's transaction callbacks (Laravel's `db.transactions`).
///
/// When one is bound into the container as `dyn TransactionManager`, jobs
/// that should [dispatch after commit](crate::ShouldQueue::after_commit)
/// wait for the open transactions to commit, and are discarded when they
/// roll back.
pub trait TransactionManager: Send + Sync + 'static {
    /// Determine if a database transaction is currently open.
    fn in_transaction(&self) -> bool;

    /// Run the callback after the open transactions commit.
    fn add_callback(&self, callback: TransactionCallback);

    /// Run the callback if the open transactions roll back.
    fn add_callback_for_rollback(&self, callback: TransactionCallback);
}
