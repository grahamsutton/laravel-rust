//! The `Http` facade.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use illuminate_container::Container;
use illuminate_support::{Collection, Error, Result, Value};
use serde::Serialize;

use crate::batch::Batch;
use crate::encoding::Part;
use crate::exceptions::RequestException;
use crate::factory::{Factory, RecordedPair};
use crate::fake::{FakeResponse, ResponseSequence};
use crate::middleware::{BoxFuture, Next};
use crate::pending_request::{BodyFormat, PendingRequest, ResponseFuture, Tries};
use crate::pool::{Pool, PoolResponses};
use crate::request::Request;
use crate::response::Response;

/// The `Http` facade: an expressive, minimal API for making HTTP requests.
///
/// ```
/// use illuminate_http_client::{Http, Request};
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// Http::fake_urls([("example.com/*", Http::response(json!({"id": 1, "name": "Steve"}), 201, &[]))]);
///
/// let response = Http::post("http://example.com/users", json!({
///     "name": "Steve",
///     "role": "Network Administrator",
/// }))
/// .await?;
///
/// assert!(response.created());
/// assert_eq!(response["name"], "Steve");
///
/// Http::assert_sent(|request: &Request| {
///     request.method() == "POST" && request["role"] == "Network Administrator"
/// });
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
///
/// The facade resolves the [`Factory`] from the container at call time
/// (registering one if the application hasn't), so tests that install
/// their own container get their own fakes and recorded requests.
#[derive(Clone, Copy, Debug, Default)]
pub struct Http;

impl Http {
    /// Get the HTTP client factory from the container, registering one if
    /// the application hasn't yet.
    pub fn factory() -> Arc<Factory> {
        let container = Container::get_instance();
        if let Ok(factory) = container.try_make::<Factory>() {
            return factory;
        }
        container.singleton_if::<Factory>(|_| Arc::new(Factory::new()));
        container.make::<Factory>()
    }

    /// Create a new pending request.
    pub fn new_request() -> PendingRequest {
        Self::factory().create_pending_request()
    }

