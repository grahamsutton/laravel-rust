//! Routing-aware methods for [`Request`]: the matched route and URL
//! signature validation, like Laravel's `$request->route()` and
//! `$request->hasValidSignature()`.

use std::sync::Arc;

use illuminate_http::Request;

use crate::route::CurrentRoute;
use crate::url::url_generator;

/// Routing helpers on the request. Bring them into scope with
/// `use illuminate_routing::prelude::*`.
///
/// ```
/// use illuminate_http::Request;
/// use illuminate_routing::{RoutingRequestExt, Router};
///
/// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// # runtime.block_on(async {
/// let router = Router::new();
/// router.get("/users/{id}", |request: Request| async move {
///     let route = request.current_route().unwrap();
///     format!("{} {}", route.uri(), request.has_valid_signature())
/// });
///
/// let response = router.dispatch(Request::create("/users/1", "GET")).await;
/// assert_eq!(response.content_string(), "users/{id} false");
/// # });
/// ```
pub trait RoutingRequestExt {
    /// The route that matched the request.
    fn current_route(&self) -> Option<Arc<CurrentRoute>>;

    /// Determine if the request has a valid (absolute) URL signature.
    fn has_valid_signature(&self) -> bool;

    /// Determine if the request has a valid relative URL signature.
    fn has_valid_relative_signature(&self) -> bool;

    /// Determine if the request has a valid signature, ignoring the given
    /// query parameters.
    fn has_valid_signature_while_ignoring(&self, ignore: &[&str]) -> bool;
}

impl RoutingRequestExt for Request {
    fn current_route(&self) -> Option<Arc<CurrentRoute>> {
        self.extension::<CurrentRoute>()
    }

    fn has_valid_signature(&self) -> bool {
        url_generator().has_valid_signature(self)
    }

    fn has_valid_relative_signature(&self) -> bool {
        url_generator().has_valid_relative_signature(self)
    }

    fn has_valid_signature_while_ignoring(&self, ignore: &[&str]) -> bool {
        url_generator().has_valid_signature_while_ignoring(self, ignore, true)
    }
}
