//! The session store.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use serde::de::DeserializeOwned;

use illuminate_encryption::Encrypter;
use illuminate_http::Request;
use illuminate_support::{Arr, Map, Result, Str, Value, ValueExt, cast, json};

use crate::handlers::{SessionHandler, current_time};

/// One or many session keys: a `&str`, a `String`, or a list of them.
///
/// Methods like [`Store::has`], [`Store::forget`] and [`Store::keep`] accept
/// either, just like Laravel's `has('key')` and `has(['a', 'b'])`.
pub trait SessionKeys {
    /// The keys, as owned strings.
    fn session_keys(&self) -> Vec<String>;
}

impl SessionKeys for str {
    fn session_keys(&self) -> Vec<String> {
        vec![self.to_string()]
    }
}

impl SessionKeys for String {
    fn session_keys(&self) -> Vec<String> {
        vec![self.clone()]
    }
}

impl<T: SessionKeys + ?Sized> SessionKeys for &T {
    fn session_keys(&self) -> Vec<String> {
        (**self).session_keys()
    }
}

impl SessionKeys for [&str] {
    fn session_keys(&self) -> Vec<String> {
        self.iter().map(|key| key.to_string()).collect()
    }
}

impl SessionKeys for [String] {
    fn session_keys(&self) -> Vec<String> {
        self.to_vec()
    }
}

impl<const N: usize> SessionKeys for [&str; N] {
    fn session_keys(&self) -> Vec<String> {
        self.as_slice().session_keys()
    }
}

impl<const N: usize> SessionKeys for [String; N] {
    fn session_keys(&self) -> Vec<String> {
        self.to_vec()
    }
}

impl SessionKeys for Vec<&str> {
    fn session_keys(&self) -> Vec<String> {
        self.as_slice().session_keys()
    }
}

impl SessionKeys for Vec<String> {
    fn session_keys(&self) -> Vec<String> {
        self.clone()
    }
}

/// The session store: the data for one user's session, loaded from (and
/// saved to) a [`SessionHandler`].
///
/// A `Store` is shared as an `Arc<Store>` and uses interior mutability, so
/// every clone of the handle — the request extension, the `session()`
/// helper, the `Session` facade — sees and changes the same data.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_session::{ArraySessionHandler, Store};
/// use illuminate_support::json;
///
/// let session = Store::new("laravel_session", Arc::new(ArraySessionHandler::new(120)), None);
///
/// session.put("user.name", "Taylor");
/// session.push("user.teams", "developers");
///
/// assert_eq!(session.get("user"), json!({"name": "Taylor", "teams": ["developers"]}));
/// assert!(session.has("user.name"));
/// assert_eq!(session.pull("user.name"), json!("Taylor"));
/// assert!(session.missing("user.name"));
/// ```
pub struct Store {
    id: RwLock<String>,
    name: RwLock<String>,
    attributes: RwLock<Value>,
    handler: RwLock<Arc<dyn SessionHandler>>,
    encrypter: Option<Arc<Encrypter>>,
    started: AtomicBool,
    dirty: AtomicBool,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("name", &self.name())
            .field("id", &self.id())
            .field("started", &self.is_started())
            .field("encrypted", &self.is_encrypted())
            .finish()
    }
}

impl Store {
    /// The length of session ID strings (and CSRF tokens).
    pub const ID_LENGTH: usize = 40;

    /// Create a new session instance. An invalid (or missing) ID is replaced
    /// with a fresh random one.
    pub fn new(
        name: impl Into<String>,
        handler: Arc<dyn SessionHandler>,
        id: Option<&str>,
    ) -> Self {
        Self {
            id: RwLock::new(Self::id_or_generate(id)),
            name: RwLock::new(name.into()),
            attributes: RwLock::new(Value::Object(Map::new())),
            handler: RwLock::new(handler),
            encrypter: None,
            started: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
        }
    }

    /// Encrypt the session payload with the given encrypter before it is
    /// handed to the handler (Laravel's `EncryptedStore`).
    pub fn with_encrypter(mut self, encrypter: Arc<Encrypter>) -> Self {
        self.encrypter = Some(encrypter);
        self
    }

    /// Determine if the session payload is encrypted.
    pub fn is_encrypted(&self) -> bool {
        self.encrypter.is_some()
    }

