//! Running tasks concurrently, deferring them, and choosing drivers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use illuminate_concurrency::{
    Concurrency, ConcurrencyManager, ConcurrencyServiceProvider, DeferredCallbacks, IndexMap,
    SyncDriver, TaskTimedOutException, task,
};
use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider, app};
use illuminate_process::Process;
use illuminate_support::error::RuntimeException;
use illuminate_support::{Result, json};

fn container() -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    ConcurrencyServiceProvider.register(&container);
    let guard = Container::set_local_instance(container.clone());
    (container, guard)
}

async fn sleep_then<T>(millis: u64, value: T) -> Result<T> {
    tokio::time::sleep(Duration::from_millis(millis)).await;
    Ok(value)
}

#[tokio::test]
async fn work_can_be_distributed() {
    let _guard = container();

    let (first, second) = Concurrency::run((async { Ok(1 + 1) }, async { Ok(2 + 2) }))
        .await
        .unwrap();

    assert_eq!(first, 2);
    assert_eq!(second, 4);
}

#[tokio::test]
async fn tasks_run_at_the_same_time() {
    let _guard = container();
    let started = Instant::now();

    let results = Concurrency::run(vec![
        sleep_then(300, 1),
        sleep_then(300, 2),
        sleep_then(300, 3),
    ])
    .await
    .unwrap();

    assert_eq!(results, vec![1, 2, 3]);
    assert!(started.elapsed() < Duration::from_millis(800));
}

#[tokio::test]
async fn results_keep_the_order_of_their_tasks() {
    let _guard = container();

    let results = Concurrency::run([sleep_then(200, "slow"), sleep_then(10, "fast")])
        .await
        .unwrap();

    assert_eq!(results, ["slow", "fast"]);
}

#[tokio::test]
async fn output_is_mapped_to_named_input() {
    let _guard = container();

    let results = Concurrency::run(IndexMap::from([
        ("first", task(async { Ok(1 + 1) })),
        ("second", task(async { Ok(2 + 2) })),
    ]))
    .await
    .unwrap();

    assert_eq!(results["first"], 2);
    assert_eq!(results["second"], 4);

    let results = Concurrency::driver("sync")
        .unwrap()
        .run(IndexMap::from([
            ("first", task(async { Ok(1 + 1) })),
            ("second", task(async { Ok(2 + 2) })),
        ]))
        .await
        .unwrap();

    assert_eq!(
        results.keys().copied().collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}

#[tokio::test]
async fn tasks_may_have_different_types() {
    let _guard = container();

    let (count, name, flags) = Concurrency::run((
        async { Ok(42_u64) },
        async { Ok(String::from("Taylor")) },
        async { Ok(vec![true, false]) },
    ))
    .await
    .unwrap();

    assert_eq!(count, 42);
    assert_eq!(name, "Taylor");
    assert_eq!(flags, vec![true, false]);
}

#[tokio::test]
async fn the_first_failure_is_returned_after_every_task_finishes() {
    let _guard = container();
    let finished = Arc::new(AtomicUsize::new(0));

    let slow = finished.clone();
    let fast = finished.clone();
    let error = Concurrency::run(vec![
        task(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            slow.fetch_add(1, Ordering::SeqCst);
            Err(RuntimeException::new("This is the first exception").into())
        }),
        task(async move {
            fast.fetch_add(1, Ordering::SeqCst);
            Err(RuntimeException::new("This is a different exception").into())
        }),
        task(async { Ok(()) }),
    ])
    .await
    .unwrap_err();

    assert_eq!(error.to_string(), "This is the first exception");
    assert!(error.is::<RuntimeException>());
    assert_eq!(finished.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn errors_keep_their_type() {
    let _guard = container();

    #[derive(Debug, thiserror::Error)]
    #[error("API request to {url} failed with status {status}")]
    struct ApiException {
        url: String,
        status: u16,
    }

    let error = Concurrency::run((async {
        Err::<(), _>(
            ApiException {
                url: "https://api.example.com".into(),
                status: 400,
            }
            .into(),
        )
    },))
    .await
    .unwrap_err();

    let exception = error.downcast_ref::<ApiException>().unwrap();
    assert_eq!(exception.status, 400);
    assert_eq!(
        error.to_string(),
        "API request to https://api.example.com failed with status 400"
    );
}

#[tokio::test]
async fn panicking_tasks_fail_the_run() {
    let _guard = container();

    let error = Concurrency::run((async { Ok(1) }, async {
        if true {
            panic!("Something went terribly wrong.");
        }
        Ok(2)
    }))
    .await
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "A concurrent task panicked: Something went terribly wrong."
    );
}

#[tokio::test]
async fn tasks_can_time_out() {
    let _guard = container();
    let started = Instant::now();

    let error = Concurrency::run_with_timeout(
        (sleep_then(5_000, ()), sleep_then(10, ())),
        Duration::from_millis(100),
    )
    .await
    .unwrap_err();

    let exception = error.downcast_ref::<TaskTimedOutException>().unwrap();
    assert_eq!(exception.timeout, Duration::from_millis(100));
    assert_eq!(
        error.to_string(),
        "The concurrent tasks exceeded the timeout of 0.1 seconds."
    );
    assert!(started.elapsed() < Duration::from_secs(2));

    let (value,) = Concurrency::run_with_timeout((sleep_then(10, 5),), 1)
        .await
        .unwrap();
    assert_eq!(value, 5);
}

#[tokio::test]
async fn the_sync_driver_runs_tasks_in_sequence() {
    let _guard = container();
    let order = Arc::new(Mutex::new(Vec::new()));

    let first = order.clone();
    let second = order.clone();
    let started = Instant::now();
    Concurrency::driver("sync")
        .unwrap()
        .run((
            async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                first.lock().unwrap().push("first");
                Ok(())
            },
            async move {
                second.lock().unwrap().push("second");
                Ok(())
            },
        ))
        .await
        .unwrap();

    assert_eq!(*order.lock().unwrap(), vec!["first", "second"]);
    assert!(started.elapsed() >= Duration::from_millis(100));
}

