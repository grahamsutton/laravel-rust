use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use illuminate_container::Container;
use illuminate_events::{Dispatcher, Event, Listener, async_trait};
use illuminate_support::{Result, json};

#[derive(Debug)]
struct OrderShipped {
    order_id: u64,
}

#[derive(Debug)]
struct OrderCreated;

#[derive(Debug)]
struct OrderFailedToShip;

struct SendShipmentNotification;

#[async_trait]
impl Listener<OrderShipped> for SendShipmentNotification {
    async fn handle(&self, _: &OrderShipped) -> Result<()> {
        Ok(())
    }
}

fn container() -> (Arc<Container>, illuminate_container::LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    (container, guard)
}

#[track_caller]
fn panic_message(f: impl FnOnce()) -> String {
    let error = catch_unwind(AssertUnwindSafe(f)).expect_err("the assertion should have failed");
    if let Some(message) = error.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = error.downcast_ref::<&str>() {
        message.to_string()
    } else {
        String::new()
    }
}

fn counter_listener(counter: &Arc<AtomicUsize>) {
    let counter = counter.clone();
    Event::listen(move |_: &OrderShipped| {
        counter.fetch_add(1, Ordering::SeqCst);
        async {}
    });
}

#[tokio::test]
async fn faking_records_events_instead_of_running_listeners() {
    let (_container, _guard) = container();
    let calls = Arc::new(AtomicUsize::new(0));
    counter_listener(&calls);

    Event::fake();
    Event::dispatch(OrderShipped { order_id: 1 }).await.unwrap();
    Event::dispatch(OrderShipped { order_id: 2 }).await.unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    Event::assert_dispatched::<OrderShipped>();
    Event::assert_dispatched_times::<OrderShipped>(2);
    Event::assert_dispatched_with(|event: &OrderShipped| event.order_id == 2);
    Event::assert_not_dispatched_with(|event: &OrderShipped| event.order_id == 3);
    Event::assert_not_dispatched::<OrderFailedToShip>();

    let dispatched = Event::dispatched::<OrderShipped>();
    assert_eq!(
        dispatched.iter().map(|e| e.order_id).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(Event::has_dispatched::<OrderShipped>());
}

#[tokio::test]
async fn assertion_failures_use_laravels_messages() {
    let (_container, _guard) = container();
    Event::fake();
    Event::dispatch(OrderShipped { order_id: 1 }).await.unwrap();

    let name = std::any::type_name::<OrderShipped>();
    let failed = std::any::type_name::<OrderFailedToShip>();

    assert_eq!(
        panic_message(|| Event::assert_dispatched::<OrderFailedToShip>()),
        format!("The expected [{failed}] event was not dispatched.")
    );
    assert_eq!(
        panic_message(|| Event::assert_not_dispatched::<OrderShipped>()),
        format!("The unexpected [{name}] event was dispatched.")
    );
    assert!(
        panic_message(|| Event::assert_dispatched_times::<OrderShipped>(2)).starts_with(&format!(
            "assertion `left == right` failed: The expected [{name}] event was dispatched 1 time instead of 2 times."
        ))
    );
    assert_eq!(
        panic_message(|| Event::assert_dispatched_with(|e: &OrderShipped| e.order_id == 9)),
        format!("The expected [{name}] event was not dispatched.")
    );
    assert_eq!(
        panic_message(|| Event::assert_nothing_dispatched()),
        format!("1 unexpected events were dispatched:\n\n- {name} dispatched 1 time\n")
    );
}

#[tokio::test]
async fn nothing_dispatched_passes_with_no_events() {
    let (_container, _guard) = container();
    Event::fake();
    Event::assert_nothing_dispatched();
    Event::assert_dispatched_times::<OrderShipped>(0);
}

#[tokio::test]
async fn faking_a_subset_dispatches_everything_else() {
    let (_container, _guard) = container();
    let calls = Arc::new(AtomicUsize::new(0));
    counter_listener(&calls);

    Event::fake_only::<OrderCreated>();

    Event::dispatch(OrderCreated).await.unwrap();
    Event::dispatch(OrderShipped { order_id: 1 }).await.unwrap();

    Event::assert_dispatched::<OrderCreated>();
    Event::assert_not_dispatched::<OrderShipped>();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn subsets_can_grow() {
    let (_container, _guard) = container();
    Event::fake_only::<OrderCreated>().also_fake::<OrderShipped>();

    Event::dispatch(OrderCreated).await.unwrap();
    Event::dispatch(OrderShipped { order_id: 1 }).await.unwrap();

    Event::assert_dispatched::<OrderCreated>();
    Event::assert_dispatched::<OrderShipped>();
}

#[tokio::test]
async fn except_dispatches_the_given_events_normally() {
    let (_container, _guard) = container();
    let calls = Arc::new(AtomicUsize::new(0));
    counter_listener(&calls);

    Event::fake().except::<OrderShipped>();

    Event::dispatch(OrderShipped { order_id: 1 }).await.unwrap();
    Event::dispatch(OrderCreated).await.unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    Event::assert_not_dispatched::<OrderShipped>();
    Event::assert_dispatched::<OrderCreated>();
}

#[tokio::test]
async fn listeners_registered_while_faked_reach_the_real_dispatcher() {
    let (container, _guard) = container();
    let fake = Event::fake();

    Event::listen(SendShipmentNotification);

    assert!(Event::has_listeners::<OrderShipped>());
    Event::assert_listening::<OrderShipped, SendShipmentNotification>();
    Event::assert_has_listeners::<OrderShipped>();

    let real = fake.original().unwrap();
    assert!(real.has_listeners::<OrderShipped>());
    assert!(!real.is_fake());
    assert!(container.make::<Dispatcher>().is_fake());
}

#[tokio::test]
async fn assert_listening_reports_missing_listeners() {
    let (_container, _guard) = container();
    Event::fake();
    Event::listen(|_: &OrderShipped| async {});

    let message = panic_message(|| {
        Event::assert_listening::<OrderShipped, SendShipmentNotification>()
    });
    assert_eq!(
        message,
        format!(
            "Event [{}] does not have the [{}] listener attached to it",
            std::any::type_name::<OrderShipped>(),
            std::any::type_name::<SendShipmentNotification>()
        )
    );
}

#[tokio::test]
async fn named_events_can_be_faked() {
    let (_container, _guard) = container();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    Event::listen_named("eloquent.created: *", move |_, _| {
        c.fetch_add(1, Ordering::SeqCst);
        async {}
    });

    Event::fake_only_named(&["eloquent.created: App\\Models\\User"]);

    Event::dispatch_named("eloquent.created: App\\Models\\User", json!({"id": 1}))
        .await
        .unwrap();
    Event::dispatch_named("eloquent.created: App\\Models\\Post", json!({"id": 2}))
        .await
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    Event::assert_dispatched_named("eloquent.created: App\\Models\\User");
    Event::assert_dispatched_named_with("eloquent.created: *", |payload| payload["id"] == 1);
    Event::assert_dispatched_named_times("eloquent.created: App\\Models\\User", 1);
    Event::assert_not_dispatched_named("eloquent.created: App\\Models\\Post");
    assert_eq!(
        Event::dispatched_named("eloquent.created: App\\Models\\User"),
        vec![json!({"id": 1})]
    );
}

#[tokio::test]
async fn faked_until_reports_completion() {
    let (_container, _guard) = container();
    Event::listen(|_: &OrderShipped| async { false });
    Event::fake();
    assert!(Event::until(OrderShipped { order_id: 1 }).await.unwrap());
}

#[tokio::test]
async fn fake_for_restores_the_real_dispatcher() {
    let (_container, _guard) = container();
    let calls = Arc::new(AtomicUsize::new(0));
    counter_listener(&calls);

    Event::fake_for(|| async {
        Event::dispatch(OrderShipped { order_id: 1 }).await.unwrap();
        Event::assert_dispatched::<OrderShipped>();
    })
    .await;

    assert!(!Event::dispatcher().is_fake());
    Event::dispatch(OrderShipped { order_id: 2 }).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn faking_twice_wraps_the_real_dispatcher() {
    let (_container, _guard) = container();
    let real = Event::dispatcher();
    Event::fake();
    let second = Event::fake();
    assert!(Arc::ptr_eq(&second.original().unwrap(), &real));
}

#[tokio::test]
async fn assertions_require_a_fake() {
    let (_container, _guard) = container();
    let message = panic_message(|| Event::assert_nothing_dispatched());
    assert!(message.contains("Event::fake()"));
}

#[tokio::test]
async fn forgetting_is_ignored_while_faked() {
    let (_container, _guard) = container();
    Event::listen(|_: &OrderShipped| async {});
    Event::fake();
    Event::forget::<OrderShipped>();
    assert!(Event::has_listeners::<OrderShipped>());
}

#[tokio::test]
async fn dispatched_events_lists_names_in_order() {
    let (_container, _guard) = container();
    let fake = Event::fake();
    Event::dispatch(OrderCreated).await.unwrap();
    Event::dispatch_named("audit", json!(null)).await.unwrap();
    assert_eq!(
        fake.dispatched_events(),
        vec![std::any::type_name::<OrderCreated>().to_string(), "audit".to_string()]
    );
}
