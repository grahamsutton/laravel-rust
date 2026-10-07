//! The queued job handler (Laravel's `CallQueuedHandler`): turns a popped
//! payload back into a job, runs it through its middleware, and takes care
//! of everything that happens around it — unique locks, chains, batches,
//! and failure hooks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use illuminate_support::{Error, Result};

use crate::bus::batch::find_batch;
use crate::bus::unique::UniqueLock;
use crate::callbacks::{self, CallbackRef, ChainCatchCallback};
use crate::context::{self, JobContext};
use crate::envelope::{Chained, Envelope};
use crate::job::ShouldQueue;
use crate::middleware::{Next, destination};
use crate::queued_job::QueuedJob;

/// Process the given queued job.
pub(crate) async fn call(job: &QueuedJob) -> Result<()> {
    let envelope = match job.data().and_then(Envelope::from_serialized) {
        Ok(envelope) => envelope,
        Err(error) => {
            // The job can't be built: there is no point in retrying it.
            job.fail_with(Arc::new(error)).await?;
            return Ok(());
        }
    };

    let command = envelope.job_arc();
    let context = Arc::new(JobContext::new(job.clone(), &envelope));
    let until_processing = is_unique(&*command) && command.unique_until_processing();
    let owner = envelope.unique_lock_owner.clone();
    let lock_released = Arc::new(AtomicBool::new(false));

    let middleware = command.middleware();
    let run = {
        let job = job.clone();
        let owner = owner.clone();
        let lock_released = lock_released.clone();
        destination(move |command: &dyn ShouldQueue| {
            let job = job.clone();
            let owner = owner.clone();
            let lock_released = lock_released.clone();
            Box::pin(async move {
                if until_processing && unique_lock_should_be_released(&job, owner.as_deref()) {
                    release_unique_lock(command, owner.as_deref()).await;
                    lock_released.store(true, Ordering::SeqCst);
                }
                command.handle().await
            })
        })
    };

    let result = context::scope(context.clone(), Next::new(&middleware, &run).run(&*command)).await;

    if until_processing
        && !lock_released.load(Ordering::SeqCst)
        && !job.is_released()
        && unique_lock_should_be_released(job, owner.as_deref())
    {
        release_unique_lock(&*command, owner.as_deref()).await;
    }

    result?;

    if !job.is_released() && !until_processing {
        release_unique_lock(&*command, owner.as_deref()).await;
    }

    if !job.has_failed() && !job.is_released() {
        dispatch_next_job_in_chain(&context).await?;
        record_successful_batch_job(job, &context).await?;
    }

    if !job.is_deleted_or_released() {
        job.delete().await?;
    }

    Ok(())
}

/// Handle a job failure: release its unique lock, record the failure on
/// its batch, call the chain's `catch` callbacks, and finally the job's own
/// `failed` hook.
pub(crate) async fn failed(job: &QueuedJob, error: &Arc<Error>) -> Result<()> {
    let Ok(envelope) = job.data().and_then(Envelope::from_serialized) else {
        return Ok(());
    };
    let command = envelope.job();

    if !command.unique_until_processing() {
        release_unique_lock(command, envelope.unique_lock_owner()).await;
    }

    if let (Some(batch_id), Some(uuid)) = (envelope.batch_id(), job.uuid())
        && let Some(batch) = find_batch(batch_id).await?
    {
        batch.record_failed_job(uuid, error.clone()).await?;
    }

    invoke_chain_catch_callbacks(&envelope.chain_catch_callbacks, error).await;

    if let Err(failure) = command.failed(error).await {
        crate::report(&failure);
    }

    Ok(())
}

fn is_unique(command: &dyn ShouldQueue) -> bool {
    command.unique_id().is_some()
}

fn unique_lock_should_be_released(job: &QueuedJob, owner: Option<&str>) -> bool {
    job.attempts() <= 1 || owner.is_some_and(|owner| !owner.is_empty())
}

/// Release the job's unique lock, reporting (rather than throwing) errors.
pub(crate) async fn release_unique_lock(command: &dyn ShouldQueue, owner: Option<&str>) {
    if !is_unique(command) {
        return;
    }
    if let Err(error) = UniqueLock::new(None).release(command, owner).await {
        crate::report(&error);
    }
}

async fn dispatch_next_job_in_chain(context: &JobContext) -> Result<()> {
    let (next, rest, chain_connection, chain_queue, catch_callbacks) = {
        let mut chain = context.chain.lock().unwrap();
        if chain.consumed {
            return Ok(());
        }
        if chain.chained.is_empty() {
            // The chain is complete: its catch callbacks are no longer needed.
            callbacks::forget_all(&chain.chain_catch_callbacks);
            return Ok(());
        }
        let next = chain.chained.remove(0);
        let rest: Vec<Chained> = std::mem::take(&mut chain.chained);
        (
            next,
            rest,
            chain.chain_connection.clone(),
            chain.chain_queue.clone(),
            chain.chain_catch_callbacks.clone(),
        )
    };

    let mut next = next.into_envelope()?;
    next.chained = rest;
    if next.connection.is_none() {
        next.connection = chain_connection.clone();
    }
    if next.queue.is_none() {
        next.queue = chain_queue.clone();
    }
    next.chain_connection = chain_connection;
    next.chain_queue = chain_queue;
    next.chain_catch_callbacks = catch_callbacks;

    crate::bus::pending_dispatch::PendingDispatch::from_envelope(next).await
}

async fn record_successful_batch_job(job: &QueuedJob, context: &JobContext) -> Result<()> {
    let (Some(batch_id), Some(uuid)) = (context.batch_id.as_deref(), job.uuid()) else {
        return Ok(());
    };
    if let Some(batch) = find_batch(batch_id).await? {
        batch.record_successful_job(uuid).await?;
    }
    Ok(())
}

/// Invoke a chain's `catch` callbacks with the error that failed it.
pub(crate) async fn invoke_chain_catch_callbacks(callbacks: &[CallbackRef], error: &Arc<Error>) {
    for callback in callbacks {
        match callback {
            CallbackRef::Closure { id } => {
                if let Some(callback) = callbacks::get::<ChainCatchCallback>(id) {
                    if let Err(error) = callback(error.clone()).await {
                        crate::report(&error);
                    }
                    callbacks::forget(id);
                }
            }
            CallbackRef::Job { job, .. } => {
                let dispatched = match Envelope::from_serialized(job.clone()) {
                    Ok(envelope) => {
                        crate::bus::dispatcher::dispatcher()
                            .dispatch(envelope)
                            .await
                    }
                    Err(error) => Err(error),
                };
                if let Err(error) = dispatched {
                    crate::report(&error);
                }
            }
        }
    }
}
