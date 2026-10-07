//! Stubbed responses for testing.
//!
//! ```
//! use illuminate_http_client::{FakeResponse, Http};
//! use illuminate_support::json;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! Http::fake_urls([
//!     // Stub a JSON response for GitHub endpoints...
//!     ("github.com/*", Http::response(json!({"foo": "bar"}), 200, &[("X-Custom", "yes")])),
//!     // Stub a string response for Google endpoints...
//!     ("google.com/*", Http::response("Hello World", 200, &[])),
//!     // Fail connections to Laravel...
//!     ("laravel.com/*", Http::failed_connection()),
//!     // Stub a status code for every other endpoint...
//!     ("*", FakeResponse::from(404)),
//! ]);
//!
//! assert_eq!(Http::get("https://github.com/laravel").await?["foo"], "bar");
//! assert_eq!(Http::get("https://google.com/search").await?.body(), "Hello World");
//! assert!(Http::get("https://laravel.com/docs").await.is_err());
//! assert!(Http::get("https://forge.laravel.com").await?.not_found());
//! # Ok::<(), illuminate_support::Error>(())
//! # }).unwrap();
//! ```

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue};
use illuminate_support::error::{InvalidArgumentException, RuntimeException};
use illuminate_support::{Result, Value};

use crate::request::Request;
use crate::response::Response;

type FakeCallback = Arc<dyn Fn(&Request) -> FakeResponse + Send + Sync>;

/// A stubbed response: what a faked request should receive.
///
/// Fake responses are usually built with [`Http::response`](crate::Http::response),
/// but strings, JSON values, status codes, [`Response`]s and
/// [`ResponseSequence`]s all convert into one.
#[derive(Clone)]
pub struct FakeResponse {
    stub: Stub,
}

#[derive(Clone)]
enum Stub {
    Response(Response),
    Status(u16),
    FailedConnection(Option<String>),
    Sequence(ResponseSequence),
    Callback(FakeCallback),
    Passthrough,
}

/// What a fake resolved to for a particular request.
pub(crate) enum Outcome {
    Response(Response),
    FailedConnection(String),
}

impl FakeResponse {
    /// Create a fake response.
    ///
    /// Strings are used as the raw body; any other JSON value is encoded
    /// as JSON (with an `application/json` content type), and `null` is an
    /// empty body.
    pub fn new(body: impl Into<Value>, status: u16, headers: &[(&str, &str)]) -> Self {
        let mut map = HeaderMap::new();

        let body = match body.into() {
            Value::Null => Bytes::new(),
            Value::String(text) => Bytes::from(text),
            json => {
                map.insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                Bytes::from(json.to_string())
            }
        };

        for (name, value) in headers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_bytes(value.as_bytes()),
            ) {
                map.insert(name, value);
            }
        }

        Self::from(Response::new(status, map, body))
    }

    /// A fake that fails to connect, with an optional custom message.
    pub fn failed_connection(message: Option<String>) -> Self {
        Self {
            stub: Stub::FailedConnection(message),
        }
    }

    /// A fake computed by the given callback for each request.
    pub fn using<F, R>(callback: F) -> Self
    where
        F: Fn(&Request) -> R + Send + Sync + 'static,
        R: Into<FakeResponse>,
    {
        Self {
            stub: Stub::Callback(Arc::new(move |request| callback(request).into())),
        }
    }

    /// A "fake" that lets the request through to the network.
    pub fn passthrough() -> Self {
        Self {
            stub: Stub::Passthrough,
        }
    }

    /// Resolve the fake for the given request (`None` lets the request through).
    pub(crate) fn resolve(&self, request: &Request) -> Result<Option<Outcome>> {
        let mut current = self.clone();

        loop {
            current = match current.stub {
                Stub::Response(response) => return Ok(Some(Outcome::Response(response))),
                Stub::Status(status) => {
                    if !(100..600).contains(&status) {
                        return Err(InvalidArgumentException::new(
                            "HTTP status code must be between 100 and 599.",
                        )
                        .into());
                    }
                    return Ok(Some(Outcome::Response(Response::new(
                        status,
                        HeaderMap::new(),
                        "",
                    ))));
                }
                Stub::FailedConnection(message) => {
                    let message = message.unwrap_or_else(|| default_connection_failure(request));
                    return Ok(Some(Outcome::FailedConnection(message)));
                }
                Stub::Passthrough => return Ok(None),
                Stub::Sequence(sequence) => sequence.next()?,
                Stub::Callback(callback) => callback(request),
            };
        }
    }
}

