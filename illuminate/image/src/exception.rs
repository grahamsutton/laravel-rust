//! The exception thrown when an image can't be read, processed, or encoded.

use std::error::Error as StdError;

use illuminate_support::Error;

/// Thrown when an image can't be read, processed, or encoded.
///
/// Errors raised while a driver processes an image are wrapped in an
/// `ImageException` ("Failed to process image: ...") that keeps the
/// original error as its [`source`](std::error::Error::source) — Laravel's
/// `getPrevious()`.
///
/// ```
/// use illuminate_image::ImageException;
///
/// let exception = ImageException::new("Invalid base64 image data.");
/// assert_eq!(exception.to_string(), "Invalid base64 image data.");
/// ```
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ImageException {
    /// The exception's message.
    pub message: String,
    #[source]
    previous: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl ImageException {
    /// Create a new image exception with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            previous: None,
        }
    }

    /// Attach the error that caused this exception.
    pub fn with_previous(
        mut self,
        previous: impl Into<Box<dyn StdError + Send + Sync + 'static>>,
    ) -> Self {
        self.previous = Some(previous.into());
        self
    }

    /// The error that caused this exception, if any.
    pub fn previous(&self) -> Option<&(dyn StdError + Send + Sync + 'static)> {
        self.previous.as_deref()
    }

    /// Wrap a processing failure: image exceptions pass through untouched,
    /// anything else becomes "Failed to process image: {message}".
    pub(crate) fn wrap(error: Error) -> Error {
        if error.is::<ImageException>() {
            return error;
        }
        let message = format!("Failed to process image: {error}");
        let previous: Box<dyn StdError + Send + Sync + 'static> = error.into();
        ImageException::new(message).with_previous(previous).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::error::RuntimeException;

    #[test]
    fn processing_failures_are_wrapped_and_keep_the_previous_error() {
        let wrapped = ImageException::wrap(RuntimeException::new("Boom.").into());
        let exception = wrapped.downcast_ref::<ImageException>().unwrap();

        assert_eq!(exception.to_string(), "Failed to process image: Boom.");
        assert_eq!(exception.previous().unwrap().to_string(), "Boom.");
        assert!(exception.source().is_some());
    }

    #[test]
    fn image_exceptions_are_not_wrapped_twice() {
        let wrapped =
            ImageException::wrap(ImageException::new("Invalid base64 image data.").into());

        assert_eq!(wrapped.to_string(), "Invalid base64 image data.");
        assert!(
            wrapped
                .downcast_ref::<ImageException>()
                .unwrap()
                .previous()
                .is_none()
        );
    }
}
