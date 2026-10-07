//! The `session` cache store: cache items in the current user's session.

use std::sync::Arc;

use async_trait::async_trait;
use illuminate_cache::Store;
use illuminate_session::Store as Session;
use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Result, Value, ValueExt, json};

/// Caches items in the current request's session, so each user gets their
/// own cache:
///
/// ```json
/// "session": {"driver": "session", "key": "_cache"}
/// ```
pub struct SessionStore {
    key: String,
}

impl SessionStore {
    /// Create a store keeping its items under the given session key.
    pub fn new(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }

    /// The session key holding the items.
    pub fn key(&self) -> &str {
        &self.key
    }

    fn session() -> Result<Arc<Session>> {
        illuminate_session::try_session()
            .ok_or_else(|| RuntimeException::new("Session store not set on request.").into())
    }

    fn item_key(&self, key: &str) -> String {
        format!("{}.{key}", self.key)
    }

    fn now() -> f64 {
        Carbon::now().timestamp_millis() as f64 / 1000.0
    }

    fn expires_at(seconds: u64) -> f64 {
        if seconds > 0 { Self::now() + seconds as f64 } else { 0.0 }
    }

    /// Every item in the store.
    pub fn all(&self) -> Result<Value> {
        Ok(Self::session()?.get_or(&self.key, json!({})))
    }
}

#[async_trait]
impl Store for SessionStore {
    async fn get(&self, key: &str) -> Result<Option<Value>> {
        let session = Self::session()?;
        let item_key = self.item_key(key);
        if !session.exists(item_key.as_str()) {
            return Ok(None);
        }
        let item = session.get(&item_key);
        let expires_at = item.get("expiresAt").and_then(Value::as_f64).unwrap_or(0.0);
        if expires_at != 0.0 && Self::now() >= expires_at {
            session.forget(item_key.as_str());
            return Ok(None);
        }
        Ok(item.get("value").cloned())
    }

    async fn put(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        Self::session()?.put(
            &self.item_key(key),
            json!({"value": value, "expiresAt": Self::expires_at(seconds)}),
        );
        Ok(true)
    }

    async fn increment(&self, key: &str, value: i64) -> Result<i64> {
        match self.get(key).await? {
            Some(existing) => {
                let incremented = existing.to_i64_lossy().unwrap_or(0) + value;
                Self::session()?.put(&self.item_key(&format!("{key}.value")), incremented);
                Ok(incremented)
            }
            None => {
                self.forever(key, Value::from(value)).await?;
                Ok(value)
            }
        }
    }

    async fn forever(&self, key: &str, value: Value) -> Result<bool> {
        self.put(key, value, 0).await
    }

    async fn touch(&self, key: &str, seconds: u64) -> Result<bool> {
        match self.get(key).await? {
            Some(value) => self.put(key, value, seconds).await,
            None => Ok(false),
        }
    }

    async fn forget(&self, key: &str) -> Result<bool> {
        let session = Self::session()?;
        let item_key = self.item_key(key);
        if session.exists(item_key.as_str()) {
            session.forget(item_key.as_str());
            return Ok(true);
        }
        Ok(false)
    }

    async fn flush(&self) -> Result<bool> {
        Self::session()?.put(&self.key, json!({}));
        Ok(true)
    }
}

/// Register the `session` cache driver.
pub(crate) fn boot() {
    if let Ok(manager) = illuminate_cache::Cache::manager() {
        manager.extend("session", |_, config| {
            let key = config
                .get("key")
                .and_then(Value::as_str)
                .filter(|key| !key.is_empty())
                .unwrap_or("_cache");
            let name = config.get("store").map(ValueExt::to_string_lossy).unwrap_or_default();
            Ok(illuminate_cache::Repository::new(SessionStore::new(key)).with_name(name))
        });
    }
}