fn default_connection_failure(request: &Request) -> String {
    let host = url::Url::parse(request.url())
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_default();

    format!("Could not resolve host: {host} for {}.", request.url())
}

impl fmt::Debug for FakeResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.stub {
            Stub::Response(response) => f.debug_tuple("FakeResponse").field(response).finish(),
            Stub::Status(status) => f.debug_tuple("FakeResponse::Status").field(status).finish(),
            Stub::FailedConnection(message) => f
                .debug_tuple("FakeResponse::FailedConnection")
                .field(message)
                .finish(),
            Stub::Sequence(sequence) => f
                .debug_tuple("FakeResponse::Sequence")
                .field(sequence)
                .finish(),
            Stub::Callback(_) => f.write_str("FakeResponse::Callback"),
            Stub::Passthrough => f.write_str("FakeResponse::Passthrough"),
        }
    }
}

impl From<Response> for FakeResponse {
    fn from(response: Response) -> Self {
        Self {
            stub: Stub::Response(response),
        }
    }
}

/// Strings are stubbed as a `200` response with the string as its body.
impl From<&str> for FakeResponse {
    fn from(body: &str) -> Self {
        Self::new(body, 200, &[])
    }
}

/// Strings are stubbed as a `200` response with the string as its body.
impl From<String> for FakeResponse {
    fn from(body: String) -> Self {
        Self::new(body, 200, &[])
    }
}

/// JSON values are stubbed as a `200` JSON response.
impl From<Value> for FakeResponse {
    fn from(body: Value) -> Self {
        Self::new(body, 200, &[])
    }
}

/// Status codes are stubbed as an empty response with that status.
impl From<u16> for FakeResponse {
    fn from(status: u16) -> Self {
        Self {
            stub: Stub::Status(status),
        }
    }
}

impl From<ResponseSequence> for FakeResponse {
    fn from(sequence: ResponseSequence) -> Self {
        Self {
            stub: Stub::Sequence(sequence),
        }
    }
}

/// `None` lets the request through to the network.
impl<T: Into<FakeResponse>> From<Option<T>> for FakeResponse {
    fn from(fake: Option<T>) -> Self {
        fake.map(Into::into).unwrap_or_else(Self::passthrough)
    }
}

/// A sequence of fake responses, returned in order.
///
/// Sequences are shared handles: clones push to, and pop from, the same
/// queue.
///
/// ```
/// use illuminate_http_client::Http;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// Http::fake_sequence("github.com/*")
///     .push("Hello World", 200)
///     .push(json!({"foo": "bar"}), 200)
///     .push_status(404)
///     .when_empty(Http::response("Done", 200, &[]));
///
/// assert_eq!(Http::get("https://github.com/a").await?.body(), "Hello World");
/// assert_eq!(Http::get("https://github.com/b").await?["foo"], "bar");
/// assert!(Http::get("https://github.com/c").await?.not_found());
/// assert_eq!(Http::get("https://github.com/d").await?.body(), "Done");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Clone, Default)]
pub struct ResponseSequence {
    state: Arc<Mutex<SequenceState>>,
}

#[derive(Default)]
struct SequenceState {
    responses: VecDeque<FakeResponse>,
    allow_empty: bool,
    empty_response: Option<FakeResponse>,
}

