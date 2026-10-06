//! The exceptions thrown by the queue.

use std::fmt;
use std::sync::Arc;

use illuminate_support::Error;

/// Thrown (and handed to the job's `failed` hook) when a job has been
/// attempted more times than it is allowed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{name} has been attempted too many times.")]
pub struct MaxAttemptsExceededException {
    /// The display name of the job.
    pub name: String,
}

impl MaxAttemptsExceededException {
    /// Create the exception for the given job name.
    pub fn for_job(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

/// Thrown when a job runs longer than its timeout.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{name} has timed out.")]
pub struct TimeoutExceededException {
    /// The display name of the job.
    pub name: String,
}

impl TimeoutExceededException {
    /// Create the exception for the given job name.
    pub fn for_job(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

/// Handed to the `failed` hook when a job was failed by hand with no reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("This job was manually marked as failed.")]
pub struct ManuallyFailedException;

/// Thrown when a worker pops a job whose type it doesn't know.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "Unable to resolve the queued job [{name}]. Make sure the job is registered with `register_job!({name})` (or `Queue::register::<T>()`) in the worker process."
)]
pub struct UnknownJobException {
    /// The registered name the payload referred to.
    pub name: String,
}

/// Thrown when a job's payload can't be built or read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct InvalidPayloadException {
    /// What went wrong.
    pub message: String,
}

impl InvalidPayloadException {
    /// Create a new invalid payload exception.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Thrown when a queued closure (or a closure callback) isn't available in
/// the current process.
///
/// Rust closures can't be serialized, so queued closures and closure
/// callbacks live in the memory of the process that dispatched them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "The queued closure [{id}] is not available in this process. Queued closures only run in the process that dispatched them; dispatch a job instead."
)]
pub struct MissingClosureException {
    /// The closure's identifier.
    pub id: String,
}

/// Thrown when a queue driver doesn't support an operation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct UnsupportedOperationException {
    /// What isn't supported.
    pub message: String,
}

impl UnsupportedOperationException {
    /// Create a new exception.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// An error shared between several listeners (events, hooks, callbacks).
///
/// Errors aren't `Clone`, so the queue keeps the original error in an `Arc`
/// while it is handed around. When it can't get the original back, it
/// returns this wrapper, which displays (and debug-prints) exactly like the
/// original.
#[derive(Clone)]
pub struct SharedError(pub Arc<Error>);

impl SharedError {
    /// The original error.
    pub fn inner(&self) -> &Error {
        &self.0
    }
}

impl fmt::Display for SharedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&*self.0, f)
    }
}

impl fmt::Debug for SharedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

impl std::error::Error for SharedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

/// Take the original error back out of an `Arc`, or wrap it when it is
/// still shared.
pub(crate) fn unshare(error: Arc<Error>) -> Error {
    match Arc::try_unwrap(error) {
        Ok(error) => error,
        Err(shared) => Error::new(SharedError(shared)),
    }
}

/// Determine if the given error is (or wraps) an error of type `E`.
pub(crate) fn error_is<E>(error: &Error) -> bool
where
    E: std::fmt::Display + std::fmt::Debug + Send + Sync + 'static,
{
    if error.is::<E>() {
        return true;
    }
    error
        .downcast_ref::<SharedError>()
        .is_some_and(|shared| error_is::<E>(shared.inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_match_laravel() {
        assert_eq!(
            MaxAttemptsExceededException::for_job("ProcessPodcast").to_string(),
            "ProcessPodcast has been attempted too many times."
        );
        assert_eq!(
            TimeoutExceededException::for_job("ProcessPodcast").to_string(),
            "ProcessPodcast has timed out."
        );
    }

    #[test]
    fn shared_errors_unwrap_when_unique() {
        let error = Arc::new(Error::new(ManuallyFailedException));
        let error = unshare(error);
        assert!(error.is::<ManuallyFailedException>());

        let error = Arc::new(Error::new(ManuallyFailedException));
        let _other = error.clone();
        let error = unshare(error);
        assert!(!error.is::<ManuallyFailedException>());
        assert!(error_is::<ManuallyFailedException>(&error));
        assert_eq!(error.to_string(), "This job was manually marked as failed.");
    }
}
