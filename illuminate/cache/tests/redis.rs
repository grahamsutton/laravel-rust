//! The `redis` cache driver, end to end against a real `redis-server`
//! started for this test binary: configuration, the `Cache` facade,
//! serialization, locks, tags, and the rate limiter.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use illuminate_cache::{
    Cache, CacheManager, CacheServiceProvider, LockTimeoutException, RateLimiter, RedisStore,
    Repository as CacheRepository,
};
use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_redis::testing::RedisServer;
use illuminate_redis::{Connection, ConnectionConfig, Redis, RedisServiceProvider};
use illuminate_support::{Value, json};

struct TestApp {
    container: Arc<Container>,
    _server: Arc<RedisServer>,
    _guard: LocalInstanceGuard,
}

/// Boot an application whose `default` and `cache` Redis connections use
/// fresh databases of the shared server.
fn app_with(prefix: &str, cache: Value) -> TestApp {
    let server = RedisServer::shared();
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    let mut redis = server.config();
    redis["options"]["prefix"] = json!(prefix);
    container.instance(Repository::new(json!({
        "app": {"name": "Laravel"},
        "database": {"redis": redis},
        "cache": cache,
    })));
    RedisServiceProvider.register(&container);
    CacheServiceProvider.register(&container);
    TestApp {
        container,
        _server: server,
        _guard: guard,
    }
}

/// The skeleton's `config/cache.php`, with the redis store as the default.
fn skeleton_cache() -> Value {
    json!({
        "default": "redis",
        "stores": {
            "array": {"driver": "array", "serialize": false},
            "redis": {
                "driver": "redis",
                "connection": "cache",
                "lock_connection": "default",
            },
        },
        "prefix": "laravel-cache-",
    })
}

fn app() -> TestApp {
    app_with("", skeleton_cache())
}

/// An unprefixed connection to the database of the given connection.
fn raw(name: &str) -> Connection {
    let connection = Redis::connection(name).unwrap();
    let config = ConnectionConfig {
        prefix: String::new(),
        ..connection.config().clone()
    };
    Connection::new("raw", config).unwrap()
}

