//! Channel authorization: `routes/channels.rs` callbacks and the responses
//! Pusher, Reverb and Ably expect.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use illuminate_auth::{AuthUser, GenericUser};
use illuminate_broadcasting::{Broadcast, FromChannelParameter, async_trait};
use illuminate_http::{HttpException, Response};
use illuminate_support::{Error, Result, Value, json};

use crate::common::{
    KEY, SECRET, app, app_with, auth_request, hmac_sha256, request_with, shared_secret,
};

fn status(result: Result<Response>) -> u16 {
    match result {
        Ok(response) => response.status_code(),
        Err(error) => error_status(&error),
    }
}

fn error_status(error: &Error) -> u16 {
    error
        .downcast_ref::<HttpException>()
        .map(HttpException::status_code)
        .unwrap_or(500)
}

fn owns_order() {
    Broadcast::channel(
        "orders.{order_id}",
        |user: AuthUser, order_id: u64| async move { user.id() == json!(order_id) },
    );
}

#[tokio::test]
async fn private_channels_are_signed_for_authorized_users() {
    let _app = app();
    owns_order();

    let response = Broadcast::auth(&auth_request("private-orders.1", Some("taylor-token")))
        .await
        .unwrap();

    assert_eq!(response.status_code(), 200);
    assert_eq!(
        response.json_body(),
        json!({"auth": format!("{KEY}:{}", hmac_sha256(SECRET, "1234.1234:private-orders.1"))})
    );
}

#[tokio::test]
async fn unauthorized_users_are_forbidden() {
    let _app = app();
    owns_order();

    let result = Broadcast::auth(&auth_request("private-orders.2", Some("taylor-token"))).await;
    assert_eq!(status(result), 403);

    // Parameters that don't fit the callback deny access too.
    let result = Broadcast::auth(&auth_request("private-orders.first", Some("taylor-token"))).await;
    assert_eq!(status(result), 403);
}

