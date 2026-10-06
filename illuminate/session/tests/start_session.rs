mod common;

use std::sync::Arc;

use common::{Browser, app, array_session, destination, web};
use illuminate_cookie::EncryptCookies;
use illuminate_encryption::Encrypter;
use illuminate_http::{
    HeaderMap, HeaderValue, HttpResponseException, Request, Response, middleware_fn,
};
use illuminate_session::{
    ArraySessionHandler, RequestSessionExt, Session, SessionHandler, SessionManager, Store,
    csrf_token, old, redirect_guest, redirect_intended, session_get, session_put, view_errors,
};
use illuminate_support::{MessageBag, Value, json};

/// A small application: enough routes to exercise the session.
fn routes() -> illuminate_http::Destination {
    destination(|request: Request| async move {
        match (request.method().as_str(), request.path().as_str()) {
            ("GET", "put") => {
                request.session().put("name", "Taylor");
                Response::new("stored")
            }
            ("GET", "get") => Response::json(&json!({
                "name": request.session().get("name"),
                "status": request.session().get("status"),
            })),
            ("POST", "status") => Response::redirect("/get").with("status", "Profile updated!"),
            ("POST", "invalid") => Response::redirect("/form")
                .with_input(request.all())
                .with_errors(MessageBag::from([(
                    "email",
                    "The email field is required.",
                )])),
            ("POST", "invalid-login") => Response::redirect("/form")
                .with_errors_in(MessageBag::from([("password", "Wrong password.")]), "login"),
            ("GET", "form") => Response::json(&json!({
                "errors": view_errors(&request),
                "old_name": old("name", ""),
                "request_old": request.old("name"),
            })),
            ("GET", "previous") => Response::json(&json!({
                "attribute": request.attribute("_previous_url"),
                "session": request.session().previous_url(),
            })),
            ("GET", "helpers") => {
                session_put("via", "helper");
                Session::increment("visits");
                Response::json(&json!({
                    "via": session_get("via"),
                    "visits": Session::get("visits"),
                    "token": csrf_token(),
                }))
            }
            ("GET", "regenerate") => {
                request.session().regenerate(true).await.unwrap();
                Response::new("regenerated")
            }
            ("GET", "dashboard") => redirect_guest("/login"),
            ("POST", "login") => redirect_intended("/home"),
            _ => Response::make("Not Found", 404),
        }
    })
}

fn browser() -> Browser {
    Browser::new(web(vec![]), routes())
}

fn session_id(browser: &Browser, app: &illuminate_container::Container) -> String {
    let encrypted = browser
        .cookie("laravel_session")
        .expect("the session cookie was set");
    let decrypted = app.make::<Encrypter>().decrypt_string(&encrypted).unwrap();
    illuminate_cookie::CookieValuePrefix::remove(&decrypted)
}

fn array_handler(app: &illuminate_container::Container) -> Arc<dyn SessionHandler> {
    app.make::<SessionManager>().handler("array").unwrap()
}

#[tokio::test]
async fn the_session_cookie_is_attached_to_the_response() {
    let (app, _guard) = app(array_session());
    let browser = browser();

    let response = browser.get("/put").await;

    let cookie = response.get_cookie("laravel_session").unwrap();
    assert_eq!(cookie.minutes, Some(120));
    assert_eq!(cookie.path, "/");
    assert!(cookie.http_only);
    assert!(!cookie.secure);
    assert_eq!(cookie.same_site, Some(illuminate_http::SameSite::Lax));
    assert!(Store::is_valid_id(&session_id(&browser, &app)));
}

#[tokio::test]
async fn session_data_survives_between_requests() {
    let (app, _guard) = app(array_session());
    let browser = browser();

    browser.get("/put").await;
    let response = browser.get("/get").await;
    assert_eq!(response.json_body()["name"], json!("Taylor"));

    // The data lives in the handler under the session ID from the cookie.
    let stored = array_handler(&app)
        .read(&session_id(&browser, &app))
        .await
        .unwrap();
    assert!(stored.contains("Taylor"));

    // A different browser gets a different session.
    let stranger = self::browser();
    assert_eq!(stranger.get("/get").await.json_body()["name"], json!(null));
}

