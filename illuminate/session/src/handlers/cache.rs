use illuminate_cache::Repository;
use illuminate_http::async_trait;
use illuminate_support::{Result, Value};

use super::SessionHandler;

/// Keeps sessions in a cache store — the handler behind the `redis`,
/// `memcached`, `dynamodb` and `apc` drivers. Each session is a cache item
/// named after the session's id, expiring after the session lifetime, so
/// there is nothing to garbage collect.
///
/// ```
/// use illuminate_cache::{ArrayStore, Repository};
/// use illuminate_session::{CacheBasedSessionHandler, SessionHandler};
///
/// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// # runtime.block_on(async {
/// let handler = CacheBasedSessionHandler::new(Repository::new(ArrayStore::new()), 120);
/// handler.write("abc", "payload").await.unwrap();
///
/// assert_eq!(handler.read("abc").await.unwrap(), "payload");
/// assert_eq!(handler.read("missing").await.unwrap(), "");
/// # });
/// ```
#[derive(Debug, Clone)]
pub struct CacheBasedSessionHandler {
    cache: Repository,
    minutes: i64,
}

impl CacheBasedSessionHandler {
    /// Create a handler keeping sessions in the given cache for the given
    /// number of minutes.
    pub fn new(cache: Repository, minutes: i64) -> Self {
        Self { cache, minutes }
    }

    /// The cache the sessions are kept in.
    pub fn get_cache(&self) -> &Repository {
        &self.cache
    }
}

#[async_trait]
impl SessionHandler for CacheBasedSessionHandler {
    async fn read(&self, session_id: &str) -> Result<String> {
        Ok(match self.cache.get(session_id).await? {
            Some(Value::String(data)) => data,
            Some(other) => other.to_string(),
            None => String::new(),
        })
    }

    async fn write(&self, session_id: &str, data: &str) -> Result<()> {
        self.cache
            .put(session_id, data, self.minutes.saturating_mul(60))
            .await?;
        Ok(())
    }

    async fn destroy(&self, session_id: &str) -> Result<()> {
        self.cache.forget(session_id).await?;
        Ok(())
    }

    async fn gc(&self, _lifetime: u64) -> Result<usize> {
        Ok(0)
    }
}
