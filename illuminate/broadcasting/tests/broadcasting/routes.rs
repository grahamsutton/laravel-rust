//! The `/broadcasting/auth` and `/broadcasting/user-auth` endpoints,
//! through the router.

use illuminate_auth::AuthUser;
use illuminate_broadcasting::Broadcast;
use illuminate_http::{HeaderMap, HeaderValue, Request};
use illuminate_routing::{GroupAttributes, Route, RouteMiddleware};
use illuminate_support::json;

use crate::common::{KEY, SECRET, app, hmac_sha256};

fn define_web_group() {
    Route::middleware_group("web", Vec::<RouteMiddleware>::new());
}

fn post(uri: &str, form: &str, token: Option<&str>) -> Request {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("accept", HeaderValue::from_static("application/json"));
    if let Some(token) = token {
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
    }
    let input: serde_json::Value = serde_json::from_str(form).unwrap();
    Request::create_with(uri, "POST", input, headers)
}

#[tokio::test]
async fn the_auth_routes_are_registered_with_the_web_middleware() {
    let _app = app();
    Broadcast::routes(None);
    Broadcast::user_routes(None);

    let routes = Route::get_routes();
    assert_eq!(routes.len(), 2);

    let auth = &routes[0];
    assert_eq!(auth.uri(), "broadcasting/auth");
    assert!(auth.has_method("GET") && auth.has_method("POST"));
    assert_eq!(auth.middleware_names(), vec!["web"]);
    assert!(
        auth.excluded_middleware()
            .iter()
            .any(|middleware| middleware.name() == "csrf")
    );

    assert_eq!(routes[1].uri(), "broadcasting/user-auth");
}

#[tokio::test]
async fn channels_are_authorized_through_the_router() {
    let _app = app();
    define_web_group();
    Broadcast::routes(None);
    Broadcast::channel(
        "orders.{order_id}",
        |user: AuthUser, order_id: u64| async move { user.id() == json!(order_id) },
    );

    let response = Route::router()
        .dispatch(post(
            "/broadcasting/auth",
            r#"{"channel_name": "private-orders.1", "socket_id": "1234.1234"}"#,
            Some("taylor-token"),
        ))
        .await;
    assert_eq!(response.status_code(), 200);
    assert_eq!(
        response.json_body(),
        json!({"auth": format!("{KEY}:{}", hmac_sha256(SECRET, "1234.1234:private-orders.1"))})
    );

    let forbidden = Route::router()
        .dispatch(post(
            "/broadcasting/auth",
            r#"{"channel_name": "private-orders.2", "socket_id": "1234.1234"}"#,
            Some("taylor-token"),
        ))
        .await;
    assert_eq!(forbidden.status_code(), 403);

    let guest = Route::router()
        .dispatch(post(
            "/broadcasting/auth",
            r#"{"channel_name": "private-orders.1", "socket_id": "1234.1234"}"#,
            None,
        ))
        .await;
    assert_eq!(guest.status_code(), 403);
}

#[tokio::test]
async fn the_routes_accept_custom_attributes() {
    let _app = app();
    Broadcast::channel_routes(Some(GroupAttributes {
        prefix: Some("api".into()),
        ..GroupAttributes::default()
    }));
    Broadcast::channel("news", |_user: AuthUser| async { true });

    let route = &Route::get_routes()[0];
    assert_eq!(route.uri(), "api/broadcasting/auth");
    assert!(route.middleware_names().is_empty());

    let response = Route::router()
        .dispatch(post(
            "/api/broadcasting/auth",
            r#"{"channel_name": "private-news", "socket_id": "1.1"}"#,
            Some("abigail-token"),
        ))
        .await;
    assert_eq!(response.status_code(), 200);
}

#[tokio::test]
async fn users_are_authenticated_through_the_router() {
    let _app = app();
    define_web_group();
    Broadcast::user_routes(None);
    Broadcast::resolve_authenticated_user_using(|request: Request| async move {
        let token = request.bearer_token()?;
        (token == "taylor-token").then(|| json!({"id": "1"}))
    });

    let response = Route::router()
        .dispatch(post(
            "/broadcasting/user-auth",
            r#"{"socket_id": "1234.1234"}"#,
            Some("taylor-token"),
        ))
        .await;
    assert_eq!(response.status_code(), 200);
    assert_eq!(response.json_body()["user_data"], r#"{"id":"1"}"#);

    let guest = Route::router()
        .dispatch(post(
            "/broadcasting/user-auth",
            r#"{"socket_id": "1234.1234"}"#,
            None,
        ))
        .await;
    assert_eq!(guest.status_code(), 403);
}