impl ResponseSequence {
    /// Create a new response sequence.
    pub fn new(responses: impl IntoIterator<Item = impl Into<FakeResponse>>) -> Self {
        let sequence = Self::default();
        sequence.state.lock().unwrap().responses = responses.into_iter().map(Into::into).collect();
        sequence
    }

    /// Push a response to the sequence.
    pub fn push(self, body: impl Into<Value>, status: u16) -> Self {
        self.push_response(FakeResponse::new(body, status, &[]))
    }

    /// Push a response with the given headers to the sequence.
    pub fn push_with_headers(
        self,
        body: impl Into<Value>,
        status: u16,
        headers: &[(&str, &str)],
    ) -> Self {
        self.push_response(FakeResponse::new(body, status, headers))
    }

    /// Push an empty response with the given status code to the sequence.
    pub fn push_status(self, status: u16) -> Self {
        self.push_response(FakeResponse::new(Value::Null, status, &[]))
    }

    /// Push the contents of a file as a response to the sequence.
    ///
    /// # Panics
    ///
    /// Panics if the file can't be read: a broken test fixture.
    pub fn push_file(self, path: impl AsRef<std::path::Path>, status: u16) -> Self {
        let path = path.as_ref();
        let contents = std::fs::read(path).unwrap_or_else(|error| {
            panic!(
                "Unable to read fake response file [{}]: {error}",
                path.display()
            )
        });
        self.push_response(Response::new(status, HeaderMap::new(), contents))
    }

    /// Push a failed connection to the sequence.
    pub fn push_failed_connection(self) -> Self {
        self.push_response(FakeResponse::failed_connection(None))
    }

    /// Push a failed connection with the given message to the sequence.
    pub fn push_failed_connection_with(self, message: impl Into<String>) -> Self {
        self.push_response(FakeResponse::failed_connection(Some(message.into())))
    }

    /// Push any fake response to the sequence.
    pub fn push_response(self, response: impl Into<FakeResponse>) -> Self {
        self.state
            .lock()
            .unwrap()
            .responses
            .push_back(response.into());
        self
    }

    /// Make the sequence return the given response once it is empty.
    pub fn when_empty(self, response: impl Into<FakeResponse>) -> Self {
        {
            let mut state = self.state.lock().unwrap();
            state.allow_empty = true;
            state.empty_response = Some(response.into());
        }
        self
    }

    /// Make the sequence return an empty `200` response once it is empty.
    pub fn dont_fail_when_empty(self) -> Self {
        self.when_empty(FakeResponse::new(Value::Null, 200, &[]))
    }

    /// Determine if the sequence has run out of responses.
    pub fn is_empty(&self) -> bool {
        self.state.lock().unwrap().responses.is_empty()
    }

    /// The number of responses left in the sequence.
    pub fn len(&self) -> usize {
        self.state.lock().unwrap().responses.len()
    }

    /// Take the next response from the sequence.
    pub(crate) fn next(&self) -> Result<FakeResponse> {
        let mut state = self.state.lock().unwrap();

        if let Some(response) = state.responses.pop_front() {
            return Ok(response);
        }

        if state.allow_empty {
            return Ok(state
                .empty_response
                .clone()
                .unwrap_or_else(|| FakeResponse::new(Value::Null, 200, &[])));
        }

        Err(RuntimeException::new("A request was made, but the response sequence is empty.").into())
    }
}

