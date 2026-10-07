//! The fluent request builder.

use std::fmt;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::Method;
use illuminate_support::{Conditionable, Error, Map, Result, Tappable, Value, json, to_value};
use serde::Serialize;

use crate::encoding::{Part, merge_recursive, normalize_pairs, replace_recursive};
use crate::exceptions::{RequestException, Truncation};
use crate::factory::Factory;
use crate::middleware::{BoxFuture, Middleware, Next};
use crate::request::Request;
use crate::response::Response;

pub(crate) type RetryWhen = Arc<dyn Fn(&Error, &mut PendingRequest) -> bool + Send + Sync>;
pub(crate) type RetryDelayFn = Arc<dyn Fn(u32, &Error) -> u64 + Send + Sync>;
pub(crate) type ThrowCallback = Arc<dyn Fn(&Response, &RequestException) + Send + Sync>;
pub(crate) type ResponsePredicate = Arc<dyn Fn(&Response) -> bool + Send + Sync>;
pub(crate) type BeforeSending = Arc<dyn Fn(&mut Request) + Send + Sync>;
pub(crate) type AfterResponse = Arc<dyn Fn(Response, &Request) -> Response + Send + Sync>;

/// How the request's data is sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BodyFormat {
    /// A JSON payload (the default).
    #[default]
    Json,
    /// A `application/x-www-form-urlencoded` form.
    Form,
    /// A `multipart/form-data` body.
    Multipart,
    /// A raw body given to [`PendingRequest::with_body`].
    Body,
}

/// How many times a request should be attempted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tries {
    /// Attempt the request up to this many times.
    Times(u32),
    /// Retry once for each entry, sleeping that many milliseconds first.
    Backoff(Vec<u64>),
}

impl Tries {
    pub(crate) fn attempts(&self) -> u32 {
        match self {
            Tries::Times(times) => (*times).max(1),
            Tries::Backoff(backoff) => backoff.len() as u32 + 1,
        }
    }
}

impl From<u32> for Tries {
    fn from(times: u32) -> Self {
        Tries::Times(times)
    }
}

impl From<i32> for Tries {
    fn from(times: i32) -> Self {
        Tries::Times(times.max(0) as u32)
    }
}

impl From<usize> for Tries {
    fn from(times: usize) -> Self {
        Tries::Times(times.min(u32::MAX as usize) as u32)
    }
}

impl From<Vec<u64>> for Tries {
    fn from(backoff: Vec<u64>) -> Self {
        Tries::Backoff(backoff)
    }
}

impl From<&[u64]> for Tries {
    fn from(backoff: &[u64]) -> Self {
        Tries::Backoff(backoff.to_vec())
    }
}

impl<const N: usize> From<[u64; N]> for Tries {
    fn from(backoff: [u64; N]) -> Self {
        Tries::Backoff(backoff.to_vec())
    }
}

#[derive(Clone)]
pub(crate) enum RetryDelay {
    Milliseconds(u64),
    Using(RetryDelayFn),
}

/// The data given to a request verb (`get`, `post`, ...).
#[derive(Clone, Debug, Default)]
pub(crate) struct CallOptions {
    pub(crate) query: Option<Value>,
    pub(crate) data: Option<Value>,
}

/// A request being built: configure it fluently, then send it with one of
/// the verbs (`get`, `post`, `put`, `patch`, `delete`, `head`).
///
/// Pending requests are cheap to clone, so a configured client may be
/// reused for any number of requests:
///
/// ```
/// use illuminate_http_client::Http;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// Http::fake();
///
/// let github = Http::base_url("https://api.github.com")
///     .with_token("secret", "Bearer")
///     .accept_json()
///     .timeout(5)
///     .retry(3, 100);
///
/// github.clone().get("/users/taylorotwell").await?;
/// github.post("/gists", json!({"public": true})).await?;
///
/// Http::assert_sent(|request| {
///     request.url() == "https://api.github.com/gists"
///         && request.has_header_value("Authorization", "Bearer secret")
///         && request["public"] == true
/// });
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Clone)]
pub struct PendingRequest {
    pub(crate) factory: Option<Arc<Factory>>,
    pub(crate) base_url: String,
    pub(crate) url_parameters: Map<String, Value>,
    pub(crate) body_format: BodyFormat,
    pub(crate) pending_body: Option<Bytes>,
    pub(crate) pending_files: Vec<Part>,
    pub(crate) options: Map<String, Value>,
    pub(crate) tries: Tries,
    pub(crate) retry_delay: RetryDelay,
    pub(crate) retry_when: Option<RetryWhen>,
    pub(crate) retry_throw: bool,
    pub(crate) throw_callback: Option<ThrowCallback>,
    pub(crate) throw_if_callback: Option<ResponsePredicate>,
    pub(crate) before_sending: Vec<BeforeSending>,
    pub(crate) after_response: Vec<AfterResponse>,
    pub(crate) middleware: Vec<Middleware>,
    pub(crate) attributes: Map<String, Value>,
    pub(crate) truncation: Option<Truncation>,
    pub(crate) pool_key: Option<String>,
}