#[tokio::test]
async fn the_redis_store_works_out_of_the_box() {
    let app = app();

    assert!(Cache::put("name", "Taylor", 600).await.unwrap());
    assert_eq!(Cache::get("name").await.unwrap(), Some(json!("Taylor")));
    assert_eq!(Cache::string("name").await.unwrap(), "Taylor");

    // Items are plain Redis keys on the `cache` connection, expiring on their own.
    let cache = raw("cache");
    assert_eq!(
        cache.get("laravel-cache-name").await.unwrap().as_deref(),
        Some(r#""Taylor""#)
    );
    let ttl = cache.ttl("laravel-cache-name").await.unwrap();
    assert!((599..=600).contains(&ttl), "{ttl}");
    assert_eq!(
        raw("default").get("laravel-cache-name").await.unwrap(),
        None
    );

    let repository = app
        .container
        .make::<CacheManager>()
        .default_store()
        .unwrap();
    assert_eq!(repository.get_name(), Some("redis"));
    assert_eq!(repository.get_store().get_prefix(), "laravel-cache-");
    assert!(repository.supports_tags());

    let users: Vec<String> = Cache::remember("users", 60, || async {
        Ok(vec!["Taylor".to_string(), "Abigail".to_string()])
    })
    .await
    .unwrap();
    assert_eq!(users.len(), 2);
    assert_eq!(
        Cache::get("users").await.unwrap(),
        Some(json!(["Taylor", "Abigail"]))
    );

    assert!(!Cache::add("name", "Abigail", 60).await.unwrap());
    assert!(Cache::add("other", "Abigail", 60).await.unwrap());
    assert_eq!(Cache::increment("visits").await.unwrap(), 1);
    assert_eq!(Cache::increment_by("visits", 9).await.unwrap(), 10);
    assert_eq!(Cache::decrement("visits").await.unwrap(), 9);
    assert_eq!(Cache::decrement_by("visits", 4).await.unwrap(), 5);
    assert_eq!(Cache::integer("visits").await.unwrap(), 5);
    assert!(
        Cache::forever("config", json!({"debug": true}))
            .await
            .unwrap()
    );
    assert_eq!(cache.ttl("laravel-cache-config").await.unwrap(), -1);
    assert!(Cache::touch("config", 60).await.unwrap());
    assert!(cache.ttl("laravel-cache-config").await.unwrap() > 0);
    assert!(!Cache::touch("missing", 60).await.unwrap());
    assert_eq!(Cache::pull("other").await.unwrap(), Some(json!("Abigail")));
    assert!(Cache::missing("other").await.unwrap());
    assert!(!Cache::forget("other").await.unwrap());

    let many = Cache::many(["name", "missing", "visits"]).await.unwrap();
    assert_eq!(many.get("name"), Some(&Some(json!("Taylor"))));
    assert_eq!(many.get("missing"), Some(&None));
    assert_eq!(many.get("visits"), Some(&Some(json!(5))));
    assert!(Cache::many(Vec::<String>::new()).await.unwrap().is_empty());

    // Storing for zero seconds forgets the item.
    Cache::put("name", "gone", 0).await.unwrap();
    assert!(Cache::missing("name").await.unwrap());

    // Flushing empties the store's whole Redis database — and only that one.
    raw("default").set("untouched", 1).await.unwrap();
    assert!(Cache::flush().await.unwrap());
    assert!(cache.keys("*").await.unwrap().is_empty());
    assert!(raw("default").get("untouched").await.unwrap().is_some());
}

#[tokio::test]
async fn numbers_are_stored_raw_so_redis_can_increment_them() {
    let _app = app();
    let cache = raw("cache");

    Cache::put("count", 5, 60).await.unwrap();
    Cache::put("price", 9.99, 60).await.unwrap();
    Cache::put("numeric-string", "42", 60).await.unwrap();
    Cache::forever("forever-count", 10).await.unwrap();

    assert_eq!(
        cache.get("laravel-cache-count").await.unwrap().as_deref(),
        Some("5")
    );
    assert_eq!(
        cache.get("laravel-cache-price").await.unwrap().as_deref(),
        Some("9.99")
    );
    assert_eq!(
        cache
            .get("laravel-cache-numeric-string")
            .await
            .unwrap()
            .as_deref(),
        Some(r#""42""#)
    );

    // Incrementing keeps the item's expiration...
    assert_eq!(Cache::increment("count").await.unwrap(), 6);
    assert!(cache.ttl("laravel-cache-count").await.unwrap() > 0);
    assert_eq!(Cache::get("count").await.unwrap(), Some(json!(6)));
    assert_eq!(Cache::increment_by("forever-count", 5).await.unwrap(), 15);
    assert_eq!(Cache::get("price").await.unwrap(), Some(json!(9.99)));
    assert_eq!(
        Cache::get("numeric-string").await.unwrap(),
        Some(json!("42"))
    );

    // ...and values that aren't integers can't be incremented.
    assert!(Cache::increment("numeric-string").await.is_err());

    // Values written by other clients come back as strings.
    cache
        .set("laravel-cache-legacy", "plain text")
        .await
        .unwrap();
    assert_eq!(
        Cache::get("legacy").await.unwrap(),
        Some(json!("plain text"))
    );
}

#[tokio::test]
async fn every_json_value_round_trips() {
    let _app = app();
    let values = [
        json!("Taylor"),
        json!(""),
        json!(0),
        json!(-17),
        json!(1.5),
        json!(true),
        json!(false),
        json!([1, "two", null]),
        json!({"name": "Taylor", "roles": ["admin"], "nested": {"deep": 1}}),
        json!("emoji 🚀 and \"quotes\""),
    ];
    for (index, value) in values.iter().enumerate() {
        Cache::put(&format!("value:{index}"), value, 60)
            .await
            .unwrap();
    }
    for (index, value) in values.iter().enumerate() {
        assert_eq!(
            Cache::get(&format!("value:{index}")).await.unwrap(),
            Some(value.clone()),
            "{value}"
        );
    }

    // Null is stored, but reads as a miss.
    Cache::put("nothing", Value::Null, 60).await.unwrap();
    assert!(Cache::missing("nothing").await.unwrap());
}

#[tokio::test]
async fn many_items_are_stored_at_once() {
    let _app = app();
    let cache = raw("cache");

    assert!(
        Cache::put_many(
            [("a", json!(1)), ("b", json!("two")), ("c", json!([3]))],
            120
        )
        .await
        .unwrap()
    );
    let many = Cache::many(["a", "b", "c"]).await.unwrap();
    assert_eq!(
        many.into_values().collect::<Vec<_>>(),
        [Some(json!(1)), Some(json!("two")), Some(json!([3]))]
    );
    let ttl = cache.ttl("laravel-cache-b").await.unwrap();
    assert!((119..=120).contains(&ttl), "{ttl}");

    assert!(
        Cache::put_many(Vec::<(&str, i32)>::new(), 60)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn adding_is_atomic() {
    let _app = app();
    let cache = raw("cache");

    assert!(Cache::add("forever", "first", None::<i64>).await.unwrap());
    assert!(!Cache::add("forever", "second", None::<i64>).await.unwrap());
    assert_eq!(cache.ttl("laravel-cache-forever").await.unwrap(), -1);

    assert!(Cache::add("expiring", "first", 30).await.unwrap());
    assert!(cache.ttl("laravel-cache-expiring").await.unwrap() > 0);

    // Concurrent adds have a single winner.
    let winners = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for attempt in 0..20 {
        let winners = winners.clone();
        let container = Container::get_instance();
        handles.push(tokio::spawn(Container::scope(container, async move {
            if Cache::add("contested", attempt, 30).await.unwrap() {
                winners.fetch_add(1, Ordering::SeqCst);
            }
        })));
    }
    for handle in handles {
        handle.await.unwrap();
    }
    assert_eq!(winners.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn items_expire() {
    let _app = app();
    Cache::put("short", "lived", 1).await.unwrap();
    assert!(Cache::has("short").await.unwrap());

    let started = Instant::now();
    while Cache::has("short").await.unwrap() {
        assert!(started.elapsed() < Duration::from_secs(3), "never expired");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn keys_carry_both_the_redis_and_the_cache_prefix() {
    let _app = app_with("laravel-database-", skeleton_cache());
    Cache::put("name", "Taylor", 60).await.unwrap();

    assert_eq!(
        raw("cache")
            .get("laravel-database-laravel-cache-name")
            .await
            .unwrap()
            .as_deref(),
        Some(r#""Taylor""#)
    );
    assert_eq!(Cache::get("name").await.unwrap(), Some(json!("Taylor")));
}

#[tokio::test]
async fn connections_and_prefixes_are_configurable() {
    let _app = app_with(
        "",
        json!({
            "default": "redis",
            "stores": {
                "redis": {"driver": "redis", "connection": "default", "prefix": "app_"},
                "defaults": {"driver": "redis"},
            },
        }),
    );

    Cache::put("key", "value", 60).await.unwrap();
    assert!(raw("default").get("app_key").await.unwrap().is_some());
    assert!(raw("cache").get("app_key").await.unwrap().is_none());

    // Locks use the `default` connection, the items the `cache` connection,
    // and keys are prefixed with the application's name.
    let defaults = Cache::store("defaults").unwrap();
    defaults.put("key", "value", 60).await.unwrap();
    assert!(
        raw("cache")
            .get("laravel-cache-key")
            .await
            .unwrap()
            .is_some()
    );
    assert!(defaults.lock("job", 10).get().await.unwrap());
    assert!(
        raw("default")
            .get("laravel-cache-job")
            .await
            .unwrap()
            .is_some()
    );

    let store = RedisStore::new(Redis::manager().unwrap(), "custom:", "cache");
    let repository = CacheRepository::new(store);
    repository.put("key", 1, 60).await.unwrap();
    assert!(raw("cache").get("custom:key").await.unwrap().is_some());
    assert!(
        repository.flush_locks().await.is_err(),
        "the lock store isn't separate"
    );
}

// ----------------------------------------------------------------------
// Locks
// ----------------------------------------------------------------------

#[tokio::test]
async fn locks_live_on_the_lock_connection() {
    let _app = app();
    let locks = raw("default");

    let lock = Cache::lock("processing", 10).unwrap();
    assert!(lock.get().await.unwrap());
    assert!(!Cache::lock("processing", 10).unwrap().get().await.unwrap());
    assert!(lock.is_locked().await.unwrap());
    assert!(lock.is_owned_by_current_process().await.unwrap());

    // The lock is a key holding its owner, expiring on its own.
    assert_eq!(
        locks
            .get("laravel-cache-processing")
            .await
            .unwrap()
            .as_deref(),
        Some(lock.owner())
    );
    let ttl = locks.ttl("laravel-cache-processing").await.unwrap();
    assert!((9..=10).contains(&ttl), "{ttl}");
    assert!(
        raw("cache")
            .get("laravel-cache-processing")
            .await
            .unwrap()
            .is_none()
    );

    // Only the owner can release it...
    let other = Cache::lock("processing", 10).unwrap();
    assert!(!other.release().await.unwrap());
    assert!(!other.is_owned_by_current_process().await.unwrap());
    assert!(lock.is_locked().await.unwrap());

    // ...or anyone holding its owner token.
    let restored = Cache::restore_lock("processing", lock.owner()).unwrap();
    assert!(restored.release().await.unwrap());
    assert!(!lock.is_locked().await.unwrap());

    // Force releasing ignores the owner.
    assert!(lock.get().await.unwrap());
    other.force_release().await.unwrap();
    assert!(!lock.is_locked().await.unwrap());

    // Locks without a duration never expire.
    let forever = Cache::lock("forever", 0).unwrap();
    assert!(forever.get().await.unwrap());
    assert!(!Cache::lock("forever", 0).unwrap().get().await.unwrap());
    assert_eq!(locks.ttl("laravel-cache-forever").await.unwrap(), -1);
    assert!(forever.release().await.unwrap());

    // Flushing locks empties the lock connection's database.
    assert!(Cache::lock("flushed", 10).unwrap().get().await.unwrap());
    assert!(Cache::flush_locks().await.unwrap());
    assert!(locks.keys("*").await.unwrap().is_empty());
}

#[tokio::test]
async fn locks_can_be_refreshed_while_owned() {
    let _app = app();
    let locks = raw("default");

    let lock = Cache::lock("refresh", 10).unwrap();
    assert!(lock.get().await.unwrap());
    assert!(lock.refresh(Some(100)).await.unwrap());
    assert!(locks.ttl("laravel-cache-refresh").await.unwrap() > 10);
    assert!(lock.refresh(Some(0)).await.unwrap());
    assert_eq!(locks.ttl("laravel-cache-refresh").await.unwrap(), -1);
    assert!(lock.refresh(None).await.unwrap());

    let stranger = Cache::lock("refresh", 10).unwrap();
    assert!(!stranger.refresh(Some(100)).await.unwrap());
}

#[tokio::test]
async fn locks_run_callbacks_and_block_for_their_owner() {
    let _app = app();

    let value = Cache::lock("report", 10)
        .unwrap()
        .get_with(|| async { "generated" })
        .await
        .unwrap();
    assert_eq!(value, Some("generated"));
    assert!(
        !Cache::lock("report", 10)
            .unwrap()
            .is_locked()
            .await
            .unwrap()
    );

    let held = Cache::lock("report", 10).unwrap();
    assert!(held.get().await.unwrap());
    assert_eq!(
        Cache::lock("report", 10)
            .unwrap()
            .get_with(|| async { "never" })
            .await
            .unwrap(),
        None
    );

    // A blocked lock waits for the owner to release it...
    let release = {
        let held = held.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            held.release().await.unwrap();
        })
    };
    let waited = Cache::lock("report", 10)
        .unwrap()
        .between_blocked_attempts_sleep_for(20)
        .block_with(5, || async { "after waiting" })
        .await
        .unwrap();
    assert_eq!(waited, "after waiting");
    release.await.unwrap();

    // ...and gives up once its time is up.
    let held = Cache::lock("report", 10).unwrap();
    assert!(held.get().await.unwrap());
    let error = Cache::lock("report", 10)
        .unwrap()
        .between_blocked_attempts_sleep_for(50)
        .block(1)
        .await
        .unwrap_err();
    assert!(error.is::<LockTimeoutException>());

    let value = Cache::without_overlapping("exclusive", || async { 42 }, 10, 1)
        .await
        .unwrap();
    assert_eq!(value, 42);
}

#[tokio::test]
async fn lock_contention_has_a_single_winner() {
    let _app = app();
    let winners = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..20 {
        let winners = winners.clone();
        let container = Container::get_instance();
        handles.push(tokio::spawn(Container::scope(container, async move {
            if Cache::lock("contested", 10).unwrap().get().await.unwrap() {
                winners.fetch_add(1, Ordering::SeqCst);
            }
        })));
    }
    for handle in handles {
        handle.await.unwrap();
    }
    assert_eq!(winners.load(Ordering::SeqCst), 1);
}

// ----------------------------------------------------------------------
// Tags & the rate limiter
// ----------------------------------------------------------------------

#[tokio::test]
async fn tagged_items_can_be_flushed_together() {
    let _app = app();

    Cache::tags(["people", "artists"])
        .unwrap()
        .put("John", "Lennon", 60)
        .await
        .unwrap();
    Cache::tags(["people", "authors"])
        .unwrap()
        .put("Anne", "Rice", 60)
        .await
        .unwrap();

    assert_eq!(
        Cache::tags(["people", "artists"])
            .unwrap()
            .get("John")
            .await
            .unwrap(),
        Some(json!("Lennon"))
    );

    Cache::tags(["authors"]).unwrap().flush().await.unwrap();
    assert!(
        Cache::tags(["people", "artists"])
            .unwrap()
            .has("John")
            .await
            .unwrap()
    );
    assert!(
        Cache::tags(["people", "authors"])
            .unwrap()
            .missing("Anne")
            .await
            .unwrap()
    );

    Cache::tags(["people"]).unwrap().flush().await.unwrap();
    assert!(
        Cache::tags(["people", "artists"])
            .unwrap()
            .missing("John")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn the_rate_limiter_counts_attempts_in_redis() {
    let app = app();
    let limiter = app.container.make::<RateLimiter>();

    for _ in 0..3 {
        limiter.hit("login:taylor", 60).await.unwrap();
    }
    assert_eq!(limiter.attempts("login:taylor").await.unwrap(), 3);
    assert!(limiter.too_many_attempts("login:taylor", 3).await.unwrap());
    assert!(!limiter.too_many_attempts("login:taylor", 5).await.unwrap());
    let available_in = limiter.available_in("login:taylor").await.unwrap();
    assert!(available_in <= 60, "{available_in}");

    let cache = raw("cache");
    assert_eq!(
        cache
            .get("laravel-cache-login:taylor")
            .await
            .unwrap()
            .as_deref(),
        Some("3")
    );
    assert!(cache.ttl("laravel-cache-login:taylor").await.unwrap() > 0);

    limiter.clear("login:taylor").await.unwrap();
    assert_eq!(limiter.attempts("login:taylor").await.unwrap(), 0);
}

#[tokio::test]
async fn unconfigured_connections_are_reported() {
    let _app = app_with(
        "",
        json!({"default": "redis", "stores": {"redis": {"driver": "redis", "connection": "missing"}}}),
    );
    let error = Cache::get("anything").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Redis connection [missing] not configured."
    );
}
