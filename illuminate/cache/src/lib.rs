//! # Illuminate Cache
//!
//! An expressive, unified API for caching: the [`Cache`] facade in front of
//! array, database, file, and null stores (and any store you
//! [`extend`](CacheManager::extend) it with), atomic [`Lock`]s, and the
//! [`RateLimiter`] with its `throttle` middleware.
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_cache::{Cache, CacheServiceProvider};
//! use illuminate_config::Repository;
//! use illuminate_container::{Container, ServiceProvider};
//! use illuminate_support::json;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container.clone());
//! container.instance(Repository::new(json!({
//!     "cache": {
//!         "default": "array",
//!         "stores": {"array": {"driver": "array", "serialize": false}},
//!     },
//! })));
//! CacheServiceProvider.register(&container);
//!
//! let value: String = Cache::remember("users.count", 600, || async {
//!     Ok("42 users".to_string())
//! }).await.unwrap();
//!
//! assert_eq!(value, "42 users");
//! assert!(Cache::has("users.count").await.unwrap());
//!
//! let lock = Cache::lock("reports", 10).unwrap();
//! assert!(lock.get().await.unwrap());
//! # });
//! ```
//!
//! The `database` driver ([`DatabaseStore`]) keeps items in the `cache`
//! table and locks in the `cache_locks` table of a database connection;
//! set `cache.default` to `"database"` and it is ready to go.

pub mod array_store;
pub mod database_store;
pub mod facade;
pub mod facades;
pub mod file_store;
pub mod limit;
pub mod lock;
pub mod manager;
pub mod null_store;
pub mod provider;
pub mod rate_limiter;
pub mod repository;
pub mod store;
pub mod tags;
pub mod throttle;
pub mod ttl;

mod util;

pub use array_store::ArrayStore;
pub use database_store::{DatabaseLock, DatabaseStore};
pub use facade::{Cache, cache};
pub use file_store::{FileLock, FileStore};
pub use limit::{AfterCallback, Limit, LimiterResponse, ResponseCallback};
pub use lock::{ArrayLock, CacheLock, Lock, LockDriver, LockInfo, LockTimeoutException, NoLock};
pub use manager::{CacheManager, StoreCreator};
pub use null_store::NullStore;
pub use provider::CacheServiceProvider;
pub use rate_limiter::{LimiterCallback, RateLimiter};
pub use repository::{FLEXIBLE_CREATED_KEY_PREFIX, Repository};
pub use store::{BadMethodCallException, LockProvider, Store};
pub use tags::TagSet;
pub use throttle::{
    MissingRateLimiterException, ThrottleRequests, throttle_middleware, throttle_requests_exception,
};
pub use ttl::Ttl;

/// Everything you need in one import.
pub mod prelude {
    pub use crate::facades::{Cache, RateLimiter};
    pub use crate::{Limit, Ttl, cache};
}

/// Freezing time in tests.
///
/// `Carbon::set_test_now` is process wide, so tests that depend on the
/// clock take a shared lock for their duration.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Mutex, MutexGuard};

    use illuminate_support::Carbon;

    static TIME: Mutex<()> = Mutex::new(());

    /// Holds the clock frozen until dropped.
    pub struct FrozenTime {
        _guard: MutexGuard<'static, ()>,
    }

    impl FrozenTime {
        /// Move the frozen clock forward.
        pub fn travel_seconds(&self, seconds: i64) {
            Carbon::set_test_now(Some(Carbon::now().add_seconds(seconds)));
        }
    }

    impl Drop for FrozenTime {
        fn drop(&mut self) {
            Carbon::set_test_now(None);
        }
    }

    /// Freeze the clock at the given moment.
    pub fn freeze_time(now: Carbon) -> FrozenTime {
        let guard = TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Carbon::set_test_now(Some(now));
        FrozenTime { _guard: guard }
    }
}
