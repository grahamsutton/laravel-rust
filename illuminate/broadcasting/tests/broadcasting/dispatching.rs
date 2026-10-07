//! Broadcasting events: the event dispatcher, the queue, sockets, and the
//! fakes.

use std::sync::{Arc, Mutex};

use illuminate_broadcasting::{
    AnonymousEvent, Broadcast, BroadcastEvent, BroadcastException, Channel, broadcast,
    is_registered, registered_events,
};
use illuminate_events::{Dispatchable, Event};
use illuminate_http::with_request;
use illuminate_http_client::Http;
use illuminate_queue::{Queue, Worker, WorkerOptions};
use illuminate_support::json;

use crate::common::{
    KEY, SECRET, app, app_with, record_logs, request_with, sent, verify_pusher_call,
};
use crate::events::{
    Announcement, InvoicePaid, NewMessage, OrderPlaced, OrderShipmentStatusUpdated, ServerCreated,
    TypingStarted,
};

fn use_array_queue(config: &mut illuminate_support::Value) {
    config["queue"]["default"] = json!("array");
}

#[tokio::test]
async fn broadcasting_an_event_sends_it_to_pusher() {
    let _app = app();

    broadcast(OrderShipmentStatusUpdated::new(1)).await.unwrap();

    let requests = sent();
    assert_eq!(requests.len(), 1);
    let call = verify_pusher_call(&requests[0], KEY, SECRET);
    assert_eq!(
        call.body,
        json!({
            "name": "Events\\OrderShipmentStatusUpdated",
            "data": r#"{"order_id":1,"status":"shipped"}"#,
            "channels": ["private-orders.1"],
        })
    );
}

#[tokio::test]
async fn events_are_broadcast_through_the_queue() {
    let _app = app_with(use_array_queue);
    Http::fake();

    broadcast(OrderShipmentStatusUpdated::new(7)).await.unwrap();

    // Nothing is sent until a worker processes the job...
    assert!(sent().is_empty());
    assert_eq!(Queue::size(Some("default")).await.unwrap(), 1);

    let worker = Worker::make();
    worker
        .daemon(
            "array",
            "default",
            &WorkerOptions::new().sleep(0.0).stop_when_empty(),
        )
        .await
        .unwrap();
    assert_eq!(worker.jobs_processed(), 1);

    let call = verify_pusher_call(&sent()[0], KEY, SECRET);
    assert_eq!(call.body["channels"], json!(["private-orders.7"]));
    assert_eq!(call.data(), json!({"order_id": 7, "status": "shipped"}));
}

#[tokio::test]
async fn the_queued_job_carries_the_serialized_event() {
    let _app = app();
    Queue::fake();

    broadcast(OrderShipmentStatusUpdated::new(3)).await.unwrap();

    Queue::assert_pushed_with::<BroadcastEvent>(|job| {
        job.event == "Events\\OrderShipmentStatusUpdated"
            && job.name == "Events\\OrderShipmentStatusUpdated"
            && job.broadcasts_on("private-orders.3")
            && job.payload["order_id"] == 3
            && job.socket().is_none()
    });
    assert!(sent().is_empty());
}

#[tokio::test]
async fn events_choose_their_queue_and_conditions() {
    let _app = app();

    // Not valuable enough to broadcast...
    broadcast(OrderPlaced { value: 50 }).await.unwrap();
    let queue = Queue::connection("array").unwrap();
    assert_eq!(queue.size(Some("broadcasts")).await.unwrap(), 0);

    broadcast(OrderPlaced { value: 500 }).await.unwrap();
    assert_eq!(queue.size(Some("broadcasts")).await.unwrap(), 1);
    assert_eq!(queue.size(Some("default")).await.unwrap(), 0);
    assert!(sent().is_empty());
}

#[tokio::test]
async fn should_broadcast_now_skips_the_queue() {
    let _app = app_with(use_array_queue);
    Http::fake();

    broadcast(ServerCreated {
        id: 1,
        secret: "hidden".into(),
    })
    .await
    .unwrap();

    assert_eq!(Queue::size(Some("default")).await.unwrap(), 0);
    let call = verify_pusher_call(&sent()[0], KEY, SECRET);
    assert_eq!(call.body["name"], "server.created");
    assert_eq!(call.data(), json!({"id": 1}));
}

