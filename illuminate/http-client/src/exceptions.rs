//! The HTTP client's exceptions.
//!
//! Every fallible client API returns [`illuminate_support::Result`], so these
//! are recovered by downcasting:
//!
//! ```
//! use illuminate_http_client::{ConnectionException, Http, RequestException};
//! use illuminate_support::json;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! Http::fake_urls([
//!     ("github.com/*", Http::response(json!({"message": "Not Found"}), 404, &[])),
//!     ("laravel.com/*", Http::failed_connection()),
//! ]);
//!
//! let error = Http::throw().get("https://github.com/laravel/nope").await.unwrap_err();
//! let exception = error.downcast_ref::<RequestException>().unwrap();
//! assert_eq!(exception.response.status(), 404);
//!
//! let error = Http::get("https://laravel.com/docs").await.unwrap_err();
//! assert!(error.is::<ConnectionException>());
//! # });
//! ```

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::response::Response;

/// How request exception messages summarize the response body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Truncation {
    /// Truncate the body summary at the given number of bytes.
    At(usize),
    /// Include the entire response message.
    Never,
}

/// Sentinel stored in [`TRUNCATE_AT`] when truncation is disabled.
const NEVER: usize = usize::MAX;

static TRUNCATE_AT: AtomicUsize = AtomicUsize::new(120);

fn default_truncation() -> Truncation {
    match TRUNCATE_AT.load(Ordering::Relaxed) {
        NEVER => Truncation::Never,
        length => Truncation::At(length),
    }
}

/// Thrown when a response has a client or server error status code.
///
/// The failed [`Response`] is available on the public `response` field, so
/// you may inspect exactly what the server said.
#[derive(Clone)]
pub struct RequestException {
    /// The response that triggered the exception.
    pub response: Response,
    message: String,
}

impl RequestException {
    /// Create a new exception for the given response.
    pub fn new(response: Response) -> Self {
        let truncation = response.truncation();
        Self::with_truncation(response, truncation)
    }

    pub(crate) fn with_truncation(response: Response, truncation: Option<Truncation>) -> Self {
        let message = prepare_message(&response, truncation.unwrap_or_else(default_truncation));
        Self { response, message }
    }

    /// The exception message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The exception "code": the response's status code.
    pub fn code(&self) -> u16 {
        self.response.status()
    }

    /// Restore the default truncation of exception messages (120 bytes).
    pub fn truncate() {
        TRUNCATE_AT.store(120, Ordering::Relaxed);
    }

    /// Truncate the response body included in exception messages at the
    /// given length. Typically called while bootstrapping your application.
    pub fn truncate_at(length: usize) {
        TRUNCATE_AT.store(length.clamp(1, NEVER - 1), Ordering::Relaxed);
    }

    /// Include the entire response in exception messages.
    pub fn dont_truncate() {
        TRUNCATE_AT.store(NEVER, Ordering::Relaxed);
    }
}

impl fmt::Display for RequestException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for RequestException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestException")
            .field("status", &self.response.status())
            .field("message", &self.message)
            .finish()
    }
}

impl std::error::Error for RequestException {}

/// Build Laravel's message: `HTTP request returned status code 404:\n{summary}\n`.
fn prepare_message(response: &Response, truncation: Truncation) -> String {
    let message = format!("HTTP request returned status code {}", response.status());

    let summary = match truncation {
        Truncation::At(length) => body_summary(response.bytes(), length),
        Truncation::Never => Some(message_to_string(response)),
    };

    match summary {
        Some(summary) => format!("{message}:\n{summary}\n"),
        None => message,
    }
}