    /// The encrypter used for the payload, if the session is encrypted.
    pub fn get_encrypter(&self) -> Option<Arc<Encrypter>> {
        self.encrypter.clone()
    }

    // ------------------------------------------------------------------
    // Lifecycle
    // ------------------------------------------------------------------

    /// Start the session, reading the data from the handler.
    pub async fn start(&self) -> Result<bool> {
        self.load_session().await?;
        self.dirty.store(false, Ordering::SeqCst);

        if !self.has("_token") {
            self.regenerate_token();
        }

        self.started.store(true, Ordering::SeqCst);
        Ok(true)
    }

    async fn load_session(&self) -> Result<()> {
        let data = self.read_from_handler().await?;
        let mut attributes = self.attributes.write().unwrap();
        if let Value::Object(current) = &mut *attributes {
            for (key, value) in data {
                current.insert(key, value);
            }
        }
        Ok(())
    }

    async fn read_from_handler(&self) -> Result<Map<String, Value>> {
        let handler = self.get_handler();
        let data = handler.read(&self.id()).await?;
        if data.is_empty() {
            return Ok(Map::new());
        }
        match serde_json::from_str::<Value>(&self.prepare_for_unserialize(data)) {
            Ok(Value::Object(attributes)) => Ok(attributes),
            _ => Ok(Map::new()),
        }
    }

    /// Decrypt the raw payload of an encrypted session (an empty session
    /// when it can't be decrypted).
    fn prepare_for_unserialize(&self, data: String) -> String {
        match &self.encrypter {
            Some(encrypter) => encrypter
                .decrypt_string(&data)
                .unwrap_or_else(|_| "{}".to_string()),
            None => data,
        }
    }

    fn prepare_for_storage(&self, data: String) -> Result<String> {
        match &self.encrypter {
            Some(encrypter) => Ok(encrypter.encrypt_string(&data)?),
            None => Ok(data),
        }
    }