#[tokio::test]
async fn the_sync_driver_stops_at_the_first_failure() {
    let _guard = container();
    let ran = Arc::new(AtomicUsize::new(0));
    let counter = ran.clone();

    let error = Concurrency::driver("sync")
        .unwrap()
        .run(vec![
            task(async { Err(RuntimeException::new("Stop.").into()) }),
            task(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        ])
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "Stop.");
    assert_eq!(ran.load(Ordering::SeqCst), 0);

    let error = Concurrency::driver("sync")
        .unwrap()
        .run_with_timeout((sleep_then(5_000, ()),), Duration::from_millis(50))
        .await
        .unwrap_err();
    assert!(error.is::<TaskTimedOutException>());
}

#[tokio::test]
async fn the_default_driver_is_configurable() {
    let (container, _guard) = container();
    container.instance(Repository::new(json!({"concurrency": {"default": "sync"}})));

    assert_eq!(Concurrency::manager().get_default_instance(), "sync");

    let order = Arc::new(Mutex::new(Vec::new()));
    let first = order.clone();
    let second = order.clone();
    Concurrency::run((
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            first.lock().unwrap().push(1);
            Ok(())
        },
        async move {
            second.lock().unwrap().push(2);
            Ok(())
        },
    ))
    .await
    .unwrap();

    assert_eq!(*order.lock().unwrap(), vec![1, 2]);
}

#[tokio::test]
async fn the_process_driver_is_not_supported() {
    let (container, _guard) = container();
    container.instance(Repository::new(
        json!({"concurrency": {"default": "process"}}),
    ));

    let error = Concurrency::run((async { Ok(1) },)).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("The [process] concurrency driver is not supported")
    );
    assert!(Concurrency::defer((async { Ok(()) },)).is_err());
}

#[tokio::test]
async fn custom_drivers_can_be_registered() {
    let _guard = container();
    Concurrency::extend("sequential", || Arc::new(SyncDriver));

    let results = Concurrency::driver("sequential")
        .unwrap()
        .run(vec![async { Ok("custom") }])
        .await
        .unwrap();

    assert_eq!(results, vec!["custom"]);
}

