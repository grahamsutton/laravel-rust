//! The job currently being processed, and the `InteractsWithQueue` methods
//! that let a job talk to it.

use std::future::Future;
use std::sync::{Arc, Mutex};

use illuminate_http::BoxFuture;
use illuminate_support::Result;

use crate::bus::batch::Batch;
use crate::callbacks::CallbackRef;
use crate::delay::IntoDelay;
use crate::envelope::{Chained, Envelope};
use crate::job::{IntoFailure, ShouldQueue};
use crate::queued_job::QueuedJob;

tokio::task_local! {
    static CURRENT_JOB: Arc<JobContext>;
}

/// The chain state of the running job (Laravel's `$this->chained`).
pub(crate) struct ChainState {
    pub(crate) chained: Vec<Chained>,
    pub(crate) chain_connection: Option<String>,
    pub(crate) chain_queue: Option<String>,
    pub(crate) chain_catch_callbacks: Vec<CallbackRef>,
    /// Set when the job took the rest of the chain over (chained batches).
    pub(crate) consumed: bool,
}

/// Everything a running job may need to interact with its queue.
pub(crate) struct JobContext {
    pub(crate) job: QueuedJob,
    pub(crate) batch_id: Option<String>,
    pub(crate) chain: Mutex<ChainState>,
}

impl JobContext {
    pub(crate) fn new(job: QueuedJob, envelope: &Envelope) -> Self {
        Self {
            job,
            batch_id: envelope.batch_id.clone(),
            chain: Mutex::new(ChainState {
                chained: envelope.chained.clone(),
                chain_connection: envelope.chain_connection.clone(),
                chain_queue: envelope.chain_queue.clone(),
                chain_catch_callbacks: envelope.chain_catch_callbacks.clone(),
                consumed: false,
            }),
        }
    }

    fn detached(job: QueuedJob) -> Self {
        let batch_id = job.batch_id().map(String::from);
        Self {
            job,
            batch_id,
            chain: Mutex::new(ChainState {
                chained: Vec::new(),
                chain_connection: None,
                chain_queue: None,
                chain_catch_callbacks: Vec::new(),
                consumed: false,
            }),
        }
    }

    /// The registered names of the jobs left in the chain.
    pub(crate) fn chained_job_names(&self) -> Vec<String> {
        self.chain
            .lock()
            .unwrap()
            .chained
            .iter()
            .map(|job| job.command_name().to_string())
            .collect()
    }
}

/// Run the future with `job` as the current job.
pub(crate) async fn scope<F: Future>(context: Arc<JobContext>, future: F) -> F::Output {
    CURRENT_JOB.scope(context, future).await
}

/// The job context of the running job.
pub(crate) fn current_context() -> Option<Arc<JobContext>> {
    CURRENT_JOB.try_with(Arc::clone).ok()
}

/// The queued job currently being processed by this task, if any.
pub fn current_job() -> Option<QueuedJob> {
    CURRENT_JOB.try_with(|context| context.job.clone()).ok()
}

/// Run a future with the given job as the current job.
///
/// This is how you test a job's interactions with the queue: pair it with
/// [`QueuedJob::fake`] and call the job's `handle` inside.
pub async fn with_job<F: Future>(job: QueuedJob, future: F) -> F::Output {
    CURRENT_JOB
        .scope(Arc::new(JobContext::detached(job)), future)
        .await
}

/// Lets a job interact with the queue while it runs (Laravel's
/// `InteractsWithQueue` trait).
///
/// Implemented for every job. Outside of a queue worker (when you call
/// `handle` yourself) these do nothing, just like in Laravel.
///
/// ```
/// use illuminate_queue::{InteractsWithQueue, ShouldQueue, async_trait};
/// use illuminate_support::Result;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct SyncInventory {
///     warehouse_online: bool,
/// }
///
/// #[async_trait]
/// impl ShouldQueue for SyncInventory {
///     async fn handle(&self) -> Result<()> {
///         if !self.warehouse_online {
///             // Try again in a minute...
///             return self.release(60).await;
///         }
///
///         if self.attempts() > 3 {
///             return self.fail("The warehouse keeps timing out.").await;
///         }
///
///         Ok(())
///     }
/// }
/// ```
pub trait InteractsWithQueue {
    /// The queued job being processed.
    fn job(&self) -> Option<QueuedJob> {
        current_job()
    }

    /// The number of times the job has been attempted.
    fn attempts(&self) -> u32 {
        current_job().map_or(1, |job| job.attempts())
    }

    /// Delete the job from the queue.
    fn delete(&self) -> BoxFuture<'static, Result<()>> {
        let job = current_job();
        Box::pin(async move {
            match job {
                Some(job) => job.delete().await,
                None => Ok(()),
            }
        })
    }

    /// Release the job back onto the queue after the given delay.
    fn release(&self, delay: impl IntoDelay) -> BoxFuture<'static, Result<()>> {
        let job = current_job();
        let delay = delay.into_delay();
        Box::pin(async move {
            match job {
                Some(job) => job.release(delay).await,
                None => Ok(()),
            }
        })
    }

    /// Fail the job: it won't be retried, and its `failed` hook runs.
    fn fail(&self, error: impl IntoFailure) -> BoxFuture<'static, Result<()>> {
        let job = current_job();
        let error = error.into_failure();
        Box::pin(async move {
            match job {
                Some(job) => job.fail(error).await,
                None => Ok(()),
            }
        })
    }

    /// The id of the batch the job belongs to.
    fn batch_id(&self) -> Option<String> {
        current_context().and_then(|context| context.batch_id.clone())
    }

    /// The batch the job belongs to (Laravel's `Batchable::batch`).
    fn batch(&self) -> BoxFuture<'static, Result<Option<Batch>>> {
        let batch_id = self.batch_id();
        Box::pin(async move {
            match batch_id {
                Some(id) => crate::bus::batch::find_batch(&id).await,
                None => Ok(None),
            }
        })
    }

    /// Run the given job right after this one, before the rest of the
    /// chain.
    fn prepend_to_chain(&self, job: impl Into<Envelope>) {
        if let Some(context) = current_context() {
            let job = Chained::Live(Box::new(job.into()));
            context.chain.lock().unwrap().chained.insert(0, job);
        }
    }

    /// Run the given job at the end of the chain.
    fn append_to_chain(&self, job: impl Into<Envelope>) {
        if let Some(context) = current_context() {
            let job = Chained::Live(Box::new(job.into()));
            context.chain.lock().unwrap().chained.push(job);
        }
    }

    /// The registered names of the jobs left in the chain (Laravel's
    /// `assertHasChain` helper).
    fn chained_jobs(&self) -> Vec<String> {
        current_context()
            .map(|context| context.chained_job_names())
            .unwrap_or_default()
    }
}

impl<T: ShouldQueue + ?Sized> InteractsWithQueue for T {}
