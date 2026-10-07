//! The manager and the Ably, log and null drivers.

use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use illuminate_broadcasting::{
    Broadcast, BroadcastException, BroadcastManager, Broadcaster, Channel, NullBroadcaster,
    PresenceChannel, PrivateChannel, async_trait, broadcast,
};
use illuminate_container::Container;
use illuminate_http::Request;
use illuminate_http_client::Http;
use illuminate_support::{Map, Result, Value, json};

use crate::common::{app, app_with, record_logs, sent};
use crate::events::OrderShipmentStatusUpdated;

fn payload(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn ably_publishes_through_its_rest_api() {
    let _app = app();

    Broadcast::connection("ably")
        .unwrap()
        .broadcast(
            &[
                PrivateChannel::new("orders.1"),
                PresenceChannel::new("chat.1"),
                Channel::new("news"),
            ],
            "OrderShipped",
            payload(json!({"order_id": 1, "socket": "abc-123"})),
        )
        .await
        .unwrap();

    let requests = sent();
    let urls: Vec<&str> = requests.iter().map(|request| request.url()).collect();
    assert_eq!(
        urls,
        vec![
            "https://rest.ably.io/channels/private%3Aorders.1/messages",
            "https://rest.ably.io/channels/presence%3Achat.1/messages",
            "https://rest.ably.io/channels/public%3Anews/messages",
        ]
    );

    let request = &requests[0];
    assert_eq!(request.method(), "POST");
    assert_eq!(
        request.header("Authorization"),
        vec![format!("Basic {}", BASE64.encode("abcd.efgh:ijkl"))]
    );
    let body: Value = serde_json::from_str(&request.body()).unwrap();
    assert_eq!(
        body,
        json!({
            "name": "OrderShipped",
            "connectionKey": "abc-123",
            "data": r#"{"order_id":1,"socket":"abc-123"}"#,
            "encoding": "json",
        })
    );
}

#[tokio::test]
async fn ably_failures_become_broadcast_exceptions() {
    let _app = app_with(|_| {});
    Http::fake_urls([(
        "rest.ably.io/*",
        Http::response(
            json!({"error": {"message": "Unauthorized", "code": 40100}}),
            401,
            &[],
        ),
    )]);

    let error = Broadcast::connection("ably")
        .unwrap()
        .broadcast(&[Channel::new("news")], "Breaking", Map::new())
        .await
        .unwrap_err();
    assert!(error.is::<BroadcastException>());
    assert_eq!(error.to_string(), "Ably error: Unauthorized");
}

#[tokio::test]
async fn the_log_driver_writes_the_payload_to_the_log() {
    let _app = app_with(|config| config["broadcasting"]["default"] = json!("log"));
    let logs = record_logs();

    broadcast(OrderShipmentStatusUpdated::new(1)).await.unwrap();

    assert_eq!(
        *logs.lock().unwrap(),
        vec![
            "Broadcasting [Events\\OrderShipmentStatusUpdated] on channels [private-orders.1] with payload:\n{\n    \"order_id\": 1,\n    \"status\": \"shipped\",\n    \"socket\": null\n}"
                .to_string()
        ]
    );
}

#[tokio::test]
async fn the_null_driver_discards_everything() {
    let _app = app_with(|config| config["broadcasting"]["default"] = json!("null"));
    let logs = record_logs();

    broadcast(OrderShipmentStatusUpdated::new(1)).await.unwrap();

    assert!(logs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn broadcasting_defaults_to_the_null_connection() {
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container);

    assert_eq!(Broadcast::get_default_driver(), "null");
    Broadcast::driver()
        .unwrap()
        .broadcast(&[Channel::new("orders")], "OrderShipped", Map::new())
        .await
        .unwrap();
    assert!(Broadcast::connection("pusher").is_err());
}

#[tokio::test]
async fn unknown_connections_and_drivers_are_reported() {
    let _app = app_with(|config| {
        config["broadcasting"]["connections"]["mercure"] = json!({"driver": "mercure"});
    });

    let error = Broadcast::connection("missing").err().unwrap();
    assert_eq!(
        error.to_string(),
        "Broadcast connection [missing] is not defined."
    );

    let error = Broadcast::connection("mercure").err().unwrap();
    assert_eq!(error.to_string(), "Driver [mercure] is not supported.");
}

/// One recorded broadcast: the channels, the event name and the payload.
type Recorded = (Vec<String>, String, Map<String, Value>);

/// A broadcaster recording what it's asked to broadcast.
#[derive(Default)]
struct RecordingBroadcaster {
    broadcasts: Mutex<Vec<Recorded>>,
}

#[async_trait]
impl Broadcaster for RecordingBroadcaster {
    async fn auth(&self, request: &Request) -> Result<Value> {
        let channel = request.string("channel_name");
        Broadcast::channels()
            .verify_user_can_access_channel(request, &channel, self)
            .await
    }

    async fn valid_authentication_response(
        &self,
        _request: &Request,
        result: Value,
    ) -> Result<Value> {
        Ok(json!({"authorized": result}))
    }

    async fn broadcast(
        &self,
        channels: &[Channel],
        event: &str,
        payload: Map<String, Value>,
    ) -> Result<()> {
        self.broadcasts.lock().unwrap().push((
            channels
                .iter()
                .map(|channel| channel.name().to_string())
                .collect(),
            event.to_string(),
            payload,
        ));
        Ok(())
    }
}

#[tokio::test]
async fn custom_drivers_may_be_registered() {
    let _app = app_with(|config| {
        config["broadcasting"]["default"] = json!("custom");
        config["broadcasting"]["connections"]["custom"] =
            json!({"driver": "recording", "label": "x"});
    });
    let recorder = Arc::new(RecordingBroadcaster::default());
    let seen_config = Arc::new(Mutex::new(Value::Null));
    {
        let recorder = recorder.clone();
        let seen_config = seen_config.clone();
        Broadcast::extend("recording", move |config| {
            *seen_config.lock().unwrap() = config.clone();
            Ok(recorder.clone() as Arc<dyn Broadcaster>)
        });
    }

    broadcast(OrderShipmentStatusUpdated::new(4)).await.unwrap();

    assert_eq!(seen_config.lock().unwrap()["label"], "x");
    let broadcasts = recorder.broadcasts.lock().unwrap().clone();
    assert_eq!(broadcasts.len(), 1);
    assert_eq!(broadcasts[0].0, vec!["private-orders.4"]);
    assert_eq!(broadcasts[0].1, "Events\\OrderShipmentStatusUpdated");
    assert_eq!(broadcasts[0].2["order_id"], 4);

    // Custom drivers get channel authorization for free.
    Broadcast::channel(
        "orders.{id}",
        |_user: illuminate_auth::AuthUser, id: u64| async move { id == 4 },
    );
    let response = Broadcast::auth(&crate::common::auth_request(
        "orders.4",
        Some("taylor-token"),
    ))
    .await
    .unwrap();
    assert_eq!(response.json_body(), json!({"authorized": true}));
}

#[tokio::test]
async fn connections_are_cached_until_purged() {
    let _app = app_with(|config| {
        config["broadcasting"]["default"] = json!("custom");
        config["broadcasting"]["connections"]["custom"] = json!({"driver": "counting"});
    });
    let created = Arc::new(Mutex::new(0));
    {
        let created = created.clone();
        Broadcast::extend("counting", move |_| {
            *created.lock().unwrap() += 1;
            Ok(Arc::new(NullBroadcaster) as Arc<dyn Broadcaster>)
        });
    }

    Broadcast::driver().unwrap();
    Broadcast::connection("custom").unwrap();
    assert_eq!(*created.lock().unwrap(), 1);

    Broadcast::purge(None);
    Broadcast::driver().unwrap();
    assert_eq!(*created.lock().unwrap(), 2);

    Broadcast::manager().forget_drivers();
    Broadcast::driver().unwrap();
    assert_eq!(*created.lock().unwrap(), 3);

    Broadcast::set_default_driver("log");
    assert_eq!(Broadcast::get_default_driver(), "log");
    assert!(format!("{:?}", Broadcast::manager()).contains("\"log\""));
}

#[tokio::test]
async fn the_provider_registers_the_manager() {
    let app = app();
    assert!(app.container.bound::<BroadcastManager>());
    assert!(Arc::ptr_eq(
        &Broadcast::manager(),
        &app.container.make::<BroadcastManager>()
    ));
}
