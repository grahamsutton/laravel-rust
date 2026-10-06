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

/// A streamed body chunk.
pub type BodyStream = Pin<Box<dyn Stream<Item = Result<Bytes, Error>> + Send>>;

/// The body of a response.
pub enum Body {
    /// No body at all.
    Empty,
    /// A fully buffered body.
    Bytes(Bytes),
    /// A body streamed to the client chunk by chunk.
    Stream(BodyStream),
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
        response.body = Body::Stream(Box::pin(stream));
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
        Ok(response.with_header(
            "content-disposition",
            &format!("attachment; filename=\"{}\"", name.replace('"', "")),
        ))
    }

    /// Create a download response from in-memory content.
    pub fn download_content(content: impl Into<Bytes>, name: &str) -> Self {
        let mime = mime_guess::from_path(name).first_or_octet_stream();
        Self::new(content)
            .with_header("content-type", mime.essence_str())
            .with_header(
                "content-disposition",
                &format!("attachment; filename=\"{}\"", name.replace('"', "")),
            )
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
    pub fn with_fragment(mut self, fragment: &str) -> Self {
        if let Some(location) = self.header("location") {
            let base = location.split('#').next().unwrap_or_default().to_string();
            self.set_header("location", &format!("{base}#{}", fragment.trim_start_matches('#')));
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
                let frames = stream.map(|chunk| {
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
