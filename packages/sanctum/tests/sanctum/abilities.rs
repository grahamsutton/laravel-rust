//! The `abilities` and `ability` middleware.

use illuminate_support::json;
use std::sync::Arc;

use illuminate_http::{Destination, Middleware, Request, Response, run_middleware, with_request};
use laravel_sanctum::{CheckAbilities, CheckForAnyAbility, HasApiTokens, MissingAbilityException};

use illuminate_database::eloquent::Model;

use crate::support::{App, User, api, app, guest, taylor};

fn orders_routes(app: &App) {
    let router = app.router();
    router
        .get("/api/orders", || async { "orders" })
        .middleware(("auth:sanctum", "abilities:check-status,place-orders"));
    router
        .get("/api/orders/any", || async { "orders" })
        .middleware(("auth:sanctum", "ability:check-status,place-orders"));
}

/// A token for Taylor (created on first use) with the given abilities.
async fn token(abilities: &[&str]) -> String {
    let user = match User::first_where("email", "taylor@laravel.com")
        .await
        .unwrap()
    {
        Some(user) => user,
        None => taylor().await,
    };
    user.create_token("orders", abilities)
        .await
        .unwrap()
        .plain_text_token
}

#[tokio::test]
async fn the_abilities_middleware_requires_every_ability() {
    let app = app().await;
    orders_routes(&app);

    let both = token(&["check-status", "place-orders"]).await;
    let response = api(&app, "GET", "/api/orders", &both).await;
    assert_eq!(response.status_code(), 200);
    assert_eq!(response.content_string(), "orders");

    let one = api(&app, "GET", "/api/orders", &token(&["check-status"]).await).await;
    assert_eq!(one.status_code(), 403);
    assert_eq!(
        one.json_body(),
        json!({"message": "Invalid ability provided."})
    );

    let everything = api(&app, "GET", "/api/orders", &token(&["*"]).await).await;
    assert_eq!(everything.status_code(), 200);
}

#[tokio::test]
async fn the_ability_middleware_requires_any_ability() {
    let app = app().await;
    orders_routes(&app);

    let one = api(
        &app,
        "GET",
        "/api/orders/any",
        &token(&["place-orders"]).await,
    )
    .await;
    assert_eq!(one.status_code(), 200);

    let none = api(
        &app,
        "GET",
        "/api/orders/any",
        &token(&["cancel-orders"]).await,
    )
    .await;
    assert_eq!(none.status_code(), 403);
    assert_eq!(
        none.json_body(),
        json!({"message": "Invalid ability provided."})
    );
}

#[tokio::test]
async fn ability_middleware_requires_an_authenticated_token() {
    let app = app().await;
    app.router()
        .get("/api/abilities-only", || async { "secret" })
        .middleware("abilities:check-status");
    app.router()
        .get("/api/ability-only", || async { "secret" })
        .middleware("ability:check-status");

    assert_eq!(
        guest(&app, "GET", "/api/abilities-only")
            .await
            .status_code(),
        401
    );
    assert_eq!(
        guest(&app, "GET", "/api/ability-only").await.status_code(),
        401
    );
}

#[tokio::test]
async fn the_exception_names_the_missing_abilities() {
    let app = app().await;
    let token = token(&["check-status"]).await;
    let destination: Destination = Arc::new(|_request| Box::pin(async { Response::new("orders") }));

    let send = |middleware: Arc<dyn Middleware>| {
        let destination = destination.clone();
        let request = crate::support::Browser::new(&app).request(
            "GET",
            "/api/orders",
            &[("authorization", &format!("Bearer {token}"))],
            json!({}),
        );
        async move {
            let guard = illuminate_auth::middleware::Authenticate::using(&["sanctum"]);
            let stack: Vec<Arc<dyn Middleware>> = vec![Arc::new(guard), middleware];
            with_request(request.clone(), run_middleware(request, stack, destination)).await
        }
    };

    let response = send(Arc::new(CheckAbilities::new([
        "check-status",
        "place-orders",
    ])))
    .await;
    assert_eq!(response.status_code(), 403);
    let exception = response.exception().unwrap();
    let missing = exception.downcast_ref::<MissingAbilityException>().unwrap();
    assert_eq!(missing.abilities(), ["place-orders"]);

    let response = send(Arc::new(CheckForAnyAbility::new([
        "place-orders",
        "cancel-orders",
    ])))
    .await;
    let exception = response.exception().unwrap();
    let missing = exception.downcast_ref::<MissingAbilityException>().unwrap();
    assert_eq!(missing.abilities(), ["place-orders", "cancel-orders"]);

    let response = send(Arc::new(CheckForAnyAbility::new(["check-status"]))).await;
    assert_eq!(response.content_string(), "orders");
}

#[test]
fn the_middleware_are_built_from_route_parameters() {
    let abilities = CheckAbilities::factory();
    let _middleware = abilities(&["check-status".into(), " place-orders ".into(), "".into()]);
    let parsed = CheckAbilities::from_parameters(&["check-status".into(), " place-orders ".into()]);
    assert_eq!(parsed.abilities(), ["check-status", "place-orders"]);

    let any = CheckForAnyAbility::from_parameters(&["a".into(), "b".into()]);
    assert_eq!(any.abilities(), ["a", "b"]);
    assert_eq!(CheckForAnyAbility::ALIAS, "ability");
    assert_eq!(CheckAbilities::ALIAS, "abilities");
}

#[tokio::test]
async fn abilities_are_checked_against_the_request_user() {
    let app = app().await;
    app.router()
        .get("/api/servers", |request: Request| async move {
            use illuminate_auth::RequestAuthExt;
            let user: User = request.user().unwrap();
            format!(
                "update:{} delete:{}",
                user.token_can("server:update"),
                user.token_can("server:delete")
            )
        })
        .middleware(("auth:sanctum", CheckAbilities::using(&["server:update"])));

    let response = api(
        &app,
        "GET",
        "/api/servers",
        &token(&["server:update"]).await,
    )
    .await;

    assert_eq!(response.content_string(), "update:true delete:false");
}