impl Default for PendingRequest {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingRequest {
    /// Create a new pending request that isn't attached to a [`Factory`]
    /// (so it is never faked or recorded). Most applications should use
    /// the [`Http`](crate::Http) facade instead.
    pub fn new() -> Self {
        Self {
            factory: None,
            base_url: String::new(),
            url_parameters: Map::new(),
            body_format: BodyFormat::Json,
            pending_body: None,
            pending_files: Vec::new(),
            options: json!({"connect_timeout": 10, "http_errors": false, "timeout": 30})
                .as_object()
                .cloned()
                .unwrap_or_default(),
            tries: Tries::Times(1),
            retry_delay: RetryDelay::Milliseconds(0),
            retry_when: None,
            retry_throw: true,
            throw_callback: None,
            throw_if_callback: None,
            before_sending: Vec::new(),
            after_response: Vec::new(),
            middleware: Vec::new(),
            attributes: Map::new(),
            truncation: None,
            pool_key: None,
        }
    }

    pub(crate) fn for_factory(factory: Arc<Factory>, middleware: Vec<Middleware>) -> Self {
        Self {
            factory: Some(factory),
            middleware,
            ..Self::new()
        }
    }

    // ------------------------------------------------------------------
    // URLs
    // ------------------------------------------------------------------

    /// Set the base URL for the pending request.
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Specify the URL parameters that can be substituted into the request
    /// URL using [URI templates](https://www.rfc-editor.org/rfc/rfc6570).
    ///
    /// ```
    /// use illuminate_http_client::Http;
    /// use illuminate_support::json;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// Http::fake();
    ///
    /// Http::with_url_parameters(json!({
    ///     "endpoint": "https://laravel.com",
    ///     "page": "docs",
    ///     "version": "13.x",
    ///     "topic": "validation",
    /// }))
    /// .get("{+endpoint}/{page}/{version}/{topic}")
    /// .await?;
    ///
    /// Http::assert_sent(|request| request.url() == "https://laravel.com/docs/13.x/validation");
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn with_url_parameters(mut self, parameters: impl Serialize) -> Self {
        if let Value::Object(parameters) = normalize_pairs(to_value(&parameters)) {
            self.url_parameters.extend(parameters);
        }
        self
    }

    /// Set the given query parameters in the request URI.
    pub fn with_query_parameters(mut self, parameters: impl Serialize) -> Self {
        let parameters = normalize_pairs(to_value(&parameters));
        match self.options.get_mut("query") {
            Some(existing) if existing.is_object() && parameters.is_object() => {
                replace_recursive(existing, parameters)
            }
            _ => {
                self.options.insert("query".into(), parameters);
            }
        }
        self
    }

    // ------------------------------------------------------------------
    // Bodies
    // ------------------------------------------------------------------

    /// Attach a raw body to the request.
    ///
    /// ```
    /// # use illuminate_http_client::Http;
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// Http::fake();
    ///
    /// Http::with_body("<xml/>", "application/xml").post("https://example.com/photo", ()).await?;
    ///
    /// Http::assert_sent(|request| request.body() == "<xml/>");
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn with_body(mut self, content: impl Into<Bytes>, content_type: &str) -> Self {
        self.body_format = BodyFormat::Body;
        self.pending_body = Some(content.into());
        self.content_type(content_type)
    }

