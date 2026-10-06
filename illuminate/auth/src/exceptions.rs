//! Authentication exceptions.

use illuminate_http::{Request, Response};
use illuminate_session::RequestSessionExt;
use illuminate_support::json;

/// Thrown when a request must be authenticated but isn't.
///
/// The exception handler renders it as a `401` JSON response for requests
/// expecting JSON, and as a redirect to the login page otherwise (see
/// [`AuthenticationException::render`]).
///
/// ```
/// use illuminate_auth::AuthenticationException;
///
/// let exception = AuthenticationException::new(vec!["web".into()], Some("/login".into()));
///
/// assert_eq!(exception.to_string(), "Unauthenticated.");
/// assert_eq!(exception.guards(), ["web"]);
/// ```
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct AuthenticationException {
    message: String,
    guards: Vec<String>,
    redirect_to: Option<String>,
}

impl AuthenticationException {
    /// Create the exception with Laravel's "Unauthenticated." message.
    pub fn new(guards: Vec<String>, redirect_to: Option<String>) -> Self {
        Self::with_message("Unauthenticated.", guards, redirect_to)
    }

    /// Create the exception with a custom message.
    pub fn with_message(
        message: impl Into<String>,
        guards: Vec<String>,
        redirect_to: Option<String>,
    ) -> Self {
        Self {
            message: message.into(),
            guards,
            redirect_to,
        }
    }

    /// The exception message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The guards that were checked.
    pub fn guards(&self) -> &[String] {
        &self.guards
    }

    /// The path the user should be redirected to: the one given when the
    /// exception was thrown, or the application's guest redirect (see
    /// [`AuthenticationException::redirect_using`]).
    pub fn redirect_to(&self, request: &Request) -> Option<String> {
        if let Some(to) = &self.redirect_to {
            return Some(to.clone());
        }
        crate::facade::manager().hooks().guest_redirect(request)
    }

    /// The redirect given when the exception was thrown, if any.
    pub fn redirect_path(&self) -> Option<&str> {
        self.redirect_to.as_deref()
    }

    /// Specify where unauthenticated users should be redirected (shared
    /// with the `auth` middleware and `Authenticate::redirect_using`).
    pub fn redirect_using(
        callback: impl Fn(&Request) -> Option<String> + Send + Sync + 'static,
    ) {
        crate::facade::manager().hooks().set_guest_redirect(callback);
    }

    /// Render the exception the way Laravel's exception handler does:
    /// `401 {"message": "Unauthenticated."}` for JSON requests, a redirect
    /// to the login page (remembering the intended URL) otherwise, or an
    /// empty `401` when there's nowhere to redirect to.
    pub fn render(&self, request: &Request) -> Response {
        if request.expects_json() {
            return Response::json(&json!({ "message": self.message })).with_status(401);
        }
        match self.redirect_to(request) {
            Some(to) => redirect_guest(request, &to),
            None => Response::no_content().with_status(401),
        }
    }
}

/// Redirect to `to`, remembering the current URL as the "intended" one (the
/// equivalent of `redirect()->guest(...)` for an explicit request).
pub(crate) fn redirect_guest(request: &Request, to: &str) -> Response {
    let intended = if request.is_method("GET") && !request.expects_json() {
        Some(request.full_url())
    } else {
        request
            .header("referer")
            .or_else(|| request.try_session().and_then(|session| session.previous_url()))
    };
    if let (Some(intended), Some(session)) = (intended, request.try_session()) {
        session.put("url.intended", intended);
    }
    Response::redirect(absolute_url(request, to))
}

/// Turn a path into a full URL for the request.
pub(crate) fn absolute_url(request: &Request, path: &str) -> String {
    let absolute = ["http://", "https://", "//"]
        .iter()
        .any(|scheme| path.starts_with(scheme));
    if absolute {
        return path.to_string();
    }
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        request.root()
    } else {
        format!("{}/{path}", request.root())
    }
}
