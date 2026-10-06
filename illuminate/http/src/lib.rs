//! # Illuminate Http
//!
//! Laravel's HTTP layer: an expressive [`Request`], a flexible [`Response`],
//! and the [`IntoResponse`] conversion that lets route handlers return
//! strings, JSON values, views, redirects, or errors and simply do the right
//! thing.
//!
//! ```
//! use illuminate_http::{Request, Response, IntoResponse};
//! use illuminate_support::json;
//!
//! let request = Request::create("/users?sort=name", "GET");
//! assert_eq!(request.path(), "users");
//! assert_eq!(request.query("sort"), json!("name"));
//!
//! let response = json!({"name": "Taylor"}).into_response();
//! assert_eq!(response.status().as_u16(), 200);
//! assert_eq!(response.header("content-type").unwrap(), "application/json");
//! ```

pub mod context;
pub mod cookie;
pub mod exceptions;
pub mod input;
pub mod into_response;
pub mod middleware;
pub mod request;
pub mod response;
pub mod server;
pub mod uploaded_file;

pub use context::{current_request, request, with_request, with_request_sync};
pub use cookie::{Cookie, SameSite, cookie};
pub use exceptions::{
    ExceptionHandler, HttpException, HttpResponseException, abort, abort_if, abort_unless,
    abort_with, render_exception,
};
pub use into_response::{IntoResponse, Json};
pub use middleware::{Destination, Middleware, Next, build_pipeline, middleware_fn, run_middleware};
pub use request::Request;
pub use response::{Body, BodyStream, Response, ResponseFactory, SyncStream, response};
pub use uploaded_file::UploadedFile;

/// Re-exports of the underlying `http` crate types.
pub use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header};

/// A boxed, sendable future — the currency of middleware and handlers.
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Re-exported so implementors of [`Middleware`] don't need their own dependency.
pub use async_trait::async_trait;