#[tokio::test]
async fn the_manager_is_resolved_without_a_provider() {
    let _guard = Container::set_local_instance(Arc::new(Container::new()));

    let (value,) = Concurrency::run((async { Ok("works") },)).await.unwrap();

    assert_eq!(value, "works");
    assert_eq!(Concurrency::manager().get_default_instance(), "tokio");
}

struct Marker(&'static str);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tasks_see_the_current_container() {
    let (container, _guard) = container();
    container.instance(Marker("from the test container"));

    let results = Concurrency::run(vec![
        task(async {
            tokio::task::yield_now().await;
            Ok(app::<Marker>().0)
        }),
        task(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok(app::<Marker>().0)
        }),
    ])
    .await
    .unwrap();

    assert_eq!(results, vec!["from the test container"; 2]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cpu_bound_tasks_run_in_parallel_on_a_multi_threaded_runtime() {
    let _guard = container();

    // Each task hogs its worker thread, so the others must run elsewhere.
    let busy = || async {
        let until = Instant::now() + Duration::from_millis(200);
        while Instant::now() < until {
            std::hint::spin_loop();
        }
        Ok(std::thread::current().id())
    };

    let (first, second, third) = Concurrency::run((busy(), busy(), busy())).await.unwrap();

    let mut threads = vec![first, second, third];
    threads.sort_by_key(|id| format!("{id:?}"));
    threads.dedup();
    assert!(threads.len() > 1);
}

#[tokio::test]
async fn processes_can_run_concurrently() {
    let _guard = container();
    let started = Instant::now();

    let (first, second) = Concurrency::run((
        async { Process::run("sleep 0.5; echo first").await },
        async { Process::run("sleep 0.5; echo second").await },
    ))
    .await
    .unwrap();

    assert_eq!(first.output(), "first\n");
    assert_eq!(second.output(), "second\n");
    assert!(started.elapsed() < Duration::from_millis(950));
}

// ----------------------------------------------------------------------
// Deferring tasks
// ----------------------------------------------------------------------

#[tokio::test]
async fn deferred_tasks_run_once_invoked() {
    let (container, _guard) = container();
    let reported = Arc::new(Mutex::new(Vec::new()));

    let users = reported.clone();
    let orders = reported.clone();
    Concurrency::defer((
        async move {
            users.lock().unwrap().push("users");
            Ok(())
        },
        async move {
            orders.lock().unwrap().push("orders");
            Ok(())
        },
    ))
    .unwrap();

    assert!(reported.lock().unwrap().is_empty());
    assert_eq!(container.make::<DeferredCallbacks>().len(), 1);

    let errors = DeferredCallbacks::current().invoke().await;

    assert!(errors.is_empty());
    let mut reported = reported.lock().unwrap().clone();
    reported.sort();
    assert_eq!(reported, vec!["orders", "users"]);
}

#[tokio::test]
async fn deferred_failures_are_collected() {
    let _guard = container();

    Concurrency::defer(
        (async { Err::<(), _>(RuntimeException::new("Metrics are down.").into()) },),
    )
    .unwrap();
    Concurrency::defer((async { Ok(()) },)).unwrap();

    let errors = DeferredCallbacks::current().invoke().await;

    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].to_string(), "Metrics are down.");
}

#[tokio::test]
async fn deferred_tasks_are_scoped_to_the_request() {
    let (container, _guard) = container();
    let request = Arc::new(DeferredCallbacks::new());

    request
        .clone()
        .scope(async {
            Concurrency::defer((async { Ok(()) },)).unwrap();
        })
        .await;

    assert_eq!(request.len(), 1);
    assert!(container.make::<DeferredCallbacks>().is_empty());
    assert!(request.invoke().await.is_empty());
}

#[tokio::test]
async fn the_manager_can_be_built_directly() {
    let manager = ConcurrencyManager::new();
    let (value,) = manager
        .driver(None)
        .unwrap()
        .run((async { Ok(7) },))
        .await
        .unwrap();

    assert_eq!(value, 7);
}