#[tokio::test]
async fn flashed_data_lives_for_exactly_one_more_request() {
    let (_app, _guard) = app(array_session());
    let browser = browser();

    let response = browser.post("/status", json!({})).await;
    assert!(response.is_redirect());

    assert_eq!(
        browser.get("/get").await.json_body()["status"],
        json!("Profile updated!")
    );
    assert_eq!(browser.get("/get").await.json_body()["status"], json!(null));
}

#[tokio::test]
async fn errors_and_input_are_flashed_for_the_next_request() {
    let (_app, _guard) = app(array_session());
    let browser = browser();

    browser
        .post("/invalid", json!({"name": "Taylor", "email": ""}))
        .await;
    let body = browser.get("/form").await.json_body();
    assert_eq!(
        body["errors"],
        json!({"default": {"email": ["The email field is required."]}})
    );
    assert_eq!(body["old_name"], json!("Taylor"));
    assert_eq!(body["request_old"], json!("Taylor"));

    // Named bags merge with the bags already flashed.
    browser.post("/invalid", json!({"name": "Taylor"})).await;
    browser.post("/invalid-login", json!({})).await;
    let body = browser.get("/form").await.json_body();
    assert_eq!(
        body["errors"],
        json!({
            "default": {"email": ["The email field is required."]},
            "login": {"password": ["Wrong password."]},
        })
    );

    // ...and everything is gone a request later.
    let body = browser.get("/form").await.json_body();
    assert_eq!(body["errors"], json!({}));
    assert_eq!(body["old_name"], json!(""));
}

#[tokio::test]
async fn flash_data_from_rendered_errors_is_applied() {
    let (_app, _guard) = app(array_session());
    // Validation fails deep inside the pipeline; the exception is rendered into a redirect.
    let validate = middleware_fn(|request: Request, next: illuminate_http::Next| async move {
        if request.is_method("POST") {
            let redirect = Response::redirect("/form")
                .with_input(json!({"name": "Abigail"}))
                .with_errors(MessageBag::from([("name", "The name is taken.")]));
            return Err(HttpResponseException::new(redirect).into());
        }
        Ok(next.run(request).await)
    });
    let browser = Browser::new(web(vec![validate]), routes());

    let response = browser.post("/anything", json!({})).await;
    assert!(response.is_redirect());

    let body = browser.get("/form").await.json_body();
    assert_eq!(
        body["errors"]["default"]["name"],
        json!(["The name is taken."])
    );
    assert_eq!(body["old_name"], json!("Abigail"));
}

#[tokio::test]
async fn the_previous_url_is_remembered_for_get_requests() {
    let (_app, _guard) = app(array_session());
    let browser = browser();

    browser.get("/put?tab=1").await;
    let body = browser.get("/previous").await.json_body();
    assert_eq!(body["attribute"], json!("http://localhost/put?tab=1"));
    assert_eq!(body["session"], json!("http://localhost/put?tab=1"));

    // POST, AJAX, prefetch, and unmatched (404) requests are not remembered.
    browser.post("/status", json!({})).await;
    let mut ajax = HeaderMap::new();
    ajax.insert(
        "x-requested-with",
        HeaderValue::from_static("XMLHttpRequest"),
    );
    browser.send("/get", "GET", json!({}), ajax).await;
    let mut prefetch = HeaderMap::new();
    prefetch.insert("purpose", HeaderValue::from_static("prefetch"));
    browser.send("/get", "GET", json!({}), prefetch).await;
    browser.get("/favicon.ico").await;

    let body = browser.get("/get").await;
    assert!(body.is_ok());
    let body = browser.get("/previous").await.json_body();
    assert_eq!(body["attribute"], json!("http://localhost/get"));
}

