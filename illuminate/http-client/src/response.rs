//! The response returned by the HTTP client.

use std::borrow::Cow;
use std::fmt;
use std::ops::Index;
use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Version};
use illuminate_support::{
    Collection, Conditionable, Fluent, Map, Result, Tappable, Value, data_get, data_get_or,
};
use serde::de::DeserializeOwned;

use crate::cookies::CookieJar;
use crate::exceptions::{RequestException, Truncation};

/// An HTTP response received by the client.
///
/// Responses are cheap to clone, and offer a variety of methods for
/// inspecting the status, headers and body. The JSON body may also be
/// indexed directly:
///
/// ```
/// use illuminate_http_client::Http;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// Http::fake_urls([("example.com/*", Http::response(json!({"name": "Taylor", "roles": ["admin"]}), 200, &[]))]);
///
/// let response = Http::get("https://example.com/users/1").await?;
///
/// assert!(response.ok());
/// assert_eq!(response["name"], "Taylor");
/// assert_eq!(response.json_path("roles.0"), "admin");
/// assert_eq!(response.header("Content-Type"), "application/json");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Clone)]
pub struct Response {
    inner: Arc<Inner>,
}

#[derive(Clone)]
struct Inner {
    status: StatusCode,
    version: Version,
    headers: HeaderMap,
    body: Bytes,
    effective_uri: Option<String>,
    cookies: CookieJar,
    truncation: Option<Truncation>,
    decoded: OnceLock<Value>,
}

impl Response {
    /// Create a new response.
    ///
    /// Invalid status codes fall back to `500 Internal Server Error`.
    pub fn new(status: u16, headers: HeaderMap, body: impl Into<Bytes>) -> Self {
        let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        Self::from_parts(status, Version::HTTP_11, headers, body.into())
    }