    /// Indicate the request contains JSON.
    pub fn as_json(mut self) -> Self {
        self.body_format = BodyFormat::Json;
        self.content_type("application/json")
    }

    /// Indicate the request contains form parameters.
    pub fn as_form(mut self) -> Self {
        self.body_format = BodyFormat::Form;
        self.content_type("application/x-www-form-urlencoded")
    }

    /// Indicate the request is a multi-part form request.
    pub fn as_multipart(mut self) -> Self {
        self.body_format = BodyFormat::Multipart;
        self.remove_header("Content-Type");
        self
    }

    /// Specify the body format of the request.
    pub fn body_format(mut self, format: BodyFormat) -> Self {
        self.body_format = format;
        self
    }

    /// Attach a file to the request (making it a multipart request). An
    /// empty filename sends the contents as a plain field.
    ///
    /// ```
    /// # use illuminate_http_client::Http;
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// Http::fake();
    ///
    /// Http::attach("attachment", b"PNG...".to_vec(), "photo.png")
    ///     .post("https://example.com/attachments", ())
    ///     .await?;
    ///
    /// Http::assert_sent(|request| request.has_file("attachment"));
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn attach(
        self,
        name: impl Into<String>,
        contents: impl Into<Vec<u8>>,
        filename: impl Into<String>,
    ) -> Self {
        self.attach_part(Part::new(name, contents.into()).filename(filename))
    }

    /// Attach a file to the request with headers for the part.
    pub fn attach_with_headers(
        self,
        name: impl Into<String>,
        contents: impl Into<Vec<u8>>,
        filename: impl Into<String>,
        headers: &[(&str, &str)],
    ) -> Self {
        let part = headers.iter().fold(
            Part::new(name, contents.into()).filename(filename),
            |part, (name, value)| part.header(*name, *value),
        );
        self.attach_part(part)
    }

    /// Attach a multipart [`Part`] to the request.
    pub fn attach_part(mut self, part: Part) -> Self {
        self = self.as_multipart();
        self.pending_files.push(part);
        self
    }

    // ------------------------------------------------------------------
    // Headers
    // ------------------------------------------------------------------

    /// Add the given headers to the request.
    ///
    /// Headers merge with those already set: setting a header twice sends
    /// both values.
    pub fn with_headers(
        mut self,
        headers: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        for (name, value) in headers {
            self.merge_header(name.into(), Value::String(value.into()));
        }
        self
    }

    /// Add the given header to the request.
    pub fn with_header(self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.with_headers([(name.into(), value.into())])
    }

    /// Replace the given headers on the request.
    pub fn replace_headers(
        mut self,
        headers: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        for (name, value) in headers {
            self = self.set_header(&name.into(), value.into());
        }
        self
    }

    /// Specify the request's content type.
    pub fn content_type(self, content_type: &str) -> Self {
        self.set_header("Content-Type", content_type.to_string())
    }

    /// Indicate that JSON should be returned by the server.
    pub fn accept_json(self) -> Self {
        self.accept("application/json")
    }

    /// Indicate the type of content that should be returned by the server.
    pub fn accept(self, content_type: &str) -> Self {
        self.with_headers([("Accept", content_type)])
    }

    /// Specify the user agent for the request.
    pub fn with_user_agent(self, user_agent: &str) -> Self {
        self.set_header("User-Agent", user_agent.trim().to_string())
    }

    fn headers_mut(&mut self) -> &mut Map<String, Value> {
        let headers = self
            .options
            .entry("headers")
            .or_insert_with(|| Value::Object(Map::new()));
        if !headers.is_object() {
            *headers = Value::Object(Map::new());
        }
        headers.as_object_mut().expect("headers are an object")
    }

    fn header_key(&mut self, name: &str) -> Option<String> {
        self.headers_mut()
            .keys()
            .find(|key| key.eq_ignore_ascii_case(name))
            .cloned()
    }

    fn merge_header(&mut self, name: String, value: Value) {
        let key = self.header_key(&name).unwrap_or(name);
        let mut incoming = Map::new();
        incoming.insert(key, value);
        merge_recursive(self.headers_mut(), incoming);
    }

    fn set_header(mut self, name: &str, value: String) -> Self {
        self.remove_header(name);
        self.headers_mut()
            .insert(name.to_string(), Value::String(value));
        self
    }

