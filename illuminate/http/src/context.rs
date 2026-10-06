//! The current request.
//!
//! While a request is being handled, it is available anywhere in the task
//! handling it — which is what lets helpers like `request()`, `session()`,
//! `old()` and `auth()` work without passing the request around.

use std::future::Future;

use crate::request::Request;

tokio::task_local! {
    static CURRENT_REQUEST: Request;
}

/// Run the future with the given request as the "current" request.
pub async fn with_request<F: Future>(request: Request, future: F) -> F::Output {
    CURRENT_REQUEST.scope(request, future).await
}

/// Run a synchronous closure with the given request as the current request.
pub fn with_request_sync<R>(request: Request, callback: impl FnOnce() -> R) -> R {
    CURRENT_REQUEST.sync_scope(request, callback)
}

/// Get the request currently being handled, if any.
pub fn current_request() -> Option<Request> {
    CURRENT_REQUEST.try_with(|r| r.clone()).ok()
}

/// Get the current request (an empty `GET /` request outside of HTTP).
pub fn request() -> Request {
    current_request().unwrap_or_default()
}
