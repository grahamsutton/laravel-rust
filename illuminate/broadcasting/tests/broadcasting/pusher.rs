//! The Pusher and Reverb drivers, against a faked Pusher HTTP API.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use illuminate_broadcasting::{
    Broadcast, BroadcastException, Channel, EncryptedPrivateChannel, PrivateChannel, secretbox,
};
use illuminate_http_client::Http;
use illuminate_support::{Map, Value, json};

use crate::common::{KEY, SECRET, app, app_with, sent, shared_secret, verify_pusher_call};

fn payload(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn events_are_triggered_through_the_pusher_http_api() {
    let _app = app();

    Broadcast::connection("pusher")
        .unwrap()
        .broadcast(
            &[PrivateChannel::new("orders.1"), Channel::new("orders")],
            "OrderShipmentStatusUpdated",
            payload(json!({"order_id": 1, "status": "shipped", "socket": null})),
        )
        .await
        .unwrap();

    let requests = sent();
    assert_eq!(requests.len(), 1);

    let call = verify_pusher_call(&requests[0], KEY, SECRET);
    assert_eq!(
        call.endpoint,
        "https://api-mt1.pusher.com:443/apps/3/events"
    );
    assert_eq!(
        call.body,
        json!({
            "name": "OrderShipmentStatusUpdated",
            "data": r#"{"order_id":1,"status":"shipped"}"#,
            "channels": ["private-orders.1", "orders"],
        })
    );
    assert_eq!(call.data(), json!({"order_id": 1, "status": "shipped"}));
}

#[tokio::test]
async fn the_signature_matches_the_pusher_documentation() {
    let _app = app();

    // Pusher's worked example: the same body must yield the documented
    // `body_md5` and `auth_signature`.
    let pusher = Broadcast::pusher(&json!({"key": KEY, "secret": SECRET, "app_id": "3"})).unwrap();
    let mut query = indexmap::IndexMap::new();
    query.insert(
        "body_md5".to_string(),
        hex::encode({
            use md5::{Digest, Md5};
            Md5::digest(r#"{"name":"foo","channels":["project-3"],"data":"{\"some\":\"data\"}"}"#)
        }),
    );
    assert_eq!(query["body_md5"], "ec365a775a4cd0599faeb73354201b6f");

    let settings = pusher.settings();
    let params = illuminate_broadcasting::Pusher::build_auth_query_params(
        &settings.auth_key,
        &settings.secret,
        "POST",
        "/apps/3/events",
        query,
        "1.0",
        crate::common::TIMESTAMP,
    );
    assert_eq!(
        params["auth_signature"],
        "da454824c97ba181a32ccc17a72625ba02771f50b50e1e7430e47a1f3f457e6c"
    );
}

#[tokio::test]
async fn the_current_users_socket_is_excluded() {
    let _app = app();

    Broadcast::connection("pusher")
        .unwrap()
        .broadcast(
            &[Channel::new("orders")],
            "OrderShipped",
            payload(json!({"id": 1, "socket": "1234.5678"})),
        )
        .await
        .unwrap();

    let call = verify_pusher_call(&sent()[0], KEY, SECRET);
    assert_eq!(call.body["socket_id"], "1234.5678");
    assert_eq!(call.data(), json!({"id": 1}));
}

#[tokio::test]
async fn channels_are_sent_in_chunks_of_one_hundred() {
    let _app = app();

    let channels: Vec<Channel> = (1..=150)
        .map(|id| Channel::new(format!("users.{id}")))
        .collect();
    Broadcast::connection("pusher")
        .unwrap()
        .broadcast(&channels, "Maintenance", Map::new())
        .await
        .unwrap();

    let requests = sent();
    assert_eq!(requests.len(), 2);

    let first = verify_pusher_call(&requests[0], KEY, SECRET);
    let second = verify_pusher_call(&requests[1], KEY, SECRET);
    assert_eq!(first.body["channels"].as_array().unwrap().len(), 100);
    assert_eq!(first.body["channels"][0], "users.1");
    assert_eq!(second.body["channels"].as_array().unwrap().len(), 50);
    assert_eq!(second.body["channels"][0], "users.101");
    assert_eq!(second.body["channels"][49], "users.150");
}

#[tokio::test]
async fn reverb_speaks_the_pusher_protocol_to_your_own_server() {
    let _app = app();

    Broadcast::connection("reverb")
        .unwrap()
        .broadcast(&[Channel::new("orders")], "OrderShipped", Map::new())
        .await
        .unwrap();

    let call = verify_pusher_call(&sent()[0], "reverb-key", "reverb-secret");
    assert_eq!(call.endpoint, "http://localhost:8080/apps/1001/events");
}

#[tokio::test]
async fn clusters_hosts_and_path_prefixes_are_honored() {
    let _app = app_with(|config| {
        config["broadcasting"]["connections"]["eu"] = json!({
            "driver": "pusher", "key": KEY, "secret": SECRET, "app_id": "3",
            "options": {"cluster": "eu"},
        });
        config["broadcasting"]["connections"]["proxied"] = json!({
            "driver": "pusher", "key": KEY, "secret": SECRET, "app_id": "3",
            "options": {"host": "https://ws.example.com", "port": 6001, "path": "/pusher"},
        });
    });

    Http::fake();
    for connection in ["eu", "proxied"] {
        Broadcast::connection(connection)
            .unwrap()
            .broadcast(&[Channel::new("orders")], "OrderShipped", Map::new())
            .await
            .unwrap();
    }

    let requests = sent();
    assert_eq!(
        verify_pusher_call(&requests[0], KEY, SECRET).endpoint,
        "https://api-eu.pusher.com:443/apps/3/events"
    );
    assert_eq!(
        verify_pusher_call(&requests[1], KEY, SECRET).endpoint,
        "https://ws.example.com:6001/pusher/apps/3/events"
    );
}

#[tokio::test]
async fn api_errors_become_broadcast_exceptions() {
    let _app = app_with(|_| {});
    Http::fake_urls([(
        "api-mt1.pusher.com*",
        Http::response("Unknown auth_key", 401, &[]),
    )]);

    let error = Broadcast::connection("pusher")
        .unwrap()
        .broadcast(&[Channel::new("orders")], "OrderShipped", Map::new())
        .await
        .unwrap_err();

    assert!(error.is::<BroadcastException>());
    assert_eq!(error.to_string(), "Pusher error: Unknown auth_key.");
}

#[tokio::test]
async fn invalid_sockets_and_channels_are_rejected() {
    let _app = app();
    let pusher = Broadcast::connection("pusher").unwrap();

    let error = pusher
        .broadcast(
            &[Channel::new("orders")],
            "OrderShipped",
            payload(json!({"socket": "not-a-socket"})),
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Invalid socket ID not-a-socket");

    let error = pusher
        .broadcast(&[Channel::new("orders 1")], "OrderShipped", Map::new())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Invalid channel name orders 1");
    assert!(sent().is_empty());
}

#[tokio::test]
async fn encrypted_channels_are_encrypted_end_to_end() {
    let _app = app();
    let channel = EncryptedPrivateChannel::new("invoices.1");

    Broadcast::connection("pusher")
        .unwrap()
        .broadcast(
            std::slice::from_ref(&channel),
            "InvoicePaid",
            payload(json!({"amount": 100, "socket": null})),
        )
        .await
        .unwrap();

    let call = verify_pusher_call(&sent()[0], KEY, SECRET);
    assert_eq!(
        call.body["channels"],
        json!(["private-encrypted-invoices.1"])
    );

    // Decrypt the way Echo does, with the channel's shared secret.
    let message = call.data();
    let nonce: [u8; 24] = BASE64
        .decode(message["nonce"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let ciphertext = BASE64
        .decode(message["ciphertext"].as_str().unwrap())
        .unwrap();
    let plaintext = secretbox::open(&ciphertext, &nonce, &shared_secret(channel.name())).unwrap();
    assert_eq!(String::from_utf8(plaintext).unwrap(), r#"{"amount":100}"#);
}

#[tokio::test]
async fn encrypted_channels_cannot_be_mixed_with_others() {
    let _app = app();

    let error = Broadcast::connection("pusher")
        .unwrap()
        .broadcast(
            &[
                EncryptedPrivateChannel::new("invoices.1"),
                Channel::new("orders"),
            ],
            "InvoicePaid",
            Map::new(),
        )
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "You cannot trigger to multiple channels when using encrypted channels"
    );
}

#[tokio::test]
async fn misconfigured_connections_explain_themselves() {
    let _app = app_with(|config| {
        config["broadcasting"]["connections"]["pusher"]["key"] = Value::Null;
    });

    let error = Broadcast::connection("pusher").err().unwrap();
    assert_eq!(
        error.to_string(),
        "Failed to create broadcaster for connection \"pusher\" with error: The Pusher [key] has not been configured."
    );
}
