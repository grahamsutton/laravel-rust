//! The `Session` facade.

use std::sync::Arc;

use serde::de::DeserializeOwned;

use illuminate_config::Repository;
use illuminate_container::{Container, try_app};
use illuminate_support::{Result, Value};

use crate::exceptions::SessionNotFoundException;
use crate::handlers::SessionHandler;
use crate::helpers::try_session;
use crate::manager::SessionManager;
use crate::store::{SessionKeys, Store};

/// The `Session` facade: the current request's session.
///
/// Every data method works on the session the `StartSession` middleware
/// attached to the current request, and panics with "Session store not set
/// on request." outside of one. Use [`Session::try_store`] to check first.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_http::{Request, with_request};
/// use illuminate_session::{NullSessionHandler, RequestSessionExt, Session, Store};
/// use illuminate_support::json;
///
/// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// # runtime.block_on(async {
/// let request = Request::create("/", "GET");
/// request.set_session(Arc::new(Store::new("laravel_session", Arc::new(NullSessionHandler), None)));
///
/// with_request(request, async {
///     Session::put("key", "value");
///     Session::flash("status", "Task was successful!");
///
///     assert_eq!(Session::get("key"), json!("value"));
///     assert!(Session::has("status"));
/// }).await;
/// # });
/// ```
pub struct Session;

impl Session {
    /// The current request's session.
    pub fn store() -> Arc<Store> {
        try_session().unwrap_or_else(|| panic!("{}", SessionNotFoundException))
    }

    /// The current request's session, if there is one.
    pub fn try_store() -> Option<Arc<Store>> {
        try_session()
    }

    /// The session manager.
    pub fn manager() -> Arc<SessionManager> {
        if let Some(manager) = try_app::<SessionManager>() {
            return manager;
        }
        let container = Container::get_instance();
        container.singleton_if::<SessionManager>(|c| {
            let config = c
                .try_make::<Repository>()
                .unwrap_or_else(|_| Arc::new(Repository::empty()));
            Arc::new(SessionManager::new(config))
        });
        container.make::<SessionManager>()
    }

    /// Register a custom session driver.
    pub fn extend(
        driver: impl Into<String>,
        factory: impl Fn(&Container) -> Arc<dyn SessionHandler> + Send + Sync + 'static,
    ) {
        Self::manager().extend(driver, factory);
    }

    /// Get all of the session data.
    pub fn all() -> Value {
        Self::store().all()
    }

    /// Get a subset of the session data.
    pub fn only(keys: impl SessionKeys) -> Value {
        Self::store().only(keys)
    }

    /// Get all of the session data except the given keys.
    pub fn except(keys: impl SessionKeys) -> Value {
        Self::store().except(keys)
    }

    /// Determine if the given keys exist (even if null).
    pub fn exists(keys: impl SessionKeys) -> bool {
        Self::store().exists(keys)
    }

    /// Determine if any of the given keys is missing.
    pub fn missing(keys: impl SessionKeys) -> bool {
        Self::store().missing(keys)
    }

    /// Determine if the given keys are present and not null.
    pub fn has(keys: impl SessionKeys) -> bool {
        Self::store().has(keys)
    }

    /// Determine if any of the given keys is present and not null.
    pub fn has_any(keys: impl SessionKeys) -> bool {
        Self::store().has_any(keys)
    }

    /// Get an item from the session.
    pub fn get(key: &str) -> Value {
        Self::store().get(key)
    }

    /// Get an item from the session, or a default.
    pub fn get_or(key: &str, default: impl Into<Value>) -> Value {
        Self::store().get_or(key, default)
    }

    /// Get an item from the session, deserialized into a type.
    pub fn get_as<T: DeserializeOwned>(key: &str) -> Option<T> {
        Self::store().get_as(key)
    }

    /// Get an item and forget it.
    pub fn pull(key: &str) -> Value {
        Self::store().pull(key)
    }

    /// Put a key / value pair in the session.
    pub fn put(key: &str, value: impl Into<Value>) {
        Self::store().put(key, value)
    }

    /// Put many key / value pairs in the session.
    pub fn put_many<K: AsRef<str>, V: Into<Value>>(values: impl IntoIterator<Item = (K, V)>) {
        Self::store().put_many(values)
    }

    /// Get an item from the session, or store the default value.
    pub fn remember(key: &str, callback: impl FnOnce() -> Value) -> Value {
        Self::store().remember(key, callback)
    }

    /// Push a value onto a session array.
    pub fn push(key: &str, value: impl Into<Value>) {
        Self::store().push(key, value)
    }

    /// Increment the value of an item in the session.
    pub fn increment(key: &str) -> i64 {
        Self::store().increment(key)
    }

    /// Increment the value of an item in the session by the given amount.
    pub fn increment_by(key: &str, amount: i64) -> i64 {
        Self::store().increment_by(key, amount)
    }

    /// Decrement the value of an item in the session.
    pub fn decrement(key: &str) -> i64 {
        Self::store().decrement(key)
    }

    /// Decrement the value of an item in the session by the given amount.
    pub fn decrement_by(key: &str, amount: i64) -> i64 {
        Self::store().decrement_by(key, amount)
    }

    /// Flash a key / value pair to the session.
    pub fn flash(key: impl Into<String>, value: impl Into<Value>) {
        Self::store().flash(key, value)
    }

    /// Flash a key / value pair for the current request only.
    pub fn now(key: impl Into<String>, value: impl Into<Value>) {
        Self::store().now(key, value)
    }

    /// Reflash all of the session flash data.
    pub fn reflash() {
        Self::store().reflash()
    }

    /// Reflash a subset of the current flash data.
    pub fn keep(keys: impl SessionKeys) {
        Self::store().keep(keys)
    }

    /// Flash an input array to the session.
    pub fn flash_input(input: Value) {
        Self::store().flash_input(input)
    }

    /// Determine if the session contains old input.
    pub fn has_old_input() -> bool {
        Self::store().has_old_input()
    }

    /// Get an item from the flashed input.
    pub fn get_old_input(key: &str) -> Value {
        Self::store().get_old_input(key)
    }

    /// Remove an item from the session, returning its value.
    pub fn remove(key: &str) -> Value {
        Self::store().remove(key)
    }

    /// Remove one or many items from the session.
    pub fn forget(keys: impl SessionKeys) {
        Self::store().forget(keys)
    }

    /// Remove all of the items from the session.
    pub fn flush() {
        Self::store().flush()
    }

    /// Flush the session data and regenerate the ID.
    pub async fn invalidate() -> Result<bool> {
        Self::store().invalidate().await
    }

    /// Generate a new session identifier.
    pub async fn regenerate(destroy: bool) -> Result<bool> {
        Self::store().regenerate(destroy).await
    }

    /// Generate a new session ID, keeping the data.
    pub async fn migrate(destroy: bool) -> Result<bool> {
        Self::store().migrate(destroy).await
    }

    /// The session ID.
    pub fn id() -> String {
        Self::store().id()
    }

    /// The CSRF token.
    pub fn token() -> Option<String> {
        Self::store().token()
    }

    /// Regenerate the CSRF token.
    pub fn regenerate_token() {
        Self::store().regenerate_token()
    }

    /// The previous URL.
    pub fn previous_url() -> Option<String> {
        Self::store().previous_url()
    }

    /// Set the previous URL.
    pub fn set_previous_url(url: impl Into<String>) {
        Self::store().set_previous_url(url)
    }

    /// Determine if the current request's session has been started.
    pub fn is_started() -> bool {
        Self::store().is_started()
    }
}