    fn remove_header(&mut self, name: &str) {
        if let Some(key) = self.header_key(name) {
            self.headers_mut().remove(&key);
        }
    }

    // ------------------------------------------------------------------
    // Authentication & cookies
    // ------------------------------------------------------------------

    /// Specify the basic authentication username and password for the request.
    pub fn with_basic_auth(mut self, username: &str, password: &str) -> Self {
        self.options
            .insert("auth".into(), json!([username, password]));
        self
    }

    /// Specify the digest authentication username and password for the request.
    pub fn with_digest_auth(mut self, username: &str, password: &str) -> Self {
        self.options
            .insert("auth".into(), json!([username, password, "digest"]));
        self
    }

    /// Specify an authorization token for the request.
    ///
    /// ```
    /// # use illuminate_http_client::Http;
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// Http::fake();
    ///
    /// Http::with_token("secret", "Bearer").get("https://example.com").await?;
    ///
    /// Http::assert_sent(|request| request.has_header_value("Authorization", "Bearer secret"));
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn with_token(self, token: &str, token_type: &str) -> Self {
        let value = format!("{token_type} {token}").trim().to_string();
        self.set_header("Authorization", value)
    }

    /// Specify the cookies that should be included with the request.
    pub fn with_cookies(
        mut self,
        cookies: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
        domain: &str,
    ) -> Self {
        let cookies: Vec<Value> = cookies
            .into_iter()
            .map(|(name, value)| json!({"name": name.into(), "value": value.into(), "domain": domain}))
            .collect();

        let mut incoming = Map::new();
        incoming.insert("cookies".into(), Value::Array(cookies));
        merge_recursive(&mut self.options, incoming);
        self
    }

    // ------------------------------------------------------------------
    // Transfer options
    // ------------------------------------------------------------------

    /// Specify the maximum number of redirects to allow (the default is 5).
    pub fn max_redirects(mut self, max: usize) -> Self {
        let mut redirects = match self.options.get("allow_redirects") {
            Some(Value::Object(existing)) => existing.clone(),
            _ => Map::new(),
        };
        redirects.insert("max".into(), max.into());
        self.options
            .insert("allow_redirects".into(), Value::Object(redirects));
        self
    }

    /// Indicate that redirects should not be followed.
    pub fn without_redirecting(mut self) -> Self {
        self.options
            .insert("allow_redirects".into(), Value::Bool(false));
        self
    }

    /// Indicate that TLS certificates should not be verified.
    ///
    /// The connection is still encrypted, but the server's identity is not
    /// checked; only use this for local development.
    pub fn without_verifying(mut self) -> Self {
        self.options.insert("verify".into(), Value::Bool(false));
        self
    }

    /// Specify the path where the body of the response should be stored.
    pub fn sink(mut self, path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_string_lossy().into_owned();
        self.options.insert("sink".into(), Value::String(path));
        self
    }

    /// Specify the timeout (in seconds) for the request. Zero waits forever.
    pub fn timeout(mut self, seconds: impl Into<f64>) -> Self {
        self.options
            .insert("timeout".into(), seconds_value(seconds.into()));
        self
    }

    /// Specify the connect timeout (in seconds) for the request.
    pub fn connect_timeout(mut self, seconds: impl Into<f64>) -> Self {
        self.options
            .insert("connect_timeout".into(), seconds_value(seconds.into()));
        self
    }

    /// Replace the specified (Guzzle-style) options on the request.
    ///
    /// The supported options are `timeout`, `connect_timeout`,
    /// `allow_redirects` (`false` or `{"max": 10}`), `verify` (`false`, or
    /// the path of a PEM certificate bundle), `headers`, `query`, `auth`
    /// (`[user, password]` or `[user, password, "digest"]`), `cookies` and
    /// `sink`. Any other options are kept, and returned by
    /// [`PendingRequest::get_options`].
    pub fn with_options(mut self, options: Value) -> Self {
        let mut current = Value::Object(std::mem::take(&mut self.options));
        replace_recursive(&mut current, options);
        if let Value::Object(options) = current {
            self.options = options;
        }
        self
    }

    /// Get the pending request options.
    pub fn get_options(&self) -> Value {
        Value::Object(self.options.clone())
    }

