//! Global helper functions: `route()`, `url()`, `asset()`, and friends.
//!
//! The redirect helpers (`redirect()`, `to_route()`, `back()`) live in
//! [the `redirect` module](mod@crate::redirect) and are re-exported from
//! the crate root.

use illuminate_support::Result;

use crate::params::IntoRouteParameters;
use crate::url::url_generator;

/// Generate the absolute URL to a named route.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_routing::{route, Route};
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container);
///
/// Route::get("/post/{post}/comment/{comment}", || async { "" }).name("comment.show");
///
/// assert_eq!(
///     route("comment.show", [("post", 1), ("comment", 3)]).unwrap(),
///     "http://localhost/post/1/comment/3"
/// );
/// assert_eq!(route("comment.show", (1, 3)).unwrap(), "http://localhost/post/1/comment/3");
/// assert!(route("missing", ()).is_err());
/// ```
pub fn route<'a>(name: &str, parameters: impl IntoRouteParameters<'a>) -> Result<String> {
    url_generator().route(name, parameters)
}

/// Generate an absolute URL to the given path. Use [`url_generator`] (or
/// the `URL` facade) for Laravel's argument-less `url()`.
pub fn url(path: &str) -> String {
    url_generator().to(path)
}

/// Generate a secure (HTTPS) URL to the given path.
pub fn secure_url(path: &str) -> String {
    url_generator().secure(path)
}

/// Generate a URL to an application asset.
pub fn asset(path: &str) -> String {
    url_generator().asset(path)
}

/// Generate a URL to an application asset over HTTPS.
pub fn secure_asset(path: &str) -> String {
    url_generator().secure_asset(path)
}
