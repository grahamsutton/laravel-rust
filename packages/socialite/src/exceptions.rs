//! Socialite's exceptions.

use std::fmt;

/// The `state` returned by the OAuth provider doesn't match the one stored
/// in the session when the user was redirected — the callback didn't come
/// from the redirect your application started (or the session expired).
///
/// [`Provider::user`](crate::Provider::user) returns it; the exception
/// handler may downcast to it:
///
/// ```
/// use illuminate_support::Error;
/// use laravel_socialite::InvalidStateException;
///
/// let error: Error = InvalidStateException.into();
///
/// assert!(error.is::<InvalidStateException>());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InvalidStateException;

impl InvalidStateException {
    /// Create a new exception.
    pub fn new() -> Self {
        Self
    }
}

impl fmt::Display for InvalidStateException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Invalid state.")
    }
}

impl std::error::Error for InvalidStateException {}

/// A provider's `services.*` configuration is missing one of the required
/// keys: `client_id`, `client_secret` or `redirect`.
///
/// ```
/// use laravel_socialite::DriverMissingConfigurationException;
///
/// let error = DriverMissingConfigurationException::make("GithubProvider", &["client_secret", "redirect"]);
///
/// assert_eq!(
///     error.to_string(),
///     "Missing required configuration keys [client_secret, redirect] for [GithubProvider] OAuth provider.",
/// );
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverMissingConfigurationException {
    /// The provider being built (`GithubProvider`).
    pub provider: String,
    /// The missing configuration keys.
    pub keys: Vec<String>,
}

impl DriverMissingConfigurationException {
    /// Create a new exception for the provider and its missing keys.
    pub fn make(provider: impl Into<String>, keys: &[&str]) -> Self {
        Self {
            provider: provider.into(),
            keys: keys.iter().map(|key| key.to_string()).collect(),
        }
    }
}

impl fmt::Display for DriverMissingConfigurationException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Missing required configuration keys [{}] for [{}] OAuth provider.",
            self.keys.join(", "),
            self.provider
        )
    }
}

impl std::error::Error for DriverMissingConfigurationException {}
