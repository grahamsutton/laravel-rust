//! HTTP exceptions, `abort()`, and the exception handler contract.

use std::sync::{Arc, Mutex};

use illuminate_container::try_app;
use illuminate_support::{Error, json};

use crate::context::current_request;
use crate::request::Request;
use crate::response::Response;

/// An exception that results in an HTTP error response.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{}", self.message())]
pub struct HttpException {
    pub status: u16,
    pub message: Option<String>,
    pub headers: Vec<(String, String)>,
}

impl HttpException {
    pub fn new(status: u16) -> Self {
        Self {
            status,
            message: None,
            headers: Vec::new(),
        }
    }

    pub fn with_message(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: Some(message.into()),
            headers: Vec::new(),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The message: the custom one, or the standard reason phrase.
    pub fn message(&self) -> String {
        self.message
            .clone()
            .unwrap_or_else(|| status_text(self.status).to_string())
    }

    pub fn status_code(&self) -> u16 {
        self.status
    }
}

/// An exception carrying a ready-made response, which is returned as-is.
#[derive(Debug, thiserror::Error)]
#[error("HTTP response exception")]
pub struct HttpResponseException {
    response: Mutex<Option<Response>>,
}

impl HttpResponseException {
    pub fn new(response: Response) -> Self {
        Self {
            response: Mutex::new(Some(response)),
        }
    }

    /// Take the response out of the exception.
    pub fn take_response(&self) -> Option<Response> {
        self.response.lock().unwrap().take()
    }
}

/// Throw an HTTP exception with the given status code.
///
/// ```
/// use illuminate_http::{abort, HttpException};
///
/// fn find_podcast(id: u64) -> Result<String, HttpException> {
///     if id != 1 {
///         return abort(404);
///     }
///     Ok("The Laravel Podcast".into())
/// }
///
/// assert_eq!(find_podcast(2).unwrap_err().status, 404);
/// ```
pub fn abort<T>(status: u16) -> Result<T, HttpException> {
    Err(HttpException::new(status))
}

/// Throw an HTTP exception with a status code and custom message.
pub fn abort_with<T>(status: u16, message: impl Into<String>) -> Result<T, HttpException> {
    Err(HttpException::with_message(status, message))
}

/// Throw an HTTP exception if the condition is true.
///
/// ```
/// use illuminate_http::abort_if;
///
/// let is_admin = false;
/// assert!(abort_if(!is_admin, 403).is_err());
/// ```
pub fn abort_if(condition: bool, status: u16) -> Result<(), HttpException> {
    if condition { abort(status) } else { Ok(()) }
}

/// Throw an HTTP exception unless the condition is true.
pub fn abort_unless(condition: bool, status: u16) -> Result<(), HttpException> {
    abort_if(!condition, status)
}

/// The contract for the application's exception handler.
pub trait ExceptionHandler: Send + Sync {
    /// Report or log an exception.
    fn report(&self, error: &Error);

    /// Render an exception into an HTTP response.
    fn render(&self, request: &Request, error: Error) -> Response;

    /// Determine if the exception should be reported.
    fn should_report(&self, _error: &Error) -> bool {
        true
    }
}

/// Report and render an error using the application's exception handler,
/// falling back to a sensible default when none is registered.
pub fn render_exception(error: Error) -> Response {
    let request = current_request().unwrap_or_default();
    match try_app::<dyn ExceptionHandler>() {
        Some(handler) => {
            if handler.should_report(&error) {
                handler.report(&error);
            }
            handler.render(&request, error)
        }
        None => default_render(&request, error),
    }
}

/// The default rendering used when no exception handler is bound.
pub fn default_render(request: &Request, error: Error) -> Response {
    if let Some(exception) = error.downcast_ref::<HttpResponseException>() {
        if let Some(response) = exception.take_response() {
            return response;
        }
    }

    let (status, message, headers) = match error.downcast_ref::<HttpException>() {
        Some(http) => (http.status, http.message(), http.headers.clone()),
        None => (500, "Server Error".to_string(), Vec::new()),
    };

    let mut response = if request.expects_json() {
        Response::json(&json!({ "message": message }))
    } else {
        Response::new(format!("{status} | {message}"))
    };
    response = response.with_status(status);
    for (name, value) in headers {
        response.set_header(&name, &value);
    }
    response.with_exception(Arc::new(error))
}

/// The standard reason phrase for an HTTP status code.
pub fn status_text(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Content Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        418 => "I'm a teapot",
        419 => "Page Expired",
        422 => "Unprocessable Content",
        423 => "Locked",
        425 => "Too Early",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Unknown Status",
    }
}
