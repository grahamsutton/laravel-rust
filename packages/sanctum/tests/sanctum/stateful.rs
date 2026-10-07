//! SPA authentication: stateful requests from the first-party frontend, the
//! CSRF cookie route, and session authentication.

use std::sync::Arc;

use illuminate_auth::{Auth, AuthUser};
use illuminate_database::eloquent::*;
use illuminate_http::{HeaderMap, HeaderValue, Json, Middleware, Request, Response, middleware_fn};
use illuminate_support::Error;
use laravel_sanctum::{
    CSRF_COOKIE_ROUTE, EnsureFrontendRequestsAreStateful, Sanctum, define_routes,
};

use crate::support::{App, Browser, User, app, app_with, taylor, user_route};

const SPA: &str = "http://localhost:3000/dashboard";

fn spa_routes(app: &App) {
    user_route(app);
    let router = app.router();
    router
        .post("/login", |request: Request| async move {
            let user = User::where_("email", request.input("email"))
                .first()
                .await?
                .unwrap();
            Auth::login(AuthUser::new(user), false).await?;
            Ok::<_, Error>(Response::no_content())
        })
        .middleware("web");
    router
        .post("/api/posts", |request: Request| async move {
            Json(json!({
                "created": true,
                "custom_csrf": request.attribute("custom_csrf"),
            }))
        })
        .middleware(("api", "auth:sanctum"));
}

/// Initialize CSRF protection and log in, the way an SPA does.
async fn log_in(browser: &mut Browser) {
    let response = browser
        .get("/sanctum/csrf-cookie", &[("referer", SPA)])
        .await;
    assert_eq!(response.status_code(), 204);
    let xsrf = browser
        .cookie("XSRF-TOKEN")
        .expect("the XSRF-TOKEN cookie is set");

    let response = browser
        .post(
            "/login",
            &[("referer", SPA), ("x-xsrf-token", &xsrf)],
            json!({"email": "taylor@laravel.com"}),
        )
        .await;
    assert_eq!(response.status_code(), 204, "{}", response.content_string());
}

#[tokio::test]
async fn the_csrf_cookie_route_starts_the_session() {
    let app = app().await;
    let mut browser = Browser::new(&app);

    let response = browser.get("/sanctum/csrf-cookie", &[]).await;

    assert_eq!(response.status_code(), 204);
    assert!(response.content().is_empty());
    let xsrf = response.get_cookie("XSRF-TOKEN").unwrap();
    assert!(!xsrf.http_only, "JavaScript must be able to read it");
    assert!(browser.cookie("laravel_session").is_some());
    assert!(app.router().has(CSRF_COOKIE_ROUTE));
}

#[tokio::test]
async fn the_csrf_cookie_route_honors_the_configured_prefix() {
    let app = app_with(json!({"sanctum": {"prefix": "auth"}})).await;
    let mut browser = Browser::new(&app);

    assert_eq!(
        browser.get("/auth/csrf-cookie", &[]).await.status_code(),
        204
    );
    assert_eq!(
        browser.get("/sanctum/csrf-cookie", &[]).await.status_code(),
        404
    );

    define_routes(&app.router());
    assert_eq!(
        app.router()
            .get_routes()
            .iter()
            .filter(|route| route.get_name().as_deref() == Some(CSRF_COOKIE_ROUTE))
            .count(),
        1,
        "routes are only defined once"
    );
}

#[tokio::test]
async fn the_csrf_cookie_route_may_be_disabled() {
    let app = app_with(json!({"sanctum": {"routes": false}})).await;
    let mut browser = Browser::new(&app);

    assert_eq!(
        browser.get("/sanctum/csrf-cookie", &[]).await.status_code(),
        404
    );
    assert!(!app.router().has(CSRF_COOKIE_ROUTE));
}

#[tokio::test]
async fn spa_requests_are_authenticated_by_the_session() {
    let app = app().await;
    spa_routes(&app);
    taylor().await;
    let mut browser = Browser::new(&app);
    log_in(&mut browser).await;

    let response = browser
        .get(
            "/api/user",
            &[("referer", SPA), ("accept", "application/json")],
        )
        .await;

    assert_eq!(response.status_code(), 200, "{}", response.content_string());
    let body = response.json_body();
    assert_eq!(body["name"], json!("Taylor"));
    assert_eq!(
        body["transient"],
        json!(true),
        "SPA requests carry a transient token"
    );
    assert_eq!(body["token"], json!(null));
    assert_eq!(
        body["can_update"],
        json!(true),
        "the transient token can do anything"
    );
    assert_eq!(body["stateful"], json!(true));
}

