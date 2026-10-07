//! Job chains: jobs that run one after another.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use illuminate_support::{Conditionable, Error, Result};

use super::dispatcher::dispatcher;
use crate::callbacks::{self, CallbackRef, ChainCatchCallback};
use crate::delay::IntoDelay;
use crate::envelope::{Envelope, SerializedJob};
use crate::job::ShouldQueue;

/// A callback that is resolved into a [`CallbackRef`] when dispatched.
#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum PendingCallback {
    Ref(CallbackRef),
    Dispatch {
        job: Box<Envelope>,
        unless_cancelled: bool,
    },
}

impl PendingCallback {
    pub(crate) fn resolve(&self) -> Result<CallbackRef> {
        Ok(match self {
            PendingCallback::Ref(reference) => reference.clone(),
            PendingCallback::Dispatch {
                job,
                unless_cancelled,
            } => CallbackRef::Job {
                job: job.to_serialized()?,
                unless_cancelled: *unless_cancelled,
            },
        })
    }
}

/// A chain of jobs waiting to be dispatched.
///
/// Each job runs once the previous one succeeds; when a job fails, the rest
/// of the chain is not run and the `catch` callbacks are called:
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_queue::{Bus, ShouldQueue, async_trait};
/// use illuminate_support::Result;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct ProcessPodcast;
/// #[derive(Serialize, Deserialize)]
/// struct OptimizePodcast;
/// #[derive(Serialize, Deserialize)]
/// struct ReleasePodcast;
///
/// #[async_trait]
/// impl ShouldQueue for ProcessPodcast {
///     async fn handle(&self) -> Result<()> { Ok(()) }
/// }
/// #[async_trait]
/// impl ShouldQueue for OptimizePodcast {
///     async fn handle(&self) -> Result<()> { Ok(()) }
/// }
/// #[async_trait]
/// impl ShouldQueue for ReleasePodcast {
///     async fn handle(&self) -> Result<()> { Ok(()) }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// Bus::fake();
///
/// Bus::chain(vec![
///     Box::new(ProcessPodcast),
///     Box::new(OptimizePodcast),
///     Box::new(ReleasePodcast),
/// ])
/// .catch(|error| async move {
///     eprintln!("A job within the chain has failed: {error}");
///     Ok(())
/// })
/// .dispatch()
/// .await?;
///
/// Bus::assert_chained::<(ProcessPodcast, OptimizePodcast, ReleasePodcast)>();
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Clone)]
pub struct PendingChain {
    pub(crate) jobs: Vec<Envelope>,
    connection: Option<String>,
    queue: Option<String>,
    delay: Option<Duration>,
    catch_callbacks: Vec<PendingCallback>,
}

impl PendingChain {
    /// Create a chain of the given jobs.
    pub fn new(jobs: Vec<Box<dyn ShouldQueue>>) -> Self {
        Self::from_envelopes(super::prepare_jobs(jobs))
    }

    /// Create a chain of the given envelopes.
    pub fn from_envelopes(jobs: Vec<Envelope>) -> Self {
        Self {
            jobs,
            connection: None,
            queue: None,
            delay: None,
            catch_callbacks: Vec::new(),
        }
    }

    /// The jobs in the chain.
    pub fn jobs(&self) -> &[Envelope] {
        &self.jobs
    }

    /// Set the connection the chain should run on.
    pub fn on_connection(mut self, connection: impl Into<String>) -> Self {
        self.connection = Some(connection.into());
        self
    }

    /// Set the queue the chain should run on.
    pub fn on_queue(mut self, queue: impl Into<String>) -> Self {
        self.queue = Some(queue.into());
        self
    }

    /// Delay the first job of the chain.
    pub fn delay(mut self, delay: impl IntoDelay) -> Self {
        self.delay = Some(delay.into_delay());
        self
    }

    /// Run the given job first.
    pub fn prepend(mut self, job: impl Into<Envelope>) -> Self {
        self.jobs.insert(0, job.into());
        self
    }

    /// Run the given job last.
    pub fn append(mut self, job: impl Into<Envelope>) -> Self {
        self.jobs.push(job.into());
        self
    }