/// A printable summary of the body, like Guzzle's `Message::bodySummary`.
///
/// Binary bodies (anything with non-printable characters) are left out.
fn body_summary(body: &[u8], truncate_at: usize) -> Option<String> {
    if body.is_empty() {
        return None;
    }

    let slice = &body[..body.len().min(truncate_at)];
    let text = match std::str::from_utf8(slice) {
        Ok(text) => text,
        // A multi-byte character was cut in half by the truncation...
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&slice[..error.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };

    let printable = text
        .chars()
        .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'));

    if !printable {
        return None;
    }

    let mut summary = text.to_string();
    if body.len() > truncate_at {
        summary.push_str(" (truncated...)");
    }

    Some(summary)
}

/// The full HTTP message, like Guzzle's `Message::toString`.
fn message_to_string(response: &Response) -> String {
    let mut message = format!(
        "{:?} {} {}",
        response.version(),
        response.status(),
        response.reason()
    );

    for name in response.headers().keys() {
        message.push_str(&format!("\r\n{}: {}", name, response.header(name.as_str())));
    }

    message.push_str("\r\n\r\n");
    message.push_str(&response.body());
    message
}

/// Thrown when a connection to the remote server can't be established, the
/// request times out, or the transfer otherwise fails before a response is
/// received.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct ConnectionException {
    message: String,
}

impl ConnectionException {
    /// Create a new connection exception.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The exception message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Thrown when stray requests are being prevented and a request has no
/// matching fake.
#[derive(Clone, Debug, thiserror::Error)]
#[error("Attempted request to [{uri}] without a matching fake.")]
pub struct StrayRequestException {
    /// The URI that was requested.
    pub uri: String,
}

impl StrayRequestException {
    /// Create a new stray request exception for the given URI.
    pub fn new(uri: impl Into<String>) -> Self {
        Self { uri: uri.into() }
    }
}

/// Determine if the given error is one of the HTTP client's exceptions
/// (Laravel's `HttpClientException` hierarchy).
pub fn is_http_client_exception(error: &illuminate_support::Error) -> bool {
    error.is::<RequestException>() || error.is::<ConnectionException>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;

    fn response(status: u16, body: &str) -> Response {
        Response::new(status, HeaderMap::new(), body.to_string())
    }

    #[test]
    fn messages_include_a_summary_of_the_body() {
        let exception = RequestException::new(response(404, "Not Found"));
        assert_eq!(
            exception.to_string(),
            "HTTP request returned status code 404:\nNot Found\n"
        );
        assert_eq!(exception.code(), 404);
    }

    #[test]
    fn empty_bodies_are_left_out() {
        let exception = RequestException::new(response(500, ""));
        assert_eq!(exception.message(), "HTTP request returned status code 500");
    }

    #[test]
    fn long_bodies_are_truncated() {
        let body = "a".repeat(200);
        let exception =
            RequestException::with_truncation(response(500, &body), Some(Truncation::At(10)));
        assert_eq!(
            exception.message(),
            "HTTP request returned status code 500:\naaaaaaaaaa (truncated...)\n"
        );
    }

    #[test]
    fn binary_bodies_are_not_summarized() {
        let binary = Response::new(500, HeaderMap::new(), vec![0u8, 159, 146, 150]);
        let exception = RequestException::new(binary);
        assert_eq!(exception.message(), "HTTP request returned status code 500");
    }

    #[test]
    fn multibyte_characters_are_not_split() {
        let exception =
            RequestException::with_truncation(response(422, "héllo"), Some(Truncation::At(2)));
        assert_eq!(
            exception.message(),
            "HTTP request returned status code 422:\nh (truncated...)\n"
        );
    }

    #[test]
    fn untruncated_messages_include_the_whole_response() {
        let mut headers = HeaderMap::new();
        headers.insert("x-reason", "nope".parse().unwrap());
        let failed = Response::new(403, headers, "Forbidden!");
        let exception = RequestException::with_truncation(failed, Some(Truncation::Never));
        assert_eq!(
            exception.message(),
            "HTTP request returned status code 403:\nHTTP/1.1 403 Forbidden\r\nx-reason: nope\r\n\r\nForbidden!\n"
        );
    }

    #[test]
    fn other_exceptions_have_laravel_messages() {
        assert_eq!(
            StrayRequestException::new("https://laravel.com").to_string(),
            "Attempted request to [https://laravel.com] without a matching fake."
        );
        assert_eq!(ConnectionException::new("Timed out").message(), "Timed out");

        let error: illuminate_support::Error = ConnectionException::new("Timed out").into();
        assert!(is_http_client_exception(&error));
        let error: illuminate_support::Error = StrayRequestException::new("/").into();
        assert!(!is_http_client_exception(&error));
    }
}
