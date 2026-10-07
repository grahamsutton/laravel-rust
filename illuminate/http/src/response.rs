//! The outgoing HTTP response.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use futures::Stream;
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use serde::Serialize;

use illuminate_support::{Error, MessageBag, Value, to_value};

use crate::cookie::Cookie;
use crate::streamed::{StreamedEvent, StreamedJson};

/// A streamed body chunk.
pub type BodyStream = Pin<Box<dyn Stream<Item = Result<Bytes, Error>> + Send>>;

/// The body of a response.
pub enum Body {
    /// No body at all.
    Empty,
    /// A fully buffered body.
    Bytes(Bytes),
    /// A body streamed to the client chunk by chunk.
    Stream(SyncStream),
}

/// A streamed body that can be shared between threads (the stream itself is
/// only ever polled by the server, so a mutex makes the response `Sync`).
pub struct SyncStream(std::sync::Mutex<BodyStream>);

impl SyncStream {
    /// Wrap a stream.
    pub fn new(stream: BodyStream) -> Self {
        Self(std::sync::Mutex::new(stream))
    }

    /// Take the underlying stream.
    pub fn into_inner(self) -> BodyStream {
        self.0.into_inner().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Body::Empty => f.write_str("Body::Empty"),
            Body::Bytes(b) => write!(f, "Body::Bytes({} bytes)", b.len()),
            Body::Stream(_) => f.write_str("Body::Stream"),
        }
    }
}

/// An HTTP response.
///
/// A single type covers every flavor of response Laravel offers — plain,
/// JSON, redirects, downloads, and streams — and carries along the things a
/// response may ask the session to remember (`with()`, `with_errors()`,
/// `with_input()`), which the session middleware applies on the way out.
#[derive(Debug)]
pub struct Response {
    status: StatusCode,
    headers: HeaderMap,
    body: Body,
    cookies: Vec<Cookie>,
    flash: Vec<(String, Value)>,
    flash_input: Option<Value>,
    errors: Option<(String, MessageBag)>,
    exception: Option<Arc<Error>>,
    original: Option<Value>,
    extensions: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl Default for Response {
    fn default() -> Self {
        Self::new("")
    }
}

impl Response {
    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    /// Create a new `200 OK` HTML response with the given content.
    pub fn new(content: impl Into<Bytes>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=UTF-8"),
        );
        Self {
            status: StatusCode::OK,
            headers,
            body: Body::Bytes(content.into()),
            cookies: Vec::new(),
            flash: Vec::new(),
            flash_input: None,
            errors: None,
            exception: None,
            original: None,
            extensions: HashMap::new(),
        }
    }

    /// Create a response with the given content and status code.
    pub fn make(content: impl Into<Bytes>, status: u16) -> Self {
        Self::new(content).with_status(status)
    }

