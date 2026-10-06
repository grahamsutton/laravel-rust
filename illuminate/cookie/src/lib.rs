//! # Illuminate Cookie
//!
//! Cookies, the Laravel way: a [`CookieJar`] that creates cookies with your
//! application's defaults and *queues* them for the outgoing response, the
//! [`facades::Cookie`] facade, and the middleware that encrypts every cookie
//! your application sends ([`EncryptCookies`]) and attaches queued cookies
//! to the response ([`AddQueuedCookiesToResponse`]).
//!
//! ```
//! use illuminate_cookie::CookieJar;
//!
//! let jar = CookieJar::new();
//!
//! jar.queue_make("name", "value", 60);
//!
//! assert!(jar.has_queued("name"));
//! assert_eq!(jar.queued("name").unwrap().value, "value");
//! ```
//!
//! Queued cookies belong to the request being handled: while a request is
//! current (see `illuminate_http::with_request`), the queue lives on that
//! request, so concurrent requests never see each other's cookies.

mod facade;
mod jar;
pub mod middleware;
mod prefix;
mod provider;

pub use facade::cookie;
pub use jar::{CookieJar, CookieQueue};
pub use middleware::{AddQueuedCookiesToResponse, EncryptCookies};
pub use prefix::CookieValuePrefix;
pub use provider::CookieServiceProvider;

/// The facades provided by this component.
pub mod facades {
    pub use crate::facade::Cookie;
}