    // ------------------------------------------------------------------
    // Retries
    // ------------------------------------------------------------------

    /// Specify the number of times the request should be attempted, and
    /// the number of milliseconds to wait between attempts.
    ///
    /// Pass a list of delays instead of a number of times to back off:
    /// `retry([100, 200, 500], 0)` attempts the request four times.
    ///
    /// If every attempt fails, the last failure is returned as an error
    /// (see [`PendingRequest::retry_throw`]).
    pub fn retry(mut self, times: impl Into<Tries>, sleep_milliseconds: u64) -> Self {
        self.tries = times.into();
        self.retry_delay = RetryDelay::Milliseconds(sleep_milliseconds);
        self.retry_when = None;
        self.retry_throw = true;
        self
    }

    /// Retry the request, calculating the milliseconds to sleep before
    /// each attempt with the given closure (receiving the attempt number
    /// and the error).
    pub fn retry_using<F>(mut self, times: impl Into<Tries>, sleep: F) -> Self
    where
        F: Fn(u32, &Error) -> u64 + Send + Sync + 'static,
    {
        self = self.retry(times, 0);
        self.retry_delay = RetryDelay::Using(Arc::new(sleep));
        self
    }

    /// Only retry when the given closure returns `true`. The closure
    /// receives the error and the pending request, which it may modify
    /// before the next attempt.
    ///
    /// ```
    /// use illuminate_http_client::{ConnectionException, Http, RequestException};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// Http::fake_sequence("*").push_status(401).push("ok", 200);
    ///
    /// let response = Http::with_token("expired", "Bearer")
    ///     .retry(2, 0)
    ///     .retry_when(|exception, request| {
    ///         let unauthorized = exception
    ///             .downcast_ref::<RequestException>()
    ///             .is_some_and(|e| e.response.status() == 401);
    ///
    ///         if unauthorized {
    ///             *request = request.clone().with_token("fresh", "Bearer");
    ///         }
    ///
    ///         unauthorized || exception.is::<ConnectionException>()
    ///     })
    ///     .get("https://example.com")
    ///     .await?;
    ///
    /// assert_eq!(response.body(), "ok");
    /// Http::assert_sent(|request| request.has_header_value("Authorization", "Bearer fresh"));
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn retry_when<F>(mut self, when: F) -> Self
    where
        F: Fn(&Error, &mut PendingRequest) -> bool + Send + Sync + 'static,
    {
        self.retry_when = Some(Arc::new(when));
        self
    }

    /// Determine if an error is returned once every retry has failed
    /// (`true` by default). When `false`, the last response is returned
    /// instead; connection failures are always returned as errors.
    pub fn retry_throw(mut self, throw: bool) -> Self {
        self.retry_throw = throw;
        self
    }

    // ------------------------------------------------------------------
    // Middleware & callbacks
    // ------------------------------------------------------------------

