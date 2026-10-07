//! The outgoing request, as seen by middleware, fakes and assertions.

use std::borrow::Cow;
use std::fmt;
use std::ops::Index;
use std::sync::OnceLock;

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use illuminate_support::{Map, Uri, Value};

use crate::encoding::{Part, parse_form};

/// An outgoing HTTP request.
///
/// This is what [`Http::assert_sent`](crate::Http::assert_sent), fake
/// callbacks and middleware receive. Request data may be indexed directly:
///
/// ```
/// use illuminate_http_client::{Http, Request};
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// Http::fake();
///
/// Http::with_headers([("X-First", "foo")])
///     .post("http://example.com/users", json!({"name": "Taylor", "role": "Developer"}))
///     .await?;
///
/// Http::assert_sent(|request: &Request| {
///     request.has_header_value("X-First", "foo")
///         && request.url() == "http://example.com/users"
///         && request["name"] == "Taylor"
///         && request["role"] == "Developer"
/// });
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Clone)]
pub struct Request {
    method: Method,
    url: String,
    headers: HeaderMap,
    body: Bytes,
    data: Value,
    parts: Vec<Part>,
    attributes: Map<String, Value>,
    parsed: OnceLock<Value>,
}

impl Request {
    /// Create a new request.
    ///
    /// Unknown methods fall back to `GET`.
    pub fn new(method: &str, url: impl Into<String>) -> Self {
        Self {
            method: Method::from_bytes(method.to_ascii_uppercase().as_bytes())
                .unwrap_or(Method::GET),
            url: url.into(),
            headers: HeaderMap::new(),
            body: Bytes::new(),
            data: Value::Object(Map::new()),
            parts: Vec::new(),
            attributes: Map::new(),
            parsed: OnceLock::new(),
        }
    }

    pub(crate) fn from_parts(method: Method, url: String, headers: HeaderMap, body: Bytes) -> Self {
        Self {
            method,
            headers,
            body,
            ..Self::new("GET", url)
        }
    }

    // ------------------------------------------------------------------
    // Inspecting the request
    // ------------------------------------------------------------------

    /// Get the request method (`GET`, `POST`, ...).
    pub fn method(&self) -> &str {
        self.method.as_str()
    }

    /// Get the request method as an [`http::Method`].
    pub fn http_method(&self) -> &Method {
        &self.method
    }

    /// Get the URL of the request.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Get the URL of the request as a [`Uri`] instance.
    pub fn uri(&self) -> Uri {
        Uri::of(&self.url)
    }

    /// Determine if the request has the given header.
    pub fn has_header(&self, name: &str) -> bool {
        self.headers.contains_key(name)
    }

    /// Determine if the request has the given header with the given value.
    pub fn has_header_value(&self, name: &str, value: &str) -> bool {
        self.header(name).iter().any(|existing| existing == value)
    }

    /// Determine if the request has all of the given headers (and values).
    pub fn has_headers(&self, headers: &[(&str, &str)]) -> bool {
        headers
            .iter()
            .all(|(name, value)| self.has_header_value(name, value))
    }

    /// Get the values for the given header.
    pub fn header(&self, name: &str) -> Vec<String> {
        self.headers
            .get_all(name)
            .iter()
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
            .collect()
    }

    /// Get the request headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Get the body of the request.
    pub fn body(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// Get the raw bytes of the request body.
    pub fn bytes(&self) -> &Bytes {
        &self.body
    }

    /// Get the request's data: the form parameters, the JSON payload, the
    /// query parameters of a `GET` request, or the multipart parts.
    pub fn data(&self) -> &Value {
        self.parsed.get_or_init(|| {
            if is_filled(&self.data) {
                return self.data.clone();
            }
            if self.is_form() {
                return parse_form(&self.body());
            }
            if self.is_json() {
                return serde_json::from_slice::<Value>(&self.body)
                    .ok()
                    .filter(|value| !value.is_null())
                    .unwrap_or_else(|| Value::Object(Map::new()));
            }
            self.data.clone()
        })
    }

    /// Determine if the request is simple form data.
    pub fn is_form(&self) -> bool {
        self.content_type().is_some_and(|content_type| {
            content_type.starts_with("application/x-www-form-urlencoded")
        })
    }

    /// Determine if the request is JSON.
    pub fn is_json(&self) -> bool {
        self.content_type()
            .is_some_and(|content_type| content_type.contains("json"))
    }

    /// Determine if the request is multipart.
    pub fn is_multipart(&self) -> bool {
        self.content_type()
            .is_some_and(|content_type| content_type.contains("multipart"))
    }

    fn content_type(&self) -> Option<&str> {
        self.headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
    }

    /// Determine if the request contains the given file.
    pub fn has_file(&self, name: &str) -> bool {
        self.has_file_with(name, None, None)
    }

    /// Determine if the request contains the given file, optionally matching
    /// its contents and filename.
    pub fn has_file_with(
        &self,
        name: &str,
        contents: Option<&str>,
        filename: Option<&str>,
    ) -> bool {
        self.is_multipart()
            && self.parts.iter().any(|part| {
                part.name == name
                    && contents.is_none_or(|contents| part.contents == contents.as_bytes())
                    && filename.is_none_or(|filename| part.filename.as_deref() == Some(filename))
            })
    }

    /// Get the multipart parts of the request.
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// Get the attributes set on the request with
    /// [`PendingRequest::with_attributes`](crate::PendingRequest::with_attributes).
    pub fn attributes(&self) -> &Map<String, Value> {
        &self.attributes
    }

    // ------------------------------------------------------------------
    // Building modified requests (handy in request middleware)
    // ------------------------------------------------------------------

    /// Return a copy of the request with the given header set.
    ///
    /// Invalid header names or values are ignored.
    pub fn with_header(mut self, name: &str, value: impl AsRef<str>) -> Self {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_bytes(value.as_ref().as_bytes()),
        ) {
            self.headers.insert(name, value);
        }
        self
    }