#[tokio::test]
async fn the_origin_header_identifies_the_frontend_too() {
    let app = app().await;
    spa_routes(&app);
    taylor().await;
    let mut browser = Browser::new(&app);
    log_in(&mut browser).await;

    let response = browser
        .get(
            "/api/user",
            &[
                ("origin", "http://localhost:3000"),
                ("accept", "application/json"),
            ],
        )
        .await;

    assert_eq!(response.status_code(), 200);
}

#[tokio::test]
async fn requests_from_elsewhere_are_stateless() {
    let app = app().await;
    spa_routes(&app);
    taylor().await;
    let mut browser = Browser::new(&app);
    log_in(&mut browser).await;

    // Same cookies, but no first-party referer: no session, no user.
    let response = browser
        .get("/api/user", &[("accept", "application/json")])
        .await;
    assert_eq!(response.status_code(), 401);

    let response = browser
        .get(
            "/api/user",
            &[
                ("referer", "https://evil.test/"),
                ("accept", "application/json"),
            ],
        )
        .await;
    assert_eq!(response.status_code(), 401);
}

#[tokio::test]
async fn spa_requests_must_carry_the_csrf_token() {
    let app = app().await;
    spa_routes(&app);
    taylor().await;
    let mut browser = Browser::new(&app);
    log_in(&mut browser).await;

    let response = browser
        .post(
            "/api/posts",
            &[("referer", SPA), ("accept", "application/json")],
            json!({}),
        )
        .await;
    assert_eq!(response.status_code(), 419);
    assert_eq!(
        response.json_body(),
        json!({"message": "CSRF token mismatch."})
    );

    let xsrf = browser.cookie("XSRF-TOKEN").unwrap();
    let response = browser
        .post(
            "/api/posts",
            &[
                ("referer", SPA),
                ("accept", "application/json"),
                ("x-xsrf-token", &xsrf),
            ],
            json!({}),
        )
        .await;
    assert_eq!(response.status_code(), 200);
    assert_eq!(response.json_body()["created"], json!(true));
}

#[tokio::test]
async fn csrf_validation_may_be_turned_off() {
    let app = app_with(json!({"sanctum": {"middleware": {"validate_csrf_token": false}}})).await;
    spa_routes(&app);
    taylor().await;
    let mut browser = Browser::new(&app);
    log_in(&mut browser).await;

    let response = browser
        .post(
            "/api/posts",
            &[("referer", SPA), ("accept", "application/json")],
            json!({}),
        )
        .await;

    assert_eq!(response.status_code(), 200);
}

#[tokio::test]
async fn stateful_middleware_may_be_replaced_by_route_middleware_aliases() {
    let app =
        app_with(json!({"sanctum": {"middleware": {"validate_csrf_token": "custom-csrf"}}})).await;
    spa_routes(&app);
    app.router().alias_middleware("custom-csrf", |_parameters| {
        middleware_fn(|request: Request, next| async move {
            request.set_attribute("custom_csrf", true);
            Ok(next.run(request).await)
        })
    });
    taylor().await;
    let mut browser = Browser::new(&app);
    log_in(&mut browser).await;

    let response = browser
        .post(
            "/api/posts",
            &[("referer", SPA), ("accept", "application/json")],
            json!({}),
        )
        .await;

    assert_eq!(response.status_code(), 200);
    assert_eq!(response.json_body()["custom_csrf"], json!(true));
}

#[tokio::test]
async fn unknown_middleware_aliases_are_reported() {
    let app = app_with(json!({"sanctum": {"middleware": {"encrypt_cookies": "missing"}}})).await;
    spa_routes(&app);
    let mut browser = Browser::new(&app);

    let response = browser
        .get(
            "/api/user",
            &[("referer", SPA), ("accept", "application/json")],
        )
        .await;

    assert_eq!(response.status_code(), 500);
    assert!(
        response
            .exception()
            .unwrap()
            .to_string()
            .contains("missing")
    );
}

