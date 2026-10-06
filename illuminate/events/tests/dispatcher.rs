use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use illuminate_container::{Container, ServiceProvider};
use illuminate_events::{
    Dispatcher, Event, EventServiceProvider, EventSubscriber, Listener, Propagation,
    QueuedListener, ShouldQueue, async_trait, event, stop_propagation,
};
use illuminate_support::error::bail;
use illuminate_support::{Result, Value, json};

#[derive(Debug, Clone)]
struct OrderShipped {
    order_id: u64,
}

#[derive(Debug)]
struct OrderFailedToShip;

type Log = Arc<Mutex<Vec<String>>>;

fn log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

fn entries(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

struct SendShipmentNotification {
    log: Log,
}

#[async_trait]
impl Listener<OrderShipped> for SendShipmentNotification {
    async fn handle(&self, event: &OrderShipped) -> Result<()> {
        self.log
            .lock()
            .unwrap()
            .push(format!("notify {}", event.order_id));
        Ok(())
    }
}

#[tokio::test]
async fn listeners_run_in_registration_order() {
    let events = Dispatcher::new();
    let log = log();

    let first = log.clone();
    events.listen(move |event: &OrderShipped| {
        first
            .lock()
            .unwrap()
            .push(format!("first {}", event.order_id));
        async { Ok(()) }
    });

    let second = log.clone();
    events.listen(move |event: Arc<OrderShipped>| {
        let log = second.clone();
        async move {
            tokio::task::yield_now().await;
            log.lock()
                .unwrap()
                .push(format!("second {}", event.order_id));
            Ok(())
        }
    });

    events.listen_with::<OrderShipped, _>(SendShipmentNotification { log: log.clone() });

    events.dispatch(OrderShipped { order_id: 7 }).await.unwrap();

    assert_eq!(entries(&log), vec!["first 7", "second 7", "notify 7"]);
}

#[tokio::test]
async fn listener_structs_can_be_registered_by_inference() {
    let events = Dispatcher::new();
    let log = log();
    events.listen(SendShipmentNotification { log: log.clone() });

    events.dispatch(OrderShipped { order_id: 1 }).await.unwrap();

    assert_eq!(entries(&log), vec!["notify 1"]);
    assert_eq!(
        events.get_listeners::<OrderShipped>(),
        vec![std::any::type_name::<SendShipmentNotification>()]
    );
}

#[tokio::test]
async fn closures_may_return_unit_bool_or_propagation() {
    let events = Dispatcher::new();
    let calls = Arc::new(AtomicUsize::new(0));

    let c = calls.clone();
    events.listen(move |_: &OrderShipped| {
        c.fetch_add(1, Ordering::SeqCst);
        async {}
    });
    let c = calls.clone();
    events.listen(move |_: &OrderShipped| {
        c.fetch_add(1, Ordering::SeqCst);
        async { true }
    });
    let c = calls.clone();
    events.listen(move |_: &OrderShipped| {
        c.fetch_add(1, Ordering::SeqCst);
        async { Propagation::Continue }
    });
    let c = calls.clone();
    events.listen(move |_: &OrderShipped| {
        c.fetch_add(1, Ordering::SeqCst);
        async { Ok(true) }
    });

    events.dispatch(OrderShipped { order_id: 1 }).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn returning_false_stops_propagation() {
    let events = Dispatcher::new();
    let log = log();

    let l = log.clone();
    events.listen(move |_: &OrderShipped| {
        l.lock().unwrap().push("first".into());
        async { false }
    });
    let l = log.clone();
    events.listen(move |_: &OrderShipped| {
        l.lock().unwrap().push("second".into());
        async {}
    });

    let completed = events.until(OrderShipped { order_id: 1 }).await.unwrap();

    assert!(!completed);
    assert_eq!(entries(&log), vec!["first"]);

    // `dispatch` treats halting as success.
    assert!(events.dispatch(OrderShipped { order_id: 1 }).await.is_ok());
}

#[tokio::test]
async fn listener_structs_stop_propagation_with_a_dedicated_error() {
    struct Halting;

    #[async_trait]
    impl Listener<OrderShipped> for Halting {
        async fn handle(&self, _: &OrderShipped) -> Result<()> {
            stop_propagation()
        }
    }

    let events = Dispatcher::new();
    let log = log();
    events.listen(Halting);
    events.listen(SendShipmentNotification { log: log.clone() });

    assert!(!events.until(OrderShipped { order_id: 1 }).await.unwrap());
    assert!(entries(&log).is_empty());
}

#[tokio::test]
async fn errors_propagate_and_stop_the_dispatch() {
    let events = Dispatcher::new();
    let log = log();

    events.listen(|event: &OrderShipped| {
        let id = event.order_id;
        async move {
            if id == 13 {
                bail!("Unlucky order {id}.");
            }
            Ok(())
        }
    });
    events.listen(SendShipmentNotification { log: log.clone() });

    let error = events
        .dispatch(OrderShipped { order_id: 13 })
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "Unlucky order 13.");
    assert!(entries(&log).is_empty());
}

#[tokio::test]
async fn events_without_listeners_are_fine() {
    let events = Dispatcher::new();
    assert!(!events.has_listeners::<OrderShipped>());
    events.dispatch(OrderShipped { order_id: 1 }).await.unwrap();
    assert!(events.until(OrderShipped { order_id: 1 }).await.unwrap());
}

#[tokio::test]
async fn listeners_can_be_forgotten() {
    let events = Dispatcher::new();
    events.listen(|_: &OrderShipped| async {});
    events.listen(|_: &OrderFailedToShip| async {});
    assert!(events.has_listeners::<OrderShipped>());

    events.forget::<OrderShipped>();

    assert!(!events.has_listeners::<OrderShipped>());
    assert!(events.has_listeners::<OrderFailedToShip>());
}

#[tokio::test]
async fn named_events_reach_exact_then_wildcard_listeners() {
    let events = Dispatcher::new();
    let log = log();

    let l = log.clone();
    events.listen_named("eloquent.*", move |name: &str, _: &Value| {
        l.lock().unwrap().push(format!("all models: {name}"));
        async { Ok(()) }
    });
    let l = log.clone();
    events.listen_named(
        "eloquent.created: App\\Models\\User",
        move |_, payload: &Value| {
            l.lock()
                .unwrap()
                .push(format!("exact: {}", payload["name"]));
            async { Ok(()) }
        },
    );
    let l = log.clone();
    events.listen_named("eloquent.created: *", move |name, _| {
        l.lock().unwrap().push(format!("created: {name}"));
        async {}
    });
    let l = log.clone();
    events.listen_named("eloquent.deleted: *", move |name, _| {
        l.lock().unwrap().push(format!("deleted: {name}"));
        async {}
    });

    events
        .dispatch_named(
            "eloquent.created: App\\Models\\User",
            json!({"name": "Taylor"}),
        )
        .await
        .unwrap();

    assert_eq!(
        entries(&log),
        vec![
            "exact: \"Taylor\"",
            "all models: eloquent.created: App\\Models\\User",
            "created: eloquent.created: App\\Models\\User",
        ]
    );
}

#[tokio::test]
async fn named_events_can_be_halted() {
    let events = Dispatcher::new();
    events.listen_named("eloquent.saving: *", |_, payload: &Value| {
        let allowed = payload["valid"] == json!(true);
        async move { allowed }
    });

    assert!(
        events
            .until_named("eloquent.saving: App\\Models\\Post", json!({"valid": true}))
            .await
            .unwrap()
    );
    assert!(
        !events
            .until_named(
                "eloquent.saving: App\\Models\\Post",
                json!({"valid": false})
            )
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn named_listener_inspection_and_forgetting() {
    let events = Dispatcher::new();
    events.listen_named("user.registered", |_, _| async {});
    events.listen_named("order.*", |_, _| async {});

    assert!(events.has_listeners_named("user.registered"));
    assert!(events.has_listeners_named("order.shipped"));
    assert!(events.has_listeners_named("order.*"));
    assert!(events.has_wildcard_listeners("order.placed"));
    assert!(!events.has_listeners_named("user.deleted"));
    assert_eq!(events.get_listeners_named("order.shipped").len(), 1);

    events.forget_named("order.*");
    assert!(!events.has_listeners_named("order.shipped"));
    assert_eq!(events.get_listeners_named("order.shipped").len(), 0);

    events.forget_named("user.registered");
    assert!(!events.has_listeners_named("user.registered"));
}

#[tokio::test]
async fn wildcard_cache_is_refreshed_when_listeners_are_added() {
    let events = Dispatcher::new();
    let calls = Arc::new(AtomicUsize::new(0));

    events
        .dispatch_named("cache.hit", json!(null))
        .await
        .unwrap();

    let c = calls.clone();
    events.listen_named("cache.*", move |_, _| {
        c.fetch_add(1, Ordering::SeqCst);
        async {}
    });
    events
        .dispatch_named("cache.hit", json!(null))
        .await
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn pushed_events_are_dispatched_when_flushed() {
    let events = Dispatcher::new();
    let log = log();
    let l = log.clone();
    events.listen_named("report.ready", move |_, payload| {
        l.lock().unwrap().push(payload.to_string());
        async {}
    });

    events.push("report.ready", json!(1));
    events.push("report.ready", json!(2));
    assert!(entries(&log).is_empty());

    events.flush("report.ready").await.unwrap();
    assert_eq!(entries(&log), vec!["1", "2"]);

    events.forget_pushed();
    events.flush("report.ready").await.unwrap();
    assert_eq!(entries(&log).len(), 2);
}

#[tokio::test]
async fn subscribers_register_several_listeners() {
    struct Login;
    struct Logout;

    struct UserEventSubscriber {
        log: Log,
    }

    impl EventSubscriber for UserEventSubscriber {
        fn subscribe(&self, events: &Dispatcher) {
            let log = self.log.clone();
            events.listen(move |_: &Login| {
                log.lock().unwrap().push("login".into());
                async {}
            });
            let log = self.log.clone();
            events.listen(move |_: &Logout| {
                log.lock().unwrap().push("logout".into());
                async {}
            });
        }
    }

    let events = Dispatcher::new();
    let log = log();
    events.subscribe(UserEventSubscriber { log: log.clone() });

    events.dispatch(Login).await.unwrap();
    events.dispatch(Logout).await.unwrap();

    assert_eq!(entries(&log), vec!["login", "logout"]);
}

struct QueuedNotification {
    log: Log,
    only_even: bool,
}

#[async_trait]
impl Listener<OrderShipped> for QueuedNotification {
    async fn handle(&self, event: &OrderShipped) -> Result<()> {
        self.log
            .lock()
            .unwrap()
            .push(format!("queued {}", event.order_id));
        Ok(())
    }

    fn should_queue(&self, event: &OrderShipped) -> bool {
        !self.only_even || event.order_id.is_multiple_of(2)
    }
}

impl ShouldQueue for QueuedNotification {
    fn via_queue(&self) -> Option<String> {
        Some("listeners".into())
    }

    fn with_delay(&self) -> Option<Duration> {
        Some(Duration::from_secs(60))
    }
}

#[tokio::test]
async fn queued_listeners_run_inline_without_a_queue_hook() {
    let events = Dispatcher::new();
    let log = log();
    events.listen_queued::<OrderShipped, _>(QueuedNotification {
        log: log.clone(),
        only_even: false,
    });

    assert!(!events.has_queue_hook());
    events.dispatch(OrderShipped { order_id: 3 }).await.unwrap();

    assert_eq!(entries(&log), vec!["queued 3"]);
}

#[tokio::test]
async fn queued_listeners_are_handed_to_the_queue_hook() {
    let events = Dispatcher::new();
    let log = log();
    let jobs: Arc<Mutex<Vec<QueuedListener>>> = Arc::new(Mutex::new(Vec::new()));

    events.listen_queued::<OrderShipped, _>(QueuedNotification {
        log: log.clone(),
        only_even: true,
    });
    let queue = jobs.clone();
    events.queue_listeners_using(move |job| {
        queue.lock().unwrap().push(job);
        async { Ok(()) }
    });

    events.dispatch(OrderShipped { order_id: 1 }).await.unwrap();
    events.dispatch(OrderShipped { order_id: 2 }).await.unwrap();

    // Nothing ran yet: the listener is waiting on the "queue"...
    assert!(entries(&log).is_empty());
    let mut pending = std::mem::take(&mut *jobs.lock().unwrap());
    assert_eq!(pending.len(), 1, "should_queue skips odd orders");

    let job = pending.pop().unwrap();
    assert_eq!(job.listener, std::any::type_name::<QueuedNotification>());
    assert_eq!(job.event, std::any::type_name::<OrderShipped>());
    assert_eq!(job.queue.as_deref(), Some("listeners"));
    assert_eq!(job.delay, Some(Duration::from_secs(60)));
    assert_eq!(job.connection, None);

    // ...until a worker processes it.
    job.handle().await.unwrap();
    assert_eq!(entries(&log), vec!["queued 2"]);
}

#[tokio::test]
async fn raw_listeners_describe_every_event() {
    let events = Dispatcher::new();
    events.listen(SendShipmentNotification { log: log() });
    events.listen_named("user.*", |_, _| async {});

    let raw = events.get_raw_listeners();
    assert_eq!(raw.len(), 2);
    assert_eq!(raw[0].0, std::any::type_name::<OrderShipped>());
    assert_eq!(
        raw[0].1,
        vec![std::any::type_name::<SendShipmentNotification>()]
    );
    assert_eq!(raw[1].0, "user.*");
}

#[tokio::test]
async fn the_facade_and_helper_use_the_container_dispatcher() {
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    EventServiceProvider.register(&container);

    let log = log();
    let l = log.clone();
    Event::listen(move |event: &OrderShipped| {
        l.lock().unwrap().push(format!("facade {}", event.order_id));
        async {}
    });

    event(OrderShipped { order_id: 5 }).await.unwrap();
    Event::dispatch(OrderShipped { order_id: 6 }).await.unwrap();

    assert_eq!(entries(&log), vec!["facade 5", "facade 6"]);
    assert!(
        container
            .make::<Dispatcher>()
            .has_listeners::<OrderShipped>()
    );
}

#[tokio::test]
async fn the_facade_registers_a_dispatcher_when_none_is_bound() {
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());

    Event::listen(|_: &OrderShipped| async {});

    assert!(container.bound::<Dispatcher>());
    assert!(Event::has_listeners::<OrderShipped>());
}

#[tokio::test]
async fn dispatch_futures_are_send() {
    fn assert_send<T: Send>(_: &T) {}

    let events = Arc::new(Dispatcher::new());
    events.listen(|_: &OrderShipped| async {});
    let future = {
        let events = events.clone();
        async move { events.dispatch(OrderShipped { order_id: 1 }).await }
    };
    assert_send(&future);
    tokio::spawn(future).await.unwrap().unwrap();

    let named = Event::dispatch_named("x", json!(null));
    assert_send(&named);
}

#[tokio::test]
async fn closures_can_use_the_question_mark_operator_with_any_error() {
    let events = Dispatcher::new();
    events.listen(|event: &OrderShipped| {
        let raw = format!("{}x", event.order_id);
        async move {
            let _id: u64 = raw.parse()?;
            Ok(())
        }
    });

    let error = events
        .dispatch(OrderShipped { order_id: 4 })
        .await
        .unwrap_err();
    assert!(error.is::<std::num::ParseIntError>());
}
