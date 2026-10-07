//! The bus dispatcher: sends jobs to the right connection and queue.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use indexmap::IndexMap;

use illuminate_container::{Container, try_app};
use illuminate_support::Result;

use super::batch::{Batch, PendingBatch};
use crate::contracts::{Queue, TransactionManager};
use crate::deferred::DeferredCallbacks;
use crate::envelope::Envelope;
use crate::manager::queue_manager;

/// The bus dispatcher contract (Laravel's `QueueingDispatcher`).
///
/// The [`Bus`](crate::Bus) facade, the [`Dispatchable`](crate::Dispatchable)
/// methods and the [`dispatch`](crate::dispatch) helper all go through the
/// `dyn QueueingDispatcher` bound in the container — which is how
/// [`Bus::fake`](crate::Bus::fake) intercepts them.
#[async_trait]
pub trait QueueingDispatcher: Send + Sync + 'static {
    /// Dispatch a job to its queue.
    async fn dispatch(&self, job: Envelope) -> Result<()>;

    /// Dispatch a job immediately, in the current process (on the `sync`
    /// connection).
    async fn dispatch_sync(&self, job: Envelope) -> Result<()>;

    /// Push a job onto its queue, returning the driver's job id.
    async fn dispatch_to_queue(&self, job: Envelope) -> Result<Option<String>>;

    /// Dispatch a job after the current response has been sent.
    async fn dispatch_after_response(&self, job: Envelope) -> Result<()>;

    /// Dispatch several jobs, grouped by connection and queue.
    async fn bulk(&self, jobs: Vec<Envelope>) -> Result<()>;

    /// Store and dispatch a batch of jobs.
    async fn dispatch_batch(&self, batch: PendingBatch) -> Result<Batch>;

    /// Find the batch with the given id.
    async fn find_batch(&self, batch_id: &str) -> Result<Option<Batch>>;
}

/// The default bus dispatcher.
pub struct Dispatcher {
    allows_dispatching_after_responses: AtomicBool,
}

impl Default for Dispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Dispatcher {
    /// Create a new dispatcher.
    pub fn new() -> Self {
        Self {
            allows_dispatching_after_responses: AtomicBool::new(true),
        }
    }

    /// Allow jobs to be dispatched after the response (the default).
    pub fn with_dispatching_after_responses(&self) -> &Self {
        self.allows_dispatching_after_responses
            .store(true, Ordering::SeqCst);
        self
    }

    /// Run "after response" jobs immediately instead.
    pub fn without_dispatching_after_responses(&self) -> &Self {
        self.allows_dispatching_after_responses
            .store(false, Ordering::SeqCst);
        self
    }

    async fn push_to_queue(
        queue: Arc<dyn Queue>,
        job: Envelope,
        queue_name: String,
    ) -> Result<Option<String>> {
        match job.get_delay().filter(|delay| !delay.is_zero()) {
            Some(delay) => queue.later(delay, &job, Some(&queue_name)).await,
            None => queue.push(&job, Some(&queue_name)).await,
        }
    }
}

#[async_trait]
impl QueueingDispatcher for Dispatcher {
    async fn dispatch(&self, job: Envelope) -> Result<()> {
        self.dispatch_to_queue(job).await.map(drop)
    }

    async fn dispatch_sync(&self, job: Envelope) -> Result<()> {
        self.dispatch_to_queue(job.on_connection("sync"))
            .await
            .map(drop)
    }

    async fn dispatch_to_queue(&self, job: Envelope) -> Result<Option<String>> {
        let manager = queue_manager();
        let routes = manager.routes();

        let connection_name = job
            .connection_name()
            .map(String::from)
            .or_else(|| routes.connection_for(job.command_name(), job.queue_name()));
        let queue = manager.connection(connection_name.as_deref())?;

        let queue_name = job
            .queue_name()
            .map(String::from)
            .or_else(|| routes.queue_for(job.command_name()))
            .unwrap_or_else(|| queue.default_queue().to_string());
        let queue_name = routes.forwarded_queue(&queue_name, queue.connection_name());

        let after_commit = job
            .get_after_commit()
            .unwrap_or_else(|| queue.dispatches_after_commit());

        if after_commit
            && let Some(transactions) = try_app::<dyn TransactionManager>()
            && transactions.in_transaction()
        {
            if job.job().unique_id().is_some() {
                let rollback_job = job.clone();
                transactions.add_callback_for_rollback(Box::new(move || {
                    Box::pin(async move {
                        crate::handler::release_unique_lock(
                            rollback_job.job(),
                            rollback_job.unique_lock_owner(),
                        )
                        .await;
                    })
                }));
            }
            transactions.add_callback(Box::new(move || {
                Box::pin(async move {
                    if let Err(error) = Self::push_to_queue(queue, job, queue_name).await {
                        crate::report(&error);
                    }
                })
            }));
            return Ok(None);
        }

        Self::push_to_queue(queue, job, queue_name).await
    }

    async fn dispatch_after_response(&self, job: Envelope) -> Result<()> {
        if !self
            .allows_dispatching_after_responses
            .load(Ordering::SeqCst)
        {
            return self.dispatch_sync(job).await;
        }
        DeferredCallbacks::current().defer(move || async move {
            if let Err(error) = dispatcher().dispatch_sync(job).await {
                crate::report(&error);
            }
        });
        Ok(())
    }

    async fn bulk(&self, jobs: Vec<Envelope>) -> Result<()> {
        let manager = queue_manager();
        let routes = manager.routes();
        let mut groups: IndexMap<(Option<String>, Option<String>), Vec<Envelope>> = IndexMap::new();

        for job in jobs {
            let connection = job
                .connection_name()
                .map(String::from)
                .or_else(|| routes.connection_for(job.command_name(), job.queue_name()));
            let queue = job
                .queue_name()
                .map(String::from)
                .or_else(|| routes.queue_for(job.command_name()));
            groups.entry((connection, queue)).or_default().push(job);
        }

        for ((connection, queue), jobs) in groups {
            manager
                .connection(connection.as_deref())?
                .bulk(&jobs, queue.as_deref())
                .await?;
        }
        Ok(())
    }

    async fn dispatch_batch(&self, batch: PendingBatch) -> Result<Batch> {
        batch.store_and_dispatch().await
    }

    async fn find_batch(&self, batch_id: &str) -> Result<Option<Batch>> {
        super::batch::find_stored_batch(batch_id).await
    }
}

/// The dispatcher bound in the container (the default one is registered
/// on first use).
pub fn dispatcher() -> Arc<dyn QueueingDispatcher> {
    if let Some(dispatcher) = try_app::<dyn QueueingDispatcher>() {
        return dispatcher;
    }
    let container = Container::get_instance();
    container.singleton_if::<dyn QueueingDispatcher>(|_| Arc::new(Dispatcher::new()));
    container.make::<dyn QueueingDispatcher>()
}
