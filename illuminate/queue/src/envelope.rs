//! Envelopes: a job plus the options it was dispatched with.
//!
//! In Laravel, the `Queueable` trait gives every job a `connection`,
//! `queue`, `delay`, `chained` jobs and friends. A Rust job is just your
//! struct, so those options travel next to it in an [`Envelope`]. On the
//! queue, an envelope becomes the payload's `data` object (a
//! [`SerializedJob`]).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use illuminate_container::try_app;
use illuminate_support::{Result, Value};

use crate::callbacks::CallbackRef;
use crate::delay::IntoDelay;
use crate::exceptions::InvalidPayloadException;
use crate::job::ShouldQueue;
use crate::registry::JobRegistry;

/// Encrypts the data of jobs that [should be
/// encrypted](ShouldQueue::should_be_encrypted).
///
/// Bind one into the container as `dyn JobEncrypter` (the foundation wires
/// it to the application's encrypter).
pub trait JobEncrypter: Send + Sync + 'static {
    /// Encrypt the given (JSON) string.
    fn encrypt(&self, value: &str) -> Result<String>;

    /// Decrypt the given payload back into a (JSON) string.
    fn decrypt(&self, payload: &str) -> Result<String>;
}

/// A job, serialized for the queue: the payload's `data` object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SerializedJob {
    /// The name the job type is registered under.
    pub command_name: String,
    /// The job's own data.
    pub command: Value,
    /// The batch the job belongs to.
    #[serde(default)]
    pub batch_id: Option<String>,
    /// Whether `command` is encrypted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub encrypted: bool,
    /// The connection the job was dispatched to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// The queue the job was dispatched to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<String>,
    /// The job's delay, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<f64>,
    /// Whether the job dispatches after database transactions commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_commit: Option<bool>,
    /// The jobs that run after this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chained: Vec<SerializedJob>,
    /// The connection the rest of the chain runs on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_connection: Option<String>,
    /// The queue the rest of the chain runs on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_queue: Option<String>,
    /// The callbacks to run when a job in the chain fails.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chain_catch_callbacks: Vec<CallbackRef>,
    /// The owner of the job's unique lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique_lock_owner: Option<String>,
}

/// A chained job: still in memory, or already serialized.
#[derive(Clone)]
pub(crate) enum Chained {
    Live(Box<Envelope>),
    Serialized(Box<SerializedJob>),
}

impl Chained {
    pub(crate) fn command_name(&self) -> &str {
        match self {
            Chained::Live(envelope) => envelope.command_name(),
            Chained::Serialized(job) => &job.command_name,
        }
    }

    pub(crate) fn serialize(&self) -> Result<SerializedJob> {
        match self {
            Chained::Live(envelope) => envelope.to_serialized(),
            Chained::Serialized(job) => Ok((**job).clone()),
        }
    }

    pub(crate) fn into_envelope(self) -> Result<Envelope> {
        match self {
            Chained::Live(envelope) => Ok(*envelope),
            Chained::Serialized(job) => Envelope::from_serialized(*job),
        }
    }
}

/// A job ready to be dispatched, with its queueing options.
#[derive(Clone)]
pub struct Envelope {
    job: Arc<dyn ShouldQueue>,
    pub(crate) connection: Option<String>,
    pub(crate) queue: Option<String>,
    pub(crate) delay: Option<Duration>,
    pub(crate) after_commit: Option<bool>,
    pub(crate) chained: Vec<Chained>,
    pub(crate) chain_connection: Option<String>,
    pub(crate) chain_queue: Option<String>,
    pub(crate) chain_catch_callbacks: Vec<CallbackRef>,
    pub(crate) batch_id: Option<String>,
    pub(crate) unique_lock_owner: Option<String>,
}

impl Envelope {
    /// Wrap a job, picking up its default connection, queue, delay and
    /// commit behavior.
    pub fn new(job: impl ShouldQueue) -> Self {
        Self::from_arc(Arc::new(job))
    }

    /// Wrap a boxed job.
    pub fn from_box(job: Box<dyn ShouldQueue>) -> Self {
        Self::from_arc(Arc::from(job))
    }

    /// Wrap a shared job.
    pub fn from_arc(job: Arc<dyn ShouldQueue>) -> Self {
        JobRegistry::add(job.registration());
        Self {
            connection: job.connection(),
            queue: job.queue(),
            delay: job.delay(),
            after_commit: job.after_commit(),
            job,
            chained: Vec::new(),
            chain_connection: None,
            chain_queue: None,
            chain_catch_callbacks: Vec::new(),
            batch_id: None,
            unique_lock_owner: None,
        }
    }