#[tokio::test]
async fn dispatching_a_registered_event_broadcasts_it() {
    let _app = app();
    let fake = Broadcast::fake();

    // `register_broadcast!` events are hooked up when the provider boots.
    assert!(
        registered_events()
            .iter()
            .any(|name| name.ends_with("OrderShipmentStatusUpdated"))
    );
    assert!(is_registered::<OrderShipmentStatusUpdated>(
        &Event::dispatcher()
    ));

    Event::dispatch(OrderShipmentStatusUpdated::new(1))
        .await
        .unwrap();
    OrderShipmentStatusUpdated::new(2).dispatch().await.unwrap();

    fake.assert_broadcast_times::<OrderShipmentStatusUpdated>(2);
    fake.assert_broadcast_on::<OrderShipmentStatusUpdated>("private-orders.2");

    // Other events are registered at runtime.
    assert!(!is_registered::<NewMessage>(&Event::dispatcher()));
    Broadcast::register::<NewMessage>();
    Broadcast::register::<NewMessage>();
    Event::dispatch(NewMessage {
        room_id: 1,
        body: "Hi".into(),
    })
    .await
    .unwrap();
    fake.assert_broadcast_times::<NewMessage>(1);
}

#[tokio::test]
async fn listeners_still_run_when_broadcasting() {
    let _app = app();
    let fake = Broadcast::fake();
    let heard = Arc::new(Mutex::new(Vec::new()));
    let log = heard.clone();
    Event::listen(move |event: &OrderShipmentStatusUpdated| {
        log.lock().unwrap().push(event.order_id);
        async {}
    });

    broadcast(OrderShipmentStatusUpdated::new(5)).await.unwrap();

    assert_eq!(*heard.lock().unwrap(), vec![5]);
    fake.assert_broadcast::<OrderShipmentStatusUpdated>();
}

#[tokio::test]
async fn faking_events_prevents_broadcasting() {
    let _app = app();
    let fake = Broadcast::fake();
    Event::fake();

    broadcast(OrderShipmentStatusUpdated::new(1))
        .to_others()
        .await
        .unwrap();
    Broadcast::on("orders").send().await.unwrap();

    Event::assert_dispatched_with(|event: &OrderShipmentStatusUpdated| event.order_id == 1);
    Event::assert_dispatched::<AnonymousEvent>();
    fake.assert_nothing_broadcast();
    assert!(sent().is_empty());
}

#[tokio::test]
async fn to_others_excludes_the_current_socket() {
    let _app = app();
    let request = request_with(json!({}), &[("X-Socket-ID", "1234.5678")]);

    with_request(request, async {
        broadcast(OrderShipmentStatusUpdated::new(1))
            .to_others()
            .await
            .unwrap();
        broadcast(OrderShipmentStatusUpdated::new(2)).await.unwrap();
    })
    .await;

    let requests = sent();
    let first = verify_pusher_call(&requests[0], KEY, SECRET);
    assert_eq!(first.body["socket_id"], "1234.5678");
    assert_eq!(first.data(), json!({"order_id": 1, "status": "shipped"}));

    let second = verify_pusher_call(&requests[1], KEY, SECRET);
    assert!(second.body.get("socket_id").is_none());
}

#[tokio::test]
async fn events_may_always_exclude_the_current_user() {
    let _app = app();
    let fake = Broadcast::fake();
    let request = request_with(json!({}), &[("X-Socket-ID", "99.1")]);

    with_request(request.clone(), async {
        broadcast(NewMessage {
            room_id: 1,
            body: "Hi".into(),
        })
        .await
        .unwrap();
    })
    .await;

    fake.assert_broadcast_with::<NewMessage>(|broadcast| broadcast.socket() == Some("99.1"));
    assert_eq!(Broadcast::socket(Some(&request)), Some("99.1".to_string()));
    assert_eq!(Broadcast::socket(None), None);
}

#[tokio::test]
async fn events_may_be_sent_via_another_connection() {
    let _app = app();
    let logs = record_logs();

    broadcast(OrderShipmentStatusUpdated::new(1))
        .via("log")
        .await
        .unwrap();

    assert!(sent().is_empty());
    let logs = logs.lock().unwrap();
    assert_eq!(logs.len(), 1);
    assert!(logs[0].starts_with(
        "Broadcasting [Events\\OrderShipmentStatusUpdated] on channels [private-orders.1] with payload:\n"
    ));
}

