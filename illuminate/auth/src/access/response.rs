//! Authorization responses and the authorization exception.

use std::fmt;

use serde::{Serialize, Serializer};

use illuminate_http::HttpException;
use illuminate_support::{Value, json};

/// Anything that can be used as an (optional) authorization message:
/// `"You do not own this post."`, a `String`, or `None`.
pub trait IntoMessage {
    /// Convert into the optional message.
    fn into_message(self) -> Option<String>;
}

impl IntoMessage for &str {
    fn into_message(self) -> Option<String> {
        Some(self.to_string())
    }
}

impl IntoMessage for String {
    fn into_message(self) -> Option<String> {
        Some(self)
    }
}

impl IntoMessage for &String {
    fn into_message(self) -> Option<String> {
        Some(self.clone())
    }
}

impl IntoMessage for Option<String> {
    fn into_message(self) -> Option<String> {
        self
    }
}

/// The result of an authorization check, with an optional message, code,
/// and HTTP status — Laravel's `Illuminate\Auth\Access\Response`.
///
/// ```
/// use illuminate_auth::AuthResponse;
///
/// let response = AuthResponse::deny("You do not own this post.");
/// assert!(response.denied());
/// assert_eq!(response.message(), Some("You do not own this post."));
///
/// let hidden = AuthResponse::deny_as_not_found();
/// assert_eq!(hidden.status(), Some(404));
///
/// let error = hidden.authorize().unwrap_err();
/// assert_eq!(error.status(), Some(404));
///
/// assert!(AuthResponse::allow().authorize().is_ok());
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthResponse {
    allowed: bool,
    message: Option<String>,
    code: Option<String>,
    status: Option<u16>,
}

impl AuthResponse {
    /// Create a new response.
    pub fn new(allowed: bool, message: impl IntoMessage) -> Self {
        Self {
            allowed,
            message: message.into_message(),
            code: None,
            status: None,
        }
    }

    /// Create a new "allow" response.
    pub fn allow() -> Self {
        Self::new(true, None)
    }

    /// Create a new "deny" response, with an optional message.
    pub fn deny(message: impl IntoMessage) -> Self {
        Self::new(false, message)
    }

    /// Create a new "deny" response with the given HTTP status.
    pub fn deny_with_status(status: u16, message: impl IntoMessage) -> Self {
        Self::deny(message).with_status(status)
    }

    /// Create a new "deny" response with a `404` HTTP status — for hiding
    /// resources from users who may not see them.
    pub fn deny_as_not_found() -> Self {
        Self::deny_with_status(404, None)
    }

    /// Set the message.
    pub fn with_message(mut self, message: impl IntoMessage) -> Self {
        self.message = message.into_message();
        self
    }

    /// Set the response code (useful for API error codes).
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Set the HTTP status code the response should be rendered with when
    /// it's denied.
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    /// Render a denial as `404 Not Found`.
    pub fn as_not_found(self) -> Self {
        self.with_status(404)
    }

    /// Determine if the response was allowed.
    pub fn allowed(&self) -> bool {
        self.allowed
    }

    /// Determine if the response was denied.
    pub fn denied(&self) -> bool {
        !self.allowed
    }

    /// The response message.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// The response code.
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    /// The HTTP status code for a denial.
    pub fn status(&self) -> Option<u16> {
        self.status
    }

    /// Fail with an [`AuthorizationException`] if the response was denied.
    pub fn authorize(self) -> Result<Self, AuthorizationException> {
        if self.allowed {
            Ok(self)
        } else {
            Err(AuthorizationException::from_response(self))
        }
    }

    /// The response as an array (`allowed`, `message`, `code`).
    pub fn to_array(&self) -> Value {
        json!({
            "allowed": self.allowed,
            "message": self.message,
            "code": self.code,
        })
    }

    /// Whether this is a plain denial (no message, code or status), which
    /// the gate's default denial response may replace.
    pub(crate) fn is_plain_denial(&self) -> bool {
        !self.allowed && self.message.is_none() && self.code.is_none() && self.status.is_none()
    }
}

impl From<bool> for AuthResponse {
    fn from(allowed: bool) -> Self {
        Self::new(allowed, None)
    }
}

impl fmt::Display for AuthResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message.as_deref().unwrap_or_default())
    }
}

impl Serialize for AuthResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_array().serialize(serializer)
    }
}

/// Thrown when an action is not authorized — rendered as `403 Forbidden`
/// (or the status of the denial response, such as `404`).
///
/// ```
/// use illuminate_auth::{AuthResponse, AuthorizationException};
///
/// let exception = AuthorizationException::new();
/// assert_eq!(exception.to_string(), "This action is unauthorized.");
/// assert_eq!(exception.to_http_exception().status, 403);
///
/// let exception = AuthResponse::deny("You do not own this post.").authorize().unwrap_err();
/// assert_eq!(exception.to_string(), "You do not own this post.");
/// ```
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct AuthorizationException {
    message: String,
    code: Option<String>,
    status: Option<u16>,
    response: Option<AuthResponse>,
}

impl Default for AuthorizationException {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthorizationException {
    /// The message used when none is given.
    pub const DEFAULT_MESSAGE: &'static str = "This action is unauthorized.";

    /// Create the exception with Laravel's default message.
    pub fn new() -> Self {
        Self::with_message(None)
    }

    /// Create the exception with an optional custom message.
    pub fn with_message(message: impl IntoMessage) -> Self {
        Self {
            message: message
                .into_message()
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| Self::DEFAULT_MESSAGE.to_string()),
            code: None,
            status: None,
            response: None,
        }
    }

    /// Create the exception for a denied response.
    pub fn from_response(response: AuthResponse) -> Self {
        let mut exception = Self::with_message(response.message.clone());
        exception.code = response.code.clone();
        exception.status = response.status;
        exception.response = Some(response);
        exception
    }

    /// The exception message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The response code.
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    /// The response that caused the exception.
    pub fn response(&self) -> Option<&AuthResponse> {
        self.response.as_ref()
    }

    /// Set the HTTP status code the exception renders with.
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    /// Render the exception as `404 Not Found`.
    pub fn as_not_found(self) -> Self {
        self.with_status(404)
    }

    /// Determine if the exception has an explicit HTTP status.
    pub fn has_status(&self) -> bool {
        self.status.is_some()
    }

    /// The explicit HTTP status code, if any.
    pub fn status(&self) -> Option<u16> {
        self.status
    }

    /// The exception as a denied [`AuthResponse`].
    pub fn to_response(&self) -> AuthResponse {
        let mut response = AuthResponse::deny(self.message.clone());
        response.code = self.code.clone();
        response.status = self.status;
        response
    }

    /// The HTTP exception the exception handler should render: `403` with
    /// the exception's message, or the explicit status with the response's
    /// message (or the status's reason phrase).
    pub fn to_http_exception(&self) -> HttpException {
        match self.status {
            Some(status) => match self.response.as_ref().and_then(|r| r.message()) {
                Some(message) if !message.is_empty() => HttpException::with_message(status, message),
                _ => HttpException::new(status),
            },
            None => HttpException::with_message(403, self.message.clone()),
        }
    }
}
