//! The `sanctum` guard, through the router: `auth:sanctum` with bearer tokens.

use std::sync::Arc;

use illuminate_auth::{Auth, AuthUser, RequestAuthExt};
use illuminate_database::eloquent::*;
use illuminate_events::Event;
use illuminate_http::{Json, Request, Response};
use laravel_sanctum::{
    HasApiTokens, PersonalAccessToken, Sanctum, SanctumGuard, TokenAuthenticated,
};

use crate::support::{Admin, User, api, app, app_with, guest, taylor, travel_to, user_route};

#[tokio::test]
async fn requests_are_authenticated_with_bearer_tokens() {
    let app = app().await;
    user_route(&app);
    travel_to("2025-03-01 09:00:00");
    let user = taylor().await;
    let token = user
        .create_token("token-name", &["server:update"])
        .await
        .unwrap();

    travel_to("2025-03-01 10:30:00");
    let response = api(&app, "GET", "/api/user", &token.plain_text_token).await;

    assert_eq!(response.status_code(), 200);
    let body = response.json_body();
    assert_eq!(body["type"], json!("User"));
    assert_eq!(body["name"], json!("Taylor"));
    assert_eq!(body["token"], json!(token.access_token.id));
    assert_eq!(body["transient"], json!(false));
    assert_eq!(body["can_update"], json!(true));
    assert_eq!(body["auth_id"], json!(user.id));
    assert_eq!(
        body["stateful"],
        json!(null),
        "token requests are stateless"
    );

    let stored = PersonalAccessToken::find_or_fail(token.access_token.id)
        .await
        .unwrap();
    assert_eq!(
        stored.last_used_at.unwrap().to_date_time_string(),
        "2025-03-01 10:30:00"
    );
}

#[tokio::test]
async fn the_user_knows_its_current_access_token() {
    let app = app().await;
    app.router()
        .get("/api/tokens/current", |request: Request| async move {
            let user: User = request.user().unwrap();
            let token = user.current_access_token().unwrap();
            Json(json!({
                "name": token.as_personal().unwrap().name,
                "check_status": user.token_can("check-status"),
                "place_orders": user.token_can("place-orders"),
                "cant_place_orders": user.token_cant("place-orders"),
            }))
        })
        .middleware("auth:sanctum");
    let token = taylor()
        .await
        .create_token("orders", &["check-status"])
        .await
        .unwrap();

    let body = api(&app, "GET", "/api/tokens/current", &token.plain_text_token)
        .await
        .json_body();

    assert_eq!(
        body,
        json!({
            "name": "orders",
            "check_status": true,
            "place_orders": false,
            "cant_place_orders": true,
        })
    );
}

#[tokio::test]
async fn bare_tokens_authenticate_too() {
    let app = app().await;
    user_route(&app);
    let token = taylor().await.create_token("bare", &["*"]).await.unwrap();
    let secret = token.plain_text_token.split_once('|').unwrap().1;

    let response = api(&app, "GET", "/api/user", secret).await;

    assert_eq!(response.status_code(), 200);
    assert_eq!(response.json_body()["token"], json!(token.access_token.id));
}

#[tokio::test]
async fn guests_are_unauthenticated() {
    let app = app().await;
    user_route(&app);

    let response = guest(&app, "GET", "/api/user").await;

    assert_eq!(response.status_code(), 401);
    assert_eq!(response.json_body(), json!({"message": "Unauthenticated."}));
}

#[tokio::test]
async fn invalid_tokens_are_rejected() {
    let app = app().await;
    user_route(&app);
    let token = taylor()
        .await
        .create_token("token-name", &["*"])
        .await
        .unwrap();
    let (id, secret) = token.plain_text_token.split_once('|').unwrap();

    for invalid in [
        format!("{id}|wrong-secret"),
        format!("{}|{secret}", token.access_token.id + 1),
        format!("abc|{secret}"),
        format!("{id}|"),
        "not-a-token".to_string(),
    ] {
        let response = api(&app, "GET", "/api/user", &invalid).await;
        assert_eq!(response.status_code(), 401, "{invalid} should be rejected");
    }
}

#[tokio::test]
async fn revoked_tokens_are_rejected() {
    let app = app().await;
    user_route(&app);
    let user = taylor().await;
    let token = user.create_token("token-name", &["*"]).await.unwrap();
    assert_eq!(
        api(&app, "GET", "/api/user", &token.plain_text_token)
            .await
            .status_code(),
        200
    );

    user.tokens().delete().await.unwrap();

    assert_eq!(
        api(&app, "GET", "/api/user", &token.plain_text_token)
            .await
            .status_code(),
        401
    );
}

