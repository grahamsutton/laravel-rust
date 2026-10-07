//! Session blocking: `Route::block()` and `session.block`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{KEY, app_with, array_session};
use illuminate_http::{HeaderMap, HeaderValue, Middleware, Request, Response};
use illuminate_routing::Router;
use illuminate_session::{SessionManager, StartSession};
use illuminate_support::{Value, json};

type Log = Arc<Mutex<Vec<String>>>;

fn config(block: bool, block_store: Option<&str>) -> Value {
    let mut session = array_session();
    session["block"] = json!(block);
    session["block_store"] = json!(block_store);
    json!({
        "app": {"name": "Laravel", "key": KEY, "cipher": "AES-256-CBC"},
        "session": session,
        "cache": {
            "default": "array",
            "stores": {"array": {"driver": "array"}, "locks": {"driver": "array"}},
        },
    })
}

/// A route that logs when it starts and ends, sleeping in between.
fn slow_route(router: &Router, uri: &str, log: &Log) -> illuminate_routing::RouteDefinition {
    let log = log.clone();
    let name = uri.trim_start_matches('/').to_string();
    router
        .post(uri, move |_request: Request| {
            let log = log.clone();
            let name = name.clone();
            async move {
                log.lock().unwrap().push(format!("{name}:start"));
                tokio::time::sleep(Duration::from_millis(60)).await;
                log.lock().unwrap().push(format!("{name}:end"));
                Response::new(name)
            }
        })
        .middleware(Arc::new(StartSession::new()) as Arc<dyn Middleware>)
}

fn request(uri: &str, session_id: &str) -> Request {
    let mut headers = HeaderMap::new();
    headers.insert(
        "cookie",
        HeaderValue::from_str(&format!("laravel_session={session_id}")).unwrap(),
    );
    Request::create_with(uri, "POST", json!({}), headers)
}

/// Both requests started before either finished.
fn assert_overlapped(log: &Log) {
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 4);
    let mut started: Vec<&str> = log[..2].iter().map(String::as_str).collect();
    started.sort();
    assert_eq!(started, ["order:start", "profile:start"], "{log:?}");
}

fn session_id() -> String {
    illuminate_support::Str::random(40)
}

#[tokio::test]
async fn blocking_routes_wait_for_each_other() {
    let (_app, _guard) = app_with(config(false, None));
    let router = Router::new();
    let log: Log = Arc::default();
    slow_route(&router, "/profile", &log).block(10, 10);
    slow_route(&router, "/order", &log).block(10, 10);

    let id = session_id();
    let (first, second) = tokio::join!(
        router.dispatch(request("/profile", &id)),
        router.dispatch(request("/order", &id)),
    );

    assert_eq!(first.content_string(), "profile");
    assert_eq!(second.content_string(), "order");
    assert_eq!(
        *log.lock().unwrap(),
        ["profile:start", "profile:end", "order:start", "order:end"]
    );
}

#[tokio::test]
async fn non_blocking_routes_run_concurrently() {
    let (_app, _guard) = app_with(config(false, None));
    let router = Router::new();
    let log: Log = Arc::default();
    slow_route(&router, "/profile", &log);
    slow_route(&router, "/order", &log)
        .block(10, 10)
        .without_blocking();

    let id = session_id();
    tokio::join!(
        router.dispatch(request("/profile", &id)),
        router.dispatch(request("/order", &id)),
    );

    assert_overlapped(&log);
}

#[tokio::test]
async fn different_sessions_do_not_block_each_other() {
    let (_app, _guard) = app_with(config(false, None));
    let router = Router::new();
    let log: Log = Arc::default();
    slow_route(&router, "/profile", &log).block(10, 10);
    slow_route(&router, "/order", &log).block(10, 10);

    tokio::join!(
        router.dispatch(request("/profile", &session_id())),
        router.dispatch(request("/order", &session_id())),
    );

    assert_overlapped(&log);
}

#[tokio::test]
async fn the_session_block_option_blocks_every_route() {
    let (app, _guard) = app_with(config(true, Some("locks")));
    let manager = app.make::<SessionManager>();
    assert!(manager.should_block());
    assert_eq!(manager.block_driver().as_deref(), Some("locks"));
    assert_eq!(manager.default_route_block_lock_seconds(), 10);
    assert_eq!(manager.default_route_block_wait_seconds(), 10);

    let router = Router::new();
    let log: Log = Arc::default();
    slow_route(&router, "/profile", &log);
    slow_route(&router, "/order", &log);

    let id = session_id();
    tokio::join!(
        router.dispatch(request("/profile", &id)),
        router.dispatch(request("/order", &id)),
    );

    assert_eq!(
        *log.lock().unwrap(),
        ["profile:start", "profile:end", "order:start", "order:end"]
    );
}

#[tokio::test]
async fn requests_give_up_when_the_lock_is_held_too_long() {
    let (_app, _guard) = app_with(config(false, None));
    let router = Router::new();
    let log: Log = Arc::default();
    slow_route(&router, "/profile", &log).block(10, 10);
    slow_route(&router, "/order", &log).block(10, 0);

    let id = session_id();
    let (first, second) = tokio::join!(router.dispatch(request("/profile", &id)), async {
        // Let the first request take the lock.
        tokio::time::sleep(Duration::from_millis(10)).await;
        router.dispatch(request("/order", &id)).await
    },);

    assert_eq!(first.status_code(), 200);
    assert_eq!(second.status_code(), 500);
    assert_eq!(*log.lock().unwrap(), ["profile:start", "profile:end"]);

    // Once released, the session is free again.
    let third = router.dispatch(request("/order", &id)).await;
    assert_eq!(third.content_string(), "order");
}