    /// Return a copy of the request with the given header value added.
    pub fn with_added_header(mut self, name: &str, value: impl AsRef<str>) -> Self {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_bytes(value.as_ref().as_bytes()),
        ) {
            self.headers.append(name, value);
        }
        self
    }

    /// Return a copy of the request without the given header.
    pub fn without_header(mut self, name: &str) -> Self {
        self.headers.remove(name);
        self
    }

    /// Return a copy of the request with the given URL.
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Return a copy of the request with the given method.
    pub fn with_method(mut self, method: &str) -> Self {
        if let Ok(method) = Method::from_bytes(method.to_ascii_uppercase().as_bytes()) {
            self.method = method;
        }
        self
    }

    /// Return a copy of the request with the given body.
    pub fn with_body(mut self, body: impl Into<Bytes>) -> Self {
        self.body = body.into();
        self.parsed = OnceLock::new();
        self
    }

    /// Return a copy of the request with the given data attached, for
    /// convenient assertions.
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = data;
        self.parsed = OnceLock::new();
        self
    }

    pub(crate) fn with_parts(mut self, parts: Vec<Part>) -> Self {
        self.parts = parts;
        self
    }

    pub(crate) fn with_attributes(mut self, attributes: Map<String, Value>) -> Self {
        self.attributes = attributes;
        self
    }

    pub(crate) fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }
}

fn is_filled(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &self.headers)
            .field("body", &self.body())
            .finish()
    }
}

impl Index<&str> for Request {
    type Output = Value;

    fn index(&self, key: &str) -> &Value {
        &self.data()[key]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn it_inspects_headers() {
        let request = Request::new("post", "https://laravel.com/api?page=1")
            .with_header("X-First", "foo")
            .with_added_header("X-Many", "a")
            .with_added_header("X-Many", "b");

        assert_eq!(request.method(), "POST");
        assert_eq!(request.http_method(), Method::POST);
        assert_eq!(request.uri().host(), Some("laravel.com"));
        assert!(request.has_header("x-first"));
        assert!(request.has_header_value("X-First", "foo"));
        assert!(!request.has_header_value("X-First", "bar"));
        assert!(request.has_headers(&[("X-First", "foo"), ("X-Many", "b")]));
        assert_eq!(request.header("X-Many"), ["a", "b"]);
        assert!(request.header("X-Missing").is_empty());
        assert!(
            !request
                .clone()
                .without_header("X-First")
                .has_header("X-First")
        );
        assert_eq!(request.headers().len(), 3);
    }

    #[test]
    fn it_reads_json_and_form_data() {
        let json = Request::new("POST", "/")
            .with_header("Content-Type", "application/json")
            .with_body(r#"{"name": "Taylor"}"#);
        assert!(json.is_json() && !json.is_form());
        assert_eq!(json["name"], "Taylor");
        assert!(json["missing"].is_null());

        let form = Request::new("POST", "/")
            .with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_body("name=Sara&role=Privacy+Consultant");
        assert!(form.is_form());
        assert_eq!(
            form.data(),
            &json!({"name": "Sara", "role": "Privacy Consultant"})
        );

        let attached = Request::new("GET", "/?page=2").with_data(json!({"page": "2"}));
        assert_eq!(attached["page"], "2");
        assert_eq!(Request::new("GET", "/").data(), &json!({}));
    }

    #[test]
    fn it_finds_files() {
        let request = Request::new("POST", "/")
            .with_header("Content-Type", "multipart/form-data; boundary=x")
            .with_parts(vec![Part::new("photo", "PNG").filename("photo.png")]);

        assert!(request.is_multipart());
        assert!(request.has_file("photo"));
        assert!(request.has_file_with("photo", Some("PNG"), Some("photo.png")));
        assert!(!request.has_file_with("photo", Some("GIF"), None));
        assert!(!request.has_file("avatar"));
        assert_eq!(request.parts().len(), 1);
    }

    #[test]
    fn it_builds_modified_copies() {
        let request = Request::new("GET", "http://a.test")
            .with_url("http://b.test")
            .with_method("delete")
            .with_attributes(json!({"tenant": 1}).as_object().unwrap().clone());

        assert_eq!(request.url(), "http://b.test");
        assert_eq!(request.method(), "DELETE");
        assert_eq!(request.attributes()["tenant"], 1);
        assert!(format!("{request:?}").contains("b.test"));
    }
}