#[tokio::test]
async fn the_current_access_token_can_revoke_itself() {
    let app = app().await;
    app.router()
        .post("/api/logout", |request: Request| async move {
            let user: User = request.user().unwrap();
            user.current_access_token().unwrap().delete().await?;
            Ok::<_, illuminate_support::Error>(Response::no_content())
        })
        .middleware("auth:sanctum");
    user_route(&app);
    let user = taylor().await;
    let token = user.create_token("phone", &["*"]).await.unwrap();
    let other = user.create_token("laptop", &["*"]).await.unwrap();

    assert_eq!(
        api(&app, "POST", "/api/logout", &token.plain_text_token)
            .await
            .status_code(),
        204
    );

    assert_eq!(
        api(&app, "GET", "/api/user", &token.plain_text_token)
            .await
            .status_code(),
        401
    );
    assert_eq!(
        api(&app, "GET", "/api/user", &other.plain_text_token)
            .await
            .status_code(),
        200,
        "only the current token is revoked"
    );
}

#[tokio::test]
async fn tokens_past_their_expiry_are_rejected() {
    let app = app().await;
    user_route(&app);
    let now = travel_to("2025-01-01 12:00:00");
    let user = taylor().await;
    let token = user
        .create_token_with_expiry("temporary", &["*"], now.add_hours(1))
        .await
        .unwrap();

    travel_to("2025-01-01 12:59:59");
    assert_eq!(
        api(&app, "GET", "/api/user", &token.plain_text_token)
            .await
            .status_code(),
        200
    );

    travel_to("2025-01-01 13:00:01");
    assert_eq!(
        api(&app, "GET", "/api/user", &token.plain_text_token)
            .await
            .status_code(),
        401
    );
}

#[tokio::test]
async fn tokens_older_than_the_configured_expiration_are_rejected() {
    let app = app_with(json!({"sanctum": {"expiration": 60}})).await;
    user_route(&app);
    travel_to("2025-01-01 12:00:00");
    let token = taylor()
        .await
        .create_token("token-name", &["*"])
        .await
        .unwrap();

    travel_to("2025-01-01 12:59:00");
    assert_eq!(
        api(&app, "GET", "/api/user", &token.plain_text_token)
            .await
            .status_code(),
        200
    );

    travel_to("2025-01-01 13:00:01");
    assert_eq!(
        api(&app, "GET", "/api/user", &token.plain_text_token)
            .await
            .status_code(),
        401
    );
}

#[tokio::test]
async fn tokens_whose_owner_was_deleted_are_rejected() {
    let app = app().await;
    user_route(&app);
    let mut user = taylor().await;
    let token = user.create_token("token-name", &["*"]).await.unwrap();

    user.delete().await.unwrap();

    assert_eq!(
        api(&app, "GET", "/api/user", &token.plain_text_token)
            .await
            .status_code(),
        401
    );
}

#[tokio::test]
async fn any_tokenable_model_may_authenticate() {
    let app = app().await;
    user_route(&app);
    let admin = Admin::create(json!({"name": "Root"})).await.unwrap();
    let token = admin.create_token("console", &["*"]).await.unwrap();

    let body = api(&app, "GET", "/api/user", &token.plain_text_token)
        .await
        .json_body();

    assert_eq!(body["type"], json!("Admin"));
    assert_eq!(body["name"], json!("Root"));
}

#[tokio::test]
async fn a_guard_provider_restricts_tokens_to_its_model() {
    let app = app_with(json!({"auth": {"guards": {"sanctum": {"provider": "users"}}}})).await;
    user_route(&app);
    let user_token = taylor().await.create_token("user", &["*"]).await.unwrap();
    let admin = Admin::create(json!({"name": "Root"})).await.unwrap();
    let admin_token = admin.create_token("admin", &["*"]).await.unwrap();

    assert_eq!(
        api(&app, "GET", "/api/user", &user_token.plain_text_token)
            .await
            .status_code(),
        200
    );
    assert_eq!(
        api(&app, "GET", "/api/user", &admin_token.plain_text_token)
            .await
            .status_code(),
        401
    );

    let guard = Auth::guard("sanctum");
    let guard = guard.downcast_ref::<SanctumGuard>().unwrap();
    assert_eq!(guard.provider_name(), Some("users"));
    assert_eq!(
        app.config().get("auth.guards.sanctum.driver"),
        json!("sanctum"),
        "the application's guard config is merged over Sanctum's"
    );
}

#[tokio::test]
async fn unregistered_tokenable_types_are_resolved_through_the_user_providers() {
    let app = app().await;
    user_route(&app);
    let admin = Admin::create(json!({"name": "Root"})).await.unwrap();
    // A token issued elsewhere: this process never issued an `Admin` token,
    // but the `admins` user provider's model is `Admin`.
    let token = PersonalAccessToken::force_create(json!({
        "tokenable_type": "App\\Models\\Admin",
        "tokenable_id": admin.id,
        "name": "imported",
        "token": "5e884898da28047151d0e56f8dc6292773603d0d6aabbdd62a11ef721d1542d8",
        "abilities": ["*"],
    }))
    .await
    .unwrap();
    let plain = format!("{}|password", token.id);

    let body = api(&app, "GET", "/api/user", &plain).await.json_body();
    assert_eq!(body["type"], json!("Admin"));
    assert_eq!(body["name"], json!("Root"));
}

