//! Session access on the request.

use std::sync::Arc;

use illuminate_http::Request;
use illuminate_support::Value;

use crate::store::Store;

/// Adds Laravel's session methods to [`Request`].
///
/// ```
/// use std::sync::Arc;
/// use illuminate_http::Request;
/// use illuminate_session::{NullSessionHandler, RequestSessionExt, Store};
/// use illuminate_support::json;
///
/// let request = Request::create_with("/profile", "POST", json!({"name": "Taylor", "password": "secret"}), Default::default());
/// request.set_session(Arc::new(Store::new("laravel_session", Arc::new(NullSessionHandler), None)));
///
/// request.session().put("key", "value");
/// request.flash_except(&["password"]);
///
/// assert_eq!(request.old("name"), json!("Taylor"));
/// assert_eq!(request.old("password"), json!(null));
/// ```
pub trait RequestSessionExt {
    /// Get the session associated with the request.
    ///
    /// # Panics
    ///
    /// Panics with "Session store not set on request." when the
    /// `StartSession` middleware hasn't run. Use
    /// [`try_session`](RequestSessionExt::try_session) when it may be missing.
    fn session(&self) -> Arc<Store>;

    /// Get the session associated with the request, if there is one.
    fn try_session(&self) -> Option<Arc<Store>>;

    /// Determine if the request has a session.
    fn has_session(&self) -> bool;

    /// Set the session on the request (Laravel's `setLaravelSession`).
    fn set_session(&self, session: Arc<Store>);

    /// Retrieve an old input item (`null` when missing, or without a session).
    /// An empty key returns all of the old input.
    fn old(&self, key: &str) -> Value;

    /// Retrieve an old input item, or a default.
    fn old_or(&self, key: &str, default: impl Into<Value>) -> Value;

    /// Flash the request's input to the session.
    fn flash(&self);

    /// Flash only some of the input to the session.
    fn flash_only(&self, keys: &[&str]);

    /// Flash all of the input except the given keys to the session.
    fn flash_except(&self, keys: &[&str]);

    /// Flush all of the old input from the session.
    fn flush_old_input(&self);
}

impl RequestSessionExt for Request {
    fn session(&self) -> Arc<Store> {
        self.try_session()
            .expect("Session store not set on request.")
    }

    fn try_session(&self) -> Option<Arc<Store>> {
        self.extension::<Store>()
    }

    fn has_session(&self) -> bool {
        self.try_session().is_some()
    }

    fn set_session(&self, session: Arc<Store>) {
        self.set_extension(session);
    }

    fn old(&self, key: &str) -> Value {
        self.old_or(key, Value::Null)
    }

    fn old_or(&self, key: &str, default: impl Into<Value>) -> Value {
        match self.try_session() {
            Some(session) => session.get_old_input_or(key, default),
            None => default.into(),
        }
    }

    fn flash(&self) {
        self.session().flash_input(self.all());
    }

    fn flash_only(&self, keys: &[&str]) {
        self.session().flash_input(self.only(keys));
    }

    fn flash_except(&self, keys: &[&str]) {
        self.session().flash_input(self.except(keys));
    }

    fn flush_old_input(&self) {
        self.session()
            .flash_input(Value::Object(Default::default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::NullSessionHandler;
    use illuminate_support::json;

    fn request() -> Request {
        let request = Request::create_with(
            "/users?page=2",
            "POST",
            json!({"name": "Taylor", "email": "taylor@laravel.com"}),
            Default::default(),
        );
        request.set_session(Arc::new(Store::new(
            "session",
            Arc::new(NullSessionHandler),
            None,
        )));
        request
    }

    #[test]
    fn input_can_be_flashed() {
        let request = request();
        assert!(request.has_session());
        request.flash();
        assert_eq!(request.old("name"), json!("Taylor"));
        assert_eq!(request.old("page"), json!("2"));
        assert_eq!(request.old("")["email"], json!("taylor@laravel.com"));

        request.flash_only(&["email"]);
        assert_eq!(request.old("name"), json!(null));
        assert_eq!(request.old_or("name", "default"), json!("default"));
        assert_eq!(request.old("email"), json!("taylor@laravel.com"));

        request.flush_old_input();
        assert!(!request.session().has_old_input());
    }

    #[test]
    fn old_input_without_a_session_is_the_default() {
        let request = Request::create("/", "GET");
        assert!(!request.has_session());
        assert!(request.try_session().is_none());
        assert_eq!(request.old("name"), json!(null));
        assert_eq!(request.old_or("name", "Abigail"), json!("Abigail"));
    }

    #[test]
    #[should_panic(expected = "Session store not set on request.")]
    fn the_session_must_be_set() {
        Request::create("/", "GET").session();
    }
}
