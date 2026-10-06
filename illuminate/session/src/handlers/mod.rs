//! Session handlers: where session data lives between requests.

mod array;
mod cookie;
mod file;
mod null;

use std::time::{SystemTime, UNIX_EPOCH};

use illuminate_http::{Request, async_trait};
use illuminate_support::Result;

pub use array::ArraySessionHandler;
pub use cookie::CookieSessionHandler;
pub use file::FileSessionHandler;
pub use null::NullSessionHandler;

/// The contract every session driver implements — Rust's take on PHP's
/// `SessionHandlerInterface`.
///
/// Handlers move opaque strings: the [`Store`](crate::Store) serializes (and,
/// when `session.encrypt` is on, encrypts) the session before calling
/// [`write`](SessionHandler::write), so a handler never needs to.
///
/// Register your own with
/// [`SessionManager::extend`](crate::SessionManager::extend). A database
/// handler, for example, implements [`set_exists`](SessionHandler::set_exists)
/// (to choose between `INSERT` and `UPDATE`) and
/// [`set_request`](SessionHandler::set_request) (to record the IP address and
/// user agent) — the hooks Laravel's `DatabaseSessionHandler` relies on.
///
/// ```
/// use illuminate_http::async_trait;
/// use illuminate_session::SessionHandler;
/// use illuminate_support::Result;
///
/// struct MongoSessionHandler;
///
/// #[async_trait]
/// impl SessionHandler for MongoSessionHandler {
///     async fn read(&self, session_id: &str) -> Result<String> {
///         Ok(String::new())
///     }
///
///     async fn write(&self, session_id: &str, data: &str) -> Result<()> {
///         Ok(())
///     }
///
///     async fn destroy(&self, session_id: &str) -> Result<()> {
///         Ok(())
///     }
///
///     async fn gc(&self, lifetime: u64) -> Result<usize> {
///         Ok(0)
///     }
/// }
/// ```
#[async_trait]
pub trait SessionHandler: Send + Sync {
    /// Read the raw session data (an empty string when there is none, or it expired).
    async fn read(&self, session_id: &str) -> Result<String>;

    /// Persist the raw session data.
    async fn write(&self, session_id: &str, data: &str) -> Result<()>;

    /// Destroy the session's data.
    async fn destroy(&self, session_id: &str) -> Result<()>;

    /// Remove every session older than `lifetime` seconds, returning how
    /// many were removed. Self-expiring stores may simply return `Ok(0)`.
    async fn gc(&self, lifetime: u64) -> Result<usize>;

    /// Tell the handler whether the session already exists in storage
    /// (Laravel's `ExistenceAwareInterface`).
    fn set_exists(&self, _exists: bool) {}

    /// Determine if the handler needs the current request.
    fn needs_request(&self) -> bool {
        false
    }

    /// Hand the current request to the handler.
    fn set_request(&self, _request: &Request) {}
}

/// The current UNIX timestamp, in seconds.
pub(crate) fn current_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}
