//! # Illuminate HTTP Client
//!
//! Laravel provides an expressive, minimal API around an HTTP client,
//! allowing you to quickly make outgoing HTTP requests to communicate with
//! other web applications. It is focused on the most common use cases and
//! a wonderful developer experience.
//!
//! ## Making requests
//!
//! ```
//! use illuminate_http_client::Http;
//! use illuminate_support::json;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! # Http::fake_urls([("example.com/*", Http::response(json!({"name": "Taylor"}), 200, &[]))]);
//! let response = Http::get("http://example.com/users/1").await?;
//!
//! response.body();
//! response.json();
//! response.status();
//! response.successful();
//! response.failed();
//! response.header("Content-Type");
//!
//! assert_eq!(response["name"], "Taylor");
//!
//! // Sending data (JSON by default, or a form with `as_form`)...
//! Http::post("http://example.com/users", json!({"name": "Steve", "role": "Network Administrator"})).await?;
//! Http::as_form().post("http://example.com/users", json!({"name": "Sara"})).await?;
//!
//! // Headers, authentication, timeouts and retries...
//! Http::with_headers([("X-First", "foo")])
//!     .with_token("token", "Bearer")
//!     .accept_json()
//!     .timeout(3)
//!     .retry(3, 100)
//!     .get("http://example.com/users")
//!     .await?;
//! # Ok::<(), illuminate_support::Error>(())
//! # }).unwrap();
//! ```
//!
//! ## Error handling
//!
//! Like Laravel, the client doesn't return errors for `4xx` and `5xx`
//! responses: inspect them with `successful()`, `failed()`,
//! `client_error()` and `server_error()`, or turn them into a
//! [`RequestException`] with [`Response::throw`] or
//! [`PendingRequest::throw`]. Connection failures and timeouts are always
//! returned as a [`ConnectionException`].
//!
//! ## Concurrent requests
//!
//! [`Http::pool`] sends requests concurrently, and [`Http::batch`] adds
//! completion callbacks.
//!
//! ## Macros
//!
//! Rust has no runtime macros, so define an extension trait for the
//! clients you use often:
//!
//! ```
//! use illuminate_http_client::{Http, PendingRequest};
//!
//! pub trait GithubClient {
//!     fn github() -> PendingRequest;
//! }
//!
//! impl GithubClient for Http {
//!     fn github() -> PendingRequest {
//!         Http::with_headers([("X-Example", "example")]).base_url("https://github.com")
//!     }
//! }
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! # Http::fake();
//! let response = Http::github().get("/").await?;
//! # Ok::<(), illuminate_support::Error>(())
//! # }).unwrap();
//! ```
//!
//! ## Testing
//!
//! [`Http::fake`] stubs responses and records every request so you may
//! make assertions about them:
//!
//! ```
//! use illuminate_http_client::{Http, Request};
//! use illuminate_support::json;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! Http::fake_urls([
//!     ("github.com/*", Http::response(json!({"foo": "bar"}), 200, &[])),
//!     ("*", Http::response("Hello World", 200, &[])),
//! ]);
//!
//! Http::post("https://github.com/users", json!({"name": "Taylor"})).await?;
//!
//! Http::assert_sent(|request: &Request| request.url() == "https://github.com/users" && request["name"] == "Taylor");
//! Http::assert_not_sent(|request: &Request| request.url().contains("laravel.com"));
//! Http::assert_sent_count(1);
//! # Ok::<(), illuminate_support::Error>(())
//! # }).unwrap();
//! ```
//!
//! Fakes live on the [`Factory`] in the service container, so tests that
//! install their own container (`Container::set_local_instance`) are
//! isolated from each other.

mod batch;
pub mod aws;
pub mod cookies;
mod encoding;
pub mod events;
pub mod exceptions;
mod facade;
mod factory;
pub mod fake;
pub mod middleware;
mod pending_request;
mod pool;
mod provider;
pub mod request;
pub mod response;
mod sending;
mod transport;
mod uri_template;

pub use batch::Batch;
pub use cookies::{Cookie, CookieJar};
pub use encoding::Part;
pub use events::{ConnectionFailed, RequestSending, ResponseReceived};
pub use exceptions::{
    ConnectionException, RequestException, StrayRequestException, is_http_client_exception,
};
pub use facade::Http;
pub use factory::{Factory, RecordedPair};
pub use fake::{FakeResponse, ResponseSequence};
pub use middleware::{BoxFuture, Middleware, Next};
pub use pending_request::{BodyFormat, PendingRequest, ResponseFuture, Tries};
pub use pool::{Pool, PoolKey, PoolResponses};
pub use provider::HttpClientServiceProvider;
pub use request::Request;
pub use response::Response;

/// Re-exports of the underlying `http` crate types used in the API.
pub use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version};
