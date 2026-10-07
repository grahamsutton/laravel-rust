//! The incoming HTTP request.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Uri, Version};
use indexmap::IndexMap;
use serde::de::DeserializeOwned;

use illuminate_support::{Arr, Carbon, Fluent, Map, Str, Stringable, Value, ValueExt, cast};

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

    /// Create a `multipart/form-data` request carrying input and uploaded
    /// files — what a browser sends when a form uploads files.
    ///
    /// ```
    /// use illuminate_http::{Request, UploadedFile};
    /// use illuminate_support::json;
    /// use indexmap::IndexMap;
    ///
    /// let mut files = IndexMap::new();
    /// files.insert("avatar".to_string(), vec![UploadedFile::fake().image("avatar.jpg", 10, 10)]);
    ///
    /// let request = Request::create_multipart("/avatar", "POST", json!({"name": "Taylor"}), files, Default::default());
    ///
    /// assert!(request.has_file("avatar"));
    /// assert_eq!(request.input("name"), json!("Taylor"));
    /// assert!(request.header("content-type").unwrap().starts_with("multipart/form-data; boundary="));
    /// ```
    pub fn create_multipart(
        uri: &str,
        method: &str,
        parameters: Value,
        files: IndexMap<String, Vec<UploadedFile>>,
        mut headers: HeaderMap,
    ) -> Self {
        let uri: Uri = uri.parse().unwrap_or_else(|_| Uri::from_static("/"));
        let method = Method::from_bytes(method.to_ascii_uppercase().as_bytes()).unwrap_or(Method::POST);
        let parameters = if parameters.is_object() || parameters.is_array() {
            parameters
        } else {
            Value::Object(Map::new())
        };
        let boundary = crate::multipart::boundary();
        let body = crate::multipart::encode(&parameters, &files, &boundary);
        if let Ok(value) = HeaderValue::try_from(crate::multipart::content_type(&boundary)) {
            headers.insert(http::header::CONTENT_TYPE, value);
        }
        headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from(body.len()));
        let files = files
            .into_iter()
            .map(|(name, list)| (name.trim_end_matches("[]").to_string(), list))
            .collect();
        Self::from_parts_with_files(
            method,
            uri,
            Version::HTTP_11,
            headers,
            body,
            normalize_lists(parameters),
            files,
            Some(SocketAddr::from(([127, 0, 0, 1], 0))),
        )
    }

    /// Create an independent copy of the request: changes to the copy's
    /// input, headers or attributes don't affect the original.
    ///
    /// ```
    /// use illuminate_http::Request;
    /// use illuminate_support::json;
    ///
    /// let request = Request::create_with("/", "POST", json!({"name": "Taylor"}), Default::default());
    /// let copy = request.duplicate();
    /// copy.merge(json!({"name": "Abigail"}));
    ///
    /// assert_eq!(request.input("name"), json!("Taylor"));
    /// assert_eq!(copy.input("name"), json!("Abigail"));
    /// ```
    pub fn duplicate(&self) -> Self {
        self.duplicate_with(None, None)
    }

    /// Create an independent copy of the request, replacing its query
    /// parameters and/or its input.
    pub fn duplicate_with(&self, query: Option<Value>, input: Option<Value>) -> Self {
        let inner = &self.inner;
        Self {
            inner: Arc::new(Inner {
                method: RwLock::new(self.method()),
                uri: inner.uri.clone(),
                version: inner.version,
                headers: RwLock::new(self.headers()),
                body: inner.body.clone(),
                query: query.unwrap_or_else(|| inner.query.clone()),
                input: RwLock::new(input.unwrap_or_else(|| inner.input.read().unwrap().clone())),
                files: RwLock::new(inner.files.read().unwrap().clone()),
                cookies: RwLock::new(inner.cookies.read().unwrap().clone()),
                route_params: RwLock::new(inner.route_params.read().unwrap().clone()),
                route_name: RwLock::new(inner.route_name.read().unwrap().clone()),
                attributes: RwLock::new(inner.attributes.read().unwrap().clone()),
                extensions: RwLock::new(inner.extensions.read().unwrap().clone()),
                remote_addr: inner.remote_addr,
            }),
        }
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

    /// The scheme and HTTP host (`https://example.com:8080`).
    pub fn scheme_and_http_host(&self) -> String {
        self.root()
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

    /// Get a server variable, PHP's `$_SERVER` style (`REQUEST_METHOD`,
    /// `REQUEST_URI`, `SERVER_NAME`, `HTTP_USER_AGENT`, ...).
    ///
    /// ```
    /// use illuminate_http::{HeaderMap, Request};
    /// use illuminate_support::json;
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert("user-agent", "Symfony".parse().unwrap());
    /// let request = Request::create_with("/users?page=2", "GET", Default::default(), headers);
    ///
    /// assert_eq!(request.server("REQUEST_METHOD"), json!("GET"));
    /// assert_eq!(request.server("REQUEST_URI"), json!("/users?page=2"));
    /// assert_eq!(request.server("HTTP_USER_AGENT"), json!("Symfony"));
    /// assert_eq!(request.server("MISSING"), json!(null));
    /// ```
    pub fn server(&self, key: &str) -> Value {
        self.server_all().get(key).cloned().unwrap_or(Value::Null)
    }

    /// Every server variable.
    pub fn server_all(&self) -> Value {
        let mut server = Map::new();
        let uri = &self.inner.uri;
        let request_uri = uri
            .path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_else(|| "/".to_string());
        server.insert("SERVER_NAME".into(), Value::from(self.host()));
        server.insert("SERVER_PORT".into(), Value::from(self.port()));
        server.insert("SERVER_PROTOCOL".into(), Value::from(format!("{:?}", self.version())));
        server.insert("REQUEST_METHOD".into(), Value::from(self.method().as_str()));
        server.insert("REQUEST_URI".into(), Value::from(request_uri));
        server.insert("QUERY_STRING".into(), Value::from(uri.query().unwrap_or_default()));
        server.insert("SCRIPT_NAME".into(), Value::from(""));
        if self.secure() {
            server.insert("HTTPS".into(), Value::from("on"));
        }
        if let Some(addr) = self.inner.remote_addr {
            server.insert("REMOTE_ADDR".into(), Value::from(addr.ip().to_string()));
            server.insert("REMOTE_PORT".into(), Value::from(addr.port()));
        }
        for (name, value) in self.inner.headers.read().unwrap().iter() {
            let Ok(value) = value.to_str() else { continue };
            let key = name.as_str().to_ascii_uppercase().replace('-', "_");
            let key = if key == "CONTENT_TYPE" || key == "CONTENT_LENGTH" {
                key
            } else {
                format!("HTTP_{key}")
            };
            server.insert(key, Value::from(value));
        }
        Value::Object(server)
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

    /// Determine if the current request is asking for JSON (its most
    /// preferred content type is JSON).
    pub fn wants_json(&self) -> bool {
        self.acceptable_content_types()
            .first()
            .map(|first| first.to_ascii_lowercase())
            .is_some_and(|first| first.contains("/json") || first.contains("+json"))
    }

    /// Determine if the current request is asking for Markdown.
    ///
    /// ```
    /// use illuminate_http::{HeaderMap, Request};
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert("accept", "text/markdown, text/html;q=0.9".parse().unwrap());
    /// let request = Request::create_with("/docs", "GET", Default::default(), headers);
    ///
    /// assert!(request.wants_markdown());
    /// assert!(request.accepts_markdown());
    /// ```
    pub fn wants_markdown(&self) -> bool {
        self.acceptable_content_types()
            .first()
            .is_some_and(|first| first.to_ascii_lowercase().starts_with("text/markdown"))
    }

    /// The content types the client accepts, most preferred first (the
    /// `Accept` header, sorted by quality).
    ///
    /// ```
    /// use illuminate_http::{HeaderMap, Request};
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert("accept", "text/html;q=0.8, application/json, */*;q=0.1".parse().unwrap());
    /// let request = Request::create_with("/", "GET", Default::default(), headers);
    ///
    /// assert_eq!(request.acceptable_content_types(), ["application/json", "text/html", "*/*"]);
    /// ```
    pub fn acceptable_content_types(&self) -> Vec<String> {
        let Some(accept) = self.header("accept") else {
            return Vec::new();
        };
        let mut items: Vec<(usize, f64, String)> = Vec::new();
        for (index, item) in accept.split(',').enumerate() {
            let mut parts = item.split(';');
            let value = parts.next().unwrap_or_default().trim();
            if value.is_empty() {
                continue;
            }
            let quality = parts
                .filter_map(|parameter| parameter.split_once('='))
                .find(|(name, _)| name.trim().eq_ignore_ascii_case("q"))
                .and_then(|(_, q)| q.trim().parse::<f64>().ok())
                .unwrap_or(1.0);
            items.retain(|(_, _, existing)| existing != value);
            items.push((index, quality, value.to_string()));
        }
        items.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        items.into_iter().map(|(_, _, value)| value).collect()
    }

    /// Determine if the request accepts any of the given content types.
    ///
    /// ```
    /// use illuminate_http::{HeaderMap, Request};
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert("accept", "application/*".parse().unwrap());
    /// let request = Request::create_with("/", "GET", Default::default(), headers);
    ///
    /// assert!(request.accepts(&["application/json"]));
    /// assert!(!request.accepts(&["text/html"]));
    /// ```
    pub fn accepts(&self, content_types: &[&str]) -> bool {
        let accepts = self.acceptable_content_types();
        if accepts.is_empty() {
            return true;
        }
        accepts.iter().any(|accept| {
            let accept = accept.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
            if accept == "*/*" || accept == "*" {
                return true;
            }
            content_types.iter().any(|content_type| {
                let content_type = content_type.to_ascii_lowercase();
                Self::matches_type(&accept, &content_type)
                    || accept == format!("{}/*", content_type.split('/').next().unwrap_or_default())
            })
        })
    }

    /// Return the most suitable of the given content types (or formats like
    /// `json` and `html`) based on content negotiation, or `None` when the
    /// client accepts none of them.
    ///
    /// ```
    /// use illuminate_http::{HeaderMap, Request};
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert("accept", "text/html, application/json;q=0.9".parse().unwrap());
    /// let request = Request::create_with("/", "GET", Default::default(), headers);
    ///
    /// assert_eq!(request.prefers(&["json", "html"]).as_deref(), Some("html"));
    /// assert_eq!(request.prefers(&["application/json"]).as_deref(), Some("application/json"));
    /// assert_eq!(request.prefers(&["text/csv"]), None);
    /// ```
    pub fn prefers(&self, content_types: &[&str]) -> Option<String> {
        for accept in self.acceptable_content_types() {
            let accept = accept.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
            if accept == "*/*" || accept == "*" {
                return content_types.first().map(|first| first.to_string());
            }
            for content_type in content_types {
                let mime = Self::mime_type_for_format(content_type)
                    .unwrap_or(content_type)
                    .to_ascii_lowercase();
                if Self::matches_type(&mime, &accept)
                    || accept == format!("{}/*", mime.split('/').next().unwrap_or_default())
                {
                    return Some(content_type.to_string());
                }
            }
        }
        None
    }

    /// Determine if the request accepts any content type.
    pub fn accepts_any_content_type(&self) -> bool {
        let acceptable = self.acceptable_content_types();
        match acceptable.first() {
            None => true,
            Some(first) => first == "*/*" || first == "*",
        }
    }

    /// Determine if the request accepts JSON.
    pub fn accepts_json(&self) -> bool {
        self.accepts(&["application/json"])
    }

    /// Determine if the request accepts Markdown.
    pub fn accepts_markdown(&self) -> bool {
        self.accepts(&["text/markdown"])
    }

    /// Determine if the request accepts HTML.
    pub fn accepts_html(&self) -> bool {
        self.accepts(&["text/html"])
    }

    /// Determine if the given content types match: identical, or `actual`
    /// is a structured-syntax suffix match (`application/json` matches
    /// `application/vnd.api+json`).
    ///
    /// ```
    /// use illuminate_http::Request;
    ///
    /// assert!(Request::matches_type("application/json", "application/json"));
    /// assert!(Request::matches_type("application/json", "application/vnd.api+json"));
    /// assert!(!Request::matches_type("application/json", "text/html"));
    /// ```
    pub fn matches_type(actual: &str, content_type: &str) -> bool {
        if actual == content_type {
            return true;
        }
        let Some((top, sub)) = actual.split_once('/') else {
            return false;
        };
        let prefix = format!("{top}/");
        let suffix = format!("+{sub}");
        content_type.match_indices(&prefix).any(|(index, _)| {
            let rest = &content_type[index + prefix.len()..];
            rest.char_indices()
                .skip(1)
                .any(|(offset, _)| rest[offset..].starts_with(&suffix))
        })
    }

    /// The data format expected in the response (`json`, `html`, ...), from
    /// the most preferred acceptable content type that has one.
    pub fn format(&self, default: &str) -> String {
        self.acceptable_content_types()
            .iter()
            .find_map(|content_type| Self::format_for_mime_type(content_type))
            .unwrap_or(default)
            .to_string()
    }

    /// The MIME types associated with each request format.
    const FORMATS: &[(&str, &[&str])] = &[
        ("html", &["text/html", "application/xhtml+xml"]),
        ("txt", &["text/plain"]),
        ("js", &["application/javascript", "application/x-javascript", "text/javascript"]),
        ("css", &["text/css"]),
        ("json", &["application/json", "application/x-json"]),
        ("jsonld", &["application/ld+json"]),
        ("xml", &["text/xml", "application/xml", "application/x-xml"]),
        ("rdf", &["application/rdf+xml"]),
        ("atom", &["application/atom+xml"]),
        ("rss", &["application/rss+xml"]),
        ("form", &["application/x-www-form-urlencoded", "multipart/form-data"]),
        ("markdown", &["text/markdown"]),
    ];

    /// The primary MIME type of a format (`json` → `application/json`).
    pub fn mime_type_for_format(format: &str) -> Option<&'static str> {
        Self::FORMATS
            .iter()
            .find(|(name, _)| *name == format)
            .map(|(_, types)| types[0])
    }

    /// The format of a MIME type (`application/json` → `json`).
    pub fn format_for_mime_type(mime_type: &str) -> Option<&'static str> {
        let mime_type = mime_type.split(';').next().unwrap_or_default().trim();
        Self::FORMATS
            .iter()
            .find(|(_, types)| types.contains(&mime_type))
            .map(|(name, _)| *name)
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

    /// Get all of the input as a [`Fluent`] instance.
    ///
    /// ```
    /// use illuminate_http::Request;
    /// use illuminate_support::json;
    ///
    /// let request = Request::create_with("/", "POST", json!({"user": {"name": "Taylor", "age": 30}}), Default::default());
    ///
    /// assert_eq!(request.fluent().get("user.name"), json!("Taylor"));
    /// assert_eq!(request.fluent_key("user").integer("age"), 30);
    /// assert_eq!(request.fluent_only(&["missing"]).get("missing"), json!(null));
    /// ```
    pub fn fluent(&self) -> Fluent {
        Fluent::from(self.all())
    }

    /// Get an input item (an object, usually) as a [`Fluent`] instance.
    pub fn fluent_key(&self, key: &str) -> Fluent {
        match self.input(key) {
            value @ Value::Object(_) => Fluent::from(value),
            Value::Array(items) => Fluent::from(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(index, item)| (index.to_string(), item))
                    .collect::<Map<String, Value>>(),
            ),
            _ => Fluent::new(),
        }
    }

    /// Get the given input keys as a [`Fluent`] instance.
    pub fn fluent_only(&self, keys: &[&str]) -> Fluent {
        Fluent::from(self.only(keys))
    }

    /// The input as an array — Laravel's `toArray()`.
    pub fn to_array(&self) -> Value {
        self.all()
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

    /// Get an uploaded file. Nested files may be read with "dot" notation:
    /// `file("user.avatar")` finds the `user[avatar]` upload, and
    /// `file("photos.1")` the second of the `photos[]` uploads.
    pub fn file(&self, key: &str) -> Option<UploadedFile> {
        let files = self.inner.files.read().unwrap();
        if let Some(list) = files.get(key) {
            return list.first().cloned();
        }
        let (name, index) = Self::file_key(key);
        files.get(&name).and_then(|list| list.get(index.unwrap_or(0)).cloned())
    }

    /// Get every uploaded file for the given key.
    pub fn files(&self, key: &str) -> Vec<UploadedFile> {
        let files = self.inner.files.read().unwrap();
        if let Some(list) = files.get(key) {
            return list.clone();
        }
        let (name, index) = Self::file_key(key);
        match (files.get(&name), index) {
            (Some(list), None) => list.clone(),
            (Some(list), Some(index)) => list.get(index).cloned().into_iter().collect(),
            (None, _) => Vec::new(),
        }
    }

    /// Translate a dot notation key (`user.avatar`, `photos.1`) into the
    /// stored bracketed name (`user[avatar]`, `photos`) and a list index.
    fn file_key(key: &str) -> (String, Option<usize>) {
        let mut segments: Vec<&str> = key.split('.').collect();
        let index = match segments.last() {
            Some(last) if segments.len() > 1 && last.chars().all(|c| c.is_ascii_digit()) => {
                last.parse().ok()
            }
            _ => None,
        };
        if index.is_some() {
            segments.pop();
        }
        let mut name = segments[0].to_string();
        for segment in &segments[1..] {
            name.push_str(&format!("[{segment}]"));
        }
        (name, index)
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

    fn accepting(accept: &str) -> Request {
        let mut headers = HeaderMap::new();
        headers.insert("accept", HeaderValue::from_str(accept).unwrap());
        Request::create_with("/", "GET", json!({}), headers)
    }

    #[test]
    fn content_negotiation_follows_quality_order() {
        let request = accepting("text/html;q=0.5, application/json");
        assert!(request.wants_json());
        assert!(request.accepts_json());
        assert!(request.accepts_html());
        assert!(!request.accepts_markdown());
        assert!(!request.accepts_any_content_type());
        assert_eq!(request.format("html"), "json");

        let request = accepting("application/vnd.api+json");
        assert!(request.wants_json());
        assert!(!request.accepts_json());
        assert!(request.accepts(&["application/vnd.api+json"]));

        let request = accepting("*/*");
        assert!(request.accepts_any_content_type());
        assert!(request.accepts(&["anything/at-all"]));
        assert_eq!(request.prefers(&["text/html", "application/json"]).as_deref(), Some("text/html"));

        let request = Request::create("/", "GET");
        assert!(request.acceptable_content_types().is_empty());
        assert!(request.accepts_any_content_type());
        assert!(request.accepts_markdown());
        assert_eq!(request.prefers(&["json"]), None);
        assert_eq!(request.format("html"), "html");

        let request = accepting("text/*");
        assert!(request.accepts(&["text/markdown"]));
        assert_eq!(request.prefers(&["json", "markdown"]).as_deref(), Some("markdown"));
        assert!(!request.wants_markdown());
        assert!(accepting("text/markdown; charset=UTF-8").wants_markdown());
    }

    #[test]
    fn server_variables_follow_php() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("example.com:8080"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("x-custom-header", HeaderValue::from_static("yes"));
        let request = Request::create_with("/a?b=c", "POST", json!({}), headers);
        assert_eq!(request.server("SERVER_NAME"), json!("example.com"));
        assert_eq!(request.server("SERVER_PORT"), json!(8080));
        assert_eq!(request.server("HTTP_HOST"), json!("example.com:8080"));
        assert_eq!(request.server("CONTENT_TYPE"), json!("application/json"));
        assert_eq!(request.server("HTTP_X_CUSTOM_HEADER"), json!("yes"));
        assert_eq!(request.server("QUERY_STRING"), json!("b=c"));
        assert_eq!(request.server("SERVER_PROTOCOL"), json!("HTTP/1.1"));
        assert_eq!(request.server("REMOTE_ADDR"), json!("127.0.0.1"));
        assert_eq!(request.server("HTTPS"), json!(null));
        assert_eq!(request.scheme_and_http_host(), "http://example.com:8080");
    }

    #[test]
    fn nested_files_are_found_with_dot_notation() {
        let request = Request::create("/", "POST");
        request.attach_file("user[avatar]", UploadedFile::fake().create("me.jpg", 1));
        request.attach_file("photos", UploadedFile::fake().create("a.jpg", 1));
        request.attach_file("photos", UploadedFile::fake().create("b.jpg", 1));
        assert_eq!(request.file("user.avatar").unwrap().client_original_name(), "me.jpg");
        assert_eq!(request.file("photos.1").unwrap().client_original_name(), "b.jpg");
        assert_eq!(request.files("photos").len(), 2);
        assert_eq!(request.files("photos.0").len(), 1);
        assert!(request.file("photos.2").is_none());
        assert!(request.file("user.missing").is_none());
        assert!(request.has_file("user.avatar"));
    }

    #[test]
    fn multipart_requests_spoof_methods_and_carry_files() {
        let mut files = IndexMap::new();
        files.insert("photos[]".to_string(), vec![UploadedFile::fake().create("a.jpg", 1)]);
        let request = Request::create_multipart(
            "/photos/1",
            "POST",
            json!({"_method": "PUT", "title": "Hello"}),
            files,
            HeaderMap::new(),
        );
        assert_eq!(request.method(), Method::PUT);
        assert_eq!(request.files("photos").len(), 1);
        assert_eq!(request.input("title"), json!("Hello"));
        assert!(request.content().contains("name=\"photos[]\"; filename=\"a.jpg\""));
        assert_eq!(request.header("content-length").unwrap(), request.body().len().to_string());
    }

    #[test]
    fn duplicates_are_independent_copies() {
        let request = Request::create_with("/?page=1", "POST", json!({"name": "Taylor"}), HeaderMap::new());
        request.set_attribute("role", "admin");
        request.set_extension(Arc::new(7u8));
        let copy = request.duplicate_with(Some(json!({"page": "2"})), None);
        copy.set_attribute("role", "guest");
        copy.set_header("x-copy", "1");
        assert_eq!(copy.query("page"), json!("2"));
        assert_eq!(copy.input("name"), json!("Taylor"));
        assert_eq!(request.attribute("role"), json!("admin"));
        assert!(!request.has_header("x-copy"));
        assert_eq!(*copy.extension::<u8>().unwrap(), 7);
        assert_eq!(copy.fluent().get("name"), json!("Taylor"));
        assert_eq!(copy.to_array(), json!({"page": "2", "name": "Taylor"}));
    }

    #[test]
    fn fluent_keys_wrap_lists_by_index() {
        let request = Request::create_with("/", "POST", json!({"tags": ["a", "b"], "name": "x"}), HeaderMap::new());
        assert_eq!(request.fluent_key("tags").get("1"), json!("b"));
        assert_eq!(request.fluent_key("name").get("anything"), json!(null));
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
