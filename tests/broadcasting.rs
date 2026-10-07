//! Broadcasting, end to end: events broadcast from routes, the Pusher API,
//! and private channel authorization.

use laravel::prelude::*;
use laravel::testing::TestApp;

#[derive(Debug, Clone, Default, Model, Authenticatable)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
    pub password: String,
}

#[derive(Clone, Serialize)]
pub struct OrderShipmentStatusUpdated {
    pub order_id: u64,
    pub status: String,
}

impl ShouldBroadcast for OrderShipmentStatusUpdated {
    fn broadcast_on(&self) -> Vec<Channel> {
        vec![Channel::private(format!("orders.{}", self.order_id))]
    }

    fn should_broadcast_now(&self) -> bool {
        true
    }
}

fn app() -> TestApp {
    let dir = tempfile::tempdir().unwrap().keep();
    let app = TestApp::new(
        Application::configure_detached(&dir)
            .with_routing(|routing| {
                routing.web(|| {
                    Route::post("/orders/{id}/ship", |Path(id): Path<u64>| async move {
                        broadcast(OrderShipmentStatusUpdated { order_id: id, status: "shipped".into() })
                            .to_others()
                            .await?;
                        Ok::<_, Error>("Shipped")
                    });
                });
            })
            .with_broadcasting(|| {
                Broadcast::channel("orders.{order_id}", |user: User, order_id: u64| async move {
                    user.id == order_id
                });
            }),
    );
    app.app().override_config("broadcasting.default", "pusher");
    app.app().override_config(
        "broadcasting.connections.pusher",
        json!({
            "driver": "pusher",
            "key": "app-key",
            "secret": "app-secret",
            "app_id": "12345",
            "options": {"cluster": "eu", "host": "api-eu.pusher.com", "scheme": "https", "port": 443, "useTLS": true},
        }),
    );
    app
}

#[tokio::test]
async fn events_are_broadcast_to_pusher() {
    let mut app = app();
    Http::fake();

    app.with_header("X-Socket-ID", "1234.5678").post("/orders/7/ship", json!({})).await.assert_ok();

    Http::assert_sent(|request| {
        let body = request.data();
        request.url().starts_with("https://api-eu.pusher.com")
            && request.url().contains("/apps/12345/events?auth_key=app-key&")
            && request.url().contains("auth_signature=")
            && body["channels"] == json!(["private-orders.7"])
            && body["socket_id"] == "1234.5678"
            && body["data"].as_str().is_some_and(|data| data.contains("\"status\":\"shipped\""))
    });
}

#[tokio::test]
async fn broadcasts_can_be_faked() {
    let mut app = app();
    Broadcast::fake();

    app.post("/orders/7/ship", json!({})).await.assert_ok();

    Broadcast::assert_broadcast_on::<OrderShipmentStatusUpdated>("private-orders.7");
}

#[tokio::test]
async fn private_channels_are_authorized() {
    let mut app = app();
    let taylor = User { id: 7, name: "Taylor".into(), email: "taylor@laravel.com".into(), password: String::new() };
    app.acting_as(&taylor);

    let response = app
        .post("/broadcasting/auth", json!({"channel_name": "private-orders.7", "socket_id": "1234.5678"}))
        .await;
    response.assert_ok();
    let auth = response.json()["auth"].as_str().unwrap().to_string();
    assert!(auth.starts_with("app-key:"));

    app.post("/broadcasting/auth", json!({"channel_name": "private-orders.8", "socket_id": "1234.5678"}))
        .await
        .assert_forbidden();
}

#[tokio::test]
async fn channels_can_be_listed() {
    let app = app();

    app.artisan("channel:list")
        .expects_output_to_contain("orders.{order_id}")
        .expects_output_to_contain("Showing [1] private channels")
        .assert_successful()
        .await;
}
