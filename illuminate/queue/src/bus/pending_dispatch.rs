//! Dispatching jobs: `ProcessPodcast { .. }.dispatch().await`.

use std::future::IntoFuture;

use illuminate_http::BoxFuture;
use illuminate_support::error::LogicException;
use illuminate_support::{Conditionable, Result};

use super::chain::PendingChain;
use super::debounce::DebounceLock;
use super::dispatcher::dispatcher;
use super::unique::UniqueLock;
use crate::delay::IntoDelay;
use crate::envelope::Envelope;
use crate::events::{self, UniqueJobSkipped};
use crate::job::ShouldQueue;

/// A job on its way to the queue.
///
/// Configure where and when it runs, then `.await` it to dispatch:
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
/// use illuminate_container::Container;
/// use illuminate_queue::{Dispatchable, Queue, ShouldQueue, async_trait};
/// use illuminate_support::Result;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct ProcessPodcast {
///     podcast_id: u64,
/// }
///
/// #[async_trait]
/// impl ShouldQueue for ProcessPodcast {
///     async fn handle(&self) -> Result<()> {
///         Ok(())
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// Queue::fake();
///
/// ProcessPodcast { podcast_id: 1 }
///     .dispatch()
///     .on_queue("podcasts")
///     .delay(Duration::from_secs(600))
///     .await?;
///
/// Queue::assert_pushed_on::<ProcessPodcast>("podcasts");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[must_use = "jobs are only dispatched once the `PendingDispatch` is `.await`ed"]
pub struct PendingDispatch {
    job: Envelope,
    after_response: bool,
    enabled: bool,
}

impl PendingDispatch {
    /// Create a pending dispatch for the given job.
    pub fn new(job: impl Into<Envelope>) -> Self {
        Self::from_envelope(job.into())
    }

    /// Create a pending dispatch for an envelope.
    pub fn from_envelope(job: Envelope) -> Self {
        Self {
            job,
            after_response: false,
            enabled: true,
        }
    }

    /// A pending dispatch that does nothing when awaited.
    pub(crate) fn skipped(job: Envelope) -> Self {
        Self {
            enabled: false,
            ..Self::from_envelope(job)
        }
    }

    /// The job being dispatched.
    pub fn job(&self) -> &Envelope {
        &self.job
    }

    /// Take the job back out.
    pub fn into_envelope(self) -> Envelope {
        self.job
    }

    /// Set the desired connection for the job.
    pub fn on_connection(mut self, connection: impl Into<String>) -> Self {
        self.job = self.job.on_connection(connection);
        self
    }

    /// Set the desired queue for the job.
    pub fn on_queue(mut self, queue: impl Into<String>) -> Self {
        self.job = self.job.on_queue(queue);
        self
    }

    /// Set the desired connection for the job and its chain.
    pub fn all_on_connection(mut self, connection: impl Into<String>) -> Self {
        self.job = self.job.all_on_connection(connection);
        self
    }

    /// Set the desired queue for the job and its chain.
    pub fn all_on_queue(mut self, queue: impl Into<String>) -> Self {
        self.job = self.job.all_on_queue(queue);
        self
    }

    /// Delay the job: seconds, a `Duration`, or the `Carbon` moment it
    /// should become available.
    pub fn delay(mut self, delay: impl IntoDelay) -> Self {
        self.job = self.job.delay(delay);
        self
    }

    /// Ignore the job's default delay.
    pub fn without_delay(mut self) -> Self {
        self.job = self.job.without_delay();
        self
    }

    /// Dispatch the job after all open database transactions commit.
    pub fn after_commit(mut self) -> Self {
        self.job = self.job.after_commit();
        self
    }

    /// Dispatch the job immediately, even inside a database transaction.
    pub fn before_commit(mut self) -> Self {
        self.job = self.job.before_commit();
        self
    }

    /// Set the jobs that should run after this one succeeds.
    pub fn chain(mut self, jobs: Vec<Box<dyn ShouldQueue>>) -> Self {
        self.job = self.job.chain(super::prepare_jobs(jobs));
        self
    }

    /// Dispatch the job after the response has been sent to the browser.
    pub fn after_response(mut self) -> Self {
        self.after_response = true;
        self
    }

