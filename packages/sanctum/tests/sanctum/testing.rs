//! `Sanctum::acting_as`: authenticating in tests with a token's abilities.

use illuminate_auth::{Auth, RequestAuthExt};
use illuminate_http::Request;
use illuminate_support::json;
use laravel_sanctum::{HasApiTokens, Sanctum};

use crate::support::{User, app, app_with, guest, taylor, user_route};

#[tokio::test]
async fn acting_as_authenticates_with_the_given_abilities() {
    let app = app().await;
    app.router()
        .get("/api/task", |request: Request| async move {
            let user: User = request.user().unwrap();
            format!(
                "{} view:{} delete:{}",
                user.name,
                user.token_can("view-tasks"),
                user.token_can("delete-tasks")
            )
        })
        .middleware("auth:sanctum");
    app.router()
        .get("/api/task/abilities", || async { "ok" })
        .middleware(("auth:sanctum", "abilities:view-tasks"));
    app.router()
        .get("/api/task/delete", || async { "ok" })
        .middleware(("auth:sanctum", "abilities:delete-tasks"));

    let user = Sanctum::acting_as(taylor().await, &["view-tasks"], None);
    assert_eq!(user.name, "Taylor");

    let response = guest(&app, "GET", "/api/task").await;
    assert_eq!(response.status_code(), 200);
    assert_eq!(response.content_string(), "Taylor view:true delete:false");

    assert_eq!(
        guest(&app, "GET", "/api/task/abilities")
            .await
            .status_code(),
        200
    );
    assert_eq!(
        guest(&app, "GET", "/api/task/delete").await.status_code(),
        403
    );
}

#[tokio::test]
async fn acting_as_may_grant_every_ability() {
    let app = app().await;
    user_route(&app);

    let user = Sanctum::acting_as(taylor().await, &["*"], None);

    let body = guest(&app, "GET", "/api/user").await.json_body();
    assert_eq!(body["name"], json!("Taylor"));
    assert_eq!(body["can_update"], json!(true));
    assert_eq!(body["transient"], json!(false));
    assert!(user.token_can("anything"));
    assert_eq!(
        Auth::get_default_driver(),
        "sanctum",
        "the guard becomes the default"
    );
}

#[tokio::test]
async fn acting_as_without_abilities_grants_nothing() {
    let app = app().await;
    user_route(&app);

    let user = Sanctum::acting_as(taylor().await, &[], None);

    let body = guest(&app, "GET", "/api/user").await.json_body();
    assert_eq!(body["can_update"], json!(false));
    assert!(user.token_cant("server:update"));
    assert!(user.current_access_token().is_some());
}

#[tokio::test]
async fn acting_as_may_use_another_sanctum_guard() {
    let app = app_with(json!({"auth": {"guards": {"api": {"driver": "sanctum"}}}})).await;
    app.router()
        .get("/api/me", |request: Request| async move {
            let user: User = request.user().unwrap();
            format!("{} {}", user.name, user.token_can("view-tasks"))
        })
        .middleware("auth:api");

    Sanctum::acting_as(taylor().await, &["view-tasks"], "api");

    let response = guest(&app, "GET", "/api/me").await;
    assert_eq!(response.content_string(), "Taylor true");
    assert_eq!(Auth::get_default_driver(), "api");

    Sanctum::forget_acting_as();
    Auth::forget_acting_as();
    assert_eq!(guest(&app, "GET", "/api/me").await.status_code(), 401);
}

#[tokio::test]
async fn tokens_may_be_attached_by_hand() {
    let _app = app().await;
    let user = taylor().await;
    let token = user.create_token("cli", &["deploy"]).await.unwrap();

    assert!(user.current_access_token().is_none());
    assert!(user.token_cant("deploy"));

    let user = user.with_access_token(token.access_token);
    assert!(user.token_can("deploy"));
    assert!(user.token_cant("rollback"));
    assert_eq!(
        user.current_access_token()
            .unwrap()
            .as_personal()
            .unwrap()
            .name,
        "cli"
    );
}