    /// Save the session data to storage, aging the flash data first.
    pub async fn save(&self) -> Result<()> {
        self.age_flash_data();

        let payload = serde_json::to_string(&*self.attributes.read().unwrap())?;
        let payload = self.prepare_for_storage(payload)?;

        let handler = self.get_handler();
        handler.write(&self.id(), &payload).await?;

        self.started.store(false, Ordering::SeqCst);
        self.dirty.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Age the flash data: data flashed during the previous request is
    /// removed, and data flashed during this one becomes "old".
    pub fn age_flash_data(&self) {
        let old = self.string_list("_flash.old");
        self.forget(old);
        let new = self.get_or("_flash.new", json!([]));
        self.put("_flash.old", new);
        self.put("_flash.new", json!([]));
    }

    /// Determine if the session has been started.
    pub fn is_started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    /// Determine if the session data changed since it was started (or saved).
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::SeqCst)
    }

    // ------------------------------------------------------------------
    // Retrieving data
    // ------------------------------------------------------------------

    /// Get all of the session data.
    pub fn all(&self) -> Value {
        self.attributes.read().unwrap().clone()
    }

    /// Get a subset of the session data.
    pub fn only(&self, keys: impl SessionKeys) -> Value {
        let keys = keys.session_keys();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        Arr::only(&self.attributes.read().unwrap(), &keys)
    }

    /// Get all of the session data except for the given keys.
    pub fn except(&self, keys: impl SessionKeys) -> Value {
        let keys = keys.session_keys();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        Arr::except(&self.attributes.read().unwrap(), &keys)
    }

    /// Determine if every given key exists in the session, even if its value is null.
    pub fn exists(&self, keys: impl SessionKeys) -> bool {
        let attributes = self.attributes.read().unwrap();
        keys.session_keys()
            .iter()
            .all(|key| attributes.dot(key).is_some())
    }

    /// Determine if any of the given keys is missing from the session.
    pub fn missing(&self, keys: impl SessionKeys) -> bool {
        !self.exists(keys)
    }

    /// Determine if every given key is present and not null.
    pub fn has(&self, keys: impl SessionKeys) -> bool {
        let attributes = self.attributes.read().unwrap();
        keys.session_keys()
            .iter()
            .all(|key| attributes.dot(key).is_some_and(|value| !value.is_null()))
    }

    /// Determine if any of the given keys is present and not null.
    pub fn has_any(&self, keys: impl SessionKeys) -> bool {
        let attributes = self.attributes.read().unwrap();
        keys.session_keys()
            .iter()
            .any(|key| attributes.dot(key).is_some_and(|value| !value.is_null()))
    }

    /// Get an item from the session ("dot" notation; `null` when missing).
    pub fn get(&self, key: &str) -> Value {
        self.attributes.read().unwrap().dot_or_null(key)
    }

    /// Get an item from the session, or a default when the key doesn't exist.
    pub fn get_or(&self, key: &str, default: impl Into<Value>) -> Value {
        self.attributes
            .read()
            .unwrap()
            .dot(key)
            .cloned()
            .unwrap_or_else(|| default.into())
    }

    /// Get an item from the session, or compute a default when the key doesn't exist.
    pub fn get_or_else(&self, key: &str, default: impl FnOnce() -> Value) -> Value {
        let value = self.attributes.read().unwrap().dot(key).cloned();
        value.unwrap_or_else(default)
    }

    /// Get an item from the session, deserialized (leniently) into a type.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use illuminate_session::{NullSessionHandler, Store};
    /// let session = Store::new("session", Arc::new(NullSessionHandler), None);
    /// session.put("count", "3");
    ///
    /// assert_eq!(session.get_as::<i64>("count"), Some(3));
    /// assert_eq!(session.get_as::<i64>("missing"), None);
    /// ```
    pub fn get_as<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        match self.get(key) {
            Value::Null => None,
            value => cast(value).ok(),
        }
    }

    /// Get the value of a given key and then forget it.
    pub fn pull(&self, key: &str) -> Value {
        self.pull_or(key, Value::Null)
    }

    /// Get the value of a given key (or a default) and then forget it.
    pub fn pull_or(&self, key: &str, default: impl Into<Value>) -> Value {
        let value = self.get_or(key, default);
        self.forget(key);
        value
    }

    // ------------------------------------------------------------------
    // Storing data
    // ------------------------------------------------------------------

    fn mutate<R>(&self, callback: impl FnOnce(&mut Value) -> R) -> R {
        self.dirty.store(true, Ordering::SeqCst);
        callback(&mut self.attributes.write().unwrap())
    }

    /// Put a key / value pair in the session ("dot" notation creates nested data).
    pub fn put(&self, key: &str, value: impl Into<Value>) {
        let value = value.into();
        self.mutate(|attributes| Arr::set(attributes, key, value));
    }

    /// Put many key / value pairs in the session.
    pub fn put_many<K, V>(&self, values: impl IntoIterator<Item = (K, V)>)
    where
        K: AsRef<str>,
        V: Into<Value>,
    {
        for (key, value) in values {
            self.put(key.as_ref(), value);
        }
    }

    /// Put every key / value pair of the given object in the session.
    pub fn replace(&self, attributes: Value) {
        if let Value::Object(attributes) = attributes {
            self.put_many(attributes);
        }
    }

    /// Get an item from the session, or store the default value.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use illuminate_session::{NullSessionHandler, Store};
    /// # use illuminate_support::json;
    /// let session = Store::new("session", Arc::new(NullSessionHandler), None);
    ///
    /// assert_eq!(session.remember("theme", || json!("dark")), json!("dark"));
    /// assert_eq!(session.remember("theme", || json!("light")), json!("dark"));
    /// ```
    pub fn remember(&self, key: &str, callback: impl FnOnce() -> Value) -> Value {
        let value = self.get(key);
        if !value.is_null() {
            return value;
        }
        let value = callback();
        self.put(key, value.clone());
        value
    }

    /// Push a value onto a session array.
    pub fn push(&self, key: &str, value: impl Into<Value>) {
        let value = value.into();
        let list = match self.get(key) {
            Value::Null => vec![value],
            Value::Array(mut items) => {
                items.push(value);
                items
            }
            Value::Object(map) => {
                let mut items: Vec<Value> = map.into_iter().map(|(_, v)| v).collect();
                items.push(value);
                items
            }
            scalar => vec![scalar, value],
        };
        self.put(key, Value::Array(list));
    }

    /// Increment the value of an item in the session by one.
    pub fn increment(&self, key: &str) -> i64 {
        self.increment_by(key, 1)
    }

    /// Increment the value of an item in the session.
    pub fn increment_by(&self, key: &str, amount: i64) -> i64 {
        let value = self.get(key).to_i64_lossy().unwrap_or(0) + amount;
        self.put(key, value);
        value
    }

    /// Decrement the value of an item in the session by one.
    pub fn decrement(&self, key: &str) -> i64 {
        self.increment_by(key, -1)
    }

    /// Decrement the value of an item in the session.
    pub fn decrement_by(&self, key: &str, amount: i64) -> i64 {
        self.increment_by(key, -amount)
    }

    // ------------------------------------------------------------------
    // Flash data
    // ------------------------------------------------------------------

    /// Flash a key / value pair to the session: available now and during
    /// the next request, then forgotten.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use illuminate_session::{NullSessionHandler, Store};
    /// # use illuminate_support::json;
    /// let session = Store::new("session", Arc::new(NullSessionHandler), None);
    ///
    /// session.flash("status", "Task was successful!");
    /// assert_eq!(session.get("status"), json!("Task was successful!"));
    ///
    /// session.age_flash_data(); // the end of this request
    /// assert!(session.has("status"));
    ///
    /// session.age_flash_data(); // the end of the next one
    /// assert!(session.missing("status"));
    /// ```
    pub fn flash(&self, key: impl Into<String>, value: impl Into<Value>) {
        let key = key.into();
        self.put(&key, value);
        self.push("_flash.new", key.clone());
        self.remove_from_old_flash_data(&[key]);
    }

    /// Flash a key / value pair to the session for the current request only.
    pub fn now(&self, key: impl Into<String>, value: impl Into<Value>) {
        let key = key.into();
        self.put(&key, value);
        self.push("_flash.old", key);
    }

    /// Reflash all of the session flash data for another request.
    pub fn reflash(&self) {
        let old = self.string_list("_flash.old");
        self.merge_new_flashes(old);
        self.put("_flash.old", json!([]));
    }

    /// Reflash a subset of the current flash data.
    pub fn keep(&self, keys: impl SessionKeys) {
        let keys = keys.session_keys();
        self.merge_new_flashes(keys.clone());
        self.remove_from_old_flash_data(&keys);
    }

    /// Merge keys into the new flash list (`array_unique(array_merge(...))`).
    fn merge_new_flashes(&self, keys: Vec<String>) {
        let mut unique: Vec<String> = Vec::new();
        for key in self.string_list("_flash.new").into_iter().chain(keys) {
            if !unique.contains(&key) {
                unique.push(key);
            }
        }
        self.put("_flash.new", json!(unique));
    }

    fn remove_from_old_flash_data(&self, keys: &[String]) {
        let old: Vec<String> = self
            .string_list("_flash.old")
            .into_iter()
            .filter(|key| !keys.contains(key))
            .collect();
        self.put("_flash.old", json!(old));
    }

    /// Read a list of keys (`_flash.old`, `_flash.new`) from the session.
    fn string_list(&self, key: &str) -> Vec<String> {
        match self.get(key) {
            Value::Array(items) => items.iter().map(ValueExt::to_string_lossy).collect(),
            Value::Object(map) => map.values().map(ValueExt::to_string_lossy).collect(),
            _ => Vec::new(),
        }
    }

    /// Flash an input array to the session (`_old_input`).
    pub fn flash_input(&self, input: Value) {
        self.flash("_old_input", input);
    }

    /// All of the flashed input.
    pub fn old_input(&self) -> Value {
        self.get_or("_old_input", json!({}))
    }

    /// Get an item from the flashed input (`null` when missing).
    pub fn get_old_input(&self, key: &str) -> Value {
        self.old_input().dot_or_null(key)
    }

    /// Get an item from the flashed input, or a default.
    pub fn get_old_input_or(&self, key: &str, default: impl Into<Value>) -> Value {
        self.old_input()
            .dot(key)
            .cloned()
            .unwrap_or_else(|| default.into())
    }

    /// Determine if the session contains any old input.
    pub fn has_old_input(&self) -> bool {
        self.old_input().count() > 0
    }

    /// Determine if the session contains old input for the given key.
    pub fn has_old_input_for(&self, key: &str) -> bool {
        !self.get_old_input(key).is_null()
    }

    // ------------------------------------------------------------------
    // Removing data
    // ------------------------------------------------------------------

    /// Remove an item from the session, returning its value.
    pub fn remove(&self, key: &str) -> Value {
        self.mutate(|attributes| Arr::pull(attributes, key))
    }

    /// Remove one or many items from the session.
    pub fn forget(&self, keys: impl SessionKeys) {
        let keys = keys.session_keys();
        self.mutate(|attributes| {
            for key in &keys {
                Arr::forget(attributes, key);
            }
        });
    }

    /// Remove all of the items from the session.
    pub fn flush(&self) {
        self.mutate(|attributes| *attributes = Value::Object(Map::new()));
    }

    /// Flush the session data and regenerate the ID.
    pub async fn invalidate(&self) -> Result<bool> {
        self.flush();
        self.migrate(true).await
    }

    /// Generate a new session identifier (and CSRF token), optionally
    /// destroying the old session's data.
    pub async fn regenerate(&self, destroy: bool) -> Result<bool> {
        let migrated = self.migrate(destroy).await?;
        self.regenerate_token();
        Ok(migrated)
    }

    /// Generate a new session ID for the session, optionally destroying the
    /// old session's data.
    pub async fn migrate(&self, destroy: bool) -> Result<bool> {
        if destroy {
            let handler = self.get_handler();
            handler.destroy(&self.id()).await?;
        }
        self.set_exists(false);
        *self.id.write().unwrap() = Self::generate_session_id();
        self.dirty.store(true, Ordering::SeqCst);
        Ok(true)
    }

    // ------------------------------------------------------------------
    // Identity
    // ------------------------------------------------------------------

    /// The name of the session (its cookie name).
    pub fn name(&self) -> String {
        self.name.read().unwrap().clone()
    }

    /// Alias of [`Store::name`].
    pub fn get_name(&self) -> String {
        self.name()
    }

    /// Set the name of the session.
    pub fn set_name(&self, name: impl Into<String>) {
        *self.name.write().unwrap() = name.into();
    }

    /// The current session ID.
    pub fn id(&self) -> String {
        self.id.read().unwrap().clone()
    }

    /// Alias of [`Store::id`].
    pub fn get_id(&self) -> String {
        self.id()
    }

    /// Set the session ID (an invalid ID is replaced with a fresh one).
    pub fn set_id(&self, id: Option<&str>) {
        *self.id.write().unwrap() = Self::id_or_generate(id);
    }

    /// Determine if this is a valid session ID: 40 alphanumeric characters.
    ///
    /// ```
    /// use illuminate_session::Store;
    ///
    /// assert!(Store::is_valid_id(&"a".repeat(40)));
    /// assert!(!Store::is_valid_id("../../etc/passwd"));
    /// ```
    pub fn is_valid_id(id: &str) -> bool {
        id.len() == Self::ID_LENGTH && id.chars().all(|c| c.is_ascii_alphanumeric())
    }

    fn id_or_generate(id: Option<&str>) -> String {
        match id {
            Some(id) if Self::is_valid_id(id) => id.to_string(),
            _ => Self::generate_session_id(),
        }
    }

    fn generate_session_id() -> String {
        Str::random(Self::ID_LENGTH)
    }

    /// Tell the handler whether the session exists in storage.
    pub fn set_exists(&self, exists: bool) {
        self.get_handler().set_exists(exists);
    }

    // ------------------------------------------------------------------
    // CSRF token & previous URL
    // ------------------------------------------------------------------

    /// The CSRF token value (present once the session has started).
    pub fn token(&self) -> Option<String> {
        self.get("_token").as_str().map(str::to_string)
    }

    /// Regenerate the CSRF token value.
    pub fn regenerate_token(&self) {
        self.put("_token", Str::random(Self::ID_LENGTH));
    }

    /// Determine if the previous URL is available.
    pub fn has_previous_uri(&self) -> bool {
        self.previous_url().is_some()
    }

    /// The previous URL from the session (`_previous.url`).
    pub fn previous_url(&self) -> Option<String> {
        self.get("_previous.url").as_str().map(str::to_string)
    }

    /// Set the "previous" URL in the session.
    pub fn set_previous_url(&self, url: impl Into<String>) {
        self.put("_previous.url", url.into());
    }

    /// The previous route name from the session (`_previous.route`).
    pub fn previous_route(&self) -> Option<String> {
        self.get("_previous.route").as_str().map(str::to_string)
    }

    /// Set the "previous" route name in the session.
    pub fn set_previous_route(&self, route: Option<&str>) {
        self.put("_previous.route", route.map_or(Value::Null, Value::from));
    }

    /// Record that the user has just confirmed their password.
    pub fn password_confirmed(&self) {
        self.put("auth.password_confirmed_at", current_time());
    }

    // ------------------------------------------------------------------
    // The handler
    // ------------------------------------------------------------------

    /// The underlying session handler.
    pub fn get_handler(&self) -> Arc<dyn SessionHandler> {
        self.handler.read().unwrap().clone()
    }

    /// Replace the underlying session handler.
    pub fn set_handler(&self, handler: Arc<dyn SessionHandler>) {
        *self.handler.write().unwrap() = handler;
    }

    /// Determine if the session handler needs the request.
    pub fn handler_needs_request(&self) -> bool {
        self.get_handler().needs_request()
    }

    /// Hand the request to the handler, if it needs it.
    pub fn set_request_on_handler(&self, request: &Request) {
        let handler = self.get_handler();
        if handler.needs_request() {
            handler.set_request(request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::{ArraySessionHandler, NullSessionHandler};
    use illuminate_support::json;

    fn store() -> Store {
        Store::new("name", Arc::new(NullSessionHandler), None)
    }

    #[test]
    fn session_ids_are_generated_and_validated() {
        let session = store();
        assert!(Store::is_valid_id(&session.id()));
        let id = "a".repeat(40);
        session.set_id(Some(&id));
        assert_eq!(session.get_id(), id);
        session.set_id(Some("invalid"));
        assert_ne!(session.id(), "invalid");
        assert!(Store::is_valid_id(&session.id()));
        assert!(!Store::is_valid_id(&format!("{}-", "a".repeat(39))));
        assert_eq!(session.get_name(), "name");
        session.set_name("other");
        assert_eq!(session.name(), "other");
    }

    #[test]
    fn data_can_be_stored_and_retrieved() {
        let session = store();
        session.put("foo", "bar");
        session.put("baz", json!({"boom": "bang"}));
        session.put_many([("a", 1), ("b", 2)]);
        session.replace(json!({"c": 3}));

        assert_eq!(session.get("foo"), json!("bar"));
        assert_eq!(session.get("baz.boom"), json!("bang"));
        assert_eq!(session.get_or("missing", "default"), json!("default"));
        assert_eq!(session.get_or_else("missing", || json!(5)), json!(5));
        assert_eq!(session.get_as::<String>("foo").as_deref(), Some("bar"));
        assert_eq!(session.only(["foo", "a"]), json!({"foo": "bar", "a": 1}));
        assert_eq!(session.except(["foo", "baz", "a"]), json!({"b": 2, "c": 3}));
        assert_eq!(session.all()["c"], json!(3));
    }

    #[test]
    fn it_checks_for_keys() {
        let session = store();
        session.put("foo", "bar");
        session.put("nothing", Value::Null);

        assert!(session.has("foo"));
        assert!(!session.has("nothing"));
        assert!(session.exists("nothing"));
        assert!(session.exists(["foo", "nothing"]));
        assert!(!session.exists(["foo", "missing"]));
        assert!(session.missing("missing"));
        assert!(!session.missing(vec!["foo", "nothing"]));
        assert!(!session.has(["foo", "nothing"]));
        assert!(session.has_any(["missing", "foo"]));
        assert!(!session.has_any(vec!["missing".to_string(), "nothing".to_string()]));
    }

    #[test]
    fn values_can_be_pulled_pushed_and_counted() {
        let session = store();
        session.put("name", "Taylor");
        assert_eq!(session.pull("name"), json!("Taylor"));
        assert_eq!(session.pull_or("name", "gone"), json!("gone"));

        session.push("teams", "developers");
        session.push("teams", "designers");
        assert_eq!(session.get("teams"), json!(["developers", "designers"]));

        assert_eq!(session.increment("count"), 1);
        assert_eq!(session.increment_by("count", 4), 5);
        assert_eq!(session.decrement("count"), 4);
        assert_eq!(session.decrement_by("count", 2), 2);
        assert_eq!(session.get("count"), json!(2));
    }

    #[test]
    fn data_can_be_forgotten() {
        let session = store();
        session.put("a", 1);
        session.put("b", json!({"c": 2, "d": 3}));
        session.forget("a");
        session.forget(["b.c"]);
        assert_eq!(session.all(), json!({"b": {"d": 3}}));
        assert_eq!(session.remove("b"), json!({"d": 3}));
        session.put("x", 1);
        session.flush();
        assert_eq!(session.all(), json!({}));
    }

    #[test]
    fn flash_data_ages_like_laravel() {
        let session = store();
        session.flash("foo", "bar");
        session.flash("bar", 0);
        session.flash("baz", true);

        assert!(session.has("foo"));
        assert_eq!(session.get("bar"), json!(0));
        assert_eq!(session.get("_flash.new"), json!(["foo", "bar", "baz"]));

        session.age_flash_data();
        assert!(session.has("foo"));
        assert_eq!(session.get("_flash.old"), json!(["foo", "bar", "baz"]));
        assert_eq!(session.get("_flash.new"), json!([]));

        session.age_flash_data();
        assert!(session.missing("foo"));
        assert!(session.missing("bar"));
        assert_eq!(session.get("_flash.old"), json!([]));
    }

    #[test]
    fn flashing_a_key_again_keeps_it_alive() {
        let session = store();
        session.flash("foo", "bar");
        session.age_flash_data();
        session.flash("foo", "baz");
        assert_eq!(session.get("_flash.old"), json!([]));
        session.age_flash_data();
        assert_eq!(session.get("foo"), json!("baz"));
    }

    #[test]
    fn data_can_be_flashed_for_now_only() {
        let session = store();
        session.put("_flash.old", json!(["qu"]));
        session.now("foo", "bar");
        assert_eq!(session.get("_flash.old"), json!(["qu", "foo"]));
        assert!(session.has("foo"));
        session.age_flash_data();
        assert!(session.missing("foo"));
    }

    #[test]
    fn flash_data_can_be_reflashed() {
        let session = store();
        session.flash("foo", "bar");
        session.put("_flash.old", json!(["foo"]));
        session.reflash();
        assert_eq!(session.get("_flash.new"), json!(["foo"]));
        assert_eq!(session.get("_flash.old"), json!([]));

        let session = store();
        session.flash("foo", "bar");
        session.age_flash_data();
        session.reflash();
        session.age_flash_data();
        assert_eq!(session.get("foo"), json!("bar"));
    }

    #[test]
    fn flash_data_can_be_kept() {
        let session = store();
        session.flash("foo", "bar");
        session.flash("baz", "qux");
        session.age_flash_data();
        session.keep(["foo"]);
        assert_eq!(session.get("_flash.new"), json!(["foo"]));
        assert_eq!(session.get("_flash.old"), json!(["baz"]));
        session.age_flash_data();
        assert!(session.has("foo"));
        assert!(session.missing("baz"));
    }

    #[test]
    fn old_input_can_be_flashed_and_read() {
        let session = store();
        assert!(!session.has_old_input());
        session.flash_input(
            json!({"name": "Taylor", "address": {"city": "Little Rock"}, "empty": null}),
        );

        assert!(session.has_old_input());
        assert!(session.has_old_input_for("name"));
        assert!(!session.has_old_input_for("empty"));
        assert!(!session.has_old_input_for("missing"));
        assert_eq!(session.get_old_input("address.city"), json!("Little Rock"));
        assert_eq!(
            session.get_old_input_or("missing", "default"),
            json!("default")
        );
        assert_eq!(session.old_input()["name"], json!("Taylor"));
        assert_eq!(session.get("_flash.new"), json!(["_old_input"]));
    }

    #[test]
    fn values_can_be_remembered() {
        let session = store();
        assert_eq!(session.remember("key", || json!(1)), json!(1));
        assert_eq!(session.remember("key", || json!(2)), json!(1));
    }

    #[test]
    fn previous_urls_and_routes_are_tracked() {
        let session = store();
        assert!(!session.has_previous_uri());
        session.set_previous_url("http://localhost/users");
        session.set_previous_route(Some("users.index"));
        assert_eq!(
            session.previous_url().as_deref(),
            Some("http://localhost/users")
        );
        assert_eq!(session.previous_route().as_deref(), Some("users.index"));
        assert_eq!(
            session.get("_previous"),
            json!({"url": "http://localhost/users", "route": "users.index"})
        );
        session.set_previous_route(None);
        assert_eq!(session.previous_route(), None);
    }

    #[test]
    fn tokens_can_be_regenerated() {
        let session = store();
        assert_eq!(session.token(), None);
        session.regenerate_token();
        let token = session.token().unwrap();
        assert_eq!(token.len(), 40);
        session.regenerate_token();
        assert_ne!(session.token().unwrap(), token);
    }

    #[test]
    fn password_confirmation_is_recorded() {
        let session = store();
        session.password_confirmed();
        assert!(session.get("auth.password_confirmed_at").as_i64().unwrap() > 0);
    }

    #[tokio::test]
    async fn sessions_are_started_and_saved_through_the_handler() {
        let handler = Arc::new(ArraySessionHandler::new(120));
        let session = Store::new("name", handler.clone(), None);
        session.start().await.unwrap();
        assert!(session.is_started());
        assert!(session.is_dirty());
        assert!(session.token().is_some());
        session.put("user", "Taylor");
        session.flash("status", "saved");
        session.save().await.unwrap();
        assert!(!session.is_started());

        let stored: Value = serde_json::from_str(&handler.data(&session.id()).unwrap()).unwrap();
        assert_eq!(stored["user"], json!("Taylor"));
        assert_eq!(stored["_flash"], json!({"old": ["status"], "new": []}));

        let next = Store::new("name", handler.clone(), Some(&session.id()));
        next.start().await.unwrap();
        assert!(!next.is_dirty());
        assert_eq!(next.get("user"), json!("Taylor"));
        assert_eq!(next.get("status"), json!("saved"));
        assert_eq!(next.token(), session.token());
        next.save().await.unwrap();

        let last = Store::new("name", handler.clone(), Some(&session.id()));
        last.start().await.unwrap();
        assert!(last.missing("status"));
        assert_eq!(last.get("user"), json!("Taylor"));
    }

    #[tokio::test]
    async fn sessions_can_be_regenerated_and_invalidated() {
        let handler = Arc::new(ArraySessionHandler::new(120));
        let session = Store::new("name", handler.clone(), None);
        session.start().await.unwrap();
        session.put("user", 1);
        session.save().await.unwrap();

        let old_id = session.id();
        let old_token = session.token().unwrap();
        session.regenerate(false).await.unwrap();
        assert_ne!(session.id(), old_id);
        assert_ne!(session.token().unwrap(), old_token);
        assert!(handler.has(&old_id));
        assert_eq!(session.get("user"), json!(1));

        let id = session.id();
        session.save().await.unwrap();
        session.regenerate(true).await.unwrap();
        assert!(!handler.has(&id));

        let id = session.id();
        session.save().await.unwrap();
        session.invalidate().await.unwrap();
        assert!(!handler.has(&id));
        assert_eq!(session.all(), json!({}));
        assert_ne!(session.id(), id);
    }

    #[tokio::test]
    async fn encrypted_sessions_store_ciphertext() {
        let handler = Arc::new(ArraySessionHandler::new(120));
        let encrypter = Arc::new(Encrypter::new([7u8; 32], "aes-256-cbc").unwrap());
        let session = Store::new("name", handler.clone(), None).with_encrypter(encrypter.clone());
        assert!(session.is_encrypted());
        session.start().await.unwrap();
        session.put("secret", "value");
        session.save().await.unwrap();

        let raw = handler.data(&session.id()).unwrap();
        assert!(!raw.contains("secret"));
        assert!(encrypter.decrypt_string(&raw).unwrap().contains("secret"));

        let next =
            Store::new("name", handler.clone(), Some(&session.id())).with_encrypter(encrypter);
        next.start().await.unwrap();
        assert_eq!(next.get("secret"), json!("value"));

        // A payload that can't be decrypted is treated as an empty session.
        let other = Arc::new(Encrypter::new([8u8; 32], "aes-256-cbc").unwrap());
        let stranger = Store::new("name", handler, Some(&session.id())).with_encrypter(other);
        stranger.start().await.unwrap();
        assert!(stranger.missing("secret"));
    }

    #[test]
    fn the_handler_can_be_swapped() {
        let session = store();
        assert!(!session.handler_needs_request());
        session.set_handler(Arc::new(ArraySessionHandler::new(1)));
        session.set_request_on_handler(&Request::create("/", "GET"));
        assert!(format!("{session:?}").contains("Store"));
        assert!(session.get_encrypter().is_none());
    }
}
