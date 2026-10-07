//! The HTTP client factory: creates pending requests, holds global
//! configuration, and powers faking, recording and assertions.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use illuminate_container::try_app;
use illuminate_events::Dispatcher;
use illuminate_support::{Collection, Result, Value};

use crate::batch::Batch;
use crate::exceptions::RequestException;
use crate::fake::{FakeResponse, Outcome, ResponseSequence};
use crate::middleware::{BoxFuture, Middleware, Next};
use crate::pending_request::{PendingRequest, ResponseFuture};
use crate::pool::{Pool, PoolResponses, run_pool};
use crate::request::Request;
use crate::response::Response;
use crate::sending::url_matches;
use crate::transport::Transport;

/// A recorded request and the response it received (`None` when the
/// connection failed).
pub type RecordedPair = (Request, Option<Response>);

struct Stub {
    pattern: Option<String>,
    fake: FakeResponse,
}

/// Creates pending requests and keeps the client's global configuration,
/// fakes and recorded requests.
///
/// The factory is bound in the container by
/// [`HttpClientServiceProvider`](crate::HttpClientServiceProvider); the
/// [`Http`](crate::Http) facade resolves it from there.
pub struct Factory {
    global_middleware: RwLock<Vec<Middleware>>,
    global_options: RwLock<Option<Value>>,
    stubs: RwLock<Vec<Stub>>,
    recording: AtomicBool,
    recorded: Mutex<Vec<RecordedPair>>,
    sequences: Mutex<Vec<ResponseSequence>>,
    prevent_stray_requests: AtomicBool,
    allowed_stray_requests: RwLock<Vec<String>>,
    dispatcher: RwLock<Option<Arc<Dispatcher>>>,
    transport: Arc<Transport>,
}

impl Default for Factory {
    fn default() -> Self {
        Self::new()
    }
}

impl Factory {
    /// Create a new factory instance.
    pub fn new() -> Self {
        Self {
            global_middleware: RwLock::new(Vec::new()),
            global_options: RwLock::new(None),
            stubs: RwLock::new(Vec::new()),
            recording: AtomicBool::new(false),
            recorded: Mutex::new(Vec::new()),
            sequences: Mutex::new(Vec::new()),
            prevent_stray_requests: AtomicBool::new(false),
            allowed_stray_requests: RwLock::new(Vec::new()),
            dispatcher: RwLock::new(None),
            transport: Arc::new(Transport::new()),
        }
    }

    /// Create a new factory that dispatches events through the given dispatcher.
    pub fn with_dispatcher(dispatcher: Arc<Dispatcher>) -> Self {
        let factory = Self::new();
        factory.set_dispatcher(Some(dispatcher));
        factory
    }

    /// Set the event dispatcher.
    pub fn set_dispatcher(&self, dispatcher: Option<Arc<Dispatcher>>) {
        *self.dispatcher.write().unwrap() = dispatcher;
    }

    /// Get the event dispatcher: the one given to the factory, or the one
    /// bound in the container.
    pub fn get_dispatcher(&self) -> Option<Arc<Dispatcher>> {
        self.dispatcher
            .read()
            .unwrap()
            .clone()
            .or_else(try_app::<Dispatcher>)
    }

    pub(crate) fn transport(&self) -> Arc<Transport> {
        self.transport.clone()
    }

    // ------------------------------------------------------------------
    // Global configuration
    // ------------------------------------------------------------------

