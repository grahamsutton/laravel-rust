//! The `database` session driver, end to end through the `StartSession`
//! middleware on an in-memory SQLite database.

mod common;

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use common::{Browser, KEY, app_with, destination, web};
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_database::{DB, DatabaseServiceProvider, Schema};
use illuminate_encryption::Encrypter;
use illuminate_http::{HeaderMap, HeaderValue, Request, Response, middleware_fn};
use illuminate_session::{
    DatabaseSessionHandler, RequestSessionExt, SessionHandler, SessionManager,
};
use illuminate_support::{Value, json};

/// Boot an application whose sessions live in the database.
async fn app(session: Value) -> (Arc<Container>, LocalInstanceGuard) {
    let mut config = json!({
        "driver": "database",
        "lifetime": 120,
        "expire_on_close": false,
        "encrypt": false,
        "connection": null,
        "table": "sessions",
        "cookie": "laravel_session",
        "path": "/",
        "same_site": "lax",
        "lottery": [0, 100],
    });
    for (key, value) in session.as_object().unwrap() {
        config[key] = value.clone();
    }
    let table = config["table"].as_str().unwrap().to_string();
    let connection = config["connection"]
        .as_str()
        .unwrap_or("sqlite")
        .to_string();

    let (container, guard) = app_with(json!({
        "app": {"name": "Laravel", "key": KEY, "cipher": "AES-256-CBC"},
        "database": {
            "default": "sqlite",
            "connections": {
                "sqlite": {"driver": "sqlite", "database": ":memory:"},
                "secondary": {"driver": "sqlite", "database": ":memory:"},
            },
        },
        "session": config,
    }));
    DatabaseServiceProvider.register(&container);

    // The skeleton's `sessions` table.
    Schema::connection(&connection)
        .create(&table, |table| {
            table.string("id").primary();
            table.foreign_id("user_id").nullable().index();
            table.string_len("ip_address", 45).nullable();
            table.text("user_agent").nullable();
            table.long_text("payload");
            table.integer("last_activity").index();
        })
        .await
        .unwrap();
    (container, guard)
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
            "login" => {
                request.session().regenerate(false).await.unwrap();
                request.set_attribute("_auth_id", 7);
                Response::new("logged in")
            }
            "logout" => {
                request.session().invalidate().await.unwrap();
                request.set_attribute("_auth_id", Value::Null);
                Response::new("logged out")
            }
            _ => Response::make("Not Found", 404),
        }
    })
}

/// A browser behind a trusted proxy, so the client IP comes from `X-Forwarded-For`.
fn browser() -> Browser {
    let trust_proxies = middleware_fn(|request: Request, next: illuminate_http::Next| async move {
        request.set_trust_proxies(true);
        Ok(next.run(request).await)
    });
    Browser::new(web(vec![trust_proxies]), routes())
}

fn headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "user-agent",
        HeaderValue::from_static("Mozilla/5.0 (Laravel)"),
    );
    headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.4"));
    headers
}

async fn get(browser: &Browser, uri: &str) -> Response {
    browser.send(uri, "GET", json!({}), headers()).await
}

fn session_id(browser: &Browser, app: &Container) -> String {
    let encrypted = browser
        .cookie("laravel_session")
        .expect("the session cookie was set");
    let decrypted = app.make::<Encrypter>().decrypt_string(&encrypted).unwrap();
    illuminate_cookie::CookieValuePrefix::remove(&decrypted)
}

async fn sessions(connection: &str, table: &str) -> Vec<Value> {
    DB::connection(connection)
        .table(table)
        .get()
        .await
        .unwrap()
        .into_iter()
        .collect()
}

fn payload(row: &Value) -> Value {
    let bytes = BASE64.decode(row["payload"].as_str().unwrap()).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn sessions_round_trip_through_the_database() {
    let (app, _guard) = app(json!({})).await;
    let browser = browser();

    assert_eq!(get(&browser, "/put").await.content_string(), "stored");
    let id = session_id(&browser, &app);

    let rows = sessions("sqlite", "sessions").await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["id"], id);
    assert_eq!(row["ip_address"], "198.51.100.4");
    assert_eq!(row["user_agent"], "Mozilla/5.0 (Laravel)");
    assert_eq!(row["user_id"], Value::Null);
    assert_eq!(payload(row)["name"], "Taylor");
    assert!(payload(row)["_token"].is_string());

    // The next request reads the session back and updates the same row.
    let response = get(&browser, "/get").await;
    assert_eq!(response.json_body(), json!({"name": "Taylor"}));
    assert_eq!(session_id(&browser, &app), id);
    assert_eq!(sessions("sqlite", "sessions").await.len(), 1);

    // Flash data survives exactly one more request.
    browser.post("/flash", json!({})).await;
    let response = get(&browser, "/status").await;
    assert_eq!(response.json_body(), json!({"status": "Saved!"}));
    let response = get(&browser, "/status").await;
    assert_eq!(response.json_body(), json!({"status": null}));
}