    /// Create a JSON response.
    ///
    /// ```
    /// use illuminate_http::Response;
    /// use illuminate_support::json;
    ///
    /// let response = Response::json(&json!({"name": "Abigail", "state": "CA"}));
    /// assert_eq!(response.content_string(), r#"{"name":"Abigail","state":"CA"}"#);
    /// ```
    pub fn json<T: Serialize + ?Sized>(data: &T) -> Self {
        let value = to_value(data);
        let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"null".to_vec());
        let mut response = Self::new(body).with_header("content-type", "application/json");
        response.original = Some(value);
        response
    }

    /// Create a JSONP response, wrapping the JSON in the given callback.
    pub fn jsonp<T: Serialize + ?Sized>(callback: &str, data: &T) -> Self {
        let json = serde_json::to_string(&to_value(data)).unwrap_or_else(|_| "null".into());
        Self::new(format!("/**/{callback}({json});")).with_header("content-type", "text/javascript")
    }

    /// Create an empty `204 No Content` response.
    pub fn no_content() -> Self {
        let mut response = Self::new(Bytes::new()).with_status(204);
        response.body = Body::Empty;
        response.headers.remove(http::header::CONTENT_TYPE);
        response
    }

    /// Create a redirect response to the given URL (`302 Found`).
    pub fn redirect(to: impl AsRef<str>) -> Self {
        Self::redirect_with_status(to, 302)
    }

    /// Create a redirect response with a specific status code.
    pub fn redirect_with_status(to: impl AsRef<str>, status: u16) -> Self {
        let to = to.as_ref();
        let escaped = illuminate_support::e(to);
        let body = format!(
            "<!DOCTYPE html>\n<html>\n    <head>\n        <meta charset=\"UTF-8\" />\n        <meta http-equiv=\"refresh\" content=\"0;url='{escaped}'\" />\n\n        <title>Redirecting to {escaped}</title>\n    </head>\n    <body>\n        Redirecting to <a href=\"{escaped}\">{escaped}</a>.\n    </body>\n</html>"
        );
        Self::new(body)
            .with_status(status)
            .with_header("location", to)
    }

    /// Create a response that streams the given chunks to the client.
    pub fn stream(stream: impl Stream<Item = Result<Bytes, Error>> + Send + 'static) -> Self {
        let mut response = Self::new(Bytes::new());
        response.body = Body::Stream(SyncStream::new(Box::pin(stream)));
        response
    }

    /// Create a response that serves a file inline.
    pub async fn file(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let contents = tokio::fs::read(path).await?;
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        Ok(Self::new(contents).with_header("content-type", mime.essence_str()))
    }

    /// Create a response that forces the browser to download a file.
    pub async fn download(path: impl AsRef<Path>, name: Option<&str>) -> std::io::Result<Self> {
        let path: PathBuf = path.as_ref().to_path_buf();
        let name = name
            .map(String::from)
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "download".into());
        let response = Self::file(&path).await?;
        Ok(response.with_header("content-disposition", &make_disposition("attachment", &name)))
    }

    /// Create a download response from in-memory content.
    ///
    /// ```
    /// use illuminate_http::Response;
    ///
    /// let response = Response::download_content("id,name\n1,Taylor", "users.csv");
    /// assert_eq!(response.header("content-disposition").unwrap(), "attachment; filename=users.csv");
    /// assert_eq!(response.header("content-type").unwrap(), "text/csv");
    /// ```
    pub fn download_content(content: impl Into<Bytes>, name: &str) -> Self {
        let mime = mime_guess::from_path(name).first_or_octet_stream();
        Self::new(content)
            .with_header("content-type", mime.essence_str())
            .with_header("content-disposition", &make_disposition("attachment", name))
    }

    /// Create a Markdown response (`text/markdown`).
    ///
    /// ```
    /// use illuminate_http::Response;
    ///
    /// let response = Response::markdown("# Laravel");
    /// assert_eq!(response.header("content-type").unwrap(), "text/markdown");
    /// ```
    pub fn markdown(content: impl Into<Bytes>) -> Self {
        Self::new(content).with_header("content-type", "text/markdown")
    }

    /// Stream a download to the browser without writing it to disk first —
    /// Laravel's `response()->streamDownload()`.
    ///
    /// ```
    /// use bytes::Bytes;
    /// use illuminate_http::Response;
    ///
    /// let chunks = futures::stream::iter(vec![Ok(Bytes::from("id,name\n")), Ok(Bytes::from("1,Taylor\n"))]);
    /// let response = Response::stream_download(chunks, Some("users.csv"));
    ///
    /// assert_eq!(response.header("content-disposition").unwrap(), "attachment; filename=users.csv");
    /// ```
    pub fn stream_download(
        stream: impl Stream<Item = Result<Bytes, Error>> + Send + 'static,
        name: Option<&str>,
    ) -> Self {
        Self::stream_download_with_disposition(stream, name, "attachment")
    }

    /// Stream a file to the browser with the given disposition (`attachment`
    /// or `inline`).
    pub fn stream_download_with_disposition(
        stream: impl Stream<Item = Result<Bytes, Error>> + Send + 'static,
        name: Option<&str>,
        disposition: &str,
    ) -> Self {
        let mut response = Self::stream(stream);
        if let Some(name) = name {
            let mime = mime_guess::from_path(name).first_or_octet_stream();
            response.set_header("content-type", mime.essence_str());
            response.set_header("content-disposition", &make_disposition(disposition, name));
        }
        response
    }

    /// Stream JSON to the client as it's produced — Laravel's
    /// `response()->streamJson()`. See [`StreamedJson`].
    pub fn stream_json(data: impl Into<StreamedJson>) -> Self {
        Self::stream(data.into().into_body()).with_header("content-type", "application/json")
    }

    /// Stream Server-Sent Events to the client — Laravel's
    /// `response()->eventStream()`. Every item becomes an event (plain
    /// messages are `update` events); a final `</stream>` update event tells
    /// the client the stream is complete.
    ///
    /// ```
    /// use illuminate_http::{Response, StreamedEvent};
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// let events = futures::stream::iter(vec![
    ///     StreamedEvent::from("Hello"),
    ///     StreamedEvent::new("user", serde_json::json!({"name": "Taylor"})),
    /// ]);
    ///
    /// let response = Response::event_stream(events);
    ///
    /// assert_eq!(response.header("content-type").unwrap(), "text/event-stream");
    /// assert_eq!(
    ///     response.into_content_string().await.unwrap(),
    ///     "event: update\ndata: Hello\n\nevent: user\ndata: {\"name\":\"Taylor\"}\n\nevent: update\ndata: </stream>\n\n"
    /// );
    /// # });
    /// ```
    pub fn event_stream<S, T>(events: S) -> Self
    where
        S: Stream<Item = T> + Send + 'static,
        T: Into<StreamedEvent>,
    {
        Self::event_stream_ending_with(events, Some(StreamedEvent::from("</stream>")))
    }

    /// Stream Server-Sent Events, closing with the given event (or nothing).
    pub fn event_stream_ending_with<S, T>(events: S, end_stream_with: Option<StreamedEvent>) -> Self
    where
        S: Stream<Item = T> + Send + 'static,
        T: Into<StreamedEvent>,
    {
        Self::stream(crate::streamed::event_stream_body(events, end_stream_with)).with_headers([
            ("content-type", "text/event-stream"),
            ("cache-control", "no-cache"),
            ("x-accel-buffering", "no"),
        ])
    }

    // ------------------------------------------------------------------
    // Fluent modifiers
    // ------------------------------------------------------------------

    /// Set the status code.
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        self
    }

    /// Alias of `with_status` reading like Laravel's `setStatusCode`.
    pub fn set_status_code(&mut self, status: u16) -> &mut Self {
        self.status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        self
    }

    /// Add a header to the response (replacing any existing value).
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.set_header(name, value);
        self
    }

    /// Alias of `with_header`, matching Laravel's `header()`.
    pub fn header_with(self, name: &str, value: &str) -> Self {
        self.with_header(name, value)
    }

    /// Add many headers to the response.
    pub fn with_headers<'a>(mut self, headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        for (name, value) in headers {
            self.set_header(name, value);
        }
        self
    }

    /// Set a header in place.
    pub fn set_header(&mut self, name: &str, value: &str) {
        if let (Ok(name), Ok(value)) = (HeaderName::try_from(name), HeaderValue::try_from(value)) {
            self.headers.insert(name, value);
        }
    }

    /// Append a header in place without replacing existing values.
    pub fn append_header(&mut self, name: &str, value: &str) {
        if let (Ok(name), Ok(value)) = (HeaderName::try_from(name), HeaderValue::try_from(value)) {
            self.headers.append(name, value);
        }
    }

    /// Remove a header.
    pub fn without_header(mut self, name: &str) -> Self {
        self.headers.remove(name);
        self
    }

    /// Attach a cookie to the response.
    pub fn with_cookie(mut self, cookie: Cookie) -> Self {
        self.add_cookie(cookie);
        self
    }

    /// Alias of `with_cookie`.
    pub fn cookie(self, cookie: Cookie) -> Self {
        self.with_cookie(cookie)
    }

    /// Expire a cookie on the client.
    pub fn without_cookie(self, name: &str) -> Self {
        self.with_cookie(Cookie::forget(name))
    }

    /// Attach many cookies to the response.
    pub fn with_cookies(mut self, cookies: impl IntoIterator<Item = Cookie>) -> Self {
        for cookie in cookies {
            self.add_cookie(cookie);
        }
        self
    }

    /// Expire many cookies on the client.
    pub fn without_cookies<'a>(mut self, names: impl IntoIterator<Item = &'a str>) -> Self {
        for name in names {
            self.add_cookie(Cookie::forget(name));
        }
        self
    }

    /// Throw the response: returns an [`HttpResponseException`] carrying
    /// it, which the exception handler renders as-is.
    ///
    /// ```
    /// use illuminate_http::{HttpResponseException, Response};
    ///
    /// fn guard() -> illuminate_support::Result<()> {
    ///     Response::make("Stop right there", 403).throw_response()
    /// }
    ///
    /// let error = guard().unwrap_err();
    /// let response = error.downcast_ref::<HttpResponseException>().unwrap().take_response().unwrap();
    /// assert_eq!(response.status_code(), 403);
    /// ```
    pub fn throw_response<T>(self) -> illuminate_support::Result<T> {
        Err(crate::exceptions::HttpResponseException::new(self).into())
    }

    /// Turn a JSON response into a JSONP response for the given callback
    /// (a `None` callback leaves the response untouched). Fails when the
    /// callback isn't a valid JavaScript identifier.
    ///
    /// ```
    /// use illuminate_http::Response;
    /// use illuminate_support::json;
    ///
    /// let response = Response::json(&json!({"name": "Abigail"})).with_callback(Some("handle")).unwrap();
    /// assert_eq!(response.content_string(), r#"/**/handle({"name":"Abigail"});"#);
    /// assert_eq!(response.header("content-type").unwrap(), "text/javascript");
    ///
    /// assert!(Response::json(&json!({})).with_callback(Some("alert(1)//")).is_err());
    /// ```
    pub fn with_callback(mut self, callback: Option<&str>) -> illuminate_support::Result<Self> {
        let Some(callback) = callback else {
            return Ok(self);
        };
        if !is_valid_callback(callback) {
            return Err(illuminate_support::Error::msg("The callback name is not valid."));
        }
        let json = String::from_utf8_lossy(self.content()).into_owned();
        self.set_content(format!("/**/{callback}({json});"));
        self.set_header("content-type", "text/javascript");
        Ok(self)
    }

    /// Attach a cookie in place (replacing any cookie with the same name).
    pub fn add_cookie(&mut self, cookie: Cookie) {
        self.cookies.retain(|c| c.name != cookie.name || c.path != cookie.path);
        self.cookies.push(cookie);
    }

    /// Flash a piece of data to the session (for redirects).
    ///
    /// ```
    /// use illuminate_http::Response;
    ///
    /// let response = Response::redirect("/dashboard").with("status", "Profile updated!");
    /// assert_eq!(response.flashed()[0].0, "status");
    /// ```
    pub fn with(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.flash.push((key.into(), value.into()));
        self
    }

    /// Flash the given input (or the current request's input) to the session.
    pub fn with_input(mut self, input: Value) -> Self {
        self.flash_input = Some(input);
        self
    }

    /// Flash the current request's input to the session — Laravel's
    /// `withInput()` without arguments.
    pub fn with_request_input(self) -> Self {
        let input = crate::context::current_request()
            .map(|request| request.all())
            .unwrap_or_else(|| Value::Object(Default::default()));
        self.with_input(input)
    }

    /// Flash only the given keys of the current request's input.
    pub fn only_input(self, keys: &[&str]) -> Self {
        let input = crate::context::current_request()
            .map(|request| request.only(keys))
            .unwrap_or_else(|| Value::Object(Default::default()));
        self.with_input(input)
    }

    /// Flash all of the current request's input except the given keys.
    ///
    /// ```
    /// use illuminate_http::{Request, Response, with_request_sync};
    /// use illuminate_support::json;
    ///
    /// let request = Request::create_with("/login", "POST", json!({"email": "taylor@laravel.com", "password": "secret"}), Default::default());
    ///
    /// let response = with_request_sync(request, || Response::redirect("/login").except_input(&["password"]));
    /// assert_eq!(response.flashed_input(), Some(&json!({"email": "taylor@laravel.com"})));
    /// ```
    pub fn except_input(self, keys: &[&str]) -> Self {
        let input = crate::context::current_request()
            .map(|request| request.except(keys))
            .unwrap_or_else(|| Value::Object(Default::default()));
        self.with_input(input)
    }

    /// Flash a container of errors to the session.
    pub fn with_errors(mut self, errors: impl Into<MessageBag>) -> Self {
        self.errors = Some(("default".to_string(), errors.into()));
        self
    }

    /// Flash errors into a named error bag.
    pub fn with_errors_in(mut self, errors: impl Into<MessageBag>, bag: &str) -> Self {
        self.errors = Some((bag.to_string(), errors.into()));
        self
    }

    /// Add a fragment identifier to a redirect URL.
    pub fn with_fragment(self, fragment: &str) -> Self {
        let mut response = self.without_fragment();
        if let Some(location) = response.header("location") {
            let fragment = fragment.split_once('#').map_or(fragment, |(_, after)| after);
            response.set_header("location", &format!("{location}#{fragment}"));
        }
        response
    }

    /// Remove any fragment identifier from a redirect URL.
    ///
    /// ```
    /// use illuminate_http::Response;
    ///
    /// let response = Response::redirect("/docs#install").without_fragment();
    /// assert_eq!(response.target_url().unwrap(), "/docs");
    /// ```
    pub fn without_fragment(mut self) -> Self {
        if let Some(location) = self.header("location") {
            let base = location.split('#').next().unwrap_or_default().to_string();
            self.set_header("location", &base);
        }
        self
    }

    /// Redirect to the fallback instead when the target URL isn't on the
    /// current request's host (and, optionally, scheme and port) — a guard
    /// against open redirects.
    ///
    /// ```
    /// use illuminate_http::{Request, Response, with_request_sync};
    ///
    /// let request = Request::create("http://example.com/login", "POST");
    ///
    /// with_request_sync(request, || {
    ///     let away = Response::redirect("https://evil.test/phish").enforce_same_origin("/", true, true);
    ///     assert_eq!(away.target_url().unwrap(), "/");
    ///
    ///     let home = Response::redirect("http://example.com/home").enforce_same_origin("/", true, true);
    ///     assert_eq!(home.target_url().unwrap(), "http://example.com/home");
    /// });
    /// ```
    pub fn enforce_same_origin(mut self, fallback: &str, validate_scheme: bool, validate_port: bool) -> Self {
        let target = self.header("location").unwrap_or_default();
        let current = crate::context::current_request()
            .map(|request| request.scheme_and_http_host())
            .unwrap_or_else(|| "http://localhost".to_string());
        let (target, current) = (Origin::parse(&target), Origin::parse(&current));
        if target.host != current.host
            || (validate_scheme && target.scheme != current.scheme)
            || (validate_port && target.port != current.port)
        {
            self.set_header("location", fallback);
        }
        self
    }

    /// Replace the body of the response.
    pub fn set_content(&mut self, content: impl Into<Bytes>) {
        self.body = Body::Bytes(content.into());
    }

    /// Attach the error that produced this response (used for testing and reporting).
    pub fn with_exception(mut self, error: Arc<Error>) -> Self {
        self.exception = Some(error);
        self
    }

    /// Record the original data the response was built from.
    pub fn with_original(mut self, original: Value) -> Self {
        self.original = Some(original);
        self
    }

    /// Attach a typed extension (the rendered view, the matched route, ...).
    pub fn set_extension<T: Send + Sync + 'static>(&mut self, value: Arc<T>) {
        self.extensions.insert(TypeId::of::<T>(), value);
    }

    /// Get a typed extension.
    pub fn extension<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.extensions
            .get(&TypeId::of::<T>())
            .cloned()
            .and_then(|any| any.downcast::<T>().ok())
    }

    // ------------------------------------------------------------------
    // Inspection
    // ------------------------------------------------------------------

    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The status code as a number.
    pub fn status_code(&self) -> u16 {
        self.status.as_u16()
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }

    /// Get a header value.
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    }

    pub fn body(&self) -> &Body {
        &self.body
    }

    /// Take the body out of the response.
    pub fn take_body(&mut self) -> Body {
        std::mem::replace(&mut self.body, Body::Empty)
    }

    /// The buffered content (empty for streams).
    pub fn content(&self) -> &[u8] {
        match &self.body {
            Body::Bytes(bytes) => bytes,
            _ => &[],
        }
    }

    /// The buffered content as a string.
    pub fn content_string(&self) -> String {
        String::from_utf8_lossy(self.content()).into_owned()
    }

    /// The cookies attached to the response.
    pub fn cookies(&self) -> &[Cookie] {
        &self.cookies
    }

    /// Mutable access to the attached cookies (used by `EncryptCookies`).
    pub fn cookies_mut(&mut self) -> &mut Vec<Cookie> {
        &mut self.cookies
    }

    /// Get an attached cookie by name.
    pub fn get_cookie(&self, name: &str) -> Option<&Cookie> {
        self.cookies.iter().rev().find(|c| c.name == name)
    }

    /// Data to flash to the session.
    pub fn flashed(&self) -> &[(String, Value)] {
        &self.flash
    }

    /// Take the flash data, input, and errors so the session can store them.
    #[allow(clippy::type_complexity)]
    pub fn take_session_data(&mut self) -> (Vec<(String, Value)>, Option<Value>, Option<(String, MessageBag)>) {
        (
            std::mem::take(&mut self.flash),
            self.flash_input.take(),
            self.errors.take(),
        )
    }

    /// Input flashed with `with_input`.
    pub fn flashed_input(&self) -> Option<&Value> {
        self.flash_input.as_ref()
    }

    /// Errors flashed with `with_errors`.
    pub fn flashed_errors(&self) -> Option<&(String, MessageBag)> {
        self.errors.as_ref()
    }

    /// The error that produced this response, if any.
    pub fn exception(&self) -> Option<&Arc<Error>> {
        self.exception.as_ref()
    }

    /// The original data (the JSON value for JSON responses).
    pub fn original(&self) -> Option<&Value> {
        self.original.as_ref()
    }

    /// Decode the body as JSON.
    pub fn json_body(&self) -> Value {
        serde_json::from_slice(self.content()).unwrap_or(Value::Null)
    }

    pub fn is_successful(&self) -> bool {
        self.status.is_success()
    }

    pub fn is_ok(&self) -> bool {
        self.status == StatusCode::OK
    }

    pub fn is_redirect(&self) -> bool {
        matches!(self.status.as_u16(), 201 | 301 | 302 | 303 | 307 | 308) && self.headers.contains_key("location")
    }

    pub fn is_redirection(&self) -> bool {
        self.status.is_redirection()
    }

    pub fn is_client_error(&self) -> bool {
        self.status.is_client_error()
    }

    pub fn is_server_error(&self) -> bool {
        self.status.is_server_error()
    }

    pub fn is_not_found(&self) -> bool {
        self.status == StatusCode::NOT_FOUND
    }

    pub fn is_forbidden(&self) -> bool {
        self.status == StatusCode::FORBIDDEN
    }

    /// The redirect target, if this is a redirect.
    pub fn target_url(&self) -> Option<String> {
        self.header("location")
    }

    // ------------------------------------------------------------------
    // Conversion
    // ------------------------------------------------------------------

    /// Read the whole body — buffered or streamed — into memory.
    pub async fn into_bytes(mut self) -> Result<Bytes, Error> {
        match self.take_body() {
            Body::Empty => Ok(Bytes::new()),
            Body::Bytes(bytes) => Ok(bytes),
            Body::Stream(stream) => {
                use futures::StreamExt;
                let mut stream = stream.into_inner();
                let mut buffer = Vec::new();
                while let Some(chunk) = stream.next().await {
                    buffer.extend_from_slice(&chunk?);
                }
                Ok(Bytes::from(buffer))
            }
        }
    }

    /// Read the whole body — buffered or streamed — as a string.
    pub async fn into_content_string(self) -> Result<String, Error> {
        let bytes = self.into_bytes().await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Convert into a `hyper` response, rendering cookies into headers.
    pub fn into_hyper(mut self) -> hyper::Response<HyperBody> {
        use http_body_util::BodyExt;

        for cookie in &self.cookies {
            if let Ok(value) = HeaderValue::try_from(cookie.to_header_value()) {
                self.headers.append(http::header::SET_COOKIE, value);
            }
        }

        let body: HyperBody = match self.body {
            Body::Empty => http_body_util::Full::new(Bytes::new())
                .map_err(|never| match never {})
                .boxed_unsync(),
            Body::Bytes(bytes) => http_body_util::Full::new(bytes)
                .map_err(|never| match never {})
                .boxed_unsync(),
            Body::Stream(stream) => {
                use futures::StreamExt;
                let frames = stream.into_inner().map(|chunk| {
                    chunk
                        .map(hyper::body::Frame::data)
                        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })
                });
                BodyExt::boxed_unsync(http_body_util::StreamBody::new(frames))
            }
        };

        let mut response = hyper::Response::new(body);
        *response.status_mut() = self.status;
        *response.headers_mut() = self.headers;
        response
    }
}