    /// Add middleware to apply to every request.
    pub fn global_middleware<F, Fut>(&self, middleware: F) -> &Self
    where
        F: Fn(Request, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response>> + Send + 'static,
    {
        self.global_middleware
            .write()
            .unwrap()
            .push(Middleware::new(middleware));
        self
    }

    /// Add request middleware to apply to every request.
    pub fn global_request_middleware<F>(&self, middleware: F) -> &Self
    where
        F: Fn(Request) -> Request + Send + Sync + 'static,
    {
        self.global_middleware
            .write()
            .unwrap()
            .push(Middleware::map_request(middleware));
        self
    }

    /// Add response middleware to apply to every request.
    pub fn global_response_middleware<F>(&self, middleware: F) -> &Self
    where
        F: Fn(Response) -> Response + Send + Sync + 'static,
    {
        self.global_middleware
            .write()
            .unwrap()
            .push(Middleware::map_response(middleware));
        self
    }

    /// Get the global middleware.
    pub fn get_global_middleware(&self) -> Vec<Middleware> {
        self.global_middleware.read().unwrap().clone()
    }

    /// Set the options to apply to every request (see
    /// [`PendingRequest::with_options`]).
    pub fn global_options(&self, options: Value) -> &Self {
        *self.global_options.write().unwrap() = Some(options);
        self
    }

    /// Execute a callback while requests are created without global
    /// middleware or global options.
    pub fn without_global_configuration<R>(&self, callback: impl FnOnce() -> R) -> R {
        let middleware = std::mem::take(&mut *self.global_middleware.write().unwrap());
        let options = self.global_options.write().unwrap().take();

        struct Restore<'a> {
            factory: &'a Factory,
            middleware: Option<Vec<Middleware>>,
            options: Option<Value>,
        }

        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                *self.factory.global_middleware.write().unwrap() =
                    self.middleware.take().unwrap_or_default();
                *self.factory.global_options.write().unwrap() = self.options.take();
            }
        }

        let _restore = Restore {
            factory: self,
            middleware: Some(middleware),
            options,
        };

        callback()
    }

    // ------------------------------------------------------------------
    // Creating requests
    // ------------------------------------------------------------------

    /// Create a new pending request instance for this factory.
    pub fn create_pending_request(self: &Arc<Self>) -> PendingRequest {
        let request = PendingRequest::for_factory(self.clone(), self.get_global_middleware());

        match self.global_options.read().unwrap().clone() {
            Some(options) => request.with_options(options),
            None => request,
        }
    }

    /// Send a pool of requests concurrently.
    pub fn pool<F>(self: &Arc<Self>, callback: F) -> BoxFuture<'static, PoolResponses>
    where
        F: FnOnce(&Pool) -> Vec<ResponseFuture>,
    {
        self.pool_with_concurrency(callback, 0)
    }

    /// Send a pool of requests, with at most `concurrency` in flight at
    /// once (`0` sends them all at once).
    pub fn pool_with_concurrency<F>(
        self: &Arc<Self>,
        callback: F,
        concurrency: usize,
    ) -> BoxFuture<'static, PoolResponses>
    where
        F: FnOnce(&Pool) -> Vec<ResponseFuture>,
    {
        let requests = callback(&Pool::new(Some(self.clone())));
        Box::pin(run_pool(requests, concurrency))
    }

    /// Create a batch of requests, with callbacks for introspection.
    pub fn batch<F>(self: &Arc<Self>, callback: F) -> Batch
    where
        F: FnOnce(&Pool) -> Vec<ResponseFuture>,
    {
        Batch::new(callback(&Pool::new(Some(self.clone()))))
    }

    // ------------------------------------------------------------------
    // Faking
    // ------------------------------------------------------------------

    /// Create a new fake response.
    pub fn response(body: impl Into<Value>, status: u16, headers: &[(&str, &str)]) -> FakeResponse {
        FakeResponse::new(body, status, headers)
    }

    /// Create a new request exception, for use while stubbing.
    pub fn failed_request(
        body: impl Into<Value>,
        status: u16,
        headers: &[(&str, &str)],
    ) -> RequestException {
        let response =
            match FakeResponse::new(body, status, headers).resolve(&Request::new("GET", "")) {
                Ok(Some(Outcome::Response(response))) => response,
                _ => Response::new(status, Default::default(), ""),
            };
        RequestException::new(response)
    }

    /// Create a fake that fails to connect.
    pub fn failed_connection() -> FakeResponse {
        FakeResponse::failed_connection(None)
    }

    /// Create a fake that fails to connect with the given message.
    pub fn failed_connection_with(message: impl Into<String>) -> FakeResponse {
        FakeResponse::failed_connection(Some(message.into()))
    }

    /// Create a new (empty) response sequence.
    pub fn sequence(&self) -> ResponseSequence {
        self.sequence_of(Vec::<FakeResponse>::new())
    }

    /// Create a response sequence from the given responses.
    pub fn sequence_of(
        &self,
        responses: impl IntoIterator<Item = impl Into<FakeResponse>>,
    ) -> ResponseSequence {
        let sequence = ResponseSequence::new(responses);
        self.sequences.lock().unwrap().push(sequence.clone());
        sequence
    }

    /// Fake every request with an empty `200` response.
    pub fn fake(&self) -> &Self {
        self.push_stub(None, FakeResponse::new(Value::Null, 200, &[]))
    }

    /// Fake requests whose URLs match the given patterns (`*` is a wildcard).
    pub fn fake_urls(
        &self,
        stubs: impl IntoIterator<Item = (impl Into<String>, impl Into<FakeResponse>)>,
    ) -> &Self {
        self.start_faking();
        for (pattern, fake) in stubs {
            self.push_stub(Some(pattern.into()), fake.into());
        }
        self
    }

    /// Fake requests whose URLs match the given pattern.
    pub fn stub_url(&self, pattern: impl Into<String>, fake: impl Into<FakeResponse>) -> &Self {
        self.push_stub(Some(pattern.into()), fake.into())
    }

    /// Fake requests with the given callback. Return `None` (or
    /// [`FakeResponse::passthrough`]) to let a request through.
    pub fn fake_using<F, R>(&self, callback: F) -> &Self
    where
        F: Fn(&Request) -> R + Send + Sync + 'static,
        R: Into<FakeResponse>,
    {
        self.push_stub(None, FakeResponse::using(callback))
    }

    /// Fake requests matching the pattern with a new response sequence.
    pub fn fake_sequence(&self, pattern: impl Into<String>) -> ResponseSequence {
        let sequence = self.sequence();
        self.push_stub(Some(pattern.into()), sequence.clone().into());
        sequence
    }

    fn start_faking(&self) {
        self.record();
        self.recorded.lock().unwrap().clear();
    }

    fn push_stub(&self, pattern: Option<String>, fake: FakeResponse) -> &Self {
        self.start_faking();
        self.stubs.write().unwrap().push(Stub { pattern, fake });
        self
    }

    /// Resolve the fake for the given request, if one matches.
    pub(crate) fn stub_for(&self, request: &Request) -> Result<Option<Outcome>> {
        let stubs: Vec<FakeResponse> = self
            .stubs
            .read()
            .unwrap()
            .iter()
            .filter(|stub| {
                stub.pattern
                    .as_deref()
                    .is_none_or(|pattern| url_matches(pattern, request.url()))
            })
            .map(|stub| stub.fake.clone())
            .collect();

        for fake in stubs {
            if let Some(outcome) = fake.resolve(request)? {
                return Ok(Some(outcome));
            }
        }

        Ok(None)
    }

    /// Indicate that an error should be returned if any request is not faked.
    pub fn prevent_stray_requests(&self, prevent: bool) -> &Self {
        self.prevent_stray_requests.store(prevent, Ordering::SeqCst);
        self
    }

    /// Determine if stray requests are being prevented.
    pub fn preventing_stray_requests(&self) -> bool {
        self.prevent_stray_requests.load(Ordering::SeqCst)
    }

    /// Allow stray requests to URLs matching the given patterns.
    pub fn allow_stray_requests(
        &self,
        patterns: impl IntoIterator<Item = impl Into<String>>,
    ) -> &Self {
        *self.allowed_stray_requests.write().unwrap() =
            patterns.into_iter().map(Into::into).collect();
        self
    }

    /// Determine if the given URL may be sent without a fake.
    pub fn is_allowed_request_url(&self, url: &str) -> bool {
        !self.preventing_stray_requests()
            || self
                .allowed_stray_requests
                .read()
                .unwrap()
                .iter()
                .any(|pattern| illuminate_support::Str::is(pattern, url))
    }

    // ------------------------------------------------------------------
    // Recording & assertions
    // ------------------------------------------------------------------

    /// Begin recording request / response pairs.
    pub fn record(&self) -> &Self {
        self.recording.store(true, Ordering::SeqCst);
        self
    }

    /// Record a request / response pair (when recording).
    pub fn record_request_response_pair(&self, request: Request, response: Option<Response>) {
        if self.recording.load(Ordering::SeqCst) {
            self.recorded.lock().unwrap().push((request, response));
        }
    }

    /// Get every recorded request / response pair.
    pub fn recorded(&self) -> Collection<RecordedPair> {
        Collection::make(self.recorded.lock().unwrap().clone())
    }

    /// Get the recorded request / response pairs matching the given truth test.
    pub fn recorded_fn(
        &self,
        callback: impl Fn(&Request, Option<&Response>) -> bool,
    ) -> Collection<RecordedPair> {
        self.recorded()
            .filter(|(request, response)| callback(request, response.as_ref()))
    }

    /// Assert that a request matching the given truth test was sent.
    #[track_caller]
    pub fn assert_sent(&self, callback: impl Fn(&Request) -> bool) {
        assert!(
            self.recorded().iter().any(|(request, _)| callback(request)),
            "An expected request was not recorded."
        );
    }

    /// Assert that the requests were sent to the given URLs, in order.
    #[track_caller]
    pub fn assert_sent_in_order(&self, urls: &[&str]) {
        self.assert_sent_count(urls.len());
        let recorded = self.recorded();
        for (index, url) in urls.iter().enumerate() {
            assert!(
                recorded[index].0.url() == *url,
                "An expected request (#{}) was not recorded.",
                index + 1
            );
        }
    }

    /// Assert that no request matching the given truth test was sent.
    #[track_caller]
    pub fn assert_not_sent(&self, callback: impl Fn(&Request) -> bool) {
        assert!(
            !self.recorded().iter().any(|(request, _)| callback(request)),
            "Unexpected request was recorded."
        );
    }

    /// Assert that no requests were sent.
    #[track_caller]
    pub fn assert_nothing_sent(&self) {
        assert!(
            self.recorded.lock().unwrap().is_empty(),
            "Requests were recorded."
        );
    }

    /// Assert how many requests were sent.
    #[track_caller]
    pub fn assert_sent_count(&self, count: usize) {
        let actual = self.recorded.lock().unwrap().len();
        assert_eq!(
            actual, count,
            "Expected [{count}] requests to be sent, but [{actual}] were recorded."
        );
    }

    /// Assert that every response sequence has been used up.
    #[track_caller]
    pub fn assert_sequences_are_empty(&self) {
        for sequence in self.sequences.lock().unwrap().iter() {
            assert!(sequence.is_empty(), "Not all response sequences are empty.");
        }
    }
}

impl std::fmt::Debug for Factory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Factory")
            .field("stubs", &self.stubs.read().unwrap().len())
            .field("recorded", &self.recorded.lock().unwrap().len())
            .field(
                "preventing_stray_requests",
                &self.preventing_stray_requests(),
            )
            .finish_non_exhaustive()
    }
}