#[tokio::test]
async fn helpers_and_the_facade_use_the_current_session() {
    let (_app, _guard) = app(array_session());
    let browser = browser();

    let first = browser.get("/helpers").await.json_body();
    assert_eq!(first["via"], json!("helper"));
    assert_eq!(first["visits"], json!(1));
    assert_eq!(first["token"].as_str().unwrap().len(), 40);

    let second = browser.get("/helpers").await.json_body();
    assert_eq!(second["visits"], json!(2));
    assert_eq!(second["token"], first["token"]);
}

#[tokio::test]
async fn regenerating_issues_a_new_cookie_and_destroys_the_old_session() {
    let (app, _guard) = app(array_session());
    let browser = browser();

    browser.get("/put").await;
    let old_id = session_id(&browser, &app);
    browser.get("/regenerate").await;
    let new_id = session_id(&browser, &app);

    assert_ne!(old_id, new_id);
    assert_eq!(array_handler(&app).read(&old_id).await.unwrap(), "");
    assert_eq!(
        browser.get("/get").await.json_body()["name"],
        json!("Taylor")
    );
}

#[tokio::test]
async fn guests_are_sent_back_to_the_intended_url() {
    let (_app, _guard) = app(array_session());
    let browser = browser();

    let response = browser.get("/dashboard?tab=billing").await;
    assert_eq!(
        response.target_url().as_deref(),
        Some("http://localhost/login")
    );

    let response = browser.post("/login", json!({})).await;
    assert_eq!(
        response.target_url().as_deref(),
        Some("http://localhost/dashboard?tab=billing")
    );

    // The intended URL is pulled from the session; next time we fall back to the default.
    let response = browser.post("/login", json!({})).await;
    assert_eq!(
        response.target_url().as_deref(),
        Some("http://localhost/home")
    );
}

#[tokio::test]
async fn sessions_are_stored_in_files() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = array_session();
    config["driver"] = json!("file");
    config["files"] = json!(directory.path().to_string_lossy());
    let (app, _guard) = app(config);
    let browser = browser();

    browser.get("/put").await;
    let id = session_id(&browser, &app);
    let contents = std::fs::read_to_string(directory.path().join(&id)).unwrap();
    let stored: Value = serde_json::from_str(&contents).unwrap();
    assert_eq!(stored["name"], json!("Taylor"));
    assert_eq!(stored["_token"].as_str().unwrap().len(), 40);

    assert_eq!(
        browser.get("/get").await.json_body()["name"],
        json!("Taylor")
    );
}

#[tokio::test]
async fn sessions_can_be_stored_in_encrypted_cookies() {
    let mut config = array_session();
    config["driver"] = json!("cookie");
    let (app, _guard) = app(config);
    let browser = browser();

    browser.get("/put").await;
    let id = session_id(&browser, &app);

    // The whole payload travels in a cookie named after the session ID, encrypted.
    let payload = browser.cookie(&id).expect("the payload cookie was set");
    assert!(!payload.contains("Taylor"));
    assert!(Encrypter::appears_encrypted(&payload));

    assert_eq!(
        browser.get("/get").await.json_body()["name"],
        json!("Taylor")
    );
}

#[tokio::test]
async fn session_payloads_can_be_encrypted() {
    let mut config = array_session();
    config["encrypt"] = json!(true);
    let (app, _guard) = app(config);
    let browser = browser();

    browser.get("/put").await;
    let raw = array_handler(&app)
        .read(&session_id(&browser, &app))
        .await
        .unwrap();
    assert!(!raw.contains("Taylor"));
    assert!(
        app.make::<Encrypter>()
            .decrypt_string(&raw)
            .unwrap()
            .contains("Taylor")
    );
    assert_eq!(
        browser.get("/get").await.json_body()["name"],
        json!("Taylor")
    );
}