/// The body type handed to hyper.
pub type HyperBody =
    http_body_util::combinators::UnsyncBoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

/// The response factory returned by [`response`].
#[derive(Clone, Copy, Debug, Default)]
pub struct ResponseFactory;

impl ResponseFactory {
    /// Create a new response with content and a status code.
    pub fn make(&self, content: impl Into<Bytes>, status: u16) -> Response {
        Response::make(content, status)
    }

    /// Create a new JSON response.
    pub fn json<T: Serialize + ?Sized>(&self, data: &T) -> Response {
        Response::json(data)
    }

    /// Create a new JSON response with a status code.
    pub fn json_with_status<T: Serialize + ?Sized>(&self, data: &T, status: u16) -> Response {
        Response::json(data).with_status(status)
    }

    /// Create a new JSONP response.
    pub fn jsonp<T: Serialize + ?Sized>(&self, callback: &str, data: &T) -> Response {
        Response::jsonp(callback, data)
    }

    /// Create a new "no content" response.
    pub fn no_content(&self) -> Response {
        Response::no_content()
    }

    /// Create a redirect response.
    pub fn redirect_to(&self, to: impl AsRef<str>) -> Response {
        Response::redirect(to)
    }

    /// Create a streamed response.
    pub fn stream(&self, stream: impl Stream<Item = Result<Bytes, Error>> + Send + 'static) -> Response {
        Response::stream(stream)
    }