#[tokio::test]
async fn stateful_middleware_may_be_given_instances() {
    let app = app().await;
    taylor().await;
    let marker: Arc<dyn Middleware> = middleware_fn(|request: Request, next| async move {
        request.set_attribute("custom_csrf", "instance");
        Ok(next.run(request).await)
    });
    app.router()
        .post("/api/instances", |request: Request| async move {
            Json(json!({"custom_csrf": request.attribute("custom_csrf")}))
        })
        .middleware(illuminate_routing::RouteMiddleware::instance(
            "stateful",
            Arc::new(EnsureFrontendRequestsAreStateful::new().validate_csrf_token_using(marker)),
        ));
    let mut browser = Browser::new(&app);

    let response = browser
        .post("/api/instances", &[("referer", SPA)], json!({}))
        .await;

    assert_eq!(response.json_body()["custom_csrf"], json!("instance"));
}

#[tokio::test]
async fn stateful_sessions_use_secure_cookies() {
    let app = app().await;
    spa_routes(&app);
    app.config().set("session.http_only", false);
    app.config().set("session.same_site", "none");
    let mut browser = Browser::new(&app);

    browser.get("/api/user", &[("referer", SPA)]).await;

    assert_eq!(app.config().get("session.http_only"), json!(true));
    assert_eq!(app.config().get("session.same_site"), json!("lax"));
}

#[tokio::test]
async fn changing_the_password_elsewhere_logs_the_spa_out() {
    let app = app().await;
    spa_routes(&app);
    let mut user = taylor().await;
    let mut browser = Browser::new(&app);
    log_in(&mut browser).await;
    let headers = [("referer", SPA), ("accept", "application/json")];
    assert_eq!(browser.get("/api/user", &headers).await.status_code(), 200);

    user.password = "a-new-password-hash".into();
    user.save().await.unwrap();

    let response = browser.get("/api/user", &headers).await;
    assert_eq!(response.status_code(), 401);
    assert_eq!(browser.get("/api/user", &headers).await.status_code(), 401);
}

#[tokio::test]
async fn session_authentication_may_be_turned_off() {
    let app = app_with(json!({"sanctum": {"middleware": {"authenticate_session": false}}})).await;
    spa_routes(&app);
    let mut user = taylor().await;
    let mut browser = Browser::new(&app);
    log_in(&mut browser).await;

    user.password = "a-new-password-hash".into();
    user.save().await.unwrap();

    let response = browser
        .get(
            "/api/user",
            &[("referer", SPA), ("accept", "application/json")],
        )
        .await;
    assert_eq!(response.status_code(), 200);
}

fn request_from(uri: &str, header: &str, value: &str) -> Request {
    let mut headers = HeaderMap::new();
    headers.insert(
        illuminate_http::HeaderName::from_bytes(header.as_bytes()).unwrap(),
        HeaderValue::from_str(value).unwrap(),
    );
    Request::create_with(uri, "GET", json!({}), headers)
}

#[tokio::test]
async fn the_frontend_is_recognized_by_its_stateful_domain() {
    let _app = app_with(json!({"app": {"url": "https://laravel.test:8443"}})).await;
    let from = |header: &str, value: &str| {
        EnsureFrontendRequestsAreStateful::from_frontend(&request_from("/api/user", header, value))
    };

    assert!(from("referer", "http://localhost/"));
    assert!(from("referer", "http://localhost"));
    assert!(from("referer", "http://localhost:3000/some/page?x=1"));
    assert!(from("referer", "https://127.0.0.1:8000/"));
    assert!(from("referer", "https://laravel.test:8443/dashboard"));
    assert!(from("origin", "http://localhost:3000"));

    assert!(!from("referer", "http://localhost:4000/"));
    assert!(!from("referer", "https://laravel.test/"));
    assert!(!from("referer", "https://localhost.evil.test/"));
    assert!(!from("referer", "https://evil.test/localhost/"));
    assert!(!EnsureFrontendRequestsAreStateful::from_frontend(
        &Request::create("/", "GET")
    ));
}

#[tokio::test]
async fn the_current_request_host_may_be_stateful() {
    let stateful = format!("spa.test{}", Sanctum::current_request_host());
    let _app = app_with(json!({"sanctum": {"stateful": stateful}})).await;

    assert!(EnsureFrontendRequestsAreStateful::from_frontend(
        &request_from("http://api.test/user", "referer", "https://api.test/app")
    ));
    assert!(EnsureFrontendRequestsAreStateful::from_frontend(
        &request_from("http://api.test/user", "referer", "https://spa.test/")
    ));
    assert!(!EnsureFrontendRequestsAreStateful::from_frontend(
        &request_from("http://api.test/user", "referer", "https://other.test/")
    ));
}
