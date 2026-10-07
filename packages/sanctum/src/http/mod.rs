//! Sanctum's HTTP layer: the stateful and ability middleware, and the
//! `/sanctum/csrf-cookie` route.

pub mod middleware;

use std::future::Future;

use illuminate_auth::AuthUser;
use illuminate_http::{Request, Response, current_request, with_request};
use illuminate_routing::Router;
use illuminate_support::Result;

use crate::config;

/// Serves `GET /sanctum/csrf-cookie`: the `web` middleware group starts the
/// session and sets the `XSRF-TOKEN` cookie, so your SPA can initialize
/// CSRF protection before logging in.
///
/// ```js
/// axios.get('/sanctum/csrf-cookie').then(response => {
///     // Login...
/// });
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct CsrfCookieController;

impl CsrfCookieController {
    /// Return an empty `204` response (the cookie is added by the
    /// `ValidateCsrfToken` middleware).
    pub async fn show(_request: Request) -> Response {
        Response::no_content()
    }
}

/// The name of the CSRF cookie route.
pub const CSRF_COOKIE_ROUTE: &str = "sanctum.csrf-cookie";

/// Register Sanctum's routes on the router: `GET /{sanctum.prefix}/csrf-cookie`
/// (named `sanctum.csrf-cookie`, in the `web` middleware group). Routes that
/// are already registered aren't registered again.
pub fn define_routes(router: &Router) {
    if router.has(CSRF_COOKIE_ROUTE) {
        return;
    }
    let prefix = config::prefix();
    let prefix = prefix.trim_matches('/');
    let uri = if prefix.is_empty() {
        "/csrf-cookie".to_string()
    } else {
        format!("/{prefix}/csrf-cookie")
    };
    router
        .get(&uri, CsrfCookieController::show)
        .middleware("web")
        .name(CSRF_COOKIE_ROUTE);
}

/// Run the future with the request as the current request (unless one is
/// already being handled), so guards can see it.
pub(crate) async fn within_request<T>(request: &Request, future: impl Future<Output = T>) -> T {
    if current_request().is_some() {
        future.await
    } else {
        with_request(request.clone(), future).await
    }
}

/// The request's user, resolved by the default guard (Laravel's
/// `$request->user()`).
pub(crate) async fn request_user(request: &Request) -> Result<Option<AuthUser>> {
    within_request(request, async {
        illuminate_auth::manager().guard(None)?.try_user().await
    })
    .await
}
