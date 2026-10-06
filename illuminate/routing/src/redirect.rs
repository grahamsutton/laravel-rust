//! Redirect responses: `redirect()`, `to_route()`, `back()` and the
//! `Redirect` facade.
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_container::Container;
//! use illuminate_routing::{back, redirect, to_route, Redirect, Route};
//!
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container);
//!
//! Route::get("/profile/{id}", || async { "Profile" }).name("profile");
//!
//! let response = redirect("/home/dashboard").with("status", "Profile updated!");
//! assert_eq!(response.status_code(), 302);
//! assert_eq!(response.target_url().unwrap(), "http://localhost/home/dashboard");
//!
//! assert_eq!(to_route("profile", 1).target_url().unwrap(), "http://localhost/profile/1");
//! assert_eq!(back().target_url().unwrap(), "http://localhost");
//! assert_eq!(Redirect::away("https://www.google.com").target_url().unwrap(), "https://www.google.com");
//! ```

use illuminate_http::{Response, render_exception};
use illuminate_support::Result;

use crate::params::IntoRouteParameters;
use crate::url::{Expiration, url_generator};

fn redirect_or_render(url: Result<String>, status: u16) -> Response {
    match url {
        Ok(url) => Response::redirect_with_status(url, status),
        Err(error) => render_exception(error),
    }
}

/// The `Redirect` facade: Laravel's `Redirector`.
///
/// Every method returns a redirect [`Response`], so flash data can be
/// chained on: `Redirect::back().with_input(request.all())`. Failures (like
/// an unknown route name) are rendered by the exception handler.
pub struct Redirect;

impl Redirect {
    /// Redirect to the given path (`302`).
    pub fn to(path: &str) -> Response {
        Self::to_with_status(path, 302)
    }

    /// Redirect to the given path with a specific status code.
    pub fn to_with_status(path: &str, status: u16) -> Response {
        Response::redirect_with_status(url_generator().to(path), status)
    }

    /// Redirect to an external URL, without any URL generation.
    pub fn away(url: &str) -> Response {
        Response::redirect(url)
    }

    /// Redirect to the given path over HTTPS.
    pub fn secure(path: &str) -> Response {
        Response::redirect(url_generator().secure(path))
    }

    /// Redirect to a named route.
    pub fn route<'a>(name: &str, parameters: impl IntoRouteParameters<'a>) -> Response {
        redirect_or_render(url_generator().route(name, parameters), 302)
    }

    /// Redirect to a named route with a specific status code.
    pub fn route_with_status<'a>(
        name: &str,
        parameters: impl IntoRouteParameters<'a>,
        status: u16,
    ) -> Response {
        redirect_or_render(url_generator().route(name, parameters), status)
    }

    /// Redirect to a signed URL for a named route.
    pub fn signed_route<'a>(name: &str, parameters: impl IntoRouteParameters<'a>) -> Response {
        redirect_or_render(url_generator().signed_route(name, parameters), 302)
    }

    /// Redirect to a temporary signed URL for a named route.
    pub fn temporary_signed_route<'a>(
        name: &str,
        expiration: impl Expiration,
        parameters: impl IntoRouteParameters<'a>,
    ) -> Response {
        redirect_or_render(
            url_generator().temporary_signed_route(name, expiration, parameters),
            302,
        )
    }

    /// Redirect back to the previous location: the `Referer`, or the
    /// previous URL remembered by the session, or the root.
    pub fn back() -> Response {
        Response::redirect(url_generator().previous())
    }

    /// Redirect back, falling back to the given path when there's no
    /// previous location.
    pub fn back_or(fallback: &str) -> Response {
        Response::redirect(url_generator().previous_or(fallback))
    }

    /// Redirect to the current URL.
    pub fn refresh() -> Response {
        let generator = url_generator();
        let path = generator
            .get_request()
            .map(|request| request.path())
            .unwrap_or_else(|| "/".to_string());
        Response::redirect(generator.to(&path))
    }

    /// Redirect with a `307 Temporary Redirect`.
    pub fn temporary(path: &str) -> Response {
        Self::to_with_status(path, 307)
    }

    /// Redirect with a `301 Moved Permanently`.
    pub fn permanent(path: &str) -> Response {
        Self::to_with_status(path, 301)
    }
}

/// Create a redirect response to the given path.
pub fn redirect(to: &str) -> Response {
    Redirect::to(to)
}

/// Create a redirect response to a named route.
pub fn to_route<'a>(name: &str, parameters: impl IntoRouteParameters<'a>) -> Response {
    Redirect::route(name, parameters)
}

/// Alias of [`to_route`].
pub fn redirect_to_route<'a>(name: &str, parameters: impl IntoRouteParameters<'a>) -> Response {
    Redirect::route(name, parameters)
}

/// Create a redirect response to the user's previous location.
pub fn back() -> Response {
    Redirect::back()
}
