//! The `database` cache driver, end to end: configuration, the `Cache`
//! facade, locks, and the rate limiter on an in-memory SQLite database.

use std::sync::Arc;

use illuminate_cache::{Cache, CacheManager, CacheServiceProvider, RateLimiter};
use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_database::{DB, DatabaseServiceProvider, Schema};
use illuminate_support::{Carbon, Value, json};

/// Boot an application with two in-memory SQLite connections and the given
/// `cache` configuration.
fn app(cache: Value) -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({
        "app": {"name": "Laravel"},
        "database": {
            "default": "sqlite",
            "connections": {
                "sqlite": {"driver": "sqlite", "database": ":memory:"},
                "secondary": {"driver": "sqlite", "database": ":memory:"},
            },
        },
        "cache": cache,
    })));
    DatabaseServiceProvider.register(&container);
    CacheServiceProvider.register(&container);
    (container, guard)
}

/// The skeleton's `config/cache.php`, with the database store as the default.
fn skeleton_cache() -> Value {
    json!({
        "default": "database",
        "stores": {
            "array": {"driver": "array", "serialize": false},
            "database": {
                "driver": "database",
                "connection": null,
                "table": "cache",
                "lock_connection": null,
                "lock_table": null,
            },
        },
        "prefix": "laravel-cache-",
    })
}