    /// Add a middleware to the request.
    pub fn with_middleware<F, Fut>(mut self, middleware: F) -> Self
    where
        F: Fn(Request, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response>> + Send + 'static,
    {
        self.middleware.push(Middleware::new(middleware));
        self
    }

    /// Add a middleware that maps the outgoing request.
    pub fn with_request_middleware<F>(mut self, middleware: F) -> Self
    where
        F: Fn(Request) -> Request + Send + Sync + 'static,
    {
        self.middleware.push(Middleware::map_request(middleware));
        self
    }

    /// Add a middleware that maps the incoming response.
    pub fn with_response_middleware<F>(mut self, middleware: F) -> Self
    where
        F: Fn(Response) -> Response + Send + Sync + 'static,
    {
        self.middleware.push(Middleware::map_response(middleware));
        self
    }

    /// Set arbitrary attributes to store with the request.
    pub fn with_attributes(mut self, attributes: Value) -> Self {
        if let Value::Object(attributes) = attributes {
            merge_recursive(&mut self.attributes, attributes);
        }
        self
    }

    /// Add a callback to run before the request is sent; it may modify
    /// the request.
    pub fn before_sending<F>(mut self, callback: F) -> Self
    where
        F: Fn(&mut Request) + Send + Sync + 'static,
    {
        self.before_sending.push(Arc::new(callback));
        self
    }

    /// Add a callback to run after the response is built; it may return a
    /// different response.
    pub fn after_response<F>(mut self, callback: F) -> Self
    where
        F: Fn(Response, &Request) -> Response + Send + Sync + 'static,
    {
        self.after_response.push(Arc::new(callback));
        self
    }

    /// Dump the request before it is sent.
    pub fn dump(self) -> Self {
        self.before_sending(|request| println!("{request:#?}"))
    }

    /// Dump the request before it is sent, then end the process.
    pub fn dd(self) -> Self {
        self.before_sending(|request| {
            println!("{request:#?}");
            std::process::exit(1);
        })
    }

    // ------------------------------------------------------------------
    // Errors
    // ------------------------------------------------------------------

    /// Return a [`RequestException`] error if a server or client error occurs.
    pub fn throw(self) -> Self {
        self.throw_with(|_, _| {})
    }

    /// Return an error if a server or client error occurs, invoking the
    /// callback first.
    pub fn throw_with<F>(mut self, callback: F) -> Self
    where
        F: Fn(&Response, &RequestException) + Send + Sync + 'static,
    {
        self.throw_callback = Some(Arc::new(callback));
        self
    }

    /// Return an error if a server or client error occurs and the given
    /// condition is true.
    pub fn throw_if(self, condition: bool) -> Self {
        if condition { self.throw() } else { self }
    }

    /// Return an error if a server or client error occurs and the given
    /// callback returns true.
    pub fn throw_if_fn<F>(mut self, condition: F) -> Self
    where
        F: Fn(&Response) -> bool + Send + Sync + 'static,
    {
        self.throw_if_callback = Some(Arc::new(condition));
        self.throw()
    }

    /// Return an error if a server or client error occurs and the given
    /// condition is false.
    pub fn throw_unless(self, condition: bool) -> Self {
        self.throw_if(!condition)
    }

    /// Return an error if a server or client error occurs and the given
    /// callback returns false.
    pub fn throw_unless_fn<F>(self, condition: F) -> Self
    where
        F: Fn(&Response) -> bool + Send + Sync + 'static,
    {
        self.throw_if_fn(move |response| !condition(response))
    }

    /// Truncate response bodies in request exception messages at the given length.
    pub fn truncate_exceptions_at(mut self, length: usize) -> Self {
        self.truncation = Some(Truncation::At(length.max(1)));
        self
    }

    /// Include whole responses in request exception messages.
    pub fn dont_truncate_exceptions(mut self) -> Self {
        self.truncation = Some(Truncation::Never);
        self
    }

    // ------------------------------------------------------------------
    // Sending
    // ------------------------------------------------------------------

    /// Issue a `GET` request to the given URL.
    pub fn get(self, url: impl Into<String>) -> ResponseFuture {
        self.dispatch(Method::GET, url.into(), CallOptions::default())
    }

    /// Issue a `GET` request to the given URL with the given query parameters.
    pub fn get_with(self, url: impl Into<String>, query: impl Serialize) -> ResponseFuture {
        let query = Some(normalize_pairs(to_value(&query)));
        self.dispatch(Method::GET, url.into(), CallOptions { query, data: None })
    }

    /// Issue a `HEAD` request to the given URL.
    pub fn head(self, url: impl Into<String>) -> ResponseFuture {
        self.dispatch(Method::HEAD, url.into(), CallOptions::default())
    }

    /// Issue a `HEAD` request to the given URL with the given query parameters.
    pub fn head_with(self, url: impl Into<String>, query: impl Serialize) -> ResponseFuture {
        let query = Some(normalize_pairs(to_value(&query)));
        self.dispatch(Method::HEAD, url.into(), CallOptions { query, data: None })
    }

    /// Issue a `POST` request to the given URL with the given data.
    ///
    /// The data is sent as JSON unless [`PendingRequest::as_form`] or
    /// [`PendingRequest::as_multipart`] say otherwise. Pass `()` to send no data.
    pub fn post(self, url: impl Into<String>, data: impl Serialize) -> ResponseFuture {
        self.dispatch_with_data(Method::POST, url.into(), data)
    }

    /// Issue a `PUT` request to the given URL with the given data.
    pub fn put(self, url: impl Into<String>, data: impl Serialize) -> ResponseFuture {
        self.dispatch_with_data(Method::PUT, url.into(), data)
    }

    /// Issue a `PATCH` request to the given URL with the given data.
    pub fn patch(self, url: impl Into<String>, data: impl Serialize) -> ResponseFuture {
        self.dispatch_with_data(Method::PATCH, url.into(), data)
    }

    /// Issue a `DELETE` request to the given URL.
    pub fn delete(self, url: impl Into<String>) -> ResponseFuture {
        self.dispatch(Method::DELETE, url.into(), CallOptions::default())
    }

    /// Issue a `DELETE` request to the given URL with the given data.
    pub fn delete_with(self, url: impl Into<String>, data: impl Serialize) -> ResponseFuture {
        self.dispatch_with_data(Method::DELETE, url.into(), data)
    }

    /// Issue a `QUERY` request to the given URL with the given data.
    pub fn query(self, url: impl Into<String>, data: impl Serialize) -> ResponseFuture {
        let method = Method::from_bytes(b"QUERY").expect("QUERY is a valid method");
        self.dispatch_with_data(method, url.into(), data)
    }

    /// Send the request with the given method, using the pending body or
    /// attached files.
    pub fn send(self, method: &str, url: impl Into<String>) -> ResponseFuture {
        match Method::from_bytes(method.to_ascii_uppercase().as_bytes()) {
            Ok(method) => self.dispatch(method, url.into(), CallOptions::default()),
            Err(error) => ResponseFuture::failed(self.pool_key, error.into()),
        }
    }

    /// Send the request with the given method and data.
    pub fn send_with(
        self,
        method: &str,
        url: impl Into<String>,
        data: impl Serialize,
    ) -> ResponseFuture {
        match Method::from_bytes(method.to_ascii_uppercase().as_bytes()) {
            Ok(method) => self.dispatch_with_data(method, url.into(), data),
            Err(error) => ResponseFuture::failed(self.pool_key, error.into()),
        }
    }

    fn dispatch_with_data(
        self,
        method: Method,
        url: String,
        data: impl Serialize,
    ) -> ResponseFuture {
        match serde_json::to_value(&data) {
            Ok(data) => self.dispatch(
                method,
                url,
                CallOptions {
                    query: None,
                    data: Some(data),
                },
            ),
            Err(error) => ResponseFuture::failed(self.pool_key, error.into()),
        }
    }

    fn dispatch(self, method: Method, url: String, call: CallOptions) -> ResponseFuture {
        let key = self.pool_key.clone();
        ResponseFuture::new(key, Box::pin(self.execute(method, url, call)))
    }

    pub(crate) fn pool_key(mut self, key: String) -> Self {
        self.pool_key = Some(key);
        self
    }
}

fn seconds_value(seconds: f64) -> Value {
    let seconds = seconds.max(0.0);
    if seconds.fract() == 0.0 && seconds < u32::MAX as f64 {
        Value::from(seconds as u64)
    } else {
        Value::from(seconds)
    }
}

impl Conditionable for PendingRequest {}
impl Tappable for PendingRequest {}

impl fmt::Debug for PendingRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingRequest")
            .field("base_url", &self.base_url)
            .field("body_format", &self.body_format)
            .field("options", &self.options)
            .field("tries", &self.tries)
            .finish_non_exhaustive()
    }
}

/// The response to a request that has been built but not yet sent.
///
/// Requests are lazy: nothing happens until the future is awaited (or
/// handed to [`Http::pool`](crate::Http::pool)).
#[must_use = "requests are lazy and do nothing unless you `.await` them"]
pub struct ResponseFuture {
    key: Option<String>,
    inner: BoxFuture<'static, Result<Response>>,
}

impl ResponseFuture {
    pub(crate) fn new(key: Option<String>, inner: BoxFuture<'static, Result<Response>>) -> Self {
        Self { key, inner }
    }

    pub(crate) fn failed(key: Option<String>, error: Error) -> Self {
        Self::new(key, Box::pin(async move { Err(error) }))
    }

    /// The name given to the request in a pool, if any.
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }
}

impl Future for ResponseFuture {
    type Output = Result<Response>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

impl fmt::Debug for ResponseFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseFuture")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}