#[tokio::test]
async fn guests_never_reach_the_callback() {
    let _app = app();
    let called = Arc::new(AtomicBool::new(false));
    let flag = called.clone();
    Broadcast::channel(
        "orders.{order_id}",
        move |_user: AuthUser, _order_id: u64| {
            flag.store(true, Ordering::SeqCst);
            async { true }
        },
    );

    let result = Broadcast::auth(&auth_request("private-orders.1", None)).await;
    assert_eq!(status(result), 403);

    let result = Broadcast::auth(&auth_request("private-orders.1", Some("wrong-token"))).await;
    assert_eq!(status(result), 403);
    assert!(!called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn unknown_and_missing_channels_are_forbidden() {
    let _app = app();
    owns_order();

    let result = Broadcast::auth(&auth_request("private-invoices.1", Some("taylor-token"))).await;
    assert_eq!(status(result), 403);

    let result = Broadcast::auth(&auth_request("", Some("taylor-token"))).await;
    assert_eq!(status(result), 403);
}

#[tokio::test]
async fn presence_channels_return_the_users_data() {
    let _app = app();
    Broadcast::channel(
        "chat.{room}",
        |user: GenericUser, room: String| async move {
            (room == "lobby").then(|| json!({"id": user.get("id"), "name": user.get("name")}))
        },
    );

    let response = Broadcast::auth(&auth_request("presence-chat.lobby", Some("taylor-token")))
        .await
        .unwrap();
    let body = response.json_body();

    let channel_data = r#"{"user_id":"1","user_info":{"id":1,"name":"Taylor"}}"#;
    assert_eq!(body["channel_data"], channel_data);
    assert_eq!(
        body["auth"],
        format!(
            "{KEY}:{}",
            hmac_sha256(
                SECRET,
                &format!("1234.1234:presence-chat.lobby:{channel_data}")
            )
        )
    );

    // `None` (like PHP's `null`) is not an authorization.
    let result = Broadcast::auth(&auth_request("presence-chat.vip", Some("taylor-token"))).await;
    assert_eq!(status(result), 403);
}

#[tokio::test]
async fn encrypted_channels_share_their_secret_with_authorized_users() {
    let _app = app();
    owns_order();

    let response = Broadcast::auth(&auth_request(
        "private-encrypted-orders.1",
        Some("taylor-token"),
    ))
    .await
    .unwrap();
    let body = response.json_body();

    assert_eq!(
        body["auth"],
        format!(
            "{KEY}:{}",
            hmac_sha256(SECRET, "1234.1234:private-encrypted-orders.1")
        )
    );
    assert_eq!(
        body["shared_secret"],
        BASE64.encode(shared_secret("private-encrypted-orders.1"))
    );

    let result = Broadcast::auth(&auth_request(
        "private-encrypted-orders.2",
        Some("taylor-token"),
    ))
    .await;
    assert_eq!(status(result), 403);
}

#[tokio::test]
async fn false_denies_but_null_lets_the_next_channel_decide() {
    let _app = app();
    Broadcast::channel("teams.{team}", |_user: AuthUser, team: String| async move {
        (team == "laravel").then_some(true)
    });
    Broadcast::channel("teams.{anything}", |_user: AuthUser| async { true });
    Broadcast::channel("rooms.{room}", |_user: AuthUser, _room: String| async {
        false
    });
    Broadcast::channel("rooms.{anything}", |_user: AuthUser| async { true });

    let result = Broadcast::auth(&auth_request("private-teams.vapor", Some("taylor-token"))).await;
    assert_eq!(status(result), 200);

    let result = Broadcast::auth(&auth_request("private-rooms.1", Some("taylor-token"))).await;
    assert_eq!(status(result), 403);
}

#[tokio::test]
async fn channels_may_authenticate_with_other_guards() {
    let _app = app();
    Broadcast::channel("admin.{id}", |user: AuthUser, _id: u64| async move {
        user.id() == json!(99)
    })
    .guards(["admin"]);
    Broadcast::channel("staff.{id}", |user: AuthUser, _id: u64| async move {
        json!({"id": user.id()})
    })
    .guards(["admin", "api"]);

    let admin = request_with(
        json!({"channel_name": "private-admin.1", "socket_id": "1234.1234", "admin_token": "admin-token"}),
        &[],
    );
    assert_eq!(status(Broadcast::auth(&admin).await), 200);

    // The default guard is not consulted when guards are given.
    let user = auth_request("private-admin.1", Some("taylor-token"));
    assert_eq!(status(Broadcast::auth(&user).await), 403);

    // Guards are tried in order.
    let staff = auth_request("presence-staff.1", Some("abigail-token"));
    let response = Broadcast::auth(&staff).await.unwrap();
    assert_eq!(
        response.json_body()["channel_data"],
        r#"{"user_id":"2","user_info":{"id":2}}"#
    );
}

/// Channel model binding.
struct Order {
    user_id: u64,
}

#[async_trait]
impl FromChannelParameter for Order {
    async fn from_channel_parameter(value: String) -> Result<Option<Self>> {
        Ok(match value.as_str() {
            "1" => Some(Order { user_id: 1 }),
            "2" => Some(Order { user_id: 2 }),
            _ => None,
        })
    }
}

#[tokio::test]
async fn wildcards_may_be_bound_to_models() {
    let _app = app();
    Broadcast::channel(
        "orders.{order}",
        |user: AuthUser, order: Order| async move { user.id() == json!(order.user_id) },
    );

    assert_eq!(
        status(Broadcast::auth(&auth_request("private-orders.1", Some("taylor-token"))).await),
        200
    );
    assert_eq!(
        status(Broadcast::auth(&auth_request("private-orders.2", Some("taylor-token"))).await),
        403
    );
    // A missing model denies access.
    assert_eq!(
        status(Broadcast::auth(&auth_request("private-orders.3", Some("taylor-token"))).await),
        403
    );
}

#[tokio::test]
async fn channel_classes_are_plain_functions() {
    struct OrderChannel;

    impl OrderChannel {
        async fn join(user: GenericUser, order_id: u64, item: String) -> Result<bool> {
            Ok(user.get("id") == json!(order_id) && item == "book")
        }
    }

    let _app = app();
    Broadcast::channel("orders.{order}.items.{item}", OrderChannel::join);

    assert_eq!(
        Broadcast::get_channels(),
        vec![(
            "orders.{order}.items.{item}".to_string(),
            "OrderChannel@join".to_string()
        )]
    );
    assert_eq!(
        status(
            Broadcast::auth(&auth_request(
                "private-orders.1.items.book",
                Some("taylor-token")
            ))
            .await
        ),
        200
    );
    assert_eq!(
        status(
            Broadcast::auth(&auth_request(
                "private-orders.1.items.pen",
                Some("taylor-token")
            ))
            .await
        ),
        403
    );
}

#[tokio::test]
async fn callback_errors_are_returned() {
    let _app = app();
    Broadcast::channel("orders.{id}", |_user: AuthUser, _id: u64| async {
        Err::<bool, _>(illuminate_support::error::RuntimeException::new(
            "Database is down",
        ))
    });

    let error = Broadcast::auth(&auth_request("private-orders.1", Some("taylor-token")))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Database is down");
}

#[tokio::test]
async fn ably_signs_with_its_private_token() {
    let _app = app_with(|config| config["broadcasting"]["default"] = json!("ably"));
    owns_order();
    Broadcast::channel("chat.{room}", |user: AuthUser, _room: String| async move {
        json!({"id": user.id()})
    });

    let private = Broadcast::auth(&auth_request("private-orders.1", Some("taylor-token")))
        .await
        .unwrap();
    assert_eq!(
        private.json_body(),
        json!({"auth": format!("abcd.efgh:{}", hmac_sha256("ijkl", "1234.1234:private-orders.1"))})
    );

    let presence = Broadcast::auth(&auth_request("presence-chat.1", Some("taylor-token")))
        .await
        .unwrap()
        .json_body();
    let channel_data = r#"{"user_id":"1","user_info":{"id":1}}"#;
    assert_eq!(presence["channel_data"], channel_data);
    assert_eq!(
        presence["auth"],
        format!(
            "abcd.efgh:{}",
            hmac_sha256("ijkl", &format!("1234.1234:presence-chat.1:{channel_data}"))
        )
    );

    assert_eq!(
        status(Broadcast::auth(&auth_request("private-orders.2", Some("taylor-token"))).await),
        403
    );
    assert_eq!(
        status(Broadcast::auth(&auth_request("private-orders.1", None)).await),
        403
    );
}

#[tokio::test]
async fn the_log_and_null_drivers_answer_with_an_empty_response() {
    for driver in ["log", "null"] {
        let _app = app_with(|config| config["broadcasting"]["default"] = json!(driver));
        owns_order();

        let response = Broadcast::auth(&auth_request("private-orders.2", None))
            .await
            .unwrap();
        assert_eq!(response.status_code(), 200);
        assert_eq!(response.content_string(), "");
    }
}

#[tokio::test]
async fn jsonp_callbacks_are_supported_when_enabled() {
    let _app =
        app_with(|config| config["broadcasting"]["connections"]["pusher"]["jsonp"] = json!(true));
    owns_order();

    let request = request_with(
        json!({
            "channel_name": "private-orders.1", "socket_id": "1234.1234",
            "api_token": "taylor-token", "callback": "handleAuth",
        }),
        &[],
    );
    let response = Broadcast::auth(&request).await.unwrap();
    assert!(
        response
            .content_string()
            .starts_with("/**/handleAuth({\"auth\":")
    );
}

#[tokio::test]
async fn users_are_authenticated_for_pusher_user_authentication() {
    let _app = app();

    // Without a resolver, there's nobody to authenticate.
    let request = auth_request("", Some("taylor-token"));
    assert_eq!(
        Broadcast::resolve_authenticated_user(&request)
            .await
            .unwrap(),
        None
    );

    Broadcast::resolve_authenticated_user_using(|request: illuminate_http::Request| async move {
        (request.string("api_token") == "taylor-token")
            .then(|| json!({"id": "1", "user_info": {"name": "Taylor"}}))
    });

    let user = Broadcast::resolve_authenticated_user(&request)
        .await
        .unwrap()
        .unwrap();
    let user_data = r#"{"id":"1","user_info":{"name":"Taylor"}}"#;
    assert_eq!(user["user_data"], user_data);
    assert_eq!(
        user["auth"],
        format!(
            "{KEY}:{}",
            hmac_sha256(SECRET, &format!("1234.1234::user::{user_data}"))
        )
    );

    let guest = auth_request("", None);
    assert_eq!(
        Broadcast::resolve_authenticated_user(&guest).await.unwrap(),
        None::<Value>
    );
}
