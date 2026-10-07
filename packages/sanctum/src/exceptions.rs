//! Sanctum's exceptions.

use std::fmt;

use illuminate_http::HttpException;
use illuminate_support::Error;

/// Thrown by the `abilities` and `ability` middleware when the request's
/// token lacks the required abilities. It renders as a `403` with the
/// message "Invalid ability provided." — like Laravel's, it *is* an access
/// denied HTTP exception:
///
/// ```
/// use illuminate_http::HttpException;
/// use laravel_sanctum::MissingAbilityException;
///
/// let error = MissingAbilityException::new(["place-orders"]).into_error();
///
/// let exception = error.downcast_ref::<MissingAbilityException>().unwrap();
/// assert_eq!(exception.abilities(), ["place-orders"]);
///
/// let http = error.downcast_ref::<HttpException>().unwrap();
/// assert_eq!(http.status_code(), 403);
/// assert_eq!(http.message(), "Invalid ability provided.");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingAbilityException {
    abilities: Vec<String>,
    message: String,
}

impl MissingAbilityException {
    /// The default message.
    pub const MESSAGE: &'static str = "Invalid ability provided.";

    /// Create the exception for the missing abilities.
    pub fn new<I, S>(abilities: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::with_message(abilities, Self::MESSAGE)
    }

    /// Create the exception with a custom message.
    pub fn with_message<I, S>(abilities: I, message: impl Into<String>) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            abilities: abilities.into_iter().map(Into::into).collect(),
            message: message.into(),
        }
    }

    /// The abilities the token was missing.
    pub fn abilities(&self) -> &[String] {
        &self.abilities
    }

    /// The exception message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The HTTP status code the exception renders with (403).
    pub fn status_code(&self) -> u16 {
        403
    }

    /// The `403` HTTP exception this exception renders as.
    pub fn to_http_exception(&self) -> HttpException {
        HttpException::with_message(self.status_code(), self.message.clone())
    }

    /// Turn the exception into an error that downcasts to both
    /// `MissingAbilityException` and [`HttpException`], so any exception
    /// handler renders it as a `403`.
    pub fn into_error(self) -> Error {
        Error::new(self.to_http_exception()).context(self)
    }
}

impl fmt::Display for MissingAbilityException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MissingAbilityException {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exception_carries_the_missing_abilities() {
        let exception = MissingAbilityException::new(["check-status", "place-orders"]);
        assert_eq!(exception.abilities(), ["check-status", "place-orders"]);
        assert_eq!(exception.to_string(), "Invalid ability provided.");
        assert_eq!(exception.status_code(), 403);

        let custom = MissingAbilityException::with_message(["admin"], "Nope.");
        assert_eq!(custom.message(), "Nope.");
        assert_eq!(custom.to_http_exception().message(), "Nope.");
    }

    #[test]
    fn the_error_is_an_http_exception_too() {
        let error = MissingAbilityException::new(["admin"]).into_error();
        assert!(error.is::<MissingAbilityException>());
        assert!(error.is::<HttpException>());
        assert_eq!(error.to_string(), "Invalid ability provided.");
    }
}
