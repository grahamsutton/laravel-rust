//! Jobs: the `ShouldQueue` contract.

use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;

use illuminate_support::{Carbon, Error, Result, Value, class_basename};

use crate::exceptions::{InvalidPayloadException, ManuallyFailedException};
use crate::middleware::JobMiddleware;
use crate::registry::JobRegistration;

/// A job that should be pushed onto the queue and run in the background.
///
/// A job is a plain struct holding the data it needs (it must derive
/// `Serialize` and `Deserialize`, since it travels through the queue as
/// JSON) with a `handle` method that does the work:
///
/// ```
/// use illuminate_queue::{ShouldQueue, async_trait};
/// use illuminate_support::Result;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// pub struct ProcessPodcast {
///     pub podcast_id: u64,
/// }
///
/// #[async_trait]
/// impl ShouldQueue for ProcessPodcast {
///     async fn handle(&self) -> Result<()> {
///         // Process the uploaded podcast...
///         Ok(())
///     }
///
///     fn tries(&self) -> Option<u32> {
///         Some(3)
///     }
///
///     fn backoff(&self) -> Vec<u64> {
///         vec![1, 5, 10]
///     }
/// }
/// ```
///
/// Every other method is optional and mirrors one of Laravel's job
/// properties or attributes (`#[Tries]`, `#[Backoff]`, `#[Timeout]`,
/// `retryUntil`, `uniqueId`, ...). Inside `handle`, the
/// [`InteractsWithQueue`](crate::InteractsWithQueue) methods let the job
/// `release` itself, `fail`, check its `attempts`, or reach its `batch`.
#[async_trait]
pub trait ShouldQueue: QueueableCommand + Send + Sync + 'static {
    /// Execute the job.
    async fn handle(&self) -> Result<()>;

    /// Handle a job failure: notify a user, revert partial work, ...
    ///
    /// The error is the exception that caused the final failure, a
    /// [`MaxAttemptsExceededException`](crate::MaxAttemptsExceededException)
    /// or a [`TimeoutExceededException`](crate::TimeoutExceededException).
    /// Like Laravel, this runs on a freshly deserialized copy of the job.
    async fn failed(&self, _error: &Error) -> Result<()> {
        Ok(())
    }

    /// The middleware the job should pass through.
    fn middleware(&self) -> Vec<Arc<dyn JobMiddleware>> {
        Vec::new()
    }

    /// The number of times the job may be attempted (`None` uses the
    /// worker's `--tries`).
    fn tries(&self) -> Option<u32> {
        None
    }

    /// The maximum number of unhandled exceptions to allow before failing.
    fn max_exceptions(&self) -> Option<u32> {
        None
    }

    /// The number of seconds to wait before retrying the job after an
    /// exception. Give several values for "exponential" backoff: the
    /// `n`th retry waits the `n`th value (the last one repeats).
    fn backoff(&self) -> Vec<u64> {
        Vec::new()
    }

    /// The number of seconds the job can run before timing out.
    fn timeout(&self) -> Option<u64> {
        None
    }

    /// The time at which the job should no longer be attempted. Takes
    /// precedence over [`tries`](ShouldQueue::tries).
    fn retry_until(&self) -> Option<Carbon> {
        None
    }

    /// Whether the job should be marked as failed (instead of retried) when
    /// it times out.
    fn fail_on_timeout(&self) -> bool {
        false
    }

    /// The connection the job should be sent to by default.
    fn connection(&self) -> Option<String> {
        None
    }

    /// The queue the job should be sent to by default.
    fn queue(&self) -> Option<String> {
        None
    }

    /// The default delay before the job becomes available.
    fn delay(&self) -> Option<Duration> {
        None
    }

    /// Whether the job should be dispatched after all open database
    /// transactions have committed (Laravel's `ShouldQueueAfterCommit`).
    /// `None` defers to the connection's `after_commit` option.
    fn after_commit(&self) -> Option<bool> {
        None
    }

    /// The name shown by `queue:work`, Horizon-style dashboards, and the
    /// failed jobs table. Defaults to the type's name.
    fn display_name(&self) -> String {
        class_basename::<Self>()
    }

    /// Whether the job's data should be encrypted on the queue (Laravel's
    /// `ShouldBeEncrypted`). Requires a [`JobEncrypter`](crate::JobEncrypter)
    /// in the container.
    fn should_be_encrypted(&self) -> bool {
        false
    }

    /// Make the job unique (Laravel's `ShouldBeUnique`): while a job with
    /// the same unique id is on the queue, new dispatches are ignored.
    ///
    /// Return `Some(String::new())` to allow a single instance of the job
    /// class, or an id (`Some(self.product_id.to_string())`) to make it
    /// unique per model.
    fn unique_id(&self) -> Option<String> {
        None
    }

    /// The number of seconds after which the unique lock is released even
    /// if the job never finished (`0` keeps it until the job is done).
    fn unique_for(&self) -> u64 {
        0
    }

    /// Release the unique lock as soon as the job starts processing,
    /// rather than when it finishes (Laravel's
    /// `ShouldBeUniqueUntilProcessing`).
    fn unique_until_processing(&self) -> bool {
        false
    }

    /// The cache store that holds the unique lock (the default store when
    /// `None`).
    fn unique_via(&self) -> Option<String> {
        None
    }

    /// The stable name the job is registered under: workers use it to find
    /// the type to deserialize. Defaults to the fully qualified type name;
    /// override it to keep payloads valid when you move or rename the type.
    fn job_name() -> &'static str
    where
        Self: Sized,
    {
        std::any::type_name::<Self>()
    }
}