impl fmt::Debug for ResponseSequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseSequence")
            .field("remaining", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn resolve(fake: &FakeResponse) -> Response {
        match fake
            .resolve(&Request::new("GET", "https://laravel.com/docs"))
            .unwrap()
        {
            Some(Outcome::Response(response)) => response,
            Some(Outcome::FailedConnection(message)) => panic!("unexpected failure: {message}"),
            None => panic!("unexpected passthrough"),
        }
    }

    #[test]
    fn it_builds_fake_responses() {
        let json = resolve(&FakeResponse::new(
            json!({"foo": "bar"}),
            201,
            &[("X-Custom", "yes")],
        ));
        assert_eq!(json.status(), 201);
        assert_eq!(json.header("Content-Type"), "application/json");
        assert_eq!(json.header("X-Custom"), "yes");
        assert_eq!(json["foo"], "bar");

        let text = resolve(&"Hello World".into());
        assert_eq!(text.body(), "Hello World");
        assert!(!text.has_header("content-type"));

        assert_eq!(resolve(&String::from("Hi").into()).body(), "Hi");
        assert_eq!(resolve(&json!([1, 2]).into())[1], 2);
        assert!(resolve(&FakeResponse::new(Value::Null, 204, &[])).no_content());
        assert!(resolve(&404.into()).not_found());
    }

    #[test]
    fn invalid_status_codes_are_rejected() {
        let error = FakeResponse::from(700)
            .resolve(&Request::new("GET", "/"))
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "HTTP status code must be between 100 and 599."
        );
    }

    #[test]
    fn failed_connections_describe_the_host() {
        let request = Request::new("GET", "https://laravel.com/docs");
        match FakeResponse::failed_connection(None)
            .resolve(&request)
            .unwrap()
        {
            Some(Outcome::FailedConnection(message)) => assert_eq!(
                message,
                "Could not resolve host: laravel.com for https://laravel.com/docs."
            ),
            _ => panic!("expected a failed connection"),
        }
        assert!(
            format!("{:?}", FakeResponse::failed_connection(None)).contains("FailedConnection")
        );
    }

    #[test]
    fn callbacks_and_options_resolve_lazily() {
        let fake = FakeResponse::using(|request: &Request| {
            if request.url().contains("docs") {
                Some(FakeResponse::from("docs"))
            } else {
                None
            }
        });

        assert_eq!(resolve(&fake).body(), "docs");
        assert!(
            fake.resolve(&Request::new("GET", "https://forge.com"))
                .unwrap()
                .is_none()
        );
        assert!(
            FakeResponse::from(None::<FakeResponse>)
                .resolve(&Request::new("GET", "/"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn sequences_return_responses_in_order() {
        let sequence = ResponseSequence::new(["first"])
            .push("second", 200)
            .push_with_headers("third", 201, &[("X-Third", "3")])
            .push_status(500);

        assert_eq!(sequence.len(), 4);
        let fake = FakeResponse::from(sequence.clone());
        assert_eq!(resolve(&fake).body(), "first");
        assert_eq!(resolve(&fake).body(), "second");
        assert_eq!(resolve(&fake).header("X-Third"), "3");
        assert_eq!(resolve(&fake).status(), 500);
        assert!(sequence.is_empty());

        let error = fake.resolve(&Request::new("GET", "/")).err().unwrap();
        assert_eq!(
            error.to_string(),
            "A request was made, but the response sequence is empty."
        );

        let sequence = sequence.dont_fail_when_empty();
        assert!(resolve(&sequence.clone().into()).ok());
        let sequence = sequence.when_empty(FakeResponse::new("empty", 200, &[]));
        assert_eq!(resolve(&sequence.into()).body(), "empty");
    }

    #[test]
    fn sequences_can_fail_connections_and_read_files() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "from a file").unwrap();

        let sequence = ResponseSequence::default()
            .push_file(file.path(), 200)
            .push_failed_connection()
            .push_failed_connection_with("Nope");

        let fake = FakeResponse::from(sequence);
        assert_eq!(resolve(&fake).body(), "from a file");

        let request = Request::new("GET", "https://laravel.com");
        assert!(matches!(
            fake.resolve(&request).unwrap(),
            Some(Outcome::FailedConnection(_))
        ));
        assert!(matches!(
            fake.resolve(&request).unwrap(),
            Some(Outcome::FailedConnection(message)) if message == "Nope"
        ));
    }
}
