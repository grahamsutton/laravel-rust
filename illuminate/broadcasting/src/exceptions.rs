//! Broadcasting exceptions.

use std::fmt;

/// Thrown when an event couldn't be broadcast (Laravel's `BroadcastException`).
///
/// ```
/// use illuminate_broadcasting::BroadcastException;
///
/// let error = BroadcastException::new("Pusher error: Unknown app.");
/// assert_eq!(error.to_string(), "Pusher error: Unknown app.");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BroadcastException {
    message: String,
}

impl BroadcastException {
    /// Create a new broadcast exception.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The exception's message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for BroadcastException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BroadcastException {}

/// Report an error to the application's exception handler (or stderr when
/// none is bound) — the "rescue" in `ShouldRescue`.
pub(crate) fn report(error: &illuminate_support::Error) {
    match illuminate_container::try_app::<dyn illuminate_http::ExceptionHandler>() {
        Some(handler) => {
            if handler.should_report(error) {
                handler.report(error);
            }
        }
        None => eprintln!("[broadcasting] {error:?}"),
    }
}
