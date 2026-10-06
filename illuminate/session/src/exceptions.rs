//! The session's exceptions.

/// Thrown by [`ValidateCsrfToken`](crate::ValidateCsrfToken) when a request's
/// CSRF token doesn't match the session's.
///
/// The exception handler renders it as a `419 Page Expired` response.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct TokenMismatchException {
    pub message: String,
}

impl TokenMismatchException {
    /// Create the exception with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The HTTP status code the exception should be rendered with (419).
    pub fn status_code(&self) -> u16 {
        419
    }
}

impl Default for TokenMismatchException {
    fn default() -> Self {
        Self::new("CSRF token mismatch.")
    }
}

/// Thrown when the session is used on a request that doesn't have one
/// (the `StartSession` middleware didn't run).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Session store not set on request.")]
pub struct SessionNotFoundException;