#[tokio::test]
async fn events_may_broadcast_on_several_connections() {
    let _app = app();

    broadcast(Announcement {
        message: "Deploying!".into(),
    })
    .await
    .unwrap();

    let requests = sent();
    assert_eq!(requests.len(), 2);
    verify_pusher_call(&requests[0], KEY, SECRET);
    verify_pusher_call(&requests[1], "reverb-key", "reverb-secret");
}

#[tokio::test]
async fn encrypted_events_are_sent_encrypted() {
    let app = app();

    broadcast(InvoicePaid {
        invoice_id: 1,
        amount: 100,
    })
    .await
    .unwrap();

    let call = verify_pusher_call(&sent()[0], KEY, SECRET);
    let pusher = Broadcast::pusher(&app.config().get("broadcasting.connections.pusher")).unwrap();
    let decrypted = pusher
        .decrypt_payload(
            "private-encrypted-invoices.1",
            call.body["data"].as_str().unwrap(),
        )
        .unwrap();
    assert_eq!(decrypted, r#"{"invoice_id":1,"amount":100}"#);
}

#[tokio::test]
async fn failures_may_be_rescued() {
    let _app = app_with(|_| {});
    Http::fake_urls([("*", Http::response("Over quota", 500, &[]))]);

    let error = broadcast(TypingStarted { rescue: false })
        .await
        .unwrap_err();
    assert!(error.is::<BroadcastException>());
    assert_eq!(error.to_string(), "Pusher error: Over quota.");

    broadcast(TypingStarted { rescue: true }).await.unwrap();
}

#[tokio::test]
async fn anonymous_events_are_broadcast() {
    let _app = app();

    Broadcast::on("orders.1").send().await.unwrap();
    Broadcast::private("orders.1")
        .as_("OrderPlaced")
        .with(json!({"id": 1, "total": 100}))
        .send_now()
        .await
        .unwrap();
    Broadcast::presence("chat.1")
        .via("log")
        .send()
        .await
        .unwrap();

    let requests = sent();
    assert_eq!(requests.len(), 2);

    let first = verify_pusher_call(&requests[0], KEY, SECRET);
    assert_eq!(
        first.body,
        json!({"name": "AnonymousEvent", "data": "{}", "channels": ["orders.1"]})
    );

    let second = verify_pusher_call(&requests[1], KEY, SECRET);
    assert_eq!(second.body["name"], "OrderPlaced");
    assert_eq!(second.body["channels"], json!(["private-orders.1"]));
    assert_eq!(second.data(), json!({"id": 1, "total": 100}));
}

#[tokio::test]
async fn anonymous_events_may_exclude_the_current_user() {
    let _app = app();
    let fake = Broadcast::fake();
    let request = request_with(json!({}), &[("X-Socket-ID", "1.2")]);

    with_request(
        request,
        Broadcast::on(vec![Channel::private("a"), Channel::new("b")])
            .to_others()
            .send(),
    )
    .await
    .unwrap();

    fake.assert_broadcast_with::<AnonymousEvent>(|broadcast| {
        broadcast.socket() == Some("1.2")
            && broadcast.broadcasts_on("private-a")
            && broadcast.broadcasts_on("b")
    });
    fake.assert_broadcast_as("AnonymousEvent");
}

#[tokio::test]
async fn events_may_be_queued_without_their_listeners() {
    let _app = app();
    let fake = Broadcast::fake();
    let heard = Arc::new(Mutex::new(0));
    let counter = heard.clone();
    Event::listen(move |_event: &OrderShipmentStatusUpdated| {
        *counter.lock().unwrap() += 1;
        async {}
    });

    Broadcast::queue(&OrderShipmentStatusUpdated::new(1))
        .await
        .unwrap();

    assert_eq!(*heard.lock().unwrap(), 0);
    fake.assert_broadcast::<OrderShipmentStatusUpdated>();
    fake.assert_not_broadcast::<NewMessage>();
    assert_eq!(fake.broadcasts().len(), 1);
    fake.flush();
    fake.assert_nothing_broadcast();
}

#[tokio::test]
#[should_panic(expected = "The expected [Events\\NewMessage] event was not broadcast.")]
async fn fake_assertions_fail_loudly() {
    let _app = app();
    Broadcast::fake();
    Broadcast::assert_broadcast::<NewMessage>();
}