#[tokio::test]
async fn the_authenticated_user_is_recorded() {
    let (app, _guard) = app(json!({})).await;
    let browser = browser();

    get(&browser, "/put").await;
    let guest = session_id(&browser, &app);

    // Logging in regenerates the session ID and records the user.
    get(&browser, "/login").await;
    let user = session_id(&browser, &app);
    assert_ne!(guest, user);
    let row = DB::table("sessions")
        .find(user.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row["user_id"], 7);
    assert_eq!(payload(&row)["name"], "Taylor");

    // Requests that don't touch authentication keep the user ID.
    get(&browser, "/get").await;
    let row = DB::table("sessions")
        .find(user.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row["user_id"], 7);

    // Logging out invalidates the session: its row is destroyed and a fresh
    // guest session takes its place.
    get(&browser, "/logout").await;
    let fresh = session_id(&browser, &app);
    assert!(
        DB::table("sessions")
            .find(user.as_str())
            .await
            .unwrap()
            .is_none()
    );
    let row = DB::table("sessions")
        .find(fresh.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row["user_id"], Value::Null);
    assert_eq!(payload(&row).get("name"), None);
}

#[tokio::test]
async fn expired_sessions_start_over() {
    let (app, _guard) = app(json!({"lifetime": 1})).await;
    let browser = browser();

    get(&browser, "/put").await;
    let id = session_id(&browser, &app);
    DB::table("sessions")
        .update(json!({"last_activity": illuminate_support::Carbon::now().timestamp() - 120}))
        .await
        .unwrap();

    let response = get(&browser, "/get").await;
    assert_eq!(response.json_body(), json!({"name": null}));
    // The expired row is reused (updated), not duplicated.
    assert_eq!(session_id(&browser, &app), id);
    assert_eq!(DB::table("sessions").count().await.unwrap(), 1);
}

#[tokio::test]
async fn the_lottery_collects_garbage() {
    let (_app, _guard) = app(json!({"lottery": [1, 1]})).await;
    DB::table("sessions")
        .insert(json!({
            "id": "stale",
            "payload": BASE64.encode("{}"),
            "last_activity": illuminate_support::Carbon::now().timestamp() - 7201,
        }))
        .await
        .unwrap();

    get(&browser(), "/put").await;
    let ids: Vec<Value> = DB::table("sessions")
        .pluck("id")
        .await
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(ids.len(), 1);
    assert_ne!(ids[0], "stale");
}

#[tokio::test]
async fn the_connection_and_table_are_configurable() {
    let (app, _guard) = app(json!({"connection": "secondary", "table": "web_sessions"})).await;
    let browser = browser();

    get(&browser, "/put").await;
    let rows = sessions("secondary", "web_sessions").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], session_id(&browser, &app));

    let handler = app.make::<SessionManager>().handler("database").unwrap();
    assert!(handler.needs_request());
}

#[tokio::test]
async fn encrypted_sessions_are_stored_encrypted() {
    let (app, _guard) = app(json!({"encrypt": true})).await;
    let browser = browser();

    get(&browser, "/put").await;
    let row = DB::table("sessions").first().await.unwrap().unwrap();
    let raw = String::from_utf8(BASE64.decode(row["payload"].as_str().unwrap()).unwrap()).unwrap();
    assert!(!raw.contains("Taylor"));
    let decrypted = app.make::<Encrypter>().decrypt_string(&raw).unwrap();
    assert!(decrypted.contains("Taylor"));

    let response = get(&browser, "/get").await;
    assert_eq!(response.json_body(), json!({"name": "Taylor"}));
}

#[tokio::test]
async fn the_manager_builds_a_handler_per_session() {
    let (app, _guard) = app(json!({})).await;
    let manager = app.make::<SessionManager>();
    let a = manager.driver().unwrap();
    let b = manager.driver().unwrap();
    assert!(!Arc::ptr_eq(&a.get_handler(), &b.get_handler()));
    assert!(a.handler_needs_request());

    a.start().await.unwrap();
    a.put("answer", 42);
    a.save().await.unwrap();

    let handler = DatabaseSessionHandler::new(DB::default_connection(), "sessions", 120);
    let data: Value = serde_json::from_str(&handler.read(&a.id()).await.unwrap()).unwrap();
    assert_eq!(data["answer"], 42);
}