    /// Begin a request with the given middleware. See [`PendingRequest::with_middleware`].
    pub fn with_middleware<F, Fut>(middleware: F) -> PendingRequest
    where
        F: Fn(Request, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response>> + Send + 'static,
    {
        Self::new_request().with_middleware(middleware)
    }

    // ------------------------------------------------------------------
    // Concurrency
    // ------------------------------------------------------------------

    /// Send a pool of requests concurrently, returning their responses in
    /// the order they were added.
    ///
    /// ```
    /// use illuminate_http_client::Http;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// Http::fake();
    ///
    /// let responses = Http::pool(|pool| vec![
    ///     pool.as_("first").get("http://localhost/first"),
    ///     pool.as_("second").get("http://localhost/second"),
    /// ])
    /// .await;
    ///
    /// assert!(responses["first"].ok() && responses["second"].ok());
    /// # });
    /// ```
    pub fn pool<F>(callback: F) -> BoxFuture<'static, PoolResponses>
    where
        F: FnOnce(&Pool) -> Vec<ResponseFuture>,
    {
        Self::factory().pool(callback)
    }

    /// Send a pool of requests with at most `concurrency` requests in
    /// flight at once.
    pub fn pool_with_concurrency<F>(
        callback: F,
        concurrency: usize,
    ) -> BoxFuture<'static, PoolResponses>
    where
        F: FnOnce(&Pool) -> Vec<ResponseFuture>,
    {
        Self::factory().pool_with_concurrency(callback, concurrency)
    }

    /// Create a batch of concurrent requests with completion callbacks.
    /// See [`Batch`].
    pub fn batch<F>(callback: F) -> Batch
    where
        F: FnOnce(&Pool) -> Vec<ResponseFuture>,
    {
        Self::factory().batch(callback)
    }

    // ------------------------------------------------------------------
    // Global configuration
    // ------------------------------------------------------------------

    /// Set the options applied to every request.
    pub fn global_options(options: Value) -> Arc<Factory> {
        let factory = Self::factory();
        factory.global_options(options);
        factory
    }

    /// Add middleware applied to every request.
    pub fn global_middleware<F, Fut>(middleware: F) -> Arc<Factory>
    where
        F: Fn(Request, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response>> + Send + 'static,
    {
        let factory = Self::factory();
        factory.global_middleware(middleware);
        factory
    }

    /// Add request middleware applied to every request.
    ///
    /// ```
    /// use illuminate_http_client::Http;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// Http::global_request_middleware(|request| request.with_header("User-Agent", "Example Application/1.0"));
    /// Http::fake();
    ///
    /// Http::get("https://laravel.com").await?;
    ///
    /// Http::assert_sent(|request| request.has_header_value("User-Agent", "Example Application/1.0"));
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn global_request_middleware<F>(middleware: F) -> Arc<Factory>
    where
        F: Fn(Request) -> Request + Send + Sync + 'static,
    {
        let factory = Self::factory();
        factory.global_request_middleware(middleware);
        factory
    }

    /// Add response middleware applied to every request.
    pub fn global_response_middleware<F>(middleware: F) -> Arc<Factory>
    where
        F: Fn(Response) -> Response + Send + Sync + 'static,
    {
        let factory = Self::factory();
        factory.global_response_middleware(middleware);
        factory
    }

    /// Execute a callback while requests are created without global
    /// middleware or options.
    pub fn without_global_configuration<R>(callback: impl FnOnce() -> R) -> R {
        Self::factory().without_global_configuration(callback)
    }

    // ------------------------------------------------------------------
    // Faking
    // ------------------------------------------------------------------

    /// Fake every request with an empty `200` response, and start
    /// recording requests.
    pub fn fake() -> Arc<Factory> {
        let factory = Self::factory();
        factory.fake();
        factory
    }

    /// Fake requests whose URLs match the given patterns (`*` is a
    /// wildcard). Requests to other URLs are sent as normal.
    pub fn fake_urls(
        stubs: impl IntoIterator<Item = (impl Into<String>, impl Into<FakeResponse>)>,
    ) -> Arc<Factory> {
        let factory = Self::factory();
        factory.fake_urls(stubs);
        factory
    }

    /// Fake requests whose URLs match the given pattern.
    pub fn stub_url(pattern: impl Into<String>, fake: impl Into<FakeResponse>) -> Arc<Factory> {
        let factory = Self::factory();
        factory.stub_url(pattern, fake);
        factory
    }

    /// Fake requests with the given callback.
    ///
    /// ```
    /// use illuminate_http_client::{Http, Request};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// Http::fake_using(|request: &Request| {
    ///     Http::response(format!("You asked for {}", request.url()), 200, &[])
    /// });
    ///
    /// let response = Http::get("https://laravel.com").await?;
    /// assert_eq!(response.body(), "You asked for https://laravel.com");
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn fake_using<F, R>(callback: F) -> Arc<Factory>
    where
        F: Fn(&Request) -> R + Send + Sync + 'static,
        R: Into<FakeResponse>,
    {
        let factory = Self::factory();
        factory.fake_using(callback);
        factory
    }

    /// Fake requests matching the pattern with a sequence of responses.
    pub fn fake_sequence(pattern: impl Into<String>) -> ResponseSequence {
        Self::factory().fake_sequence(pattern)
    }

    /// Create a new response sequence.
    pub fn sequence() -> ResponseSequence {
        Self::factory().sequence()
    }

    /// Create a new response sequence from the given responses.
    pub fn sequence_of(
        responses: impl IntoIterator<Item = impl Into<FakeResponse>>,
    ) -> ResponseSequence {
        Self::factory().sequence_of(responses)
    }

    /// Create a fake response. Strings are sent as-is, other values as JSON.
    pub fn response(body: impl Into<Value>, status: u16, headers: &[(&str, &str)]) -> FakeResponse {
        Factory::response(body, status, headers)
    }

    /// Create a fake that fails to connect.
    pub fn failed_connection() -> FakeResponse {
        Factory::failed_connection()
    }

    /// Create a fake that fails to connect with the given message.
    pub fn failed_connection_with(message: impl Into<String>) -> FakeResponse {
        Factory::failed_connection_with(message)
    }

    /// Create a request exception, for use while stubbing.
    pub fn failed_request(
        body: impl Into<Value>,
        status: u16,
        headers: &[(&str, &str)],
    ) -> RequestException {
        Factory::failed_request(body, status, headers)
    }

    /// Return an error for any request that doesn't have a matching fake.
    ///
    /// ```
    /// use illuminate_http_client::{Http, StrayRequestException};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// Http::prevent_stray_requests();
    /// Http::allow_stray_requests(["http://127.0.0.1:5000/*"]);
    /// Http::fake_urls([("github.com/*", "ok")]);
    ///
    /// assert_eq!(Http::get("https://github.com/laravel/framework").await?.body(), "ok");
    ///
    /// let error = Http::get("https://laravel.com").await.unwrap_err();
    /// assert!(error.is::<StrayRequestException>());
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn prevent_stray_requests() -> Arc<Factory> {
        let factory = Self::factory();
        factory.prevent_stray_requests(true);
        factory
    }

    /// Determine if stray requests are being prevented.
    pub fn preventing_stray_requests() -> bool {
        Self::factory().preventing_stray_requests()
    }

    /// Allow stray requests to URLs matching the given patterns.
    pub fn allow_stray_requests(
        patterns: impl IntoIterator<Item = impl Into<String>>,
    ) -> Arc<Factory> {
        let factory = Self::factory();
        factory.allow_stray_requests(patterns);
        factory
    }

    // ------------------------------------------------------------------
    // Recording & assertions
    // ------------------------------------------------------------------

    /// Begin recording requests, without faking them.
    pub fn record() -> Arc<Factory> {
        let factory = Self::factory();
        factory.record();
        factory
    }

    /// Get every recorded request / response pair.
    pub fn recorded() -> Collection<RecordedPair> {
        Self::factory().recorded()
    }

    /// Get the recorded request / response pairs matching the truth test.
    pub fn recorded_fn(
        callback: impl Fn(&Request, Option<&Response>) -> bool,
    ) -> Collection<RecordedPair> {
        Self::factory().recorded_fn(callback)
    }

    /// Assert that a request matching the truth test was sent.
    #[track_caller]
    pub fn assert_sent(callback: impl Fn(&Request) -> bool) {
        Self::factory().assert_sent(callback)
    }

    /// Assert that requests were sent to the given URLs, in order.
    #[track_caller]
    pub fn assert_sent_in_order(urls: &[&str]) {
        Self::factory().assert_sent_in_order(urls)
    }

    /// Assert that no request matching the truth test was sent.
    #[track_caller]
    pub fn assert_not_sent(callback: impl Fn(&Request) -> bool) {
        Self::factory().assert_not_sent(callback)
    }

    /// Assert that no requests were sent.
    #[track_caller]
    pub fn assert_nothing_sent() {
        Self::factory().assert_nothing_sent()
    }

    /// Assert how many requests were sent.
    #[track_caller]
    pub fn assert_sent_count(count: usize) {
        Self::factory().assert_sent_count(count)
    }

    /// Assert that every response sequence has been used up.
    #[track_caller]
    pub fn assert_sequences_are_empty() {
        Self::factory().assert_sequences_are_empty()
    }
}

/// Generate the methods that begin a request, on both [`Http`] (as
/// associated functions) and [`Pool`] (as methods).
macro_rules! request_starters {
    ($( $(#[$meta:meta])* fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty; )*) => {
        impl Http {
            $(
                $(#[$meta])*
                pub fn $name($($arg: $ty),*) -> $ret {
                    Self::new_request().$name($($arg),*)
                }
            )*
        }

        impl Pool {
            $(
                $(#[$meta])*
                pub fn $name(&self, $($arg: $ty),*) -> $ret {
                    self.new_request().$name($($arg),*)
                }
            )*
        }
    };
}

request_starters! {
    /// Begin a request to the given base URL. See [`PendingRequest::base_url`].
    fn base_url(url: impl Into<String>) -> PendingRequest;
    /// Begin a request with URI template parameters. See [`PendingRequest::with_url_parameters`].
    fn with_url_parameters(parameters: impl Serialize) -> PendingRequest;
    /// Begin a request with query parameters. See [`PendingRequest::with_query_parameters`].
    fn with_query_parameters(parameters: impl Serialize) -> PendingRequest;
    /// Begin a request with a raw body. See [`PendingRequest::with_body`].
    fn with_body(content: impl Into<Bytes>, content_type: &str) -> PendingRequest;
    /// Begin a JSON request. See [`PendingRequest::as_json`].
    fn as_json() -> PendingRequest;
    /// Begin a form request. See [`PendingRequest::as_form`].
    fn as_form() -> PendingRequest;
    /// Begin a multipart request. See [`PendingRequest::as_multipart`].
    fn as_multipart() -> PendingRequest;
    /// Begin a request with the given body format. See [`PendingRequest::body_format`].
    fn body_format(format: BodyFormat) -> PendingRequest;
    /// Begin a request with a file attached. See [`PendingRequest::attach`].
    fn attach(name: impl Into<String>, contents: impl Into<Vec<u8>>, filename: impl Into<String>) -> PendingRequest;
    /// Begin a request with a file (and part headers) attached. See [`PendingRequest::attach_with_headers`].
    fn attach_with_headers(
        name: impl Into<String>,
        contents: impl Into<Vec<u8>>,
        filename: impl Into<String>,
        headers: &[(&str, &str)]
    ) -> PendingRequest;
    /// Begin a request with a multipart part attached. See [`PendingRequest::attach_part`].
    fn attach_part(part: Part) -> PendingRequest;
    /// Begin a request with the given headers. See [`PendingRequest::with_headers`].
    fn with_headers(headers: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>) -> PendingRequest;
    /// Begin a request with the given header. See [`PendingRequest::with_header`].
    fn with_header(name: impl Into<String>, value: impl Into<String>) -> PendingRequest;
    /// Begin a request replacing the given headers. See [`PendingRequest::replace_headers`].
    fn replace_headers(headers: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>) -> PendingRequest;
    /// Begin a request with the given content type. See [`PendingRequest::content_type`].
    fn content_type(content_type: &str) -> PendingRequest;
    /// Begin a request that accepts JSON. See [`PendingRequest::accept_json`].
    fn accept_json() -> PendingRequest;
    /// Begin a request that accepts the given content type. See [`PendingRequest::accept`].
    fn accept(content_type: &str) -> PendingRequest;
    /// Begin a request with the given user agent. See [`PendingRequest::with_user_agent`].
    fn with_user_agent(user_agent: &str) -> PendingRequest;
    /// Begin a request with basic authentication. See [`PendingRequest::with_basic_auth`].
    fn with_basic_auth(username: &str, password: &str) -> PendingRequest;
    /// Begin a request with digest authentication. See [`PendingRequest::with_digest_auth`].
    fn with_digest_auth(username: &str, password: &str) -> PendingRequest;
    /// Begin a request with an authorization token. See [`PendingRequest::with_token`].
    fn with_token(token: &str, token_type: &str) -> PendingRequest;
    /// Begin a request with cookies. See [`PendingRequest::with_cookies`].
    fn with_cookies(cookies: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>, domain: &str) -> PendingRequest;
    /// Begin a request following at most `max` redirects. See [`PendingRequest::max_redirects`].
    fn max_redirects(max: usize) -> PendingRequest;
    /// Begin a request that doesn't follow redirects. See [`PendingRequest::without_redirecting`].
    fn without_redirecting() -> PendingRequest;
    /// Begin a request that doesn't verify TLS certificates. See [`PendingRequest::without_verifying`].
    fn without_verifying() -> PendingRequest;
    /// Begin a request that stores its response body at the given path. See [`PendingRequest::sink`].
    fn sink(path: impl AsRef<Path>) -> PendingRequest;
    /// Begin a request with a timeout, in seconds. See [`PendingRequest::timeout`].
    fn timeout(seconds: impl Into<f64>) -> PendingRequest;
    /// Begin a request with a connect timeout, in seconds. See [`PendingRequest::connect_timeout`].
    fn connect_timeout(seconds: impl Into<f64>) -> PendingRequest;
    /// Begin a request with the given options. See [`PendingRequest::with_options`].
    fn with_options(options: Value) -> PendingRequest;
    /// Begin a request that is retried. See [`PendingRequest::retry`].
    fn retry(times: impl Into<Tries>, sleep_milliseconds: u64) -> PendingRequest;
    /// Begin a request that is retried, sleeping as the closure says. See [`PendingRequest::retry_using`].
    fn retry_using(times: impl Into<Tries>, sleep: impl Fn(u32, &Error) -> u64 + Send + Sync + 'static) -> PendingRequest;
    /// Begin a request with request attributes. See [`PendingRequest::with_attributes`].
    fn with_attributes(attributes: Value) -> PendingRequest;
    /// Begin a request with request middleware. See [`PendingRequest::with_request_middleware`].
    fn with_request_middleware(middleware: impl Fn(Request) -> Request + Send + Sync + 'static) -> PendingRequest;
    /// Begin a request with response middleware. See [`PendingRequest::with_response_middleware`].
    fn with_response_middleware(middleware: impl Fn(Response) -> Response + Send + Sync + 'static) -> PendingRequest;
    /// Begin a request with a "before sending" callback. See [`PendingRequest::before_sending`].
    fn before_sending(callback: impl Fn(&mut Request) + Send + Sync + 'static) -> PendingRequest;
    /// Begin a request with an "after response" callback. See [`PendingRequest::after_response`].
    fn after_response(callback: impl Fn(Response, &Request) -> Response + Send + Sync + 'static) -> PendingRequest;
    /// Begin a request that returns errors for failed responses. See [`PendingRequest::throw`].
    fn throw() -> PendingRequest;
    /// Begin a request that returns errors for failed responses, after the callback. See [`PendingRequest::throw_with`].
    fn throw_with(callback: impl Fn(&Response, &RequestException) + Send + Sync + 'static) -> PendingRequest;
    /// Begin a request that conditionally returns errors. See [`PendingRequest::throw_if`].
    fn throw_if(condition: bool) -> PendingRequest;
    /// Begin a request that conditionally returns errors. See [`PendingRequest::throw_if_fn`].
    fn throw_if_fn(condition: impl Fn(&Response) -> bool + Send + Sync + 'static) -> PendingRequest;
    /// Begin a request that conditionally returns errors. See [`PendingRequest::throw_unless`].
    fn throw_unless(condition: bool) -> PendingRequest;
    /// Begin a request that conditionally returns errors. See [`PendingRequest::throw_unless_fn`].
    fn throw_unless_fn(condition: impl Fn(&Response) -> bool + Send + Sync + 'static) -> PendingRequest;
    /// Begin a request truncating exception messages. See [`PendingRequest::truncate_exceptions_at`].
    fn truncate_exceptions_at(length: usize) -> PendingRequest;
    /// Begin a request with untruncated exception messages. See [`PendingRequest::dont_truncate_exceptions`].
    fn dont_truncate_exceptions() -> PendingRequest;
    /// Begin a request that is dumped before it is sent. See [`PendingRequest::dump`].
    fn dump() -> PendingRequest;
    /// Begin a request that is dumped before it is sent, ending the process. See [`PendingRequest::dd`].
    fn dd() -> PendingRequest;

    /// Issue a `GET` request to the given URL.
    ///
    /// ```
    /// use illuminate_http_client::Http;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// Http::fake();
    ///
    /// let response = Http::get("http://example.com").await?;
    ///
    /// assert!(response.successful());
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    fn get(url: impl Into<String>) -> ResponseFuture;
    /// Issue a `GET` request with query parameters.
    ///
    /// ```
    /// use illuminate_http_client::Http;
    /// use illuminate_support::json;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// Http::fake();
    ///
    /// Http::get_with("http://example.com/users", json!({"name": "Taylor", "page": 1})).await?;
    ///
    /// Http::assert_sent(|request| request.url() == "http://example.com/users?name=Taylor&page=1");
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    fn get_with(url: impl Into<String>, query: impl Serialize) -> ResponseFuture;
    /// Issue a `HEAD` request to the given URL.
    fn head(url: impl Into<String>) -> ResponseFuture;
    /// Issue a `HEAD` request with query parameters.
    fn head_with(url: impl Into<String>, query: impl Serialize) -> ResponseFuture;
    /// Issue a `POST` request with the given data (sent as JSON by default).
    fn post(url: impl Into<String>, data: impl Serialize) -> ResponseFuture;
    /// Issue a `PUT` request with the given data.
    fn put(url: impl Into<String>, data: impl Serialize) -> ResponseFuture;
    /// Issue a `PATCH` request with the given data.
    fn patch(url: impl Into<String>, data: impl Serialize) -> ResponseFuture;
    /// Issue a `DELETE` request to the given URL.
    fn delete(url: impl Into<String>) -> ResponseFuture;
    /// Issue a `DELETE` request with the given data.
    fn delete_with(url: impl Into<String>, data: impl Serialize) -> ResponseFuture;
    /// Issue a `QUERY` request with the given data.
    fn query(url: impl Into<String>, data: impl Serialize) -> ResponseFuture;
    /// Send a request with the given method.
    fn send(method: &str, url: impl Into<String>) -> ResponseFuture;
    /// Send a request with the given method and data.
    fn send_with(method: &str, url: impl Into<String>, data: impl Serialize) -> ResponseFuture;
}

impl Pool {
    /// Begin a pooled request with the given middleware. See [`PendingRequest::with_middleware`].
    pub fn with_middleware<F, Fut>(&self, middleware: F) -> PendingRequest
    where
        F: Fn(Request, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response>> + Send + 'static,
    {
        self.new_request().with_middleware(middleware)
    }
}
