//! Deferring events until a block of code finishes.

use std::sync::{Arc, Mutex};

use illuminate_events::Dispatcher;
use illuminate_support::{Value, error::error, json};

struct UserCreated(&'static str);
struct PostCreated(&'static str);

fn recording() -> (Dispatcher, Arc<Mutex<Vec<String>>>) {
    let events = Dispatcher::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    let users = log.clone();
    events.listen(move |event: &UserCreated| {
        users.lock().unwrap().push(format!("user {}", event.0));
        async {}
    });
    let posts = log.clone();
    events.listen(move |event: &PostCreated| {
        posts.lock().unwrap().push(format!("post {}", event.0));
        async {}
    });
    let named = log.clone();
    events.listen_named("eloquent.*", move |name: &str, _: &Value| {
        named.lock().unwrap().push(name.to_string());
        async { Ok(()) }
    });
    (events, log)
}

#[tokio::test]
async fn events_are_dispatched_after_the_block() {
    let (events, log) = recording();

    let result = events
        .defer(async {
            events.dispatch(UserCreated("Victoria")).await?;
            events.dispatch_named("eloquent.created: User", json!({})).await?;
            log.lock().unwrap().push("block done".into());
            Ok(42)
        })
        .await
        .unwrap();

    assert_eq!(result, 42);
    assert_eq!(*log.lock().unwrap(), ["block done", "user Victoria", "eloquent.created: User"]);
}

#[tokio::test]
async fn events_are_dropped_when_the_block_fails() {
    let (events, log) = recording();

    let error = events
        .defer(async {
            events.dispatch(UserCreated("Victoria")).await?;
            Err::<(), _>(error!("Unable to create the post."))
        })
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "Unable to create the post.");
    assert!(log.lock().unwrap().is_empty());

    // Later events are dispatched immediately again.
    events.dispatch(UserCreated("Taylor")).await.unwrap();
    assert_eq!(*log.lock().unwrap(), ["user Taylor"]);
}

#[tokio::test]
async fn only_the_given_events_may_be_deferred() {
    let (events, log) = recording();

    events
        .defer_only(["UserCreated", "eloquent.created: User"], async {
            events.dispatch(UserCreated("Victoria")).await?;
            events.dispatch(PostCreated("Hello")).await?;
            events.dispatch_named("eloquent.created: User", json!({})).await?;
            events.dispatch_named("eloquent.saved: User", json!({})).await?;
            Ok(())
        })
        .await
        .unwrap();

    assert_eq!(
        *log.lock().unwrap(),
        ["post Hello", "eloquent.saved: User", "user Victoria", "eloquent.created: User"]
    );
}

#[tokio::test]
async fn deferrals_nest() {
    let (events, log) = recording();

    events
        .defer(async {
            events.dispatch(UserCreated("outer")).await?;
            events
                .defer(async {
                    events.dispatch(PostCreated("inner")).await?;
                    Ok(())
                })
                .await?;
            log.lock().unwrap().push("outer block done".into());
            Ok(())
        })
        .await
        .unwrap();

    assert_eq!(*log.lock().unwrap(), ["post inner", "outer block done", "user outer"]);
}

#[tokio::test]
async fn listeners_may_dispatch_events_while_deferred_events_flush() {
    let events = Arc::new(Dispatcher::new());
    let log = Arc::new(Mutex::new(Vec::new()));
    let dispatcher = events.clone();
    events.listen(move |_: &UserCreated| {
        let dispatcher = dispatcher.clone();
        async move { dispatcher.dispatch(PostCreated("welcome")).await }
    });
    let posts = log.clone();
    events.listen(move |event: &PostCreated| {
        posts.lock().unwrap().push(event.0);
        async {}
    });

    events
        .defer(async {
            events.dispatch(UserCreated("Victoria")).await?;
            assert!(log.lock().unwrap().is_empty());
            Ok(())
        })
        .await
        .unwrap();

    assert_eq!(*log.lock().unwrap(), ["welcome"]);
}