    /// Create a file download response.
    pub async fn download(&self, path: impl AsRef<Path>, name: Option<&str>) -> std::io::Result<Response> {
        Response::download(path, name).await
    }

    /// Serve a file directly in the browser.
    pub async fn file(&self, path: impl AsRef<Path>) -> std::io::Result<Response> {
        Response::file(path).await
    }

    /// Serve a file directly in the browser, with extra headers.
    pub async fn file_with_headers<'a>(
        &self,
        path: impl AsRef<Path>,
        headers: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> std::io::Result<Response> {
        Ok(Response::file(path).await?.with_headers(headers))
    }

    /// Create a new "no content" response with a specific status code.
    pub fn no_content_with_status(&self, status: u16) -> Response {
        Response::no_content().with_status(status)
    }

    /// Create a new Markdown response.
    pub fn markdown(&self, content: impl Into<Bytes>) -> Response {
        Response::markdown(content)
    }

    /// Create a download response from in-memory content.
    pub fn download_content(&self, content: impl Into<Bytes>, name: &str) -> Response {
        Response::download_content(content, name)
    }

    /// Stream a download to the browser.
    pub fn stream_download(
        &self,
        stream: impl Stream<Item = Result<Bytes, Error>> + Send + 'static,
        name: Option<&str>,
    ) -> Response {
        Response::stream_download(stream, name)
    }