    pub(crate) fn from_parts(
        status: StatusCode,
        version: Version,
        headers: HeaderMap,
        body: Bytes,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                status,
                version,
                headers,
                body,
                effective_uri: None,
                cookies: CookieJar::new(),
                truncation: None,
                decoded: OnceLock::new(),
            }),
        }
    }

    /// The response's state, copied first if it is shared with a clone.
    fn inner_mut(&mut self) -> &mut Inner {
        Arc::make_mut(&mut self.inner)
    }

    // ------------------------------------------------------------------
    // The body
    // ------------------------------------------------------------------

    /// Get the body of the response.
    pub fn body(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.inner.body)
    }

    /// Get the raw bytes of the response body.
    pub fn bytes(&self) -> &Bytes {
        &self.inner.body
    }

    /// Get the JSON decoded body of the response (`Value::Null` when the
    /// body isn't valid JSON).
    pub fn json(&self) -> Value {
        self.decoded().clone()
    }

    /// Get a value from the JSON decoded body using "dot" notation.
    ///
    /// ```
    /// # use illuminate_http_client::Response;
    /// # use illuminate_support::json;
    /// let response = Response::new(200, Default::default(), r#"{"data": [{"name": "Taylor"}]}"#);
    ///
    /// assert_eq!(response.json_path("data.0.name"), "Taylor");
    /// assert_eq!(response.json_path("data.*.name"), json!(["Taylor"]));
    /// assert!(response.json_path("data.1.name").is_null());
    /// ```
    pub fn json_path(&self, key: &str) -> Value {
        data_get(self.decoded(), key)
    }

    /// Get a value from the JSON decoded body, or the given default.
    pub fn json_path_or(&self, key: &str, default: impl Into<Value>) -> Value {
        data_get_or(self.decoded(), key, default)
    }

    /// Deserialize the JSON body into the given type.
    ///
    /// ```
    /// # use illuminate_http_client::Response;
    /// #[derive(serde::Deserialize)]
    /// struct User {
    ///     name: String,
    /// }
    ///
    /// let response = Response::new(200, Default::default(), r#"{"name": "Taylor"}"#);
    /// let user: User = response.json_as()?;
    ///
    /// assert_eq!(user.name, "Taylor");
    /// # Ok::<(), illuminate_support::Error>(())
    /// ```
    pub fn json_as<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_slice(&self.inner.body)?)
    }

    /// Get the JSON decoded body as an object, if it is one.
    pub fn object(&self) -> Option<Map<String, Value>> {
        self.decoded().as_object().cloned()
    }

    /// Get the JSON decoded body as a collection.
    ///
    /// Arrays become their items and objects become their values.
    pub fn collect(&self) -> Collection<Value> {
        to_collection(self.decoded().clone())
    }

    /// Get a value from the JSON body ("dot" notation) as a collection.
    pub fn collect_path(&self, key: &str) -> Collection<Value> {
        to_collection(self.json_path(key))
    }

    /// Get the JSON decoded body as a [`Fluent`] instance.
    pub fn fluent(&self) -> Fluent {
        Fluent::from(self.decoded().clone())
    }

    fn decoded(&self) -> &Value {
        self.inner
            .decoded
            .get_or_init(|| serde_json::from_slice(&self.inner.body).unwrap_or(Value::Null))
    }

    // ------------------------------------------------------------------
    // Headers & metadata
    // ------------------------------------------------------------------

    /// Get a header from the response (multiple values are joined by `, `,
    /// and missing headers are an empty string).
    pub fn header(&self, name: &str) -> String {
        self.inner
            .headers
            .get_all(name)
            .iter()
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Determine if the response has the given header.
    pub fn has_header(&self, name: &str) -> bool {
        self.inner.headers.contains_key(name)
    }

    /// Get the headers from the response.
    pub fn headers(&self) -> &HeaderMap {
        &self.inner.headers
    }

    /// Get the status code of the response.
    pub fn status(&self) -> u16 {
        self.inner.status.as_u16()
    }

    /// Get the status code of the response as a [`StatusCode`].
    pub fn status_code(&self) -> StatusCode {
        self.inner.status
    }

    /// Get the reason phrase of the response.
    pub fn reason(&self) -> &'static str {
        self.inner.status.canonical_reason().unwrap_or("")
    }

    /// Get the HTTP version of the response.
    pub fn version(&self) -> Version {
        self.inner.version
    }

    /// Get the effective URI of the response: the last URL requested,
    /// after any redirects were followed.
    pub fn effective_uri(&self) -> Option<&str> {
        self.inner.effective_uri.as_deref()
    }

    /// Get the cookies of the transfer.
    pub fn cookies(&self) -> &CookieJar {
        &self.inner.cookies
    }

    // ------------------------------------------------------------------
    // Status codes
    // ------------------------------------------------------------------

    /// Determine if the request was successful (`2xx`).
    pub fn successful(&self) -> bool {
        self.inner.status.is_success()
    }

    /// Determine if the response was a redirect (`3xx`).
    pub fn redirect(&self) -> bool {
        self.inner.status.is_redirection()
    }

    /// Determine if the response indicates a client or server error occurred.
    pub fn failed(&self) -> bool {
        self.server_error() || self.client_error()
    }

    /// Determine if the response indicates a client error occurred (`4xx`).
    pub fn client_error(&self) -> bool {
        self.inner.status.is_client_error()
    }

    /// Determine if the response indicates a server error occurred (`5xx`).
    pub fn server_error(&self) -> bool {
        self.inner.status.as_u16() >= 500
    }

    /// Determine if the response code was `200 OK`.
    pub fn ok(&self) -> bool {
        self.status() == 200
    }

    /// Determine if the response code was `201 Created`.
    pub fn created(&self) -> bool {
        self.status() == 201
    }

    /// Determine if the response code was `202 Accepted`.
    pub fn accepted(&self) -> bool {
        self.status() == 202
    }

    /// Determine if the response code was `204 No Content` with an empty body.
    pub fn no_content(&self) -> bool {
        self.status() == 204 && self.inner.body.is_empty()
    }

    /// Determine if the response code was `301 Moved Permanently`.
    pub fn moved_permanently(&self) -> bool {
        self.status() == 301
    }

    /// Determine if the response code was `302 Found`.
    pub fn found(&self) -> bool {
        self.status() == 302
    }

    /// Determine if the response code was `304 Not Modified`.
    pub fn not_modified(&self) -> bool {
        self.status() == 304
    }

    /// Determine if the response code was `400 Bad Request`.
    pub fn bad_request(&self) -> bool {
        self.status() == 400
    }

    /// Determine if the response code was `401 Unauthorized`.
    pub fn unauthorized(&self) -> bool {
        self.status() == 401
    }

    /// Determine if the response code was `402 Payment Required`.
    pub fn payment_required(&self) -> bool {
        self.status() == 402
    }

    /// Determine if the response code was `403 Forbidden`.
    pub fn forbidden(&self) -> bool {
        self.status() == 403
    }

    /// Determine if the response code was `404 Not Found`.
    pub fn not_found(&self) -> bool {
        self.status() == 404
    }

    /// Determine if the response code was `408 Request Timeout`.
    pub fn request_timeout(&self) -> bool {
        self.status() == 408
    }

    /// Determine if the response code was `409 Conflict`.
    pub fn conflict(&self) -> bool {
        self.status() == 409
    }

    /// Determine if the response code was `422 Unprocessable Content`.
    pub fn unprocessable_content(&self) -> bool {
        self.status() == 422
    }

    /// Alias of [`Response::unprocessable_content`].
    pub fn unprocessable_entity(&self) -> bool {
        self.unprocessable_content()
    }

    /// Determine if the response code was `429 Too Many Requests`.
    pub fn too_many_requests(&self) -> bool {
        self.status() == 429
    }

    // ------------------------------------------------------------------
    // Errors
    // ------------------------------------------------------------------

    /// Execute the given callback if there was a server or client error.
    pub fn on_error(&self, callback: impl FnOnce(&Response)) -> &Self {
        if self.failed() {
            callback(self);
        }
        self
    }

    /// Create an exception if a server or client error occurred.
    pub fn to_exception(&self) -> Option<RequestException> {
        self.failed().then(|| self.exception())
    }

    fn exception(&self) -> RequestException {
        RequestException::with_truncation(self.clone(), self.inner.truncation)
    }

    /// Return an error if a server or client error occurred.
    ///
    /// ```
    /// # use illuminate_http_client::{RequestException, Response};
    /// let response = Response::new(404, Default::default(), "Not Found");
    /// let error = response.throw().unwrap_err();
    ///
    /// assert_eq!(error.to_string(), "HTTP request returned status code 404:\nNot Found\n");
    /// assert_eq!(error.response.status(), 404);
    /// ```
    pub fn throw(&self) -> Result<&Self, RequestException> {
        self.throw_with(|_, _| {})
    }

    /// Return an error if a server or client error occurred, invoking the
    /// callback with the response and exception first.
    pub fn throw_with(
        &self,
        callback: impl FnOnce(&Response, &RequestException),
    ) -> Result<&Self, RequestException> {
        match self.to_exception() {
            Some(exception) => {
                callback(self, &exception);
                Err(exception)
            }
            None => Ok(self),
        }
    }

    /// Return an error if an error occurred and the given condition is true.
    pub fn throw_if(&self, condition: bool) -> Result<&Self, RequestException> {
        if condition { self.throw() } else { Ok(self) }
    }

    /// Return an error if an error occurred and the given callback returns true.
    pub fn throw_if_fn(
        &self,
        condition: impl FnOnce(&Response) -> bool,
    ) -> Result<&Self, RequestException> {
        self.throw_if(condition(self))
    }

    /// Return an error if an error occurred and the given condition is false.
    pub fn throw_unless(&self, condition: bool) -> Result<&Self, RequestException> {
        self.throw_if(!condition)
    }

    /// Return an error if an error occurred and the given callback returns false.
    pub fn throw_unless_fn(
        &self,
        condition: impl FnOnce(&Response) -> bool,
    ) -> Result<&Self, RequestException> {
        self.throw_if(!condition(self))
    }

    /// Return an error if the response has the given status code.
    pub fn throw_if_status(&self, status: u16) -> Result<&Self, RequestException> {
        if self.status() == status {
            Err(self.exception())
        } else {
            Ok(self)
        }
    }

    /// Return an error if the callback, given the status code, returns true.
    pub fn throw_if_status_fn(
        &self,
        callback: impl FnOnce(u16, &Response) -> bool,
    ) -> Result<&Self, RequestException> {
        if callback(self.status(), self) {
            Err(self.exception())
        } else {
            Ok(self)
        }
    }

    /// Return an error unless the response has the given status code.
    pub fn throw_unless_status(&self, status: u16) -> Result<&Self, RequestException> {
        if self.status() == status {
            Ok(self)
        } else {
            Err(self.exception())
        }
    }

    /// Return an error unless the callback, given the status code, returns true.
    pub fn throw_unless_status_fn(
        &self,
        callback: impl FnOnce(u16, &Response) -> bool,
    ) -> Result<&Self, RequestException> {
        if callback(self.status(), self) {
            Ok(self)
        } else {
            Err(self.exception())
        }
    }

    /// Return an error if the response has a `4xx` status code.
    pub fn throw_if_client_error(&self) -> Result<&Self, RequestException> {
        if self.client_error() {
            self.throw()
        } else {
            Ok(self)
        }
    }

    /// Return an error if the response has a `5xx` status code.
    pub fn throw_if_server_error(&self) -> Result<&Self, RequestException> {
        if self.server_error() {
            self.throw()
        } else {
            Ok(self)
        }
    }

    /// Truncate the body in exception messages at the given length.
    pub fn truncate_exceptions_at(&mut self, length: usize) -> &mut Self {
        self.inner_mut().truncation = Some(Truncation::At(length.max(1)));
        self
    }

    /// Include the whole response in exception messages.
    pub fn dont_truncate_exceptions(&mut self) -> &mut Self {
        self.inner_mut().truncation = Some(Truncation::Never);
        self
    }

    pub(crate) fn truncation(&self) -> Option<Truncation> {
        self.inner.truncation
    }

    pub(crate) fn set_truncation(&mut self, truncation: Option<Truncation>) {
        if truncation.is_some() {
            self.inner_mut().truncation = truncation;
        }
    }

    pub(crate) fn set_effective_uri(&mut self, uri: impl Into<String>) {
        self.inner_mut().effective_uri = Some(uri.into());
    }

    pub(crate) fn set_cookies(&mut self, cookies: CookieJar) {
        self.inner_mut().cookies = cookies;
    }

    // ------------------------------------------------------------------
    // Building modified responses (handy in response middleware)
    // ------------------------------------------------------------------

    /// Return a copy of the response with the given header set.
    ///
    /// Invalid header names or values are ignored.
    pub fn with_header(mut self, name: &str, value: impl AsRef<str>) -> Self {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_bytes(value.as_ref().as_bytes()),
        ) {
            self.inner_mut().headers.insert(name, value);
        }
        self
    }

    /// Return a copy of the response without the given header.
    pub fn without_header(mut self, name: &str) -> Self {
        self.inner_mut().headers.remove(name);
        self
    }

    /// Return a copy of the response with the given status code.
    pub fn with_status(mut self, status: u16) -> Self {
        self.inner_mut().status = StatusCode::from_u16(status).unwrap_or(self.inner.status);
        self
    }

    /// Return a copy of the response with the given body.
    pub fn with_body(mut self, body: impl Into<Bytes>) -> Self {
        self.inner_mut().body = body.into();
        self.inner_mut().decoded = OnceLock::new();
        self
    }
}