    /// The job.
    pub fn job(&self) -> &dyn ShouldQueue {
        &*self.job
    }

    /// The shared job.
    pub fn job_arc(&self) -> Arc<dyn ShouldQueue> {
        self.job.clone()
    }

    /// Determine if the job is of type `T`.
    pub fn is<T: ShouldQueue>(&self) -> bool {
        self.job.is::<T>()
    }

    /// Downcast the job to its concrete type.
    pub fn downcast_ref<T: ShouldQueue>(&self) -> Option<&T> {
        self.job.downcast_ref::<T>()
    }

    /// Get the job as a shared `T`.
    pub fn downcast_arc<T: ShouldQueue>(&self) -> Option<Arc<T>> {
        self.job.clone().into_any_arc().downcast::<T>().ok()
    }

    /// The name the job's type is registered under.
    pub fn command_name(&self) -> &'static str {
        self.job.command_name()
    }

    /// The job's display name.
    pub fn display_name(&self) -> String {
        self.job.display_name()
    }

    /// The connection the job will be dispatched to.
    pub fn connection_name(&self) -> Option<&str> {
        self.connection.as_deref()
    }

    /// The queue the job will be dispatched to.
    pub fn queue_name(&self) -> Option<&str> {
        self.queue.as_deref()
    }

    /// The job's delay.
    pub fn get_delay(&self) -> Option<Duration> {
        self.delay
    }

    /// Whether the job should be dispatched after open transactions commit.
    pub fn get_after_commit(&self) -> Option<bool> {
        self.after_commit
    }

    /// The batch the job belongs to.
    pub fn batch_id(&self) -> Option<&str> {
        self.batch_id.as_deref()
    }

    /// The registered names of the jobs chained after this one.
    pub fn chained_job_names(&self) -> Vec<String> {
        self.chained
            .iter()
            .map(|job| job.command_name().to_string())
            .collect()
    }

    /// The jobs chained after this one.
    pub fn chained_jobs(&self) -> Result<Vec<Envelope>> {
        self.chained
            .iter()
            .cloned()
            .map(Chained::into_envelope)
            .collect()
    }

    /// The connection the rest of the chain runs on.
    pub fn chain_connection(&self) -> Option<&str> {
        self.chain_connection.as_deref()
    }

    /// The queue the rest of the chain runs on.
    pub fn chain_queue(&self) -> Option<&str> {
        self.chain_queue.as_deref()
    }

    /// The owner token of the job's unique lock.
    pub fn unique_lock_owner(&self) -> Option<&str> {
        self.unique_lock_owner.as_deref()
    }

    // ------------------------------------------------------------------
    // Building
    // ------------------------------------------------------------------

    /// Set the desired connection for the job.
    pub fn on_connection(mut self, connection: impl Into<String>) -> Self {
        self.connection = Some(connection.into());
        self
    }

    /// Set the desired queue for the job.
    pub fn on_queue(mut self, queue: impl Into<String>) -> Self {
        self.queue = Some(queue.into());
        self
    }

    /// Set the desired connection for the job and every job in its chain.
    pub fn all_on_connection(mut self, connection: impl Into<String>) -> Self {
        let connection = connection.into();
        self.chain_connection = Some(connection.clone());
        self.connection = Some(connection);
        self
    }

    /// Set the desired queue for the job and every job in its chain.
    pub fn all_on_queue(mut self, queue: impl Into<String>) -> Self {
        let queue = queue.into();
        self.chain_queue = Some(queue.clone());
        self.queue = Some(queue);
        self
    }

    /// Set the desired delay for the job.
    pub fn delay(mut self, delay: impl IntoDelay) -> Self {
        self.delay = Some(delay.into_delay());
        self
    }

    /// Dispatch the job immediately, ignoring any default delay.
    pub fn without_delay(mut self) -> Self {
        self.delay = Some(Duration::ZERO);
        self
    }

    /// Dispatch the job after all open database transactions commit.
    pub fn after_commit(mut self) -> Self {
        self.after_commit = Some(true);
        self
    }

    /// Dispatch the job immediately, even inside a database transaction.
    pub fn before_commit(mut self) -> Self {
        self.after_commit = Some(false);
        self
    }

    /// Set the jobs that should run after this one succeeds.
    pub fn chain(mut self, jobs: impl IntoIterator<Item = Envelope>) -> Self {
        self.chained = jobs
            .into_iter()
            .map(|job| Chained::Live(Box::new(job)))
            .collect();
        self
    }

    /// Add callbacks to run when a job in the chain fails.
    pub(crate) fn with_chain_catch_callbacks(mut self, callbacks: Vec<CallbackRef>) -> Self {
        self.chain_catch_callbacks = callbacks;
        self
    }

    /// Set the batch the job belongs to.
    pub fn with_batch_id(mut self, batch_id: impl Into<String>) -> Self {
        self.batch_id = Some(batch_id.into());
        self
    }

    // ------------------------------------------------------------------
    // Serialization
    // ------------------------------------------------------------------

    /// Serialize the envelope into the payload's `data` object.
    pub fn to_serialized(&self) -> Result<SerializedJob> {
        let mut command = self.job.serialize_command()?;
        let mut encrypted = false;

        if self.job.should_be_encrypted()
            && let Some(encrypter) = try_app::<dyn JobEncrypter>()
        {
            command = Value::String(encrypter.encrypt(&command.to_string())?);
            encrypted = true;
        }

        Ok(SerializedJob {
            command_name: self.job.command_name().to_string(),
            command,
            batch_id: self.batch_id.clone(),
            encrypted,
            connection: self.connection.clone(),
            queue: self.queue.clone(),
            delay: self.delay.map(|delay| delay.as_secs_f64()),
            after_commit: self.after_commit,
            chained: self
                .chained
                .iter()
                .map(Chained::serialize)
                .collect::<Result<_>>()?,
            chain_connection: self.chain_connection.clone(),
            chain_queue: self.chain_queue.clone(),
            chain_catch_callbacks: self.chain_catch_callbacks.clone(),
            unique_lock_owner: self.unique_lock_owner.clone(),
        })
    }

    /// Rebuild an envelope from the payload's `data` object, finding the
    /// job's type in the [`JobRegistry`].
    pub fn from_serialized(data: SerializedJob) -> Result<Envelope> {
        let command = if data.encrypted {
            let Value::String(encrypted) = &data.command else {
                return Err(InvalidPayloadException::new("Unable to extract job payload.").into());
            };
            let Some(encrypter) = try_app::<dyn JobEncrypter>() else {
                return Err(InvalidPayloadException::new(
                    "Unable to decrypt the job payload: no job encrypter is bound.",
                )
                .into());
            };
            serde_json::from_str(&encrypter.decrypt(encrypted)?).map_err(|error| {
                InvalidPayloadException::new(format!("Unable to extract job payload: {error}"))
            })?
        } else {
            data.command
        };

        let job = JobRegistry::deserialize(&data.command_name, command)?;
        let mut envelope = Envelope::from_arc(job);

        if data.connection.is_some() {
            envelope.connection = data.connection;
        }
        if data.queue.is_some() {
            envelope.queue = data.queue;
        }
        if let Some(delay) = data.delay {
            envelope.delay = Some(delay.into_delay());
        }
        if data.after_commit.is_some() {
            envelope.after_commit = data.after_commit;
        }
        envelope.batch_id = data.batch_id;
        envelope.chained = data
            .chained
            .into_iter()
            .map(|job| Chained::Serialized(Box::new(job)))
            .collect();
        envelope.chain_connection = data.chain_connection;
        envelope.chain_queue = data.chain_queue;
        envelope.chain_catch_callbacks = data.chain_catch_callbacks;
        envelope.unique_lock_owner = data.unique_lock_owner;

        Ok(envelope)
    }
}

impl fmt::Debug for Envelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Envelope")
            .field("job", &self.command_name())
            .field("connection", &self.connection)
            .field("queue", &self.queue)
            .field("delay", &self.delay)
            .field("chained", &self.chained_job_names())
            .field("batch_id", &self.batch_id)
            .finish_non_exhaustive()
    }
}

impl<T: ShouldQueue> From<T> for Envelope {
    fn from(job: T) -> Self {
        Envelope::new(job)
    }
}

impl From<Box<dyn ShouldQueue>> for Envelope {
    fn from(job: Box<dyn ShouldQueue>) -> Self {
        Envelope::from_box(job)
    }
}

impl From<Arc<dyn ShouldQueue>> for Envelope {
    fn from(job: Arc<dyn ShouldQueue>) -> Self {
        Envelope::from_arc(job)
    }
}

impl Serialize for Envelope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_serialized()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Envelope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let data = SerializedJob::deserialize(deserializer)?;
        Envelope::from_serialized(data).map_err(serde::de::Error::custom)
    }
}
