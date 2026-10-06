//! The `Cookie` facade and the `cookie()` helper.

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, try_app};
use illuminate_http::{Cookie as HttpCookie, current_request};

use crate::jar::CookieJar;

/// The `Cookie` facade: create cookies and queue them for the response.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_cookie::facades::Cookie;
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container);
///
/// Cookie::queue_make("name", "value", 60);
/// assert!(Cookie::has_queued("name"));
///
/// Cookie::expire("name");
/// assert!(Cookie::queued("name").unwrap().is_cleared());
/// ```
pub struct Cookie;

impl Cookie {
    /// Get the cookie jar behind the facade.
    pub fn jar() -> Arc<CookieJar> {
        jar()
    }

    /// Get a cookie from the current request.
    pub fn get(name: &str) -> Option<String> {
        current_request().and_then(|request| request.cookie(name))
    }

    /// Determine if the current request carries the given cookie.
    pub fn has(name: &str) -> bool {
        Self::get(name).is_some()
    }

    /// Create a new cookie that lives for the given number of minutes.
    pub fn make(name: impl Into<String>, value: impl Into<String>, minutes: i64) -> HttpCookie {
        jar().make(name, value, minutes)
    }

    /// Create a cookie that lasts "forever" (400 days).
    pub fn forever(name: impl Into<String>, value: impl Into<String>) -> HttpCookie {
        jar().forever(name, value)
    }

    /// Create a cookie that expires the given cookie on the client.
    pub fn forget(name: impl Into<String>) -> HttpCookie {
        jar().forget(name)
    }

    /// Queue a cookie to send with the response.
    pub fn queue(cookie: HttpCookie) {
        jar().queue(cookie);
    }

    /// Create a cookie and queue it to send with the response.
    pub fn queue_make(name: impl Into<String>, value: impl Into<String>, minutes: i64) {
        jar().queue_make(name, value, minutes);
    }

    /// Queue a cookie that expires the given cookie on the client.
    pub fn expire(name: impl Into<String>) {
        jar().expire(name);
    }

    /// Remove a cookie from the queue.
    pub fn unqueue(name: &str) {
        jar().unqueue(name);
    }

    /// Get a queued cookie.
    pub fn queued(name: &str) -> Option<HttpCookie> {
        jar().queued(name)
    }

    /// Determine if a cookie has been queued.
    pub fn has_queued(name: &str) -> bool {
        jar().has_queued(name)
    }

    /// Get every queued cookie.
    pub fn get_queued_cookies() -> Vec<HttpCookie> {
        jar().get_queued_cookies()
    }
}

/// Resolve the cookie jar, registering one (configured from `session.*`)
/// if the application hasn't.
fn jar() -> Arc<CookieJar> {
    if let Some(jar) = try_app::<CookieJar>() {
        return jar;
    }
    let container = Container::get_instance();
    container.singleton_if::<CookieJar>(|c| match c.try_make::<Repository>() {
        Ok(config) => Arc::new(CookieJar::from_config(&config)),
        Err(_) => Arc::new(CookieJar::new()),
    });
    container.make::<CookieJar>()
}

/// Create a new cookie with the application's cookie defaults.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_cookie::cookie;
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container);
///
/// let cookie = cookie("name", "value", 60);
/// assert_eq!(cookie.minutes, Some(60));
/// ```
pub fn cookie(name: impl Into<String>, value: impl Into<String>, minutes: i64) -> HttpCookie {
    jar().make(name, value, minutes)
}