fn to_collection(value: Value) -> Collection<Value> {
    match value {
        Value::Null => Collection::new(),
        Value::Array(items) => Collection::make(items),
        Value::Object(map) => Collection::make(map.into_iter().map(|(_, value)| value)),
        other => Collection::make([other]),
    }
}

impl Conditionable for Response {}
impl Tappable for Response {}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.inner.status)
            .field("headers", &self.inner.headers)
            .field("body", &self.body())
            .field("effective_uri", &self.inner.effective_uri)
            .finish()
    }
}

impl fmt::Display for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.body())
    }
}

impl PartialEq for Response {
    fn eq(&self, other: &Self) -> bool {
        self.inner.status == other.inner.status
            && self.inner.headers == other.inner.headers
            && self.inner.body == other.inner.body
    }
}

impl Index<&str> for Response {
    type Output = Value;

    fn index(&self, key: &str) -> &Value {
        &self.decoded()[key]
    }
}

impl Index<usize> for Response {
    type Output = Value;

    fn index(&self, index: usize) -> &Value {
        &self.decoded()[index]
    }
}

impl From<http::Response<Bytes>> for Response {
    fn from(response: http::Response<Bytes>) -> Self {
        let (parts, body) = response.into_parts();
        Self::from_parts(parts.status, parts.version, parts.headers, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;
    use std::cell::Cell;

    fn json_response(status: u16, body: Value) -> Response {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "application/json".parse().unwrap());
        Response::new(status, headers, body.to_string())
    }

    #[test]
    fn it_decodes_json() {
        let response = json_response(
            200,
            json!({"name": "Taylor", "roles": ["admin", "owner"], "team": {"id": 1}}),
        );

        assert_eq!(response.json()["name"], "Taylor");
        assert_eq!(response["roles"][1], "owner");
        assert_eq!(response.json_path("team.id"), 1);
        assert_eq!(response.json_path_or("team.name", "Laravel"), "Laravel");
        assert_eq!(response.object().unwrap()["name"], "Taylor");
        assert_eq!(response.collect_path("roles").count(), 2);
        assert_eq!(response.collect().count(), 3);
        assert_eq!(response.fluent().get("name"), "Taylor");
        assert!(response["missing"].is_null());
    }

    #[test]
    fn it_handles_arrays_and_invalid_json() {
        let response = json_response(200, json!([{"id": 1}, {"id": 2}]));
        assert_eq!(response[1]["id"], 2);
        assert_eq!(response.collect().count(), 2);
        assert!(response.object().is_none());

        let response = Response::new(200, HeaderMap::new(), "Hello World");
        assert_eq!(response.body(), "Hello World");
        assert_eq!(response.to_string(), "Hello World");
        assert!(response.json().is_null());
        assert!(response.collect().is_empty());
        assert!(response.json_as::<Value>().is_err());
    }

    #[test]
    fn it_determines_status_codes() {
        type Check = fn(&Response) -> bool;

        let cases: Vec<(u16, Check)> = vec![
            (200, Response::ok),
            (201, Response::created),
            (202, Response::accepted),
            (301, Response::moved_permanently),
            (302, Response::found),
            (304, Response::not_modified),
            (400, Response::bad_request),
            (401, Response::unauthorized),
            (402, Response::payment_required),
            (403, Response::forbidden),
            (404, Response::not_found),
            (408, Response::request_timeout),
            (409, Response::conflict),
            (422, Response::unprocessable_entity),
            (429, Response::too_many_requests),
        ];

        for (status, check) in cases {
            assert!(
                check(&Response::new(status, HeaderMap::new(), "")),
                "{status}"
            );
            assert!(
                !check(&Response::new(418, HeaderMap::new(), "")),
                "{status}"
            );
        }

        assert!(Response::new(204, HeaderMap::new(), "").no_content());
        assert!(!Response::new(204, HeaderMap::new(), "x").no_content());

        let redirect = Response::new(302, HeaderMap::new(), "");
        assert!(redirect.redirect() && !redirect.successful() && !redirect.failed());

        let client = Response::new(404, HeaderMap::new(), "");
        assert!(client.client_error() && client.failed() && !client.server_error());

        let server = Response::new(503, HeaderMap::new(), "");
        assert!(server.server_error() && server.failed() && !server.client_error());
        assert_eq!(server.reason(), "Service Unavailable");
        assert_eq!(server.status_code(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn it_reads_headers() {
        let mut headers = HeaderMap::new();
        headers.append("x-many", "a".parse().unwrap());
        headers.append("x-many", "b".parse().unwrap());
        let response = Response::new(200, headers, "");

        assert_eq!(response.header("X-Many"), "a, b");
        assert_eq!(response.header("X-Missing"), "");
        assert!(response.has_header("x-many"));
        assert_eq!(response.headers().len(), 2);

        let response = response
            .with_header("X-Added", "yes")
            .without_header("x-many");
        assert_eq!(response.header("x-added"), "yes");
        assert!(!response.has_header("x-many"));
    }

    #[test]
    fn it_throws_for_errors() {
        let ok = Response::new(200, HeaderMap::new(), "fine");
        assert!(ok.throw().is_ok());
        assert!(ok.to_exception().is_none());
        assert!(ok.throw_if_status(200).is_err());
        assert!(ok.throw_unless_status(200).is_ok());
        assert!(ok.throw_unless_status(201).is_err());

        let failed = Response::new(500, HeaderMap::new(), "Whoops");
        assert!(failed.throw().is_err());
        assert!(failed.throw_if(false).is_ok());
        assert!(failed.throw_if(true).is_err());
        assert!(failed.throw_if_fn(|r| r.status() == 500).is_err());
        assert!(failed.throw_unless(true).is_ok());
        assert!(failed.throw_unless_fn(|_| false).is_err());
        assert!(
            failed
                .throw_if_status_fn(|status, _| status >= 500)
                .is_err()
        );
        assert!(
            failed
                .throw_unless_status_fn(|status, _| status == 500)
                .is_ok()
        );
        assert!(failed.throw_if_client_error().is_ok());
        assert!(failed.throw_if_server_error().is_err());

        let called = Cell::new(false);
        let error = failed
            .throw_with(|response, exception| {
                assert_eq!(response.status(), 500);
                assert_eq!(exception.response.body(), "Whoops");
                called.set(true);
            })
            .unwrap_err();
        assert!(called.get());
        assert_eq!(
            error.to_string(),
            "HTTP request returned status code 500:\nWhoops\n"
        );

        let errored = Cell::new(0);
        failed.on_error(|_| errored.set(errored.get() + 1));
        ok.on_error(|_| errored.set(errored.get() + 1));
        assert_eq!(errored.get(), 1);
    }

    #[test]
    fn it_customizes_exception_truncation() {
        let mut response = Response::new(500, HeaderMap::new(), "abcdefghij");
        response.truncate_exceptions_at(3);
        assert_eq!(
            response.throw().unwrap_err().message(),
            "HTTP request returned status code 500:\nabc (truncated...)\n"
        );

        response.dont_truncate_exceptions();
        assert!(
            response
                .throw()
                .unwrap_err()
                .message()
                .contains("HTTP/1.1 500 Internal Server Error")
        );
    }

    #[test]
    fn it_converts_from_http_responses() {
        let response: Response = http::Response::builder()
            .status(201)
            .header("x-id", "1")
            .body(Bytes::from_static(b"{}"))
            .unwrap()
            .into();

        assert!(response.created());
        assert_eq!(response.header("x-id"), "1");
        assert_eq!(response.clone(), response);
        assert!(format!("{response:?}").contains("201"));
        assert_eq!(
            response.with_status(202).with_body("[]").collect().count(),
            0
        );
    }
}
