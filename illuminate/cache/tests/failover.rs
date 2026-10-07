//! The `failover` and `storage` stores.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use illuminate_cache::{Cache, CacheFailedOver, CacheServiceProvider, Repository as CacheRepository, Store};
use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_events::Event;
use illuminate_filesystem::FilesystemServiceProvider;
use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Result, Value, json};

/// A store whose server is down.
struct BrokenStore;

fn refused<T>() -> Result<T> {
    Err(RuntimeException::new("Connection refused.").into())
}

#[async_trait]
impl Store for BrokenStore {
    async fn get(&self, _: &str) -> Result<Option<Value>> {
        refused()
    }
    async fn put(&self, _: &str, _: Value, _: u64) -> Result<bool> {
        refused()
    }
    async fn increment(&self, _: &str, _: i64) -> Result<i64> {
        refused()
    }
    async fn forever(&self, _: &str, _: Value) -> Result<bool> {
        refused()
    }
    async fn touch(&self, _: &str, _: u64) -> Result<bool> {
        refused()
    }
    async fn forget(&self, _: &str) -> Result<bool> {
        refused()
    }
    async fn flush(&self) -> Result<bool> {
        refused()
    }
}

fn app(disk: &std::path::Path) -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({
        "app": {"name": "Laravel"},
        "filesystems": {
            "default": "local",
            "disks": {"local": {"driver": "local", "root": disk.to_string_lossy()}},
        },
        "cache": {
            "default": "failover",
            "stores": {
                "broken": {"driver": "broken"},
                "array": {"driver": "array"},
                "failover": {"driver": "failover", "stores": ["broken", "array"]},
                "hopeless": {"driver": "failover", "stores": ["broken"]},
                "empty": {"driver": "failover", "stores": []},
                "storage": {"driver": "storage", "path": "framework/cache/data"},
            },
            "prefix": "",
        },
    })));
    FilesystemServiceProvider.register(&container);
    CacheServiceProvider.register(&container);
    Cache::manager()
        .unwrap()
        .extend("broken", |_, _| Ok(CacheRepository::new(BrokenStore)));
    (container, guard)
}

#[tokio::test]
async fn the_next_store_takes_over_when_one_fails() {
    let dir = tempfile::tempdir().unwrap();
    let (_container, _guard) = app(dir.path());
    let failovers = Arc::new(AtomicUsize::new(0));
    let counter = failovers.clone();
    Event::listen(move |event: &CacheFailedOver| {
        assert_eq!(event.store_name, "broken");
        assert_eq!(event.exception.to_string(), "Connection refused.");
        counter.fetch_add(1, Ordering::SeqCst);
        async {}
    });

    Cache::put("name", "Taylor", 60).await.unwrap();
    assert_eq!(Cache::string("name").await.unwrap(), "Taylor");
    assert_eq!(Cache::increment("hits").await.unwrap(), 1);

    // The value landed in the array store...
    assert_eq!(Cache::store("array").unwrap().string("name").await.unwrap(), "Taylor");
    // ...and the failure was reported once, not on every call.
    assert_eq!(failovers.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_last_error_is_returned_when_every_store_fails() {
    let dir = tempfile::tempdir().unwrap();
    let (_container, _guard) = app(dir.path());

    let error = Cache::store("hopeless").unwrap().put("name", "Taylor", 60).await.unwrap_err();
    assert_eq!(error.to_string(), "Connection refused.");

    let error = Cache::store("empty").unwrap().get("name").await.unwrap_err();
    assert_eq!(error.to_string(), "All failover cache stores failed.");
}

#[tokio::test]
async fn locks_come_from_the_first_store_with_locks() {
    let dir = tempfile::tempdir().unwrap();
    let (_container, _guard) = app(dir.path());

    let lock = Cache::lock("reports", 10).unwrap();
    assert!(lock.get().await.unwrap());
    assert!(!Cache::lock("reports", 10).unwrap().get().await.unwrap());
}

#[tokio::test]
async fn items_are_stored_on_a_filesystem_disk() {
    let dir = tempfile::tempdir().unwrap();
    let (_container, _guard) = app(dir.path());
    let cache = Cache::store("storage").unwrap();

    assert!(cache.put("foo", "bar", 60).await.unwrap());
    let file = dir
        .path()
        .join("framework/cache/data/0b/ee/0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33");
    let contents = std::fs::read_to_string(&file).unwrap();
    assert!(contents.ends_with(r#""bar""#), "{contents}");
    assert_eq!(cache.string("foo").await.unwrap(), "bar");

    assert!(!cache.add("foo", "baz", 60).await.unwrap());
    assert_eq!(cache.increment("count").await.unwrap(), 1);
    assert_eq!(cache.increment_by("count", 5).await.unwrap(), 6);
    assert!(cache.forget("foo").await.unwrap());
    assert!(!file.exists());
    assert!(cache.get("foo").await.unwrap().is_none());

    cache.forever("forever", "always").await.unwrap();
    assert!(cache.flush().await.unwrap());
    assert!(cache.get("forever").await.unwrap().is_none());
    assert!(dir.path().join("framework/cache/data").is_dir());
}

#[tokio::test]
async fn storage_items_expire() {
    let dir = tempfile::tempdir().unwrap();
    let (_container, _guard) = app(dir.path());
    let cache = Cache::store("storage").unwrap();
    Carbon::set_thread_test_now(Some(Carbon::from_timestamp(1_700_000_000)));

    cache.put("foo", "bar", 10).await.unwrap();
    Carbon::set_thread_test_now(Some(Carbon::from_timestamp(1_700_000_009)));
    assert_eq!(cache.string("foo").await.unwrap(), "bar");
    Carbon::set_thread_test_now(Some(Carbon::from_timestamp(1_700_000_010)));
    assert!(cache.get("foo").await.unwrap().is_none());

    Carbon::set_thread_test_now(None);
}
