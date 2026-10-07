//! The incoming HTTP request.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Uri, Version};
use indexmap::IndexMap;
use serde::de::DeserializeOwned;

use illuminate_support::{Arr, Carbon, Map, Str, Stringable, Value, ValueExt, cast};

use crate::cookie::parse_cookie_header;
use crate::input::{insert_bracketed, merge_values, normalize_lists, parse_query};
use crate::uploaded_file::UploadedFile;

/// An incoming HTTP request.
///
/// A `Request` is a cheap, shared handle: cloning it gives you another view
/// of the *same* request, so data merged by a middleware (or the route
/// parameters set by the router) is visible everywhere — exactly like PHP's
/// object semantics in Laravel.
#[derive(Clone)]
pub struct Request {
    inner: Arc<Inner>,
}

struct Inner {
    method: RwLock<Method>,
    uri: Uri,
    version: Version,
    headers: RwLock<HeaderMap>,
    body: Bytes,
    query: Value,
    input: RwLock<Value>,
    files: RwLock<IndexMap<String, Vec<UploadedFile>>>,
    cookies: RwLock<IndexMap<String, String>>,
    route_params: RwLock<IndexMap<String, String>>,
    route_name: RwLock<Option<String>>,
    attributes: RwLock<Map<String, Value>>,
    extensions: RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    remote_addr: Option<SocketAddr>,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method())
            .field("uri", &self.inner.uri)
            .finish()
    }
}

impl Default for Request {
    fn default() -> Self {
        Request::create("/", "GET")
    }
}

impl Request {
    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    /// Build a request from its raw parts. JSON and URL-encoded bodies are
    /// parsed eagerly; multipart bodies should be handed over pre-parsed via
    /// [`Request::from_parts_with_files`].
    pub fn from_parts(
        method: Method,
        uri: Uri,
        version: Version,
        headers: HeaderMap,
        body: Bytes,
        remote_addr: Option<SocketAddr>,
    ) -> Self {
        let content_type = headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();

        let input = if content_type.contains("json") {
            match serde_json::from_slice::<Value>(&body) {
                Ok(Value::Object(map)) => Value::Object(map),
                Ok(other) if !body.is_empty() => {
                    let mut map = Map::new();
                    map.insert("0".into(), other);
                    normalize_lists(Value::Object(map))
                }
                _ => Value::Object(Map::new()),
            }
        } else if content_type.starts_with("application/x-www-form-urlencoded")
            || (content_type.is_empty() && method != Method::GET && !body.is_empty())
        {
            parse_query(&String::from_utf8_lossy(&body))
        } else {
            Value::Object(Map::new())
        };

        Self::from_parts_with_files(method, uri, version, headers, body, input, IndexMap::new(), remote_addr)
    }

