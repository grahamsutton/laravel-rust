//! Errors are Laravel's exceptions.
//!
//! Every fallible operation in the framework returns [`Result`]. Specific
//! exceptions (`HttpException`, `ValidationException`, `ModelNotFoundException`
//! and friends) are ordinary error types; the exception handler downcasts them
//! to decide how they should be reported and rendered.
//!
//! ```
//! use illuminate_support::{Result, throw_if};
//!
//! fn check(age: u32) -> Result<()> {
//!     throw_if(age < 18, "You must be an adult.")?;
//!     Ok(())
//! }
//!
//! assert!(check(12).is_err());
//! ```

/// The framework's error type: any error, with downcasting support.
pub type Error = anyhow::Error;

/// A `Result` defaulting to the framework's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

pub use anyhow::{Context, anyhow as error, bail, ensure};

/// A plain runtime exception carrying a message.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct RuntimeException {
    pub message: String,
}

impl RuntimeException {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Thrown when an argument handed to a function is not valid.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct InvalidArgumentException {
    pub message: String,
}

impl InvalidArgumentException {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Return an error carrying `message` when `condition` is true.
pub fn throw_if(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Err(RuntimeException::new(message).into())
    } else {
        Ok(())
    }
}

/// Return an error carrying `message` unless `condition` is true.
pub fn throw_unless(condition: bool, message: impl Into<String>) -> Result<()> {
    throw_if(!condition, message)
}
