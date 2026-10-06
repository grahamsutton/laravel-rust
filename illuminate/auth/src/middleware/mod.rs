//! The authentication and authorization middleware.
//!
//! | Alias              | Middleware                    |
//! | ------------------ | ----------------------------- |
//! | `auth`             | [`Authenticate`]              |
//! | `auth.basic`       | [`AuthenticateWithBasicAuth`] |
//! | `auth.session`     | [`AuthenticateSession`]       |
//! | `can`              | [`Authorize`]                 |
//! | `guest`            | [`RedirectIfAuthenticated`]   |
//! | `password.confirm` | [`RequirePassword`]           |
//! | `verified`         | [`EnsureEmailIsVerified`]     |
//!
//! Each middleware can be built from its route parameters
//! (`"auth:admin,api"` → `Authenticate::from_parameters(&["admin", "api"])`),
//! and [`middleware_aliases`] hands the router every alias at once.

mod authenticate;
mod authenticate_session;
mod authorize;
mod basic;
mod redirect_if_authenticated;
mod require_password;
mod verified;

use std::future::Future;
use std::sync::Arc;

use illuminate_http::{Middleware, Request, current_request, with_request};

pub use authenticate::Authenticate;
pub use authenticate_session::AuthenticateSession;
pub use authorize::Authorize;
pub use basic::AuthenticateWithBasicAuth;
pub use redirect_if_authenticated::RedirectIfAuthenticated;
pub use require_password::RequirePassword;
pub use verified::EnsureEmailIsVerified;

/// Builds a middleware from the parameters after the `:` in its name — the
/// same shape as the router's middleware factories.
pub type MiddlewareFactory = Arc<dyn Fn(&[String]) -> Arc<dyn Middleware> + Send + Sync>;

/// Every auth middleware alias with its factory, ready to register with
/// the router.
///
/// ```
/// use illuminate_auth::middleware::middleware_aliases;
///
/// let aliases: Vec<&str> = middleware_aliases().into_iter().map(|(alias, _)| alias).collect();
/// assert_eq!(aliases, ["auth", "auth.basic", "auth.session", "can", "guest", "password.confirm", "verified"]);
/// ```
pub fn middleware_aliases() -> Vec<(&'static str, MiddlewareFactory)> {
    vec![
        ("auth", Authenticate::factory()),
        ("auth.basic", AuthenticateWithBasicAuth::factory()),
        ("auth.session", AuthenticateSession::factory()),
        ("can", Authorize::factory()),
        ("guest", RedirectIfAuthenticated::factory()),
        ("password.confirm", RequirePassword::factory()),
        ("verified", EnsureEmailIsVerified::factory()),
    ]
}

/// Run the future with the request as the current request (unless a
/// request is already being handled), so the guards can see it.
pub(crate) async fn within_request<T>(request: &Request, future: impl Future<Output = T>) -> T {
    if current_request().is_some() {
        future.await
    } else {
        with_request(request.clone(), future).await
    }
}

/// The non-empty parameters.
pub(crate) fn filled(parameters: &[String]) -> Vec<String> {
    parameters
        .iter()
        .map(|parameter| parameter.trim().to_string())
        .filter(|parameter| !parameter.is_empty())
        .collect()
}

/// Turn a route name (or a path) into a URL, using the application's route
/// resolver when there is one.
pub(crate) fn route_url(name: Option<&str>, default_name: &str, default_path: &str) -> String {
    let hooks = crate::facade::manager();
    let hooks = hooks.hooks();
    match name {
        Some(name) => hooks
            .route_url(name)
            .unwrap_or_else(|| if name.starts_with('/') { name.to_string() } else { default_path.to_string() }),
        None => hooks.route_url_or(default_name, default_path),
    }
}