    /// Build a request with already-parsed input and files.
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts_with_files(
        method: Method,
        uri: Uri,
        version: Version,
        headers: HeaderMap,
        body: Bytes,
        input: Value,
        files: IndexMap<String, Vec<UploadedFile>>,
        remote_addr: Option<SocketAddr>,
    ) -> Self {
        let query = uri.query().map(parse_query).unwrap_or_else(|| Value::Object(Map::new()));

        let mut cookies = IndexMap::new();
        for header in headers.get_all(http::header::COOKIE) {
            if let Ok(header) = header.to_str() {
                for (name, value) in parse_cookie_header(header) {
                    cookies.insert(name, value);
                }
            }
        }

        // Laravel lets HTML forms "spoof" PUT, PATCH and DELETE requests.
        let mut method = method;
        if method == Method::POST {
            let spoofed = input
                .get("_method")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    headers
                        .get("x-http-method-override")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string)
                });
            if let Some(spoofed) = spoofed
                && let Ok(m) = Method::from_bytes(spoofed.to_ascii_uppercase().as_bytes()) {
                    method = m;
                }
        }

        Self {
            inner: Arc::new(Inner {
                method: RwLock::new(method),
                uri,
                version,
                headers: RwLock::new(headers),
                body,
                query,
                input: RwLock::new(input),
                files: RwLock::new(files),
                cookies: RwLock::new(cookies),
                route_params: RwLock::new(IndexMap::new()),
                route_name: RwLock::new(None),
                attributes: RwLock::new(Map::new()),
                extensions: RwLock::new(HashMap::new()),
                remote_addr,
            }),
        }
    }

    /// Create a request for the given URI and method — perfect for tests.
    ///
    /// ```
    /// use illuminate_http::Request;
    ///
    /// let request = Request::create("/users/1?tab=posts", "GET");
    /// assert_eq!(request.path(), "users/1");
    /// assert!(request.is_method("get"));
    /// ```
    pub fn create(uri: &str, method: &str) -> Self {
        Self::create_with(uri, method, Value::Object(Map::new()), HeaderMap::new())
    }

    /// Create a request with input parameters and headers.
    pub fn create_with(uri: &str, method: &str, parameters: Value, headers: HeaderMap) -> Self {
        let uri: Uri = uri.parse().unwrap_or_else(|_| Uri::from_static("/"));
        let method = Method::from_bytes(method.to_ascii_uppercase().as_bytes()).unwrap_or(Method::GET);
        let is_json = headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("json"));
        let body = if is_json && !parameters.is_blank() {
            Bytes::from(serde_json::to_vec(&parameters).unwrap_or_default())
        } else {
            Bytes::new()
        };
        let (query_input, input) = if method == Method::GET || method == Method::HEAD {
            (parameters, Value::Object(Map::new()))
        } else {
            (Value::Object(Map::new()), parameters)
        };
        let uri = if query_input.is_blank() {
            uri
        } else {
            let query = Arr::query(&query_input);
            let joined = if uri.query().is_some() {
                format!("{uri}&{query}")
            } else {
                format!("{uri}?{query}")
            };
            joined.parse().unwrap_or(uri)
        };
        Self::from_parts_with_files(
            method,
            uri,
            Version::HTTP_11,
            headers,
            body,
            normalize_lists(input),
            IndexMap::new(),
            Some(SocketAddr::from(([127, 0, 0, 1], 0))),
        )
    }

    // ------------------------------------------------------------------
    // Method & URL
    // ------------------------------------------------------------------

    /// The request method (`GET`, `POST`, ...), respecting method spoofing.
    pub fn method(&self) -> Method {
        self.inner.method.read().unwrap().clone()
    }

    /// Change the request method.
    pub fn set_method(&self, method: Method) {
        *self.inner.method.write().unwrap() = method;
    }

    /// Determine if the request method matches (case-insensitive).
    pub fn is_method(&self, method: &str) -> bool {
        self.method().as_str().eq_ignore_ascii_case(method)
    }

    /// The HTTP protocol version.
    pub fn version(&self) -> Version {
        self.inner.version
    }

    /// The full request URI.
    pub fn uri(&self) -> &Uri {
        &self.inner.uri
    }

    /// The path of the request without leading or trailing slashes
    /// (`/` for the root).
    pub fn path(&self) -> String {
        let decoded = self.decoded_path();
        let trimmed = decoded.trim_matches('/');
        if trimmed.is_empty() {
            "/".to_string()
        } else {
            trimmed.to_string()
        }
    }

    /// The URL decoded request path, with its leading slash.
    pub fn decoded_path(&self) -> String {
        percent_encoding::percent_decode_str(self.inner.uri.path())
            .decode_utf8_lossy()
            .into_owned()
    }

    /// The scheme (`http` or `https`).
    pub fn scheme(&self) -> String {
        if let Some(scheme) = self.inner.uri.scheme_str() {
            return scheme.to_string();
        }
        if self.trusts_proxies()
            && let Some(proto) = self.header("x-forwarded-proto") {
                return proto.split(',').next().unwrap_or("http").trim().to_string();
            }
        "http".to_string()
    }

    /// Determine if the request is over HTTPS.
    pub fn secure(&self) -> bool {
        self.scheme() == "https"
    }

    /// The host name, without the port.
    pub fn host(&self) -> String {
        let host = self.http_host();
        if let Some(rest) = host.strip_prefix('[') {
            // IPv6 literal: "[::1]:8000"
            return format!("[{}]", rest.split(']').next().unwrap_or_default());
        }
        match host.rsplit_once(':') {
            Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name.to_string(),
            _ => host,
        }
    }

    /// The host name, including the port when present.
    pub fn http_host(&self) -> String {
        if self.trusts_proxies()
            && let Some(host) = self.header("x-forwarded-host") {
                return host.split(',').next().unwrap_or_default().trim().to_string();
            }
        self.inner
            .uri
            .authority()
            .map(|a| a.to_string())
            .or_else(|| self.header("host"))
            .unwrap_or_else(|| "localhost".to_string())
    }

    /// The port of the request.
    pub fn port(&self) -> u16 {
        if let Some(port) = self.inner.uri.port_u16() {
            return port;
        }
        if let Some((_, port)) = self.http_host().rsplit_once(':')
            && let Ok(port) = port.parse() {
                return port;
            }
        if self.secure() { 443 } else { 80 }
    }

    /// The root URL of the application (`https://example.com`).
    pub fn root(&self) -> String {
        format!("{}://{}", self.scheme(), self.http_host())
    }

    /// The URL without the query string.
    pub fn url(&self) -> String {
        let path = self.inner.uri.path();
        let path = if path == "/" { "" } else { path.trim_end_matches('/') };
        format!("{}{}", self.root(), path)
    }

    /// The full URL, including the query string.
    pub fn full_url(&self) -> String {
        match self.inner.uri.query() {
            Some(query) if !query.is_empty() => format!("{}?{}", self.url(), query),
            _ => self.url(),
        }
    }

    /// The full URL with additional query parameters merged in.
    pub fn full_url_with_query(&self, query: Value) -> String {
        let mut merged = self.inner.query.clone();
        merge_values(&mut merged, query);
        let query = Arr::query(&merged);
        if query.is_empty() {
            self.url()
        } else {
            format!("{}?{}", self.url(), query)
        }
    }

    /// The full URL without the given query parameters.
    pub fn full_url_without_query(&self, keys: &[&str]) -> String {
        let query = Arr::query(&Arr::except(&self.inner.query, keys));
        if query.is_empty() {
            self.url()
        } else {
            format!("{}?{}", self.url(), query)
        }
    }

    /// The raw query string.
    pub fn query_string(&self) -> Option<&str> {
        self.inner.uri.query()
    }

    /// Determine if the request path matches a pattern (`admin/*`).
    ///
    /// ```
    /// use illuminate_http::Request;
    ///
    /// let request = Request::create("/admin/users", "GET");
    /// assert!(request.is("admin/*"));
    /// assert!(!request.is("users/*"));
    /// ```
    pub fn is(&self, pattern: &str) -> bool {
        let path = self.decoded_path();
        let path = path.trim_start_matches('/');
        Str::is(pattern.trim_start_matches('/'), path)
            || (pattern == "/" && path.is_empty())
    }

    /// Determine if the request path matches any of the patterns.
    pub fn is_any(&self, patterns: &[&str]) -> bool {
        patterns.iter().any(|p| self.is(p))
    }

    /// Determine if the full URL matches a pattern.
    pub fn full_url_is(&self, pattern: &str) -> bool {
        Str::is(pattern, &self.full_url())
    }

    /// Get a segment from the URI (1-based).
    pub fn segment(&self, index: usize) -> Option<String> {
        self.segments().get(index.checked_sub(1)?).cloned()
    }

    /// Get all of the segments of the request path.
    pub fn segments(&self) -> Vec<String> {
        self.decoded_path()
            .split('/')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    }

    // ------------------------------------------------------------------
    // Headers & client information
    // ------------------------------------------------------------------

    /// Get a header value.
    pub fn header(&self, name: &str) -> Option<String> {
        self.inner
            .headers
            .read()
            .unwrap()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    }

    /// Get a header value, or a default.
    pub fn header_or(&self, name: &str, default: &str) -> String {
        self.header(name).unwrap_or_else(|| default.to_string())
    }

    /// Determine if a header is present.
    pub fn has_header(&self, name: &str) -> bool {
        self.inner.headers.read().unwrap().contains_key(name)
    }

    /// Get a copy of all of the headers.
    pub fn headers(&self) -> HeaderMap {
        self.inner.headers.read().unwrap().clone()
    }

    /// Set a header on the request.
    pub fn set_header(&self, name: &str, value: &str) {
        if let (Ok(name), Ok(value)) = (HeaderName::try_from(name), HeaderValue::try_from(value)) {
            self.inner.headers.write().unwrap().insert(name, value);
        }
    }

    /// Get the bearer token from the `Authorization` header.
    pub fn bearer_token(&self) -> Option<String> {
        let header = self.header("authorization")?;
        let position = header.to_ascii_lowercase().find("bearer ")?;
        let token = header[position + 7..].split(',').next()?.trim();
        (!token.is_empty()).then(|| token.to_string())
    }

    /// The `User-Agent` header.
    pub fn user_agent(&self) -> Option<String> {
        self.header("user-agent")
    }

    /// The client IP address.
    pub fn ip(&self) -> Option<String> {
        self.ips().into_iter().next()
    }

    /// All client IP addresses (the forwarding chain when proxies are trusted).
    pub fn ips(&self) -> Vec<String> {
        if self.trusts_proxies()
            && let Some(forwarded) = self.header("x-forwarded-for") {
                let ips: Vec<String> = forwarded
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !ips.is_empty() {
                    return ips;
                }
            }
        self.inner
            .remote_addr
            .map(|addr| vec![addr.ip().to_string()])
            .unwrap_or_default()
    }

    /// The socket address of the remote peer.
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.inner.remote_addr
    }

    /// Mark whether forwarding headers from proxies should be trusted.
    pub fn set_trust_proxies(&self, trust: bool) {
        self.set_attribute("_trust_proxies", trust);
    }

    fn trusts_proxies(&self) -> bool {
        self.attribute("_trust_proxies").truthy()
    }

    /// Determine if the request is the result of an AJAX call.
    pub fn ajax(&self) -> bool {
        self.header("x-requested-with").as_deref() == Some("XMLHttpRequest")
    }

    /// Determine if the request is the result of a PJAX call.
    pub fn pjax(&self) -> bool {
        self.header("x-pjax").as_deref() == Some("true")
    }

    /// Determine if the request is sending JSON.
    pub fn is_json(&self) -> bool {
        self.header("content-type")
            .is_some_and(|ct| ct.contains("/json") || ct.contains("+json"))
    }

    /// Determine if the current request probably expects a JSON response.
    pub fn expects_json(&self) -> bool {
        (self.ajax() && !self.pjax() && self.accepts_any_content_type()) || self.wants_json()
    }

    /// Determine if the current request is asking for JSON.
    pub fn wants_json(&self) -> bool {
        self.header("accept")
            .and_then(|accept| accept.split(',').next().map(str::to_string))
            .is_some_and(|first| first.contains("/json") || first.contains("+json"))
    }

    /// Determine if the request accepts any of the given content types.
    pub fn accepts(&self, content_types: &[&str]) -> bool {
        let accept = match self.header("accept") {
            Some(accept) if !accept.trim().is_empty() => accept,
            _ => return true,
        };
        accept.split(',').any(|accepted| {
            let accepted = accepted.split(';').next().unwrap_or_default().trim();
            if accepted == "*/*" || accepted == "*" {
                return true;
            }
            content_types.iter().any(|ct| {
                *ct == accepted
                    || accepted
                        .strip_suffix("/*")
                        .is_some_and(|prefix| ct.starts_with(&format!("{prefix}/")))
            })
        })
    }

    /// Determine if the request accepts HTML.
    pub fn accepts_html(&self) -> bool {
        self.accepts(&["text/html"])
    }

    /// Determine if the request accepts any content type.
    pub fn accepts_any_content_type(&self) -> bool {
        match self.header("accept") {
            None => true,
            Some(accept) => {
                let first = accept.split(',').next().unwrap_or_default().trim();
                first.is_empty() || first == "*/*" || first == "*"
            }
        }
    }

    /// Determine if the request is a prefetch request.
    pub fn prefetch(&self) -> bool {
        self.header("x-moz").as_deref() == Some("prefetch")
            || self.header("purpose").as_deref() == Some("prefetch")
            || self.header("sec-purpose").is_some_and(|p| p.contains("prefetch"))
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    /// Get all of the input (query string + body) as a single value.
    pub fn all(&self) -> Value {
        let mut all = self.inner.query.clone();
        merge_values(&mut all, self.inner.input.read().unwrap().clone());
        all
    }

    /// Get an input item from the request using "dot" notation (body first,
    /// then the query string). Missing keys are `null`.
    ///
    /// ```
    /// use illuminate_http::Request;
    /// use illuminate_support::json;
    ///
    /// let request = Request::create_with(
    ///     "/users",
    ///     "POST",
    ///     json!({"user": {"name": "Taylor"}}),
    ///     Default::default(),
    /// );
    ///
    /// assert_eq!(request.input("user.name"), json!("Taylor"));
    /// assert_eq!(request.input_or("user.email", "none"), json!("none"));
    /// ```
    pub fn input(&self, key: &str) -> Value {
        if let Some(value) = self.inner.input.read().unwrap().dot(key) {
            return value.clone();
        }
        self.inner.query.dot_or_null(key)
    }

    /// Get an input item, or the default when it is missing.
    pub fn input_or(&self, key: &str, default: impl Into<Value>) -> Value {
        match self.input(key) {
            Value::Null => default.into(),
            value => value,
        }
    }

    /// Get an input item deserialized into a concrete type.
    pub fn input_as<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        match self.input(key) {
            Value::Null => None,
            value => cast(value).ok(),
        }
    }

    /// Deserialize all of the input into a concrete type.
    pub fn deserialize<T: DeserializeOwned>(&self) -> illuminate_support::Result<T> {
        cast(self.all())
    }

    /// Get a query string item.
    pub fn query(&self, key: &str) -> Value {
        self.inner.query.dot_or_null(key)
    }

    /// Get a query string item, or a default.
    pub fn query_or(&self, key: &str, default: impl Into<Value>) -> Value {
        match self.query(key) {
            Value::Null => default.into(),
            value => value,
        }
    }

    /// Get all of the query string parameters.
    pub fn query_all(&self) -> Value {
        self.inner.query.clone()
    }

    /// Get an item from the request body only.
    pub fn post(&self, key: &str) -> Value {
        self.inner.input.read().unwrap().dot_or_null(key)
    }

    /// Get the request body input (without the query string).
    pub fn post_all(&self) -> Value {
        self.inner.input.read().unwrap().clone()
    }

    /// Get an input item as a string (empty when missing).
    pub fn string(&self, key: &str) -> String {
        self.input(key).to_string_lossy()
    }

    /// Get an input item as a fluent [`Stringable`].
    pub fn str(&self, key: &str) -> Stringable {
        Stringable::new(self.string(key))
    }

    /// Get an input item as an integer.
    pub fn integer(&self, key: &str) -> i64 {
        self.input(key).to_i64_lossy().unwrap_or(0)
    }

    /// Get an input item as an integer, or a default.
    pub fn integer_or(&self, key: &str, default: i64) -> i64 {
        self.input(key).to_i64_lossy().unwrap_or(default)
    }

    /// Get an input item as a float.
    pub fn float(&self, key: &str) -> f64 {
        self.input(key).to_f64_lossy().unwrap_or(0.0)
    }

    /// Get an input item as a boolean ("1", "true", "on" and "yes" are true).
    pub fn boolean(&self, key: &str) -> bool {
        match self.input(key) {
            Value::String(s) => matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"),
            other => other.truthy(),
        }
    }

    /// Get an input item as a date.
    pub fn date(&self, key: &str) -> Option<Carbon> {
        match self.input(key) {
            Value::Null => None,
            value if value.is_blank() => None,
            value => Carbon::parse(&value.to_string_lossy()).ok(),
        }
    }

    /// Get an input item as a list of values.
    pub fn array(&self, key: &str) -> Vec<Value> {
        match self.input(key) {
            Value::Array(items) => items,
            Value::Null => Vec::new(),
            Value::Object(map) => map.into_iter().map(|(_, v)| v).collect(),
            other => vec![other],
        }
    }

    /// Get a subset of the input containing only the given keys.
    pub fn only(&self, keys: &[&str]) -> Value {
        let all = self.all();
        let mut out = Value::Object(Map::new());
        for key in keys {
            if let Some(value) = all.dot(key) {
                Arr::set(&mut out, key, value.clone());
            }
        }
        out
    }

    /// Get all of the input except for the given keys.
    pub fn except(&self, keys: &[&str]) -> Value {
        Arr::except(&self.all(), keys)
    }

    /// Get the keys of all of the input.
    pub fn keys(&self) -> Vec<String> {
        Arr::keys(&self.all())
    }

    /// Determine if the request contains the given input key (even if empty).
    pub fn has(&self, key: &str) -> bool {
        self.all().dot(key).is_some()
    }

    /// Determine if the request contains all of the given keys.
    pub fn has_all(&self, keys: &[&str]) -> bool {
        keys.iter().all(|k| self.has(k))
    }

    /// Determine if the request contains any of the given keys.
    pub fn has_any(&self, keys: &[&str]) -> bool {
        keys.iter().any(|k| self.has(k))
    }

    /// Determine if the request is missing the given key.
    pub fn missing(&self, key: &str) -> bool {
        !self.has(key)
    }

    /// Determine if the given input key is present and not blank.
    pub fn filled(&self, key: &str) -> bool {
        !self.input(key).is_blank()
    }

    /// Determine if the given input key is blank or missing.
    pub fn is_not_filled(&self, key: &str) -> bool {
        !self.filled(key)
    }

    /// Determine if any of the given keys are filled.
    pub fn any_filled(&self, keys: &[&str]) -> bool {
        keys.iter().any(|k| self.filled(k))
    }

    /// Run the callback with the input value when the key is present.
    pub fn when_has(&self, key: &str, callback: impl FnOnce(Value)) {
        if self.has(key) {
            callback(self.input(key));
        }
    }

    /// Run the callback with the input value when the key is filled.
    pub fn when_filled(&self, key: &str, callback: impl FnOnce(Value)) {
        if self.filled(key) {
            callback(self.input(key));
        }
    }

    /// Merge new input into the request.
    pub fn merge(&self, input: Value) {
        merge_values(&mut self.inner.input.write().unwrap(), input);
    }

    /// Merge new input into the request for keys that are missing.
    pub fn merge_if_missing(&self, input: Value) {
        if let Value::Object(map) = input {
            for (key, value) in map {
                if self.missing(&key) {
                    Arr::set(&mut self.inner.input.write().unwrap(), &key, value);
                }
            }
        }
    }

    /// Replace the request body input.
    pub fn replace(&self, input: Value) {
        *self.inner.input.write().unwrap() = input;
    }

    /// Transform every string in the input (used by `TrimStrings` and
    /// `ConvertEmptyStringsToNull`).
    pub fn transform_input(&self, transform: &dyn Fn(&str, Value) -> Value) {
        fn walk(prefix: &str, value: Value, transform: &dyn Fn(&str, Value) -> Value) -> Value {
            match value {
                Value::Object(map) => Value::Object(
                    map.into_iter()
                        .map(|(k, v)| {
                            let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                            (k, walk(&key, v, transform))
                        })
                        .collect(),
                ),
                Value::Array(items) => Value::Array(
                    items
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| walk(&format!("{prefix}.{i}"), v, transform))
                        .collect(),
                ),
                other => transform(prefix, other),
            }
        }
        let current = std::mem::take(&mut *self.inner.input.write().unwrap());
        let transformed = walk("", current, transform);
        *self.inner.input.write().unwrap() = transformed;
    }

    /// The raw request body.
    pub fn body(&self) -> &Bytes {
        &self.inner.body
    }

    /// The raw request body as a string.
    pub fn content(&self) -> String {
        String::from_utf8_lossy(&self.inner.body).into_owned()
    }

    /// Get a value from the JSON payload.
    pub fn json(&self, key: &str) -> Value {
        serde_json::from_slice::<Value>(&self.inner.body)
            .map(|payload| payload.dot_or_null(key))
            .unwrap_or(Value::Null)
    }

    // ------------------------------------------------------------------
    // Files
    // ------------------------------------------------------------------

    /// Get an uploaded file.
    pub fn file(&self, key: &str) -> Option<UploadedFile> {
        self.inner
            .files
            .read()
            .unwrap()
            .get(key)
            .and_then(|files| files.first().cloned())
    }

    /// Get every uploaded file for the given key.
    pub fn files(&self, key: &str) -> Vec<UploadedFile> {
        self.inner
            .files
            .read()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    /// Get all of the uploaded files.
    pub fn all_files(&self) -> IndexMap<String, Vec<UploadedFile>> {
        self.inner.files.read().unwrap().clone()
    }

    /// Determine if the request contains a valid file for the key.
    pub fn has_file(&self, key: &str) -> bool {
        self.file(key).is_some_and(|f| f.is_valid())
    }

    /// Attach an uploaded file to the request (used by tests).
    pub fn attach_file(&self, key: &str, file: UploadedFile) {
        self.inner
            .files
            .write()
            .unwrap()
            .entry(key.to_string())
            .or_default()
            .push(file);
    }

    // ------------------------------------------------------------------
    // Cookies
    // ------------------------------------------------------------------

    /// Get a cookie value.
    pub fn cookie(&self, name: &str) -> Option<String> {
        self.inner.cookies.read().unwrap().get(name).cloned()
    }

    /// Determine if the request has the given cookie.
    pub fn has_cookie(&self, name: &str) -> bool {
        self.inner.cookies.read().unwrap().contains_key(name)
    }

    /// Get all of the cookies.
    pub fn cookies(&self) -> IndexMap<String, String> {
        self.inner.cookies.read().unwrap().clone()
    }

    /// Set (or replace) an incoming cookie value — `EncryptCookies` uses
    /// this to swap in decrypted values.
    pub fn set_cookie(&self, name: &str, value: impl Into<String>) {
        self.inner
            .cookies
            .write()
            .unwrap()
            .insert(name.to_string(), value.into());
    }

    /// Remove an incoming cookie.
    pub fn remove_cookie(&self, name: &str) {
        self.inner.cookies.write().unwrap().shift_remove(name);
    }

    // ------------------------------------------------------------------
    // Routing
    // ------------------------------------------------------------------

    /// Get a route parameter.
    pub fn route(&self, name: &str) -> Option<String> {
        self.inner.route_params.read().unwrap().get(name).cloned()
    }

    /// Get a route parameter, or a default.
    pub fn route_or(&self, name: &str, default: &str) -> String {
        self.route(name).unwrap_or_else(|| default.to_string())
    }

    /// Get all of the route parameters.
    pub fn route_parameters(&self) -> IndexMap<String, String> {
        self.inner.route_params.read().unwrap().clone()
    }

    /// Set the matched route's name and parameters (called by the router).
    pub fn set_route(&self, name: Option<String>, parameters: IndexMap<String, String>) {
        *self.inner.route_name.write().unwrap() = name;
        *self.inner.route_params.write().unwrap() = parameters;
    }

    /// Set a single route parameter.
    pub fn set_route_parameter(&self, name: &str, value: impl Into<String>) {
        self.inner
            .route_params
            .write()
            .unwrap()
            .insert(name.to_string(), value.into());
    }

    /// The name of the matched route.
    pub fn route_name(&self) -> Option<String> {
        self.inner.route_name.read().unwrap().clone()
    }

    /// Determine if the matched route's name matches a pattern (`admin.*`).
    pub fn route_is(&self, pattern: &str) -> bool {
        self.route_name().is_some_and(|name| Str::is(pattern, &name))
    }

    // ------------------------------------------------------------------
    // Attributes & extensions
    // ------------------------------------------------------------------

    /// Get a custom request attribute.
    pub fn attribute(&self, key: &str) -> Value {
        self.inner
            .attributes
            .read()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// Set a custom request attribute.
    pub fn set_attribute(&self, key: &str, value: impl Into<Value>) {
        self.inner
            .attributes
            .write()
            .unwrap()
            .insert(key.to_string(), value.into());
    }

    /// Get a typed extension attached to the request (the session, the
    /// authenticated user, the matched route, ...).
    pub fn extension<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.inner
            .extensions
            .read()
            .unwrap()
            .get(&TypeId::of::<T>())
            .cloned()
            .and_then(|any| any.downcast::<T>().ok())
    }

    /// Attach a typed extension to the request.
    pub fn set_extension<T: Send + Sync + 'static>(&self, value: Arc<T>) {
        self.inner
            .extensions
            .write()
            .unwrap()
            .insert(TypeId::of::<T>(), value);
    }

    /// Remove a typed extension from the request.
    pub fn forget_extension<T: Send + Sync + 'static>(&self) {
        self.inner
            .extensions
            .write()
            .unwrap()
            .remove(&TypeId::of::<T>());
    }

    /// A unique fingerprint for the request's route and IP.
    pub fn fingerprint(&self) -> String {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        self.method().as_str().hash(&mut hasher);
        self.root().hash(&mut hasher);
        self.path().hash(&mut hasher);
        self.ip().hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Insert a raw form field using PHP bracket syntax (used by multipart parsing).
    pub fn insert_input(&self, key: &str, value: Value) {
        let mut input = self.inner.input.write().unwrap();
        insert_bracketed(&mut input, key, value);
        let normalized = normalize_lists(std::mem::take(&mut *input));
        *input = normalized;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn json_request(body: Value) -> Request {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        Request::create_with("/api/users?page=2", "POST", body, headers)
    }

    #[test]
    fn it_reads_json_input() {
        let request = json_request(json!({"name": "Taylor", "roles": ["admin"]}));
        assert_eq!(request.input("name"), json!("Taylor"));
        assert_eq!(request.input("roles.0"), json!("admin"));
        assert_eq!(request.query("page"), json!("2"));
        assert_eq!(request.integer("page"), 2);
        assert!(request.is_json());
        assert!(request.has("roles"));
        assert!(!request.filled("missing"));
        assert_eq!(request.only(&["name"]), json!({"name": "Taylor"}));
    }

    #[test]
    fn it_parses_form_bodies_and_spoofs_methods() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type",
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        let request = Request::from_parts(
            Method::POST,
            "/posts/1".parse().unwrap(),
            Version::HTTP_11,
            headers,
            Bytes::from_static(b"_method=PUT&title=Hello+World&tags[]=a&tags[]=b"),
            None,
        );
        assert_eq!(request.method(), Method::PUT);
        assert_eq!(request.input("title"), json!("Hello World"));
        assert_eq!(request.input("tags"), json!(["a", "b"]));
    }

    #[test]
    fn it_understands_urls() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("example.com"));
        let request = Request::create_with("/admin/users/?q=1", "GET", json!({}), headers);
        assert_eq!(request.path(), "admin/users");
        assert_eq!(request.url(), "http://example.com/admin/users");
        assert_eq!(request.full_url(), "http://example.com/admin/users?q=1");
        assert_eq!(request.segment(2).as_deref(), Some("users"));
        assert!(request.is("admin/*"));
    }

    #[test]
    fn it_negotiates_content() {
        let mut headers = HeaderMap::new();
        headers.insert("accept", HeaderValue::from_static("application/json"));
        let request = Request::create_with("/", "GET", json!({}), headers);
        assert!(request.wants_json());
        assert!(request.expects_json());
        assert!(!request.accepts_html());
    }

    #[test]
    fn it_reads_bearer_tokens_and_cookies() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer secret-token"));
        headers.insert("cookie", HeaderValue::from_static("theme=dark; lang=en"));
        let request = Request::create_with("/", "GET", json!({}), headers);
        assert_eq!(request.bearer_token().as_deref(), Some("secret-token"));
        assert_eq!(request.cookie("theme").as_deref(), Some("dark"));
    }

    #[test]
    fn requests_are_shared_handles() {
        let request = Request::create("/", "POST");
        let clone = request.clone();
        clone.merge(json!({"name": "Taylor"}));
        assert_eq!(request.input("name"), json!("Taylor"));
        clone.set_extension(Arc::new(42u32));
        assert_eq!(*request.extension::<u32>().unwrap(), 42);
    }
}