    /// Stream JSON to the client as it's produced.
    pub fn stream_json(&self, data: impl Into<StreamedJson>) -> Response {
        Response::stream_json(data)
    }

    /// Stream Server-Sent Events to the client.
    pub fn event_stream<S, T>(&self, events: S) -> Response
    where
        S: Stream<Item = T> + Send + 'static,
        T: Into<StreamedEvent>,
    {
        Response::event_stream(events)
    }

    /// Stream Server-Sent Events, closing with the given event (or nothing).
    pub fn event_stream_ending_with<S, T>(&self, events: S, end_stream_with: Option<StreamedEvent>) -> Response
    where
        S: Stream<Item = T> + Send + 'static,
        T: Into<StreamedEvent>,
    {
        Response::event_stream_ending_with(events, end_stream_with)
    }
}

/// Get a response factory.
///
/// ```
/// use illuminate_http::response;
///
/// let hello = response().make("Hello World", 200);
/// assert_eq!(hello.content_string(), "Hello World");
///
/// let json = response().json(&vec![1, 2, 3]);
/// assert_eq!(json.content_string(), "[1,2,3]");
/// ```
pub fn response() -> ResponseFactory {
    ResponseFactory
}

/// Build a `Content-Disposition` header value the way Symfony's
/// `HeaderUtils::makeDisposition` does: the name is quoted only when it
/// needs to be, and names that aren't plain ASCII get an ASCII fallback
/// plus a UTF-8 `filename*` parameter. Path separators are never allowed,
/// so only the last segment of a path is used.
///
/// ```
/// use illuminate_http::response::make_disposition;
///
/// assert_eq!(make_disposition("attachment", "report.pdf"), "attachment; filename=report.pdf");
/// assert_eq!(make_disposition("inline", "Q1 report.pdf"), "inline; filename=\"Q1 report.pdf\"");
/// assert_eq!(
///     make_disposition("attachment", "résumé.pdf"),
///     "attachment; filename=resume.pdf; filename*=utf-8''r%C3%A9sum%C3%A9.pdf"
/// );
/// ```
pub fn make_disposition(disposition: &str, filename: &str) -> String {
    let filename = filename.rsplit(['/', '\\']).next().unwrap_or_default();
    let fallback = illuminate_support::Str::ascii(filename).replace('%', "");
    let fallback: String = fallback
        .chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '_' })
        .collect();
    let fallback = if fallback.is_empty() {
        "_".repeat(filename.chars().count())
    } else {
        fallback
    };
    let mut header = format!("{disposition}; filename={}", quote_header_value(&fallback));
    if fallback != filename {
        const RAW_URL: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
            .remove(b'-')
            .remove(b'_')
            .remove(b'.')
            .remove(b'~');
        header.push_str(&format!(
            "; filename*=utf-8''{}",
            percent_encoding::utf8_percent_encode(filename, RAW_URL)
        ));
    }
    header
}

