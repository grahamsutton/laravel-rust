//! Queued jobs: a job as the queue sees it.
//!
//! When a worker pops a job off a queue it gets a [`QueuedJob`] (Laravel's
//! `Illuminate\Contracts\Queue\Job`): the raw payload, how many times it has
//! been attempted, and the operations the driver supports — `delete`,
//! `release` and `fail`.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use illuminate_support::{Error, Result, Value, ValueExt};

use crate::delay::IntoDelay;
use crate::envelope::SerializedJob;
use crate::events::{self, JobFailed};
use crate::exceptions::{InvalidPayloadException, error_is};
use crate::failed::FailedJobProvider;
use crate::job::IntoFailure;

/// The driver side of a reserved job: how to delete it and how to put it
/// back on the queue.
///
/// Queue drivers implement this for whatever owns their storage, and hand
/// it to [`QueuedJob::new`] when a job is popped.
#[async_trait]
pub trait JobBackend: Send + Sync + 'static {
    /// Delete the job from the queue.
    async fn delete(&self, job: &QueuedJob) -> Result<()>;

    /// Release the job back onto the queue after the given delay, keeping
    /// its attempts.
    async fn release(&self, job: &QueuedJob, delay: Duration) -> Result<()>;
}

/// The backend of jobs that aren't stored anywhere (`sync` and fakes).
pub struct NullBackend;

#[async_trait]
impl JobBackend for NullBackend {
    async fn delete(&self, _job: &QueuedJob) -> Result<()> {
        Ok(())
    }

    async fn release(&self, _job: &QueuedJob, _delay: Duration) -> Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct JobState {
    deleted: bool,
    released: bool,
    failed: bool,
    release_delay: Option<Duration>,
    failure: Option<Arc<Error>>,
}

struct Inner {
    id: String,
    raw_body: String,
    payload: Value,
    attempts: u32,
    connection_name: String,
    queue: String,
    backend: Arc<dyn JobBackend>,
    state: Mutex<JobState>,
    failer: Mutex<Option<Arc<dyn FailedJobProvider>>>,
}

/// A job popped off a queue.
///
/// Handles are cheap to clone; every clone refers to the same job.
#[derive(Clone)]
pub struct QueuedJob {
    inner: Arc<Inner>,
}

impl QueuedJob {
    /// Create a queued job from a driver's record.
    ///
    /// `attempts` counts the current attempt: drivers increment it when the
    /// job is reserved.
    pub fn new(
        id: impl Into<String>,
        raw_body: impl Into<String>,
        attempts: u32,
        connection_name: impl Into<String>,
        queue: impl Into<String>,
        backend: Arc<dyn JobBackend>,
    ) -> Result<Self> {
        let raw_body = raw_body.into();
        let payload: Value = serde_json::from_str(&raw_body).map_err(|error| {
            InvalidPayloadException::new(format!("Invalid job payload: {error}"))
        })?;
        Ok(Self {
            inner: Arc::new(Inner {
                id: id.into(),
                raw_body,
                payload,
                attempts,
                connection_name: connection_name.into(),
                queue: queue.into(),
                backend,
                state: Mutex::new(JobState::default()),
                failer: Mutex::new(None),
            }),
        })
    }