/// Run the skeleton's `create_cache_table` migration on a connection.
async fn migrate(connection: &str, table: &str, lock_table: &str) {
    let schema = Schema::connection(connection);
    schema
        .create(table, |table| {
            table.string("key").primary();
            table.medium_text("value");
            table.big_integer("expiration").index();
        })
        .await
        .unwrap();
    schema
        .create(lock_table, |table| {
            table.string("key").primary();
            table.string("owner");
            table.big_integer("expiration").index();
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn the_database_store_works_out_of_the_box() {
    let (container, _guard) = app(skeleton_cache());
    migrate("sqlite", "cache", "cache_locks").await;

    let before = Carbon::now().timestamp();
    assert!(Cache::put("name", "Taylor", 600).await.unwrap());
    assert_eq!(Cache::get("name").await.unwrap(), Some(json!("Taylor")));
    assert_eq!(Cache::string("name").await.unwrap(), "Taylor");

    let row = DB::table("cache").first().await.unwrap().unwrap();
    assert_eq!(row["key"], "laravel-cache-name");
    assert_eq!(row["value"], r#""Taylor""#);
    let expiration = row["expiration"].as_i64().unwrap();
    assert!((before + 600..=Carbon::now().timestamp() + 600).contains(&expiration));

    let repository = container.make::<CacheManager>().default_store().unwrap();
    assert_eq!(repository.get_name(), Some("database"));
    assert_eq!(repository.get_store().get_prefix(), "laravel-cache-");

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
    assert!(
        Cache::forever("config", json!({"debug": true}))
            .await
            .unwrap()
    );
    assert!(Cache::touch("name", 60).await.unwrap());
    assert_eq!(Cache::pull("other").await.unwrap(), Some(json!("Abigail")));
    assert!(Cache::missing("other").await.unwrap());

    let many = Cache::many(["name", "missing"]).await.unwrap();
    assert_eq!(many.get("name"), Some(&Some(json!("Taylor"))));
    assert_eq!(many.get("missing"), Some(&None));

    // Storing for zero seconds forgets the item.
    Cache::put("name", "gone", 0).await.unwrap();
    assert!(Cache::missing("name").await.unwrap());

    assert!(Cache::flush().await.unwrap());
    assert_eq!(DB::table("cache").count().await.unwrap(), 0);
}

#[tokio::test]
async fn database_locks_go_to_the_lock_table() {
    let (_container, _guard) = app(skeleton_cache());
    migrate("sqlite", "cache", "cache_locks").await;

    let lock = Cache::lock("processing", 10).unwrap();
    assert!(lock.get().await.unwrap());
    assert!(!Cache::lock("processing", 10).unwrap().get().await.unwrap());

    let row = DB::table("cache_locks").first().await.unwrap().unwrap();
    assert_eq!(row["key"], "laravel-cache-processing");
    assert_eq!(row["owner"], lock.owner());

    let restored = Cache::restore_lock("processing", lock.owner()).unwrap();
    assert!(restored.release().await.unwrap());
    assert!(Cache::lock("processing", 10).unwrap().get().await.unwrap());

    let value = Cache::without_overlapping("report", || async { "done" }, 10, 1)
        .await
        .unwrap();
    assert_eq!(value, "done");

    assert!(Cache::flush_locks().await.unwrap());
    assert_eq!(DB::table("cache_locks").count().await.unwrap(), 0);
}

#[tokio::test]
async fn tables_connections_and_lock_options_are_configurable() {
    let (_container, _guard) = app(json!({
        "default": "database",
        "stores": {
            "database": {
                "driver": "database",
                "connection": "secondary",
                "table": "my_cache",
                "lock_connection": "sqlite",
                "lock_table": "my_locks",
                "lock_lottery": [0, 1],
                "lock_timeout": 30,
                "prefix": "app_",
            },
        },
    }));
    migrate("secondary", "my_cache", "unused_locks").await;
    migrate("sqlite", "unused_cache", "my_locks").await;

    Cache::put("key", "value", 60).await.unwrap();
    let rows = DB::connection("secondary")
        .table("my_cache")
        .get()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows.first().unwrap()["key"], "app_key");
    assert_eq!(DB::table("unused_cache").count().await.unwrap(), 0);

    let before = Carbon::now().timestamp();
    assert!(Cache::lock("forever", 0).unwrap().get().await.unwrap());
    let lock = DB::table("my_locks").first().await.unwrap().unwrap();
    assert_eq!(lock["key"], "app_forever");
    let expiration = lock["expiration"].as_i64().unwrap();
    assert!((before + 30..=Carbon::now().timestamp() + 30).contains(&expiration));
    assert_eq!(
        DB::connection("secondary")
            .table("unused_locks")
            .count()
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn the_lock_connection_defaults_to_the_cache_connection() {
    let (_container, _guard) = app(json!({
        "default": "database",
        "stores": {"database": {"driver": "database", "connection": "secondary"}},
    }));
    migrate("secondary", "cache", "cache_locks").await;

    assert!(Cache::lock("job", 10).unwrap().get().await.unwrap());
    assert_eq!(
        DB::connection("secondary")
            .table("cache_locks")
            .count()
            .await
            .unwrap(),
        1
    );
    // Without `cache.prefix`, keys are prefixed with the application's name.
    let row = DB::connection("secondary")
        .table("cache_locks")
        .first()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row["key"], "laravel-cache-job");
}

#[tokio::test]
async fn the_rate_limiter_counts_attempts_in_the_database() {
    let (container, _guard) = app(skeleton_cache());
    migrate("sqlite", "cache", "cache_locks").await;
    let limiter = container.make::<RateLimiter>();

    for _ in 0..3 {
        limiter.hit("login:taylor", 60).await.unwrap();
    }
    assert_eq!(limiter.attempts("login:taylor").await.unwrap(), 3);
    assert!(limiter.too_many_attempts("login:taylor", 3).await.unwrap());
    assert!(!limiter.too_many_attempts("login:taylor", 5).await.unwrap());
    assert!(limiter.available_in("login:taylor").await.unwrap() <= 60);

    limiter.clear("login:taylor").await.unwrap();
    assert_eq!(limiter.attempts("login:taylor").await.unwrap(), 0);
}

#[tokio::test]
async fn missing_tables_are_reported_as_query_errors() {
    let (_container, _guard) = app(skeleton_cache());
    let error = Cache::get("anything").await.unwrap_err();
    assert!(error.to_string().contains("no such table"), "{error}");
}
