//! # Illuminate Session
//!
//! HTTP is stateless; sessions remember things about a user across
//! requests. Data lives in a [`Store`], persisted between requests by a
//! [`SessionHandler`] (`file`, `cookie`, `database`, `redis` and the other
//! cache-backed drivers, `array`, `null`, or your own), and
//! is loaded and saved by the [`StartSession`] middleware.
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_http::Request;
//! use illuminate_session::{ArraySessionHandler, RequestSessionExt, Store};
//! use illuminate_support::json;
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
//! # runtime.block_on(async {
//! let session = Arc::new(Store::new("laravel_session", Arc::new(ArraySessionHandler::new(120)), None));
//! session.start().await.unwrap();
//!
//! let request = Request::create("/", "GET");
//! request.set_session(session);
//!
//! request.session().put("key", "value");
//! request.session().flash("status", "Task was successful!");
//!
//! assert_eq!(request.session().get("key"), json!("value"));
//! # });
//! ```
//!
//! The session also powers CSRF protection ([`ValidateCsrfToken`]), old
//! input (`old()`), and the "intended URL" redirects used by authentication.
//!
//! ## Data the rest of the framework relies on
//!
//! - `_token`: the CSRF token.
//! - `_flash.old` / `_flash.new`: the keys of flashed data.
//! - `_old_input`: flashed request input (`old()`).
//! - `errors`: flashed validation errors as `{bag: {field: [messages]}}`.
//! - `_previous.url` / `_previous.route`: the last page visited (also exposed
//!   to the request as the `_previous_url` attribute).
//! - `url.intended`: where to send the user after logging in.

mod exceptions;
mod facade;
mod handlers;
mod helpers;
mod manager;
pub mod middleware;
mod provider;
mod redirects;
mod request;
mod store;

pub use exceptions::{SessionNotFoundException, TokenMismatchException};
pub use facade::Session;
pub use handlers::{
    ArraySessionHandler, CacheBasedSessionHandler, CookieSessionHandler, DatabaseSessionHandler,
    FileSessionHandler, NullSessionHandler, SessionHandler,
};
pub use helpers::{
    csrf_field, csrf_token, intended_url, method_field, old, redirect_guest, redirect_intended,
    session, session_get, session_get_or, session_put, session_put_many, set_intended_url,
    try_session, view_errors,
};
pub use manager::{HandlerFactory, SessionConfig, SessionManager};
pub use middleware::{
    PreventRequestForgery, StartSession, ValidateCsrfToken, VerifyCsrfToken,
    apply_response_session_data,
};
pub use provider::SessionServiceProvider;
pub use redirects::RedirectSessionExt;
pub use request::RequestSessionExt;
pub use store::{SessionKeys, Store};

/// The facades provided by this component.
pub mod facades {
    pub use crate::facade::Session;
}

/// Everything you need to work with the session, in one import.
pub mod prelude {
    pub use crate::facade::Session;
    pub use crate::redirects::RedirectSessionExt;
    pub use crate::request::RequestSessionExt;
    pub use crate::store::Store;
}
