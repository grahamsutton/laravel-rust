//! The exceptions thrown by the router and the URL generator.

use illuminate_http::HttpException;
use illuminate_support::{Error, Str};

/// Thrown when a URL is generated for a route without all of its required
/// parameters.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct UrlGenerationException {
    pub message: String,
}

impl UrlGenerationException {
    /// Create the exception for a route that is missing parameters.
    ///
    /// ```
    /// use illuminate_routing::UrlGenerationException;
    ///
    /// let e = UrlGenerationException::for_missing_parameters(Some("profile"), "user/{id}", &["id".into()]);
    /// assert_eq!(
    ///     e.to_string(),
    ///     "Missing required parameter for [Route: profile] [URI: user/{id}] [Missing parameter: id]."
    /// );
    /// ```
    pub fn for_missing_parameters(name: Option<&str>, uri: &str, parameters: &[String]) -> Self {
        let label = Str::plural_count("parameter", parameters.len() as i64);
        let mut message = format!(
            "Missing required {label} for [Route: {}] [URI: {uri}]",
            name.unwrap_or_default()
        );
        if !parameters.is_empty() {
            message.push_str(&format!(" [Missing {label}: {}]", parameters.join(", ")));
        }
        message.push('.');
        Self { message }
    }
}

/// Thrown when a URL is generated for a route name that doesn't exist.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Route [{name}] not defined.")]
pub struct RouteNotFoundException {
    pub name: String,
}

impl RouteNotFoundException {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

/// Thrown when a request's URL signature is missing, invalid, or expired.
///
/// The error is an [`HttpException`] with a `403` status code, so it renders
/// as "403 | Invalid signature." out of the box. Both
/// `error.downcast_ref::<InvalidSignatureException>()` and
/// `error.downcast_ref::<HttpException>()` succeed on the error returned by
/// [`InvalidSignatureException::into_error`].
#[derive(Debug, Clone, Copy, Default, thiserror::Error)]
#[error("Invalid signature.")]
pub struct InvalidSignatureException;

impl InvalidSignatureException {
    /// Convert the exception into a framework error that is *also* a 403
    /// [`HttpException`].
    pub fn into_error(self) -> Error {
        Error::from(HttpException::with_message(403, "Invalid signature.")).context(self)
    }
}

/// Thrown when a route refers to a middleware alias or group that hasn't
/// been registered with the router.
#[derive(Debug, Clone, thiserror::Error)]
#[error(
    "Middleware [{name}] is not defined. Did you forget to register an alias for it with Router::alias_middleware()?"
)]
pub struct MiddlewareNotFoundException {
    pub name: String,
}

/// Thrown when a middleware group references itself.
#[derive(Debug, Clone, thiserror::Error)]
#[error("[{group}] middleware group is referencing itself.")]
pub struct RecursiveMiddlewareGroupException {
    pub group: String,
}

/// Thrown when a route's URI or constraints cannot be compiled into a
/// regular expression.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Unable to compile the route [{uri}]: {reason}")]
pub struct InvalidRouteException {
    pub uri: String,
    pub reason: String,
}

/// The `404 Not Found` thrown when no route matches the request.
pub(crate) fn not_found(path: &str) -> Error {
    HttpException::with_message(404, format!("The route {path} could not be found.")).into()
}

/// The `405 Method Not Allowed` thrown when the URI matches routes for
/// other HTTP verbs only.
pub(crate) fn method_not_allowed(method: &str, path: &str, others: &[String]) -> Error {
    HttpException::with_message(
        405,
        format!(
            "The {method} method is not supported for route {path}. Supported methods: {}.",
            others.join(", ")
        ),
    )
    .header("Allow", others.join(", ").to_uppercase())
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_signatures_are_403_http_exceptions() {
        let error = InvalidSignatureException.into_error();
        assert_eq!(error.downcast_ref::<HttpException>().unwrap().status, 403);
        assert!(error.downcast_ref::<InvalidSignatureException>().is_some());
        assert_eq!(error.to_string(), "Invalid signature.");
    }

    #[test]
    fn missing_parameter_messages_pluralize() {
        let e = UrlGenerationException::for_missing_parameters(
            Some("posts.show"),
            "users/{user}/posts/{post}",
            &["user".into(), "post".into()],
        );
        assert_eq!(
            e.to_string(),
            "Missing required parameters for [Route: posts.show] [URI: users/{user}/posts/{post}] [Missing parameters: user, post]."
        );
    }

    #[test]
    fn method_not_allowed_carries_an_allow_header() {
        let error = method_not_allowed("DELETE", "users", &["GET".into(), "HEAD".into()]);
        let http = error.downcast_ref::<HttpException>().unwrap();
        assert_eq!(http.status, 405);
        assert_eq!(
            http.headers,
            vec![("Allow".to_string(), "GET, HEAD".to_string())]
        );
    }
}