    /// Create a job that isn't stored anywhere (the `sync` driver's jobs).
    pub fn sync(
        raw_body: impl Into<String>,
        connection_name: impl Into<String>,
        queue: impl Into<String>,
    ) -> Result<Self> {
        let raw_body = raw_body.into();
        let id = serde_json::from_str::<Value>(&raw_body)
            .ok()
            .and_then(|payload| {
                payload
                    .get("uuid")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .unwrap_or_default();
        Self::new(
            id,
            raw_body,
            1,
            connection_name,
            queue,
            Arc::new(NullBackend),
        )
    }

    /// A fake job for testing a job's interactions with the queue
    /// (Laravel's `withFakeQueueInteractions`). Run the job's `handle`
    /// inside [`with_job`](crate::with_job), then assert on the fake.
    ///
    /// ```
    /// use illuminate_queue::{InteractsWithQueue, QueuedJob, ShouldQueue, async_trait, with_job};
    /// use illuminate_support::Result;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize)]
    /// struct ProcessPodcast;
    ///
    /// #[async_trait]
    /// impl ShouldQueue for ProcessPodcast {
    ///     async fn handle(&self) -> Result<()> {
    ///         self.release(30).await
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let fake = QueuedJob::fake();
    /// with_job(fake.clone(), ProcessPodcast.handle()).await.unwrap();
    ///
    /// fake.assert_released(Some(30));
    /// fake.assert_not_failed();
    /// # });
    /// ```
    pub fn fake() -> Self {
        Self::new(
            "fake",
            r#"{"uuid":"fake","displayName":"FakeJob","job":"Illuminate\\Queue\\CallQueuedHandler@call","data":{}}"#,
            1,
            "sync",
            "default",
            Arc::new(NullBackend),
        )
        .expect("the fake payload is valid JSON")
    }

    // ------------------------------------------------------------------
    // Inspecting the job
    // ------------------------------------------------------------------

    /// The driver's identifier for the job.
    pub fn job_id(&self) -> &str {
        &self.inner.id
    }

    /// The job's UUID.
    pub fn uuid(&self) -> Option<&str> {
        self.inner.payload.get("uuid").and_then(Value::as_str)
    }

    /// The raw JSON payload.
    pub fn raw_body(&self) -> &str {
        &self.inner.raw_body
    }

    /// The decoded payload.
    pub fn payload(&self) -> &Value {
        &self.inner.payload
    }

    /// The payload's `data` object.
    pub fn data(&self) -> Result<SerializedJob> {
        let data = self
            .inner
            .payload
            .get("data")
            .cloned()
            .unwrap_or(Value::Null);
        serde_json::from_value(data).map_err(|error| {
            InvalidPayloadException::new(format!("Unable to extract job payload: {error}")).into()
        })
    }

    /// The number of times the job has been attempted (including this one).
    pub fn attempts(&self) -> u32 {
        self.inner.attempts
    }

    /// The number of times the job may be attempted.
    pub fn max_tries(&self) -> Option<u32> {
        self.u32_field("maxTries")
    }

    /// The number of unhandled exceptions to allow before failing.
    pub fn max_exceptions(&self) -> Option<u32> {
        self.u32_field("maxExceptions")
    }

    /// Whether the job should fail when it times out.
    pub fn should_fail_on_timeout(&self) -> bool {
        self.inner
            .payload
            .get("failOnTimeout")
            .is_some_and(ValueExt::truthy)
    }

    /// The job's backoff, as a comma separated list of seconds.
    pub fn backoff(&self) -> Option<String> {
        match self.inner.payload.get("backoff") {
            Some(Value::Null) | None => None,
            Some(Value::String(backoff)) => Some(backoff.clone()),
            Some(other) => Some(other.to_string_lossy()),
        }
    }

    /// The number of seconds the job may run.
    pub fn timeout(&self) -> Option<u64> {
        self.inner
            .payload
            .get("timeout")
            .and_then(Value::as_f64)
            .map(|timeout| timeout as u64)
    }

    /// The UNIX timestamp after which the job should no longer be attempted.
    pub fn retry_until(&self) -> Option<i64> {
        self.inner.payload.get("retryUntil").and_then(Value::as_i64)
    }

    /// The name of the queued handler (`Illuminate\Queue\CallQueuedHandler@call`).
    pub fn get_name(&self) -> &str {
        self.inner
            .payload
            .get("job")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The job's display name.
    pub fn resolve_name(&self) -> String {
        match self
            .inner
            .payload
            .get("displayName")
            .and_then(Value::as_str)
        {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => self.get_name().to_string(),
        }
    }

    /// The registered name of the job's type.
    pub fn command_name(&self) -> Option<&str> {
        self.inner
            .payload
            .get("data")
            .and_then(|data| data.get("commandName"))
            .and_then(Value::as_str)
    }

    /// The batch the job belongs to.
    pub fn batch_id(&self) -> Option<&str> {
        self.inner
            .payload
            .get("data")
            .and_then(|data| data.get("batchId"))
            .and_then(Value::as_str)
    }

    /// The name of the connection the job belongs to.
    pub fn connection_name(&self) -> &str {
        &self.inner.connection_name
    }

    /// The name of the queue the job belongs to.
    pub fn queue(&self) -> &str {
        &self.inner.queue
    }

    fn u32_field(&self, key: &str) -> Option<u32> {
        self.inner
            .payload
            .get(key)
            .and_then(Value::as_u64)
            .map(|value| value as u32)
    }

    // ------------------------------------------------------------------
    // State
    // ------------------------------------------------------------------

    /// Determine if the job has been deleted.
    pub fn is_deleted(&self) -> bool {
        self.inner.state.lock().unwrap().deleted
    }

    /// Determine if the job was released back onto the queue.
    pub fn is_released(&self) -> bool {
        self.inner.state.lock().unwrap().released
    }

    /// Determine if the job has been deleted or released.
    pub fn is_deleted_or_released(&self) -> bool {
        let state = self.inner.state.lock().unwrap();
        state.deleted || state.released
    }

    /// Determine if the job has been marked as failed.
    pub fn has_failed(&self) -> bool {
        self.inner.state.lock().unwrap().failed
    }

    /// The delay the job was released with.
    pub fn release_delay(&self) -> Option<Duration> {
        self.inner.state.lock().unwrap().release_delay
    }

    /// The error the job failed with.
    pub fn failure(&self) -> Option<Arc<Error>> {
        self.inner.state.lock().unwrap().failure.clone()
    }

    /// Mark the job as failed (without running the failure handling).
    pub fn mark_as_failed(&self) {
        self.inner.state.lock().unwrap().failed = true;
    }

    /// Log the job to the given failed job provider if it fails. Workers
    /// do this for every job they process.
    pub fn log_failures_to(&self, failer: Arc<dyn FailedJobProvider>) {
        *self.inner.failer.lock().unwrap() = Some(failer);
    }

    // ------------------------------------------------------------------
    // Operations
    // ------------------------------------------------------------------

    /// Process the job.
    pub async fn fire(&self) -> Result<()> {
        crate::handler::call(self).await
    }

    /// Delete the job from the queue.
    pub async fn delete(&self) -> Result<()> {
        self.inner.state.lock().unwrap().deleted = true;
        self.inner.backend.delete(self).await
    }

    /// Release the job back onto the queue after the given delay.
    pub async fn release(&self, delay: impl IntoDelay) -> Result<()> {
        let delay = delay.into_delay();
        {
            let mut state = self.inner.state.lock().unwrap();
            state.released = true;
            state.release_delay = Some(delay);
        }
        self.inner.backend.release(self, delay).await
    }

    /// Delete the job, call its `failed` hook, and fire the
    /// [`JobFailed`] event.
    pub async fn fail(&self, error: impl IntoFailure) -> Result<()> {
        self.fail_with(Arc::new(error.into_failure())).await
    }

    pub(crate) async fn fail_with(&self, error: Arc<Error>) -> Result<()> {
        {
            let mut state = self.inner.state.lock().unwrap();
            state.failed = true;
            if state.deleted {
                return Ok(());
            }
            state.failure = Some(error.clone());
        }

        let result = async {
            self.delete().await?;
            crate::handler::failed(self, &error).await
        }
        .await;

        let failer = self.inner.failer.lock().unwrap().clone();
        if let Some(failer) = failer {
            let logged = failer
                .log(
                    self.connection_name(),
                    self.queue(),
                    self.raw_body(),
                    &format!("{error:?}"),
                )
                .await;
            if let Err(error) = logged {
                crate::report(&error);
            }
        }

        events::dispatch(JobFailed {
            connection_name: self.connection_name().to_string(),
            job: self.clone(),
            exception: error,
        });

        result
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    /// Assert that the job was released, optionally with the given delay
    /// in seconds.
    pub fn assert_released(&self, delay: Option<u64>) {
        assert!(
            self.is_released(),
            "Job was expected to be released, but was not released."
        );
        if let Some(delay) = delay {
            let actual = self.release_delay().unwrap_or_default().as_secs();
            assert_eq!(
                actual, delay,
                "Expected job to be released with delay of [{delay}] seconds, but was released with delay of [{actual}] seconds."
            );
        }
    }

    /// Assert that the job was not released.
    pub fn assert_not_released(&self) {
        assert!(
            !self.is_released(),
            "Job was expected not to be released, but was released."
        );
    }

    /// Assert that the job was deleted.
    pub fn assert_deleted(&self) {
        assert!(
            self.is_deleted(),
            "Job was expected to be deleted, but was not deleted."
        );
    }

    /// Assert that the job was not deleted.
    pub fn assert_not_deleted(&self) {
        assert!(
            !self.is_deleted(),
            "Job was expected not to be deleted, but was deleted."
        );
    }

    /// Assert that the job was failed.
    pub fn assert_failed(&self) {
        assert!(
            self.has_failed(),
            "Job was expected to be manually failed, but was not failed."
        );
    }

    /// Assert that the job was failed with an error of type `E`.
    pub fn assert_failed_with<E>(&self)
    where
        E: std::fmt::Display + std::fmt::Debug + Send + Sync + 'static,
    {
        self.assert_failed();
        let failure = self.failure();
        assert!(
            failure.as_deref().is_some_and(error_is::<E>),
            "Expected job to be manually failed with [{}] but job failed with [{}].",
            std::any::type_name::<E>(),
            failure.map(|error| error.to_string()).unwrap_or_default()
        );
    }

    /// Assert that the job was not failed.
    pub fn assert_not_failed(&self) {
        assert!(
            !self.has_failed(),
            "Job was expected to not be manually failed, but was failed."
        );
    }
}

impl fmt::Debug for QueuedJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueuedJob")
            .field("id", &self.inner.id)
            .field("name", &self.resolve_name())
            .field("connection", &self.inner.connection_name)
            .field("queue", &self.inner.queue)
            .field("attempts", &self.inner.attempts)
            .finish_non_exhaustive()
    }
}