/// Quote a header parameter value when it isn't a plain token.
fn quote_header_value(value: &str) -> String {
    let token = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*.^_`|~-".contains(c));
    if token {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Determine if a JSONP callback is a valid JavaScript identifier path
/// (`callback`, `app.handlers.ready`, `jQuery123["done"]`).
fn is_valid_callback(callback: &str) -> bool {
    const RESERVED: &[&str] = &[
        "break", "do", "instanceof", "typeof", "case", "else", "new", "var", "catch", "finally",
        "return", "void", "continue", "for", "switch", "while", "debugger", "function", "this",
        "with", "default", "if", "throw", "delete", "in", "try", "class", "enum", "extends",
        "super", "const", "export", "import", "implements", "let", "private", "public", "yield",
        "interface", "package", "protected", "static", "null", "true", "false",
    ];
    callback.split('.').all(|part| !RESERVED.contains(&part) && is_valid_callback_part(part))
}

fn is_valid_callback_part(part: &str) -> bool {
    let mut chars = part.chars().peekable();
    match chars.next() {
        Some(c) if c == '$' || c == '_' || c.is_alphabetic() => {}
        _ => return false,
    }
    while let Some(&c) = chars.peek() {
        if c == '$' || c == '_' || c.is_alphanumeric() || c == '\u{200C}' || c == '\u{200D}' {
            chars.next();
        } else {
            break;
        }
    }
    // Any number of `[0]`, `["key"]` or `['key']` accessors.
    while let Some(open) = chars.next() {
        if open != '[' {
            return false;
        }
        match chars.next() {
            Some(quote @ ('"' | '\'')) => {
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '\\' {
                        chars.next();
                    } else if c == quote {
                        closed = true;
                        break;
                    }
                }
                if !closed || chars.next() != Some(']') {
                    return false;
                }
            }
            Some(digit) if digit.is_ascii_digit() => loop {
                match chars.next() {
                    Some(c) if c.is_ascii_digit() => {}
                    Some(']') => break,
                    _ => return false,
                }
            },
            _ => return false,
        }
    }
    true
}

/// The scheme, host and explicit port of a URL.
#[derive(Debug, PartialEq)]
struct Origin {
    scheme: Option<String>,
    host: Option<String>,
    port: Option<u16>,
}

impl Origin {
    fn parse(url: &str) -> Self {
        let (scheme, rest) = match url.split_once("://") {
            Some((scheme, rest)) => (Some(scheme.to_ascii_lowercase()), rest),
            None => match url.strip_prefix("//") {
                Some(rest) => (None, rest),
                None => {
                    return Self {
                        scheme: None,
                        host: None,
                        port: None,
                    };
                }
            },
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let authority = authority.rsplit('@').next().unwrap_or_default();
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(']') || authority.starts_with('[') => {
                match port.parse() {
                    Ok(port) => (host.to_string(), Some(port)),
                    Err(_) => (authority.to_string(), None),
                }
            }
            _ => (authority.to_string(), None),
        };
        Self {
            scheme,
            host: (!host.is_empty()).then(|| host.to_ascii_lowercase()),
            port,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::with_request_sync;
    use crate::request::Request;
    use illuminate_support::json;

    #[test]
    fn jsonp_callbacks_are_validated_like_symfony() {
        for valid in ["callback", "$", "_jQuery123", "app.handlers.ready", "a[0]", "a[\"b\"]", "a['b'][1]", "ünïcode"] {
            assert!(is_valid_callback(valid), "{valid}");
        }
        for invalid in ["", "1abc", "alert(1)", "a b", "a.", "function", "a.true", "a[", "a[b]", "a[\"b]", "a;b"] {
            assert!(!is_valid_callback(invalid), "{invalid}");
        }
        let response = Response::json(&json!([1])).with_callback(None).unwrap();
        assert_eq!(response.content_string(), "[1]");
        let error = Response::json(&json!([1])).with_callback(Some("new")).unwrap_err();
        assert_eq!(error.to_string(), "The callback name is not valid.");
    }

    #[test]
    fn dispositions_match_symfony() {
        assert_eq!(make_disposition("attachment", "../../etc/passwd"), "attachment; filename=passwd");
        assert_eq!(make_disposition("attachment", "50%.txt"), "attachment; filename=50.txt; filename*=utf-8''50%25.txt");
        assert_eq!(make_disposition("attachment", "say \"hi\".txt"), "attachment; filename=\"say \\\"hi\\\".txt\"");
        let header = make_disposition("attachment", "日本.txt");
        assert!(header.is_ascii());
        assert!(header.ends_with("; filename*=utf-8''%E6%97%A5%E6%9C%AC.txt"), "{header}");
    }

    #[test]
    fn cookies_can_be_attached_and_expired_in_bulk() {
        let response = Response::new("")
            .with_cookies([Cookie::new("a", "1"), Cookie::new("b", "2")])
            .without_cookies(["c", "d"]);
        let names: Vec<&str> = response.cookies().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c", "d"]);
        assert!(response.get_cookie("c").unwrap().minutes.unwrap() < 0);
    }

    #[test]
    fn redirects_flash_parts_of_the_current_request_input() {
        let request = Request::create_with("/", "POST", json!({"name": "Taylor", "email": "t@laravel.com"}), Default::default());
        with_request_sync(request, || {
            let response = Response::redirect("/").only_input(&["name"]);
            assert_eq!(response.flashed_input(), Some(&json!({"name": "Taylor"})));
            let response = Response::redirect("/").with_request_input();
            assert_eq!(response.flashed_input(), Some(&json!({"name": "Taylor", "email": "t@laravel.com"})));
        });
        // Outside of a request there's no input to flash.
        assert_eq!(Response::redirect("/").only_input(&["name"]).flashed_input(), Some(&json!({})));
    }

    #[test]
    fn fragments_are_replaced_and_removed() {
        let response = Response::redirect("/docs#old").with_fragment("#new");
        assert_eq!(response.target_url().unwrap(), "/docs#new");
        let response = Response::redirect("/docs").with_fragment("section");
        assert_eq!(response.target_url().unwrap(), "/docs#section");
    }

    #[test]
    fn same_origin_is_enforced_by_host_scheme_and_port() {
        let request = Request::create("https://example.com/login", "POST");
        with_request_sync(request, || {
            let check = |target: &str, scheme: bool, port: bool| {
                Response::redirect(target)
                    .enforce_same_origin("/fallback", scheme, port)
                    .target_url()
                    .unwrap()
            };
            assert_eq!(check("https://example.com/home", true, true), "https://example.com/home");
            assert_eq!(check("http://example.com/home", true, true), "/fallback");
            assert_eq!(check("http://example.com/home", false, true), "http://example.com/home");
            assert_eq!(check("https://example.com:8443/home", true, true), "/fallback");
            assert_eq!(check("https://example.com:8443/home", true, false), "https://example.com:8443/home");
            assert_eq!(check("https://EXAMPLE.com/", true, true), "https://EXAMPLE.com/");
            assert_eq!(check("https://example.com.evil.test/", true, true), "/fallback");
            assert_eq!(check("https://user@evil.test/", true, true), "/fallback");
            assert_eq!(check("/relative", true, true), "/fallback");
        });
    }

    #[tokio::test]
    async fn event_streams_render_laravels_format() {
        let events = futures::stream::iter(vec![json!("plain"), json!(42), json!(true), json!({"a": "<b>"})]);
        let response = Response::event_stream_ending_with(events, Some(StreamedEvent::new("done", "bye")));
        assert_eq!(response.header("cache-control").unwrap(), "no-cache");
        assert_eq!(response.header("x-accel-buffering").unwrap(), "no");
        assert_eq!(
            response.into_content_string().await.unwrap(),
            "event: update\ndata: plain\n\nevent: update\ndata: 42\n\nevent: update\ndata: true\n\nevent: update\ndata: {\"a\":\"\\u003Cb\\u003E\"}\n\nevent: done\ndata: bye\n\n"
        );
        let empty = Response::event_stream_ending_with(futures::stream::iter(Vec::<String>::new()), Some(StreamedEvent::from("")));
        assert_eq!(empty.into_content_string().await.unwrap(), "");
    }

    #[tokio::test]
    async fn json_streams_nest_and_stand_alone() {
        let rows = futures::stream::iter(vec![1, 2, 3]);
        let none = futures::stream::iter(Vec::<u8>::new());
        let response = Response::stream_json(
            StreamedJson::new(json!({"a": 1, "data": {"keep": true}}))
                .with_stream("data.rows", rows)
                .with_stream("empty", none),
        );
        assert_eq!(
            response.into_content_string().await.unwrap(),
            r#"{"a":1,"data":{"keep":true,"rows":[1,2,3]},"empty":[]}"#
        );

        let response = Response::stream_json(StreamedJson::items(futures::stream::iter(vec!["a", "b"])));
        assert_eq!(response.into_content_string().await.unwrap(), r#"["a","b"]"#);

        let response = Response::stream_json(json!({"plain": true}));
        assert_eq!(response.into_content_string().await.unwrap(), r#"{"plain":true}"#);
    }

    #[tokio::test]
    async fn stream_downloads_set_the_disposition() {
        let chunks = futures::stream::iter(vec![Ok(Bytes::from("a")), Ok(Bytes::from("b"))]);
        let response = Response::stream_download_with_disposition(chunks, Some("notes.txt"), "inline");
        assert_eq!(response.header("content-disposition").unwrap(), "inline; filename=notes.txt");
        assert_eq!(response.header("content-type").unwrap(), "text/plain");
        assert_eq!(response.into_content_string().await.unwrap(), "ab");
    }

    #[test]
    fn origins_are_parsed() {
        assert_eq!(
            Origin::parse("https://user:pass@Example.com:8080/path?q#f"),
            Origin { scheme: Some("https".into()), host: Some("example.com".into()), port: Some(8080) }
        );
        assert_eq!(Origin::parse("http://[::1]:8000/").host.as_deref(), Some("[::1]"));
        assert_eq!(Origin::parse("http://[::1]/").port, None);
    }
}
