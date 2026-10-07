//! The `redis` session driver, end to end through the `StartSession`
//! middleware against a real `redis-server` started for this test binary.

mod common;

use std::sync::Arc;

use common::{Browser, KEY, app_with, destination, web};
use illuminate_cache::CacheServiceProvider;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_encryption::Encrypter;
use illuminate_http::{Request, Response};
use illuminate_redis::testing::RedisServer;
use illuminate_redis::{Connection, ConnectionConfig, Redis, RedisServiceProvider};
use illuminate_session::{
    CacheBasedSessionHandler, RequestSessionExt, SessionHandler, SessionManager,
};
use illuminate_support::{Value, json};

struct TestApp {
    container: Arc<Container>,
    _guard: LocalInstanceGuard,
    _server: Arc<RedisServer>,
}

/// Boot an application whose sessions live in Redis, through the skeleton's
/// `redis` cache store.
fn app(session: Value) -> TestApp {
    let server = RedisServer::shared();
    let mut config = json!({
        "driver": "redis",
        "lifetime": 120,
        "expire_on_close": false,
        "encrypt": false,
        "connection": null,
        "store": null,
        "cookie": "laravel_session",
        "path": "/",
        "same_site": "lax",
        "lottery": [0, 100],
    });
    for (key, value) in session.as_object().unwrap() {
        config[key] = value.clone();
    }

    let (container, guard) = app_with(json!({
        "app": {"name": "Laravel", "key": KEY, "cipher": "AES-256-CBC"},
        "database": {"redis": server.config()},
        "cache": {
            "default": "array",
            "stores": {
                "array": {"driver": "array"},
                "redis": {"driver": "redis", "connection": "cache", "lock_connection": "default"},
                "sessions": {"driver": "redis", "connection": "cache", "prefix": "sessions:"},
            },
            "prefix": "laravel-cache-",
        },
        "session": config,
    }));
    RedisServiceProvider.register(&container);
    CacheServiceProvider.register(&container);
    TestApp {
        container,
        _guard: guard,
        _server: server,
    }
}

fn routes() -> illuminate_http::Destination {
    destination(|request: Request| async move {
        match request.path().as_str() {
            "put" => {
                request.session().put("name", "Taylor");
                Response::new("stored")
            }
            "get" => Response::json(&json!({"name": request.session().get("name")})),
            "flash" => Response::redirect("/get").with("status", "Saved!"),
            "status" => Response::json(&json!({"status": request.session().get("status")})),
            "logout" => {
                request.session().invalidate().await.unwrap();
                Response::new("logged out")
            }
            _ => Response::make("Not Found", 404),
        }
    })
}

fn session_id(browser: &Browser, app: &Container) -> String {
    let encrypted = browser
        .cookie("laravel_session")
        .expect("the session cookie was set");
    let decrypted = app.make::<Encrypter>().decrypt_string(&encrypted).unwrap();
    illuminate_cookie::CookieValuePrefix::remove(&decrypted)
}

/// An unprefixed connection to the database of the given Redis connection.
fn raw(name: &str) -> Connection {
    let connection = Redis::connection(name).unwrap();
    let config = ConnectionConfig {
        prefix: String::new(),
        ..connection.config().clone()
    };
    Connection::new("raw", config).unwrap()
}

/// The session stored under the given key, decoded.
async fn stored(connection: &Connection, key: &str) -> Option<Value> {
    let cached = connection.get(key).await.unwrap()?;
    let payload: String = serde_json::from_str(&cached).unwrap();
    Some(serde_json::from_str(&payload).unwrap())
}