#[tokio::test]
async fn sessions_can_expire_when_the_browser_closes() {
    let mut config = array_session();
    config["expire_on_close"] = json!(true);
    config["secure"] = json!(true);
    config["domain"] = json!("laravel.com");
    config["same_site"] = json!("strict");
    config["partitioned"] = json!(true);
    let (_app, _guard) = app(config);

    let response = browser().get("/put").await;

    let cookie = response.get_cookie("laravel_session").unwrap();
    assert_eq!(cookie.minutes, None);
    assert!(cookie.secure);
    assert!(cookie.partitioned);
    assert_eq!(cookie.domain.as_deref(), Some("laravel.com"));
    assert_eq!(cookie.same_site, Some(illuminate_http::SameSite::Strict));
}

#[tokio::test]
async fn without_a_driver_there_is_no_session() {
    let mut config = array_session();
    config["driver"] = json!(null);
    let (_app, _guard) = app(config);
    let browser = Browser::new(
        web(vec![]),
        destination(
            |request: Request| async move { Response::new(request.has_session().to_string()) },
        ),
    );

    let response = browser.get("/").await;

    assert_eq!(response.content_string(), "false");
    assert!(response.get_cookie("laravel_session").is_none());
}

#[tokio::test]
async fn the_lottery_collects_garbage() {
    for (lottery, collected) in [([1, 1], true), ([0, 100], false)] {
        let mut config = array_session();
        config["lottery"] = json!(lottery);
        config["lifetime"] = json!(1);
        let (app, _guard) = app(config);

        let handler = Arc::new(ArraySessionHandler::new(1));
        let shared = handler.clone();
        app.make::<SessionManager>()
            .extend("array", move |_| shared.clone());
        handler.write("expired", "{}").await.unwrap();
        handler.travel("expired", 3600);

        browser().get("/put").await;

        assert_eq!(!handler.has("expired"), collected, "lottery {lottery:?}");
        assert_eq!(handler.count(), if collected { 1 } else { 2 });
    }
}

#[tokio::test]
async fn cookies_from_laravel_can_carry_the_session_id() {
    // An application sharing the key (and session store) with a PHP app accepts its cookie.
    let (app, _guard) = app(array_session());
    let id = "a".repeat(40);
    let handler = array_handler(&app);
    handler
        .write(
            &id,
            r#"{"_token":"0123456789012345678901234567890123456789","name":"From PHP"}"#,
        )
        .await
        .unwrap();

    let encrypter = app.make::<Encrypter>();
    let prefix =
        illuminate_cookie::CookieValuePrefix::create("laravel_session", encrypter.get_key());
    let cookie = encrypter.encrypt_string(&format!("{prefix}{id}")).unwrap();

    let browser = browser();
    browser
        .cookies
        .lock()
        .unwrap()
        .push(("laravel_session".into(), cookie));
    assert_eq!(
        browser.get("/get").await.json_body()["name"],
        json!("From PHP")
    );
    assert_eq!(session_id(&browser, &app), id);
}

#[tokio::test]
async fn invalid_session_ids_get_a_fresh_session() {
    let (app, _guard) = app(array_session());
    // EncryptCookies is skipped for the session cookie so a forged ID reaches the session.
    let middleware: Vec<Arc<dyn illuminate_http::Middleware>> = vec![
        Arc::new(EncryptCookies::new().except(["laravel_session"])),
        Arc::new(illuminate_session::StartSession::new()),
    ];
    let browser = Browser::new(middleware, routes());
    browser
        .cookies
        .lock()
        .unwrap()
        .push(("laravel_session".into(), "../../etc/passwd".into()));

    browser.get("/put").await;

    let id = browser.cookie("laravel_session").unwrap();
    assert!(Store::is_valid_id(&id));
    assert!(
        array_handler(&app)
            .read(&id)
            .await
            .unwrap()
            .contains("Taylor")
    );
}
