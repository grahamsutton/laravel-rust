//! The facades provided by the cache component: `Cache` and `RateLimiter`.

use std::future::Future;

use illuminate_http::Request;
use illuminate_support::Result;

use crate::limit::LimiterResponse;
use crate::rate_limiter::LimiterCallback;

pub use crate::facade::Cache;

/// The `RateLimiter` facade.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_cache::{CacheServiceProvider, Limit};
/// use illuminate_cache::facades::RateLimiter;
/// use illuminate_config::Repository;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({
///     "cache": {"default": "array", "stores": {"array": {"driver": "array"}}},
/// })));
/// CacheServiceProvider.register(&container);
///
/// RateLimiter::for_("global", |_request| Limit::per_minute(1000)).unwrap();
///
/// if RateLimiter::too_many_attempts("send-message:1", 5).await.unwrap() {
///     panic!("Too many attempts!");
/// }
/// RateLimiter::hit("send-message:1", 60).await.unwrap();
///
/// assert_eq!(RateLimiter::remaining("send-message:1", 5).await.unwrap(), 4);
/// # });
/// ```
pub struct RateLimiter;

impl RateLimiter {
    /// Get the rate limiter service.
    pub fn instance() -> Result<std::sync::Arc<crate::rate_limiter::RateLimiter>> {
        crate::provider::rate_limiter()
    }

    /// Register a named limiter configuration.
    pub fn for_<F, R>(name: &str, callback: F) -> Result<()>
    where
        F: Fn(&Request) -> R + Send + Sync + 'static,
        R: Into<LimiterResponse>,
    {
        Self::instance()?.for_(name, callback);
        Ok(())
    }

    /// Get the given named rate limiter.
    pub fn limiter(name: &str) -> Result<Option<LimiterCallback>> {
        Ok(Self::instance()?.limiter(name))
    }

    /// Attempt to execute a callback if it's not limited.
    pub async fn attempt<T, F, Fut>(
        key: &str,
        max_attempts: i64,
        callback: F,
        decay_seconds: u64,
    ) -> Result<Option<T>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        Self::instance()?
            .attempt(key, max_attempts, callback, decay_seconds)
            .await
    }

    pub async fn too_many_attempts(key: &str, max_attempts: i64) -> Result<bool> {
        Self::instance()?.too_many_attempts(key, max_attempts).await
    }

    pub async fn hit(key: &str, decay_seconds: u64) -> Result<i64> {
        Self::instance()?.hit(key, decay_seconds).await
    }

    pub async fn increment(key: &str, decay_seconds: u64, amount: i64) -> Result<i64> {
        Self::instance()?
            .increment(key, decay_seconds, amount)
            .await
    }

    pub async fn decrement(key: &str, decay_seconds: u64, amount: i64) -> Result<i64> {
        Self::instance()?
            .decrement(key, decay_seconds, amount)
            .await
    }

    pub async fn attempts(key: &str) -> Result<i64> {
        Self::instance()?.attempts(key).await
    }

    pub async fn reset_attempts(key: &str) -> Result<bool> {
        Self::instance()?.reset_attempts(key).await
    }

    pub async fn remaining(key: &str, max_attempts: i64) -> Result<i64> {
        Self::instance()?.remaining(key, max_attempts).await
    }

    pub async fn retries_left(key: &str, max_attempts: i64) -> Result<i64> {
        Self::instance()?.retries_left(key, max_attempts).await
    }

    pub async fn clear(key: &str) -> Result<()> {
        Self::instance()?.clear(key).await
    }

    pub async fn available_in(key: &str) -> Result<i64> {
        Self::instance()?.available_in(key).await
    }
}
