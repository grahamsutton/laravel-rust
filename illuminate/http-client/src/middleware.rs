//! Client middleware: inspect or modify outgoing requests and incoming
//! responses.
//!
//! ```
//! use illuminate_http_client::{Http, Request, Response};
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! Http::fake();
//!
//! let response = Http::with_request_middleware(|request: Request| request.with_header("X-Example", "Value"))
//!     .with_response_middleware(|response: Response| response.with_header("X-Seen", "yes"))
//!     .with_middleware(|request, next| async move {
//!         // Before the request is sent...
//!         let response = next.run(request).await?;
//!         // After the response is received...
//!         Ok(response)
//!     })
//!     .get("http://example.com")
//!     .await?;
//!
//! assert_eq!(response.header("X-Seen"), "yes");
//! Http::assert_sent(|request| request.has_header_value("X-Example", "Value"));
//! # Ok::<(), illuminate_support::Error>(())
//! # }).unwrap();
//! ```

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use illuminate_support::Result;

use crate::request::Request;
use crate::response::Response;

/// A boxed, sendable future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Something that turns a request into a response.
pub(crate) type Handler =
    Arc<dyn Fn(Request) -> BoxFuture<'static, Result<Response>> + Send + Sync>;

type MiddlewareFn = dyn Fn(Request, Next) -> BoxFuture<'static, Result<Response>> + Send + Sync;

/// The rest of the middleware stack.
pub struct Next {
    handler: Handler,
}

impl Next {
    pub(crate) fn new(handler: Handler) -> Self {
        Self { handler }
    }

    /// Pass the request deeper into the stack, towards the server.
    pub async fn run(self, request: Request) -> Result<Response> {
        (self.handler)(request).await
    }
}

impl fmt::Debug for Next {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Next")
    }
}

/// A client middleware.
#[derive(Clone)]
pub struct Middleware(Arc<MiddlewareFn>);

impl Middleware {
    /// Create a middleware from an async closure receiving the request and
    /// the rest of the stack.
    pub fn new<F, Fut>(middleware: F) -> Self
    where
        F: Fn(Request, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response>> + Send + 'static,
    {
        Self(Arc::new(move |request, next| {
            Box::pin(middleware(request, next))
        }))
    }

    /// Create a middleware that maps every outgoing request.
    pub fn map_request<F>(callback: F) -> Self
    where
        F: Fn(Request) -> Request + Send + Sync + 'static,
    {
        Self::new(move |request, next| next.run(callback(request)))
    }

    /// Create a middleware that maps every incoming response.
    pub fn map_response<F>(callback: F) -> Self
    where
        F: Fn(Response) -> Response + Send + Sync + 'static,
    {
        let callback = Arc::new(callback);
        Self::new(move |request, next| {
            let callback = callback.clone();
            async move { next.run(request).await.map(|response| callback(response)) }
        })
    }

    fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        (self.0)(request, next)
    }
}

impl fmt::Debug for Middleware {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Middleware")
    }
}

/// Wrap the destination in the given middleware; the first middleware is
/// the outermost layer.
pub(crate) fn build_stack(middleware: &[Middleware], destination: Handler) -> Handler {
    middleware.iter().rev().fold(destination, |inner, layer| {
        let layer = layer.clone();
        Arc::new(move |request| layer.handle(request, Next::new(inner.clone())))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;
    use std::sync::Mutex;

    #[tokio::test]
    async fn middleware_wraps_the_destination_in_order() {
        let log = Arc::new(Mutex::new(Vec::new()));

        let layer = |name: &'static str, log: Arc<Mutex<Vec<String>>>| {
            Middleware::new(move |request, next| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push(format!("before {name}"));
                    let response = next.run(request).await;
                    log.lock().unwrap().push(format!("after {name}"));
                    response
                }
            })
        };

        let destination: Handler = Arc::new(|request: Request| {
            Box::pin(async move {
                let value = request.header("X-Mapped").join(",");
                Ok(Response::new(200, HeaderMap::new(), value))
            })
        });

        let stack = build_stack(
            &[
                layer("first", log.clone()),
                Middleware::map_request(|request| request.with_header("X-Mapped", "yes")),
                Middleware::map_response(|response| response.with_header("X-Done", "1")),
                layer("second", log.clone()),
            ],
            destination,
        );

        let response = stack(Request::new("GET", "/")).await.unwrap();
        assert_eq!(response.body(), "yes");
        assert_eq!(response.header("X-Done"), "1");
        assert_eq!(
            *log.lock().unwrap(),
            [
                "before first",
                "before second",
                "after second",
                "after first"
            ]
        );
        assert_eq!(
            format!("{:?}", Middleware::map_request(|r| r)),
            "Middleware"
        );
    }
}