    /// Call the given closure when a job within the chain fails.
    ///
    /// Closures live in the dispatching process; use
    /// [`catch_dispatch`](PendingChain::catch_dispatch) when the chain runs
    /// on a worker in another process.
    pub fn catch<F, Fut>(mut self, callback: F) -> Self
    where
        F: Fn(Arc<Error>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let callback: ChainCatchCallback = Arc::new(move |error| Box::pin(callback(error)));
        let id = callbacks::store(callback);
        self.catch_callbacks
            .push(PendingCallback::Ref(CallbackRef::Closure { id }));
        self
    }

    /// Dispatch the given job when a job within the chain fails.
    pub fn catch_dispatch(mut self, job: impl Into<Envelope>) -> Self {
        self.catch_callbacks.push(PendingCallback::Dispatch {
            job: Box::new(job.into()),
            unless_cancelled: false,
        });
        self
    }

    /// The number of `catch` callbacks.
    pub fn catch_callback_count(&self) -> usize {
        self.catch_callbacks.len()
    }

    /// The first job, carrying the rest of the chain.
    pub(crate) fn into_first_job(self) -> Result<Option<Envelope>> {
        let mut jobs = self.jobs.into_iter();
        let Some(mut first) = jobs.next() else {
            return Ok(None);
        };

        if let Some(connection) = self.connection {
            first.chain_connection = Some(connection.clone());
            if first.connection.is_none() {
                first.connection = Some(connection);
            }
        }
        if let Some(queue) = self.queue {
            first.chain_queue = Some(queue.clone());
            if first.queue.is_none() {
                first.queue = Some(queue);
            }
        }
        if let Some(delay) = self.delay
            && first.delay.is_none()
        {
            first.delay = Some(delay);
        }

        let callbacks = self
            .catch_callbacks
            .iter()
            .map(PendingCallback::resolve)
            .collect::<Result<Vec<_>>>()?;

        Ok(Some(
            first.chain(jobs).with_chain_catch_callbacks(callbacks),
        ))
    }

    /// Dispatch the chain.
    pub async fn dispatch(self) -> Result<()> {
        match self.into_first_job()? {
            Some(first) => dispatcher().dispatch(first).await,
            None => Ok(()),
        }
    }

    /// Dispatch the chain if the given condition is true.
    pub async fn dispatch_if(self, condition: bool) -> Result<()> {
        if condition {
            self.dispatch().await
        } else {
            Ok(())
        }
    }

    /// Dispatch the chain unless the given condition is true.
    pub async fn dispatch_unless(self, condition: bool) -> Result<()> {
        self.dispatch_if(!condition).await
    }
}

impl Conditionable for PendingChain {}

/// The serialized form of a chain (used when a chain is queued as a job).
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SerializedChain {
    jobs: Vec<SerializedJob>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    connection: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    queue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delay: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    catch_callbacks: Vec<CallbackRef>,
}

impl Serialize for PendingChain {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let serialized = SerializedChain {
            jobs: self
                .jobs
                .iter()
                .map(Envelope::to_serialized)
                .collect::<Result<_>>()
                .map_err(serde::ser::Error::custom)?,
            connection: self.connection.clone(),
            queue: self.queue.clone(),
            delay: self.delay.map(|delay| delay.as_secs_f64()),
            catch_callbacks: self
                .catch_callbacks
                .iter()
                .map(PendingCallback::resolve)
                .collect::<Result<_>>()
                .map_err(serde::ser::Error::custom)?,
        };
        serialized.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PendingChain {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let serialized = SerializedChain::deserialize(deserializer)?;
        let jobs = serialized
            .jobs
            .into_iter()
            .map(Envelope::from_serialized)
            .collect::<Result<_>>()
            .map_err(serde::de::Error::custom)?;
        Ok(Self {
            jobs,
            connection: serialized.connection,
            queue: serialized.queue,
            delay: serialized.delay.map(IntoDelay::into_delay),
            catch_callbacks: serialized
                .catch_callbacks
                .into_iter()
                .map(PendingCallback::Ref)
                .collect(),
        })
    }
}

/// A chain may be queued as a job of its own: running it dispatches the
/// chain.
#[async_trait]
impl ShouldQueue for PendingChain {
    async fn handle(&self) -> Result<()> {
        self.clone().dispatch().await
    }

    fn display_name(&self) -> String {
        "Illuminate\\Foundation\\Bus\\PendingChain".to_string()
    }

    fn job_name() -> &'static str {
        "Illuminate\\Foundation\\Bus\\PendingChain"
    }
}

crate::register_job!(PendingChain);