#[tokio::test]
async fn sessions_round_trip_through_redis() {
    let app = app(json!({}));
    let browser = Browser::new(web(vec![]), routes());

    assert_eq!(browser.get("/put").await.content_string(), "stored");
    let id = session_id(&browser, &app.container);

    // The session is a cache item on the `default` Redis connection, which
    // expires with the session.
    let redis = raw("default");
    let key = format!("laravel-cache-{id}");
    let session = stored(&redis, &key).await.unwrap();
    assert_eq!(session["name"], "Taylor");
    assert!(session["_token"].is_string());
    let ttl = redis.ttl(&key).await.unwrap();
    assert!((7199..=7200).contains(&ttl), "{ttl}");
    assert!(raw("cache").get(&key).await.unwrap().is_none());

    // The next request reads the session back.
    let response = browser.get("/get").await;
    assert_eq!(response.json_body(), json!({"name": "Taylor"}));
    assert_eq!(session_id(&browser, &app.container), id);

    // Flash data survives exactly one more request.
    browser.post("/flash", json!({})).await;
    let response = browser.get("/status").await;
    assert_eq!(response.json_body(), json!({"status": "Saved!"}));
    let response = browser.get("/status").await;
    assert_eq!(response.json_body(), json!({"status": null}));

    // Invalidating the session removes it from Redis.
    browser.get("/logout").await;
    assert!(redis.get(&key).await.unwrap().is_none());
    let new_id = session_id(&browser, &app.container);
    assert_ne!(new_id, id);
    assert!(
        stored(&redis, &format!("laravel-cache-{new_id}"))
            .await
            .is_some()
    );

    // A fresh browser starts a fresh session.
    let stranger = Browser::new(web(vec![]), routes());
    let response = stranger.get("/get").await;
    assert_eq!(response.json_body(), json!({"name": null}));
}

#[tokio::test]
async fn the_connection_and_store_are_configurable() {
    let app = app(json!({"connection": "cache", "store": "sessions", "lifetime": 5}));
    let browser = Browser::new(web(vec![]), routes());

    browser.get("/put").await;
    let id = session_id(&browser, &app.container);

    let redis = raw("cache");
    let key = format!("sessions:{id}");
    assert_eq!(stored(&redis, &key).await.unwrap()["name"], "Taylor");
    let ttl = redis.ttl(&key).await.unwrap();
    assert!((299..=300).contains(&ttl), "{ttl}");
    assert!(raw("default").keys("*").await.unwrap().is_empty());

    let response = browser.get("/get").await;
    assert_eq!(response.json_body(), json!({"name": "Taylor"}));
}

#[tokio::test]
async fn the_handler_is_backed_by_the_redis_cache_store() {
    let app = app(json!({}));
    let manager = app.container.make::<SessionManager>();

    let handler = manager.handler("redis").unwrap();
    handler.write("abc", r#"{"name":"Taylor"}"#).await.unwrap();
    assert_eq!(handler.read("abc").await.unwrap(), r#"{"name":"Taylor"}"#);
    assert_eq!(handler.read("missing").await.unwrap(), "");
    assert_eq!(handler.gc(60).await.unwrap(), 0);
    handler.destroy("abc").await.unwrap();
    assert_eq!(handler.read("abc").await.unwrap(), "");

    let session = manager.driver().unwrap();
    session.start().await.unwrap();
    session.put("name", "Abigail");
    session.save().await.unwrap();
    let key = format!("laravel-cache-{}", session.id());
    assert_eq!(
        stored(&raw("default"), &key).await.unwrap()["name"],
        "Abigail"
    );

    let handler =
        CacheBasedSessionHandler::new(illuminate_cache::Cache::store("redis").unwrap(), 1);
    assert_eq!(handler.get_cache().get_name(), Some("redis"));
    handler.write("short", "lived").await.unwrap();
    let ttl = raw("cache").ttl("laravel-cache-short").await.unwrap();
    assert!((59..=60).contains(&ttl), "{ttl}");
}

#[tokio::test]
async fn a_missing_store_is_reported() {
    let app = app(json!({"store": "missing"}));
    let manager = app.container.make::<SessionManager>();
    assert_eq!(
        manager.driver().err().unwrap().to_string(),
        "Cache store [missing] is not defined."
    );
}
