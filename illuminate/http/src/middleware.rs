//! Middleware: a convenient mechanism for inspecting and filtering the HTTP
//! requests entering your application.
//!
//! ```
//! use illuminate_http::{Middleware, Next, Request, Response, run_middleware};
//! use illuminate_support::Result;
//! use std::sync::Arc;
//!
//! struct EnsureTokenIsValid;
//!
//! #[async_trait::async_trait]
//! impl Middleware for EnsureTokenIsValid {
//!     async fn handle(&self, request: Request, next: Next) -> Result<Response> {
//!         if request.input("token").as_str() != Some("my-secret-token") {
//!             return Ok(Response::redirect("/home"));
//!         }
//!
//!         Ok(next.run(request).await)
//!     }
//! }
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
//! # runtime.block_on(async {
//! let response = run_middleware(
//!     Request::create("/?token=nope", "GET"),
//!     vec![Arc::new(EnsureTokenIsValid)],
//!     Arc::new(|_request| Box::pin(async { Response::new("Welcome!") })),
//! ).await;
//!
//! assert!(response.is_redirect());
//! # });
//! ```

use std::future::Future;
use std::sync::Arc;

use illuminate_support::Result;

use crate::BoxFuture;
use crate::exceptions::render_exception;
use crate::request::Request;
use crate::response::Response;

/// The final destination of a middleware pipeline.
pub type Destination = Arc<dyn Fn(Request) -> BoxFuture<'static, Response> + Send + Sync>;

/// HTTP middleware.
///
/// Returning an `Err` is just like throwing an exception in Laravel: it is
/// reported and rendered by the exception handler, and the outer middleware
/// receive the rendered response.
#[async_trait::async_trait]
pub trait Middleware: Send + Sync + 'static {
    /// Handle an incoming request.
    async fn handle(&self, request: Request, next: Next) -> Result<Response>;

    /// Perform any final actions after the response has been sent.
    async fn terminate(&self, _request: &Request, _response: &Response) {}
}

/// Closures make perfectly good middleware.
#[async_trait::async_trait]
impl<F, Fut> Middleware for F
where
    F: Fn(Request, Next) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Response>> + Send + 'static,
{
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        (self)(request, next).await
    }
}

/// The rest of the middleware pipeline.
#[derive(Clone)]
pub struct Next {
    inner: Destination,
}

impl Next {
    /// Wrap the given continuation.
    pub fn new(inner: Destination) -> Self {
        Self { inner }
    }

    /// Pass the request deeper into the application.
    pub async fn run(self, request: Request) -> Response {
        (self.inner)(request).await
    }
}

/// Send the request through the given middleware, then to the destination.
///
/// Errors returned by any middleware are rendered by the exception handler,
/// so every layer always sees a `Response`.
pub async fn run_middleware(
    request: Request,
    middleware: Vec<Arc<dyn Middleware>>,
    destination: Destination,
) -> Response {
    build_pipeline(middleware, destination)(request).await
}

/// Compose middleware around a destination into a single callable.
pub fn build_pipeline(middleware: Vec<Arc<dyn Middleware>>, destination: Destination) -> Destination {
    middleware
        .into_iter()
        .rev()
        .fold(destination, |next, layer| {
            Arc::new(move |request: Request| {
                let layer = layer.clone();
                let next = Next::new(next.clone());
                Box::pin(async move {
                    match layer.handle(request, next).await {
                        Ok(response) => response,
                        Err(error) => render_exception(error),
                    }
                }) as BoxFuture<'static, Response>
            })
        })
}

/// Create middleware from a closure (handy for inline route middleware).
pub fn middleware_fn<F, Fut>(callback: F) -> Arc<dyn Middleware>
where
    F: Fn(Request, Next) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Response>> + Send + 'static,
{
    Arc::new(callback)
}