#[tokio::test]
async fn unknown_tokenable_types_are_resolved_by_the_application() {
    let app = app().await;
    user_route(&app);
    let admin = Admin::create(json!({"name": "Root"})).await.unwrap();
    // A token issued elsewhere, for a model no user provider knows.
    let token = PersonalAccessToken::force_create(json!({
        "tokenable_type": "Legacy\\Operator",
        "tokenable_id": admin.id,
        "name": "imported",
        "token": "5e884898da28047151d0e56f8dc6292773603d0d6aabbdd62a11ef721d1542d8",
        "abilities": ["*"],
    }))
    .await
    .unwrap();
    let plain = format!("{}|password", token.id);

    assert_eq!(
        api(&app, "GET", "/api/user", &plain).await.status_code(),
        401,
        "nobody knows how to find the token's owner"
    );

    Sanctum::resolve_tokenables_using(|tokenable_type, id| async move {
        if tokenable_type == "Legacy\\Operator" {
            Ok(Admin::find(id).await?.map(AuthUser::new))
        } else {
            Ok(None)
        }
    });

    let body = api(&app, "GET", "/api/user", &plain).await.json_body();
    assert_eq!(body["type"], json!("Admin"));
    assert_eq!(body["name"], json!("Root"));
}

#[tokio::test]
async fn the_token_may_be_read_from_anywhere() {
    let app = app().await;
    user_route(&app);
    let token = taylor()
        .await
        .create_token("token-name", &["*"])
        .await
        .unwrap();

    Sanctum::get_access_token_from_request_using(|request| request.header("x-api-key"));

    let request = crate::support::Browser::new(&app).request(
        "GET",
        "/api/user",
        &[
            ("x-api-key", token.plain_text_token.as_str()),
            ("accept", "application/json"),
        ],
        json!({}),
    );
    assert_eq!(app.router().dispatch(request).await.status_code(), 200);

    assert_eq!(
        api(&app, "GET", "/api/user", &token.plain_text_token)
            .await
            .status_code(),
        401,
        "the bearer token is no longer read"
    );
}

#[tokio::test]
async fn the_application_may_veto_tokens() {
    let app = app().await;
    user_route(&app);
    let user = taylor().await;
    let legacy = user.create_token("legacy", &["*"]).await.unwrap();
    let current = user.create_token("current", &["*"]).await.unwrap();

    Sanctum::authenticate_access_tokens_using(|token, is_valid| is_valid && token.name != "legacy");

    assert_eq!(
        api(&app, "GET", "/api/user", &legacy.plain_text_token)
            .await
            .status_code(),
        401
    );
    assert_eq!(
        api(&app, "GET", "/api/user", &current.plain_text_token)
            .await
            .status_code(),
        200
    );
}

#[tokio::test]
async fn the_callback_may_revive_expired_tokens() {
    let app = app().await;
    user_route(&app);
    let now = travel_to("2025-01-01 12:00:00");
    let token = taylor()
        .await
        .create_token_with_expiry("expired", &["*"], now.sub_day())
        .await
        .unwrap();

    Sanctum::authenticate_access_tokens_using(|_token, _is_valid| true);

    assert_eq!(
        api(&app, "GET", "/api/user", &token.plain_text_token)
            .await
            .status_code(),
        200
    );
}

#[tokio::test]
async fn an_event_is_dispatched_when_a_token_authenticates() {
    let app = app().await;
    user_route(&app);
    let token = taylor()
        .await
        .create_token("token-name", &["*"])
        .await
        .unwrap();
    Event::fake();

    api(&app, "GET", "/api/user", &token.plain_text_token).await;

    Event::assert_dispatched_with(|event: &TokenAuthenticated| event.token.name == "token-name");
    let events: Vec<Arc<TokenAuthenticated>> = Event::dispatched::<TokenAuthenticated>();
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn the_guard_resolves_users_once_per_request() {
    let app = app().await;
    app.router()
        .get("/api/twice", |_request: Request| async move {
            let first = Auth::guard("sanctum").user().await.unwrap();
            let second = Auth::guard("sanctum").user().await.unwrap();
            Json(json!([first.id(), second.id()]))
        })
        .middleware("auth:sanctum");
    let token = taylor()
        .await
        .create_token("token-name", &["*"])
        .await
        .unwrap();

    let response = api(&app, "GET", "/api/twice", &token.plain_text_token).await;

    assert_eq!(response.json_body(), json!([1, 1]));
}

#[tokio::test]
async fn the_guard_has_no_user_outside_of_a_request() {
    let _app = app().await;
    let guard = Auth::guard("sanctum");

    assert!(guard.user().await.is_none());
    assert!(!guard.validate(&json!({})).await.unwrap());
    assert!(
        guard.provider().is_none(),
        "no default user provider is configured"
    );

    let user = taylor().await;
    guard.set_user(AuthUser::new(user.clone()));
    assert_eq!(guard.user().await.unwrap().id(), json!(user.id));
    guard.forget_user();
    assert!(guard.user().await.is_none());
}