    /// Dispatch the job.
    pub async fn dispatch(self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let mut job = self.job;
        let mut owner = None;

        let debounce = job.job().debounce_for();
        if debounce.is_some() && job.job().unique_id().is_some() {
            return Err(LogicException::new("A debounced job cannot also implement ShouldBeUnique.").into());
        }

        if job.job().unique_id().is_some() {
            match UniqueLock::new(None).acquire(job.job()).await? {
                Some(lock_owner) => {
                    job.unique_lock_owner = Some(lock_owner.clone());
                    owner = Some(lock_owner);
                }
                None => {
                    events::dispatch(UniqueJobSkipped { job });
                    return Ok(());
                }
            }
        }

        if let Some(debounce) = debounce {
            let acquired = DebounceLock::new(None)
                .acquire(job.job(), Some(debounce.seconds), debounce.max_wait)
                .await?;
            job.debounce_owner = Some(acquired.owner);
            if job.delay.is_none() {
                let seconds = if acquired.max_wait_exceeded { 0 } else { debounce.seconds };
                job.delay = Some(std::time::Duration::from_secs(seconds));
            }
        }

        let dispatcher = dispatcher();
        let result = if self.after_response {
            dispatcher.dispatch_after_response(job.clone()).await
        } else {
            dispatcher.dispatch(job.clone()).await
        };

        if result.is_err()
            && let Some(owner) = owner
        {
            crate::handler::release_unique_lock(job.job(), Some(&owner)).await;
        }

        result
    }
}

impl Conditionable for PendingDispatch {}

impl IntoFuture for PendingDispatch {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<'static, Result<()>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.dispatch())
    }
}

impl std::fmt::Debug for PendingDispatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingDispatch")
            .field("job", &self.job)
            .field("after_response", &self.after_response)
            .field("enabled", &self.enabled)
            .finish()
    }
}

/// Dispatch methods for every job (Laravel's `Dispatchable` trait).
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_queue::{Bus, Dispatchable, ShouldQueue, async_trait};
/// use illuminate_support::Result;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct SendInvoice {
///     order_id: u64,
/// }
///
/// #[async_trait]
/// impl ShouldQueue for SendInvoice {
///     async fn handle(&self) -> Result<()> {
///         Ok(())
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// Bus::fake();
///
/// let account_active = true;
/// SendInvoice { order_id: 1 }.dispatch_if(account_active).await?;
/// SendInvoice { order_id: 2 }.dispatch_sync().await?;
///
/// Bus::assert_dispatched_with::<SendInvoice>(|job| job.order_id == 1);
/// Bus::assert_dispatched_sync::<SendInvoice>();
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub trait Dispatchable: ShouldQueue + Sized {
    /// Dispatch the job.
    fn dispatch(self) -> PendingDispatch {
        PendingDispatch::new(self)
    }

    /// Dispatch the job if the given condition is true.
    fn dispatch_if(self, condition: bool) -> PendingDispatch {
        if condition {
            PendingDispatch::new(self)
        } else {
            PendingDispatch::skipped(Envelope::new(self))
        }
    }

    /// Dispatch the job unless the given condition is true.
    fn dispatch_unless(self, condition: bool) -> PendingDispatch {
        self.dispatch_if(!condition)
    }

    /// Run the job immediately, in the current process.
    fn dispatch_sync(self) -> BoxFuture<'static, Result<()>> {
        let job = Envelope::new(self);
        Box::pin(async move { dispatcher().dispatch_sync(job).await })
    }

    /// Run the job after the response has been sent to the browser.
    fn dispatch_after_response(self) -> PendingDispatch {
        PendingDispatch::new(self).after_response()
    }

    /// Start a chain of jobs, beginning with this one.
    fn with_chain(self, chain: Vec<Box<dyn ShouldQueue>>) -> PendingChain {
        let mut jobs: Vec<Box<dyn ShouldQueue>> = vec![Box::new(self)];
        jobs.extend(chain);
        PendingChain::new(jobs)
    }
}

impl<T: ShouldQueue> Dispatchable for T {}

/// Dispatch a job to its appropriate handler.
///
/// ```
/// # use std::sync::Arc;
/// # use illuminate_container::Container;
/// use illuminate_queue::{Queue, ShouldQueue, async_trait, dispatch};
/// # use illuminate_support::Result;
/// # use serde::{Deserialize, Serialize};
/// # #[derive(Serialize, Deserialize)]
/// # struct ProcessPodcast;
/// # #[async_trait]
/// # impl ShouldQueue for ProcessPodcast {
/// #     async fn handle(&self) -> Result<()> { Ok(()) }
/// # }
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// Queue::fake();
///
/// dispatch(ProcessPodcast).on_queue("high").await?;
///
/// Queue::assert_pushed_on::<ProcessPodcast>("high");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub fn dispatch(job: impl Into<Envelope>) -> PendingDispatch {
    PendingDispatch::new(job)
}

/// Dispatch a job immediately, in the current process.
pub fn dispatch_sync(job: impl Into<Envelope>) -> BoxFuture<'static, Result<()>> {
    let job = job.into();
    Box::pin(async move { dispatcher().dispatch_sync(job).await })
}
