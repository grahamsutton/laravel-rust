//! The exceptions thrown while finding, compiling and rendering views.

use std::fmt;
use std::path::PathBuf;

use illuminate_support::Error;

pub use illuminate_support::error::{InvalidArgumentException, RuntimeException};

macro_rules! simple_exception {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, thiserror::Error)]
        #[error("{message}")]
        pub struct $name {
            pub message: String,
        }

        impl $name {
            pub fn new(message: impl Into<String>) -> Self {
                Self { message: message.into() }
            }
        }
    };
}

simple_exception!(
    /// A PHP "error exception": undefined variables, reading properties of
    /// `null`, and friends.
    ErrorException
);

simple_exception!(
    /// An operation was given a value of the wrong type.
    TypeError
);

simple_exception!(
    /// Division (or modulo) by zero.
    DivisionByZeroError
);

simple_exception!(
    /// A method or function that doesn't exist was called.
    BadMethodCallException
);

/// A template could not be compiled (for example, a malformed `@foreach`
/// or a missing `@endif`).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ViewCompilationException {
    pub message: String,
    /// The line (1-based) the problem was found on.
    pub line: usize,
}

impl ViewCompilationException {
    pub fn new(message: impl Into<String>, line: usize) -> Self {
        Self {
            message: message.into(),
            line,
        }
    }
}

/// An exception thrown while rendering a view, annotated with the view and
/// the line of the template where it happened.
///
/// The original exception is available as the error's `source()` (and via
/// [`ViewException::previous`]), so exception handlers can still inspect it.
#[derive(Debug)]
pub struct ViewException {
    /// The original exception message.
    pub message: String,
    /// The name of the view being rendered.
    pub view: String,
    /// The path of the template, when it lives on disk.
    pub path: Option<PathBuf>,
    /// The line (1-based) in the template.
    pub line: usize,
    previous: Option<Error>,
}

impl ViewException {
    /// Create a new view exception.
    pub fn new(message: impl Into<String>, view: impl Into<String>, path: Option<PathBuf>, line: usize) -> Self {
        Self {
            message: message.into(),
            view: view.into(),
            path,
            line,
            previous: None,
        }
    }

    /// Wrap an error that happened while rendering a view.
    pub fn wrap(error: Error, view: impl Into<String>, path: Option<PathBuf>, line: usize) -> Self {
        let line = error
            .downcast_ref::<ViewCompilationException>()
            .map(|e| e.line)
            .unwrap_or(line);
        Self {
            message: error.to_string(),
            view: view.into(),
            path,
            line,
            previous: Some(error),
        }
    }

    /// The exception that caused this one.
    pub fn previous(&self) -> Option<&Error> {
        self.previous.as_ref()
    }

    /// Where the problem happened, for humans.
    pub fn location(&self) -> String {
        match &self.path {
            Some(path) => path.display().to_string(),
            None => self.view.clone(),
        }
    }
}

impl fmt::Display for ViewException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (View: {}, line {})", self.message, self.location(), self.line)
    }
}

impl std::error::Error for ViewException {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.previous.as_ref().map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Errors that must travel through views untouched (Laravel lets HTTP
/// exceptions escape views so `abort(404)` inside a template still 404s).
pub(crate) fn passes_through(error: &Error) -> bool {
    error.is::<ViewException>()
        || error.is::<illuminate_http::HttpException>()
        || error.is::<illuminate_http::HttpResponseException>()
}

/// Shorthand for an [`ErrorException`].
pub(crate) fn error(message: impl Into<String>) -> Error {
    ErrorException::new(message).into()
}