/// The plumbing that lets any `Serialize + Deserialize` job travel through
/// the queue. Implemented automatically; you never write it by hand.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be queued",
    label = "queued jobs must derive `Serialize` and `Deserialize`",
    note = "add `#[derive(serde::Serialize, serde::Deserialize)]` to `{Self}`"
)]
pub trait QueueableCommand: Any + Send + Sync {
    /// The name the job is registered under.
    fn command_name(&self) -> &'static str;

    /// Serialize the job's data.
    fn serialize_command(&self) -> Result<Value>;

    /// The job as `Any`, for downcasting.
    fn as_any(&self) -> &dyn Any;

    /// The shared job as `Any`, for downcasting.
    fn into_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;

    /// The boxed job as `Any`, for downcasting.
    fn into_any_box(self: Box<Self>) -> Box<dyn Any + Send + Sync>;

    /// How to deserialize this job type again.
    fn registration(&self) -> JobRegistration;
}

impl<T> QueueableCommand for T
where
    T: ShouldQueue + Serialize + DeserializeOwned,
{
    fn command_name(&self) -> &'static str {
        T::job_name()
    }

    fn serialize_command(&self) -> Result<Value> {
        serde_json::to_value(self).map_err(|error| {
            InvalidPayloadException::new(format!(
                "Failed to serialize job of type [{}]: {error}",
                T::job_name()
            ))
            .into()
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    fn into_any_box(self: Box<Self>) -> Box<dyn Any + Send + Sync> {
        self
    }

    fn registration(&self) -> JobRegistration {
        JobRegistration::of::<T>()
    }
}

impl dyn ShouldQueue {
    /// Determine if the job is of type `T`.
    pub fn is<T: ShouldQueue>(&self) -> bool {
        self.as_any().is::<T>()
    }

    /// Downcast the job to its concrete type.
    pub fn downcast_ref<T: ShouldQueue>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }
}

/// The reasons a job may be failed by hand: an error, a message, or
/// nothing at all.
///
/// ```
/// use illuminate_queue::IntoFailure;
///
/// assert_eq!("Something went wrong.".into_failure().to_string(), "Something went wrong.");
/// assert_eq!(().into_failure().to_string(), "This job was manually marked as failed.");
/// ```
pub trait IntoFailure {
    /// The error the job fails with.
    fn into_failure(self) -> Error;
}

impl IntoFailure for Error {
    fn into_failure(self) -> Error {
        self
    }
}

impl IntoFailure for &str {
    fn into_failure(self) -> Error {
        illuminate_support::error::RuntimeException::new(self).into()
    }
}

impl IntoFailure for String {
    fn into_failure(self) -> Error {
        illuminate_support::error::RuntimeException::new(self).into()
    }
}

impl IntoFailure for () {
    fn into_failure(self) -> Error {
        ManuallyFailedException.into()
    }
}

impl<T: IntoFailure> IntoFailure for Option<T> {
    fn into_failure(self) -> Error {
        match self {
            Some(error) => error.into_failure(),
            None => ManuallyFailedException.into(),
        }
    }
}
