//! Dispatching jobs: payloads, connections, queues, delays, the sync,
//! deferred and background drivers, and closures.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::{app_default, record, recorded};
use illuminate_container::app;
use illuminate_queue::{
    Bus, CALL_QUEUED_HANDLER, CallQueuedClosure, DeferredCallbacks, Dispatchable, Envelope,
    JobRegistry, Queue, QueueManager, SerializedJob, ShouldQueue, TransactionCallback,
    TransactionManager, async_trait, create_payload, dispatch, dispatch_closure, dispatch_sync,
};
use illuminate_support::{Error, Result, Value, json};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ProcessPodcast {
    podcast_id: u64,
}

#[async_trait]
impl ShouldQueue for ProcessPodcast {
    async fn handle(&self) -> Result<()> {
        record(format!("processed:{}", self.podcast_id));
        Ok(())
    }

    fn tries(&self) -> Option<u32> {
        Some(3)
    }

    fn backoff(&self) -> Vec<u64> {
        vec![1, 5, 10]
    }

    fn timeout(&self) -> Option<u64> {
        Some(120)
    }

    fn max_exceptions(&self) -> Option<u32> {
        Some(2)
    }

    fn job_name() -> &'static str {
        "App\\Jobs\\ProcessPodcast"
    }
}

#[derive(Serialize, Deserialize)]
struct EmailsJob;

#[async_trait]
impl ShouldQueue for EmailsJob {
    async fn handle(&self) -> Result<()> {
        record("emails");
        Ok(())
    }

    fn queue(&self) -> Option<String> {
        Some("emails".into())
    }

    fn connection(&self) -> Option<String> {
        Some("secondary".into())
    }
}

#[derive(Serialize, Deserialize)]
struct ExplodingJob;

#[async_trait]
impl ShouldQueue for ExplodingJob {
    async fn handle(&self) -> Result<()> {
        Err(illuminate_support::error::RuntimeException::new("Boom!").into())
    }

    async fn failed(&self, error: &Error) -> Result<()> {
        record(format!("failed:{error}"));
        Ok(())
    }
}

async fn size(connection: &str, queue: &str) -> u64 {
    Queue::connection(connection)
        .unwrap()
        .size(Some(queue))
        .await
        .unwrap()
}

#[tokio::test]
async fn jobs_are_pushed_onto_the_default_connection() {
    let _app = app_default();

    ProcessPodcast { podcast_id: 1 }.dispatch().await.unwrap();

    assert_eq!(size("array", "default").await, 1);
    assert!(recorded().is_empty());
}

#[tokio::test]
async fn payloads_look_like_laravels() {
    let _app = app_default();
    ProcessPodcast { podcast_id: 9 }.dispatch().await.unwrap();

    let job = Queue::pop(None).await.unwrap().unwrap();
    let payload = job.payload();

    let keys: Vec<&str> = payload
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec![
            "uuid",
            "displayName",
            "job",
            "maxTries",
            "maxExceptions",
            "failOnTimeout",
            "backoff",
            "timeout",
            "retryUntil",
            "data",
            "createdAt",
            "delay",
            "attempts"
        ]
    );
    assert_eq!(payload["displayName"], json!("ProcessPodcast"));
    assert_eq!(payload["job"], json!(CALL_QUEUED_HANDLER));
    assert_eq!(payload["maxTries"], json!(3));
    assert_eq!(payload["maxExceptions"], json!(2));
    assert_eq!(payload["failOnTimeout"], json!(false));
    assert_eq!(payload["backoff"], json!("1,5,10"));
    assert_eq!(payload["timeout"], json!(120));
    assert_eq!(payload["retryUntil"], Value::Null);
    assert_eq!(payload["delay"], Value::Null);
    assert_eq!(payload["attempts"], json!(0));
    assert_eq!(
        payload["data"],
        json!({"commandName": "App\\Jobs\\ProcessPodcast", "command": {"podcast_id": 9}, "batchId": null})
    );
    assert_eq!(job.uuid().unwrap().len(), 36);
    assert_eq!(job.max_tries(), Some(3));
    assert_eq!(job.backoff().as_deref(), Some("1,5,10"));
    assert_eq!(job.timeout(), Some(120));
    assert_eq!(job.resolve_name(), "ProcessPodcast");
    assert_eq!(job.command_name(), Some("App\\Jobs\\ProcessPodcast"));
}

#[tokio::test]
async fn payloads_round_trip_through_the_registry() {
    let _app = app_default();

    let envelope = Envelope::new(ProcessPodcast { podcast_id: 4 })
        .on_queue("podcasts")
        .delay(30);
    let payload = create_payload(
        &envelope,
        "array",
        "podcasts",
        Some(Duration::from_secs(30)),
    )
    .unwrap();
    let payload: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["delay"], json!(30));

    let data: SerializedJob = serde_json::from_value(payload["data"].clone()).unwrap();
    assert_eq!(data.queue.as_deref(), Some("podcasts"));

    let restored = Envelope::from_serialized(data).unwrap();
    assert_eq!(
        restored.downcast_ref::<ProcessPodcast>(),
        Some(&ProcessPodcast { podcast_id: 4 })
    );
    assert_eq!(restored.queue_name(), Some("podcasts"));
    assert_eq!(restored.get_delay(), Some(Duration::from_secs(30)));
    assert!(JobRegistry::has("App\\Jobs\\ProcessPodcast"));
}

#[tokio::test]
async fn payload_hooks_add_keys() {
    let _app = app_default();
    Queue::create_payload_using(|connection, queue, _payload| {
        let mut extra = illuminate_support::Map::new();
        extra.insert("tenant".into(), json!(format!("{connection}:{queue}")));
        extra
    });

    ProcessPodcast { podcast_id: 1 }.dispatch().await.unwrap();

    let job = Queue::pop(None).await.unwrap().unwrap();
    assert_eq!(job.payload()["tenant"], json!("array:default"));
}

#[tokio::test]
async fn jobs_may_choose_their_queue_and_connection() {
    let _app = app_default();

    EmailsJob.dispatch().await.unwrap();
    ProcessPodcast { podcast_id: 1 }
        .dispatch()
        .on_connection("secondary")
        .on_queue("podcasts")
        .await
        .unwrap();
    // The dispatch overrides the job's own defaults.
    EmailsJob.dispatch().on_queue("priority").await.unwrap();

    assert_eq!(size("secondary", "emails").await, 1);
    assert_eq!(size("secondary", "podcasts").await, 1);
    assert_eq!(size("secondary", "priority").await, 1);
    assert_eq!(size("array", "default").await, 0);
}

#[tokio::test]
async fn jobs_may_be_routed_and_forwarded() {
    let _app = app_default();

    Queue::route::<ProcessPodcast>("podcasts", "secondary");
    Queue::forward("reports", "reports-fifo", None);

    ProcessPodcast { podcast_id: 1 }.dispatch().await.unwrap();
    EmailsJob.dispatch().on_queue("reports").await.unwrap();

    assert_eq!(size("secondary", "podcasts").await, 1);
    assert_eq!(size("secondary", "reports-fifo").await, 1);
    assert_eq!(size("secondary", "reports").await, 0);
}

#[tokio::test]
async fn delayed_jobs_wait() {
    let _app = app_default();

    ProcessPodcast { podcast_id: 1 }
        .dispatch()
        .delay(Duration::from_millis(80))
        .await
        .unwrap();

    let queue = Queue::default_connection().unwrap();
    assert_eq!(queue.delayed_size(None).await.unwrap(), 1);
    assert!(queue.pop(None).await.unwrap().is_none());

    tokio::time::sleep(Duration::from_millis(100)).await;
    let job = queue.pop(None).await.unwrap().unwrap();
    assert_eq!(job.payload()["delay"], json!(1));
}

#[tokio::test]
async fn conditional_dispatching() {
    let _app = app_default();

    ProcessPodcast { podcast_id: 1 }
        .dispatch_if(false)
        .await
        .unwrap();
    ProcessPodcast { podcast_id: 2 }
        .dispatch_unless(true)
        .await
        .unwrap();
    ProcessPodcast { podcast_id: 3 }
        .dispatch_if(true)
        .await
        .unwrap();
    ProcessPodcast { podcast_id: 4 }
        .dispatch_unless(false)
        .await
        .unwrap();

    assert_eq!(size("array", "default").await, 2);
}

#[tokio::test]
async fn jobs_can_be_dispatched_synchronously() {
    let _app = app_default();

    ProcessPodcast { podcast_id: 1 }
        .dispatch_sync()
        .await
        .unwrap();
    dispatch_sync(ProcessPodcast { podcast_id: 2 })
        .await
        .unwrap();
    Bus::dispatch_sync(ProcessPodcast { podcast_id: 3 })
        .await
        .unwrap();
    dispatch(ProcessPodcast { podcast_id: 4 })
        .on_connection("sync")
        .await
        .unwrap();

    assert_eq!(
        recorded(),
        vec!["processed:1", "processed:2", "processed:3", "processed:4"]
    );
    assert_eq!(size("array", "default").await, 0);
}

#[tokio::test]
async fn sync_failures_call_the_failed_hook_and_propagate() {
    let app = app_default();
    let failures = Arc::new(AtomicUsize::new(0));
    let counter = failures.clone();
    Queue::failing(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    let error = ExplodingJob.dispatch_sync().await.unwrap_err();

    assert_eq!(error.to_string(), "Boom!");
    assert_eq!(recorded(), vec!["failed:Boom!"]);
    assert_eq!(failures.load(Ordering::SeqCst), 1);
    // Synchronous failures are not stored with the failed jobs.
    assert!(Queue::failed_jobs().await.unwrap().is_empty());
    drop(app);
}

#[tokio::test]
async fn sync_is_the_default_without_configuration() {
    let _app = common::app_with(json!({}));

    ProcessPodcast { podcast_id: 5 }.dispatch().await.unwrap();

    assert_eq!(recorded(), vec!["processed:5"]);
    assert_eq!(app::<QueueManager>().get_default_driver(), "sync");
}

#[tokio::test]
async fn unconfigured_connections_are_reported() {
    let _app = app_default();

    let error = ProcessPodcast { podcast_id: 1 }
        .dispatch()
        .on_connection("redis")
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "The [redis] queue connection has not been configured."
    );

    let mut config = common::config();
    config["queue"]["connections"]["database"] = json!({"driver": "database"});
    let _app = common::app_with(config);
    let error = Queue::connection("database").err().unwrap();
    assert_eq!(error.to_string(), "No connector for [database].");
}

#[tokio::test]
async fn the_null_connection_discards_jobs() {
    let _app = app_default();

    ProcessPodcast { podcast_id: 1 }
        .dispatch()
        .on_connection("null")
        .await
        .unwrap();

    assert!(recorded().is_empty());
    assert_eq!(size("null", "default").await, 0);
}

#[tokio::test]
async fn jobs_can_run_after_the_response() {
    let _app = app_default();

    ProcessPodcast { podcast_id: 1 }
        .dispatch_after_response()
        .await
        .unwrap();
    ProcessPodcast { podcast_id: 2 }
        .dispatch()
        .on_connection("deferred")
        .await
        .unwrap();
    Bus::dispatch_after_response(ProcessPodcast { podcast_id: 3 })
        .await
        .unwrap();

    assert!(recorded().is_empty());
    assert_eq!(DeferredCallbacks::current().len(), 3);

    // The HTTP kernel drains the callbacks once the response is sent.
    DeferredCallbacks::current().invoke().await;

    assert_eq!(
        recorded(),
        vec!["processed:1", "processed:2", "processed:3"]
    );
    assert!(DeferredCallbacks::current().is_empty());
}

#[tokio::test]
async fn after_response_jobs_are_attached_to_the_current_request() {
    let _app = app_default();
    let request = illuminate_http::Request::default();

    illuminate_http::with_request(request.clone(), async {
        ProcessPodcast { podcast_id: 1 }
            .dispatch_after_response()
            .await
            .unwrap();
    })
    .await;

    assert!(DeferredCallbacks::current().is_empty());
    let deferred = request.extension::<DeferredCallbacks>().unwrap();
    assert_eq!(deferred.len(), 1);
    deferred.invoke().await;
    assert_eq!(recorded(), vec!["processed:1"]);
}

#[tokio::test]
async fn background_jobs_run_on_their_own() {
    let _app = app_default();

    ProcessPodcast { podcast_id: 7 }
        .dispatch()
        .on_connection("background")
        .await
        .unwrap();

    for _ in 0..50 {
        if !recorded().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(recorded(), vec!["processed:7"]);
}

#[tokio::test]
async fn closures_can_be_queued() {
    let _app = app_default();

    dispatch_closure(|| async {
        record("closure ran");
        Ok(())
    })
    .on_connection("sync")
    .await
    .unwrap();

    let error = CallQueuedClosure::new(|| async {
        Err(illuminate_support::error::RuntimeException::new("closure failed").into())
    })
    .name("Publish Podcast")
    .catch(|error| async move {
        record(format!("caught:{error}"));
        Ok(())
    })
    .dispatch_sync()
    .await
    .unwrap_err();

    assert_eq!(error.to_string(), "closure failed");
    assert_eq!(recorded(), vec!["closure ran", "caught:closure failed"]);
}

#[tokio::test]
async fn closures_on_the_queue_carry_their_name() {
    let _app = app_default();

    CallQueuedClosure::new(|| async { Ok(()) })
        .name("Publish Podcast")
        .dispatch()
        .await
        .unwrap();

    let job = Queue::pop(None).await.unwrap().unwrap();
    assert_eq!(job.resolve_name(), "Publish Podcast");
    job.fire().await.unwrap();
    assert!(job.is_deleted());
}

#[derive(Default)]
struct FakeTransactions {
    open: std::sync::atomic::AtomicBool,
    callbacks: std::sync::Mutex<Vec<TransactionCallback>>,
}

impl TransactionManager for FakeTransactions {
    fn in_transaction(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    fn add_callback(&self, callback: TransactionCallback) {
        self.callbacks.lock().unwrap().push(callback);
    }

    fn add_callback_for_rollback(&self, _callback: TransactionCallback) {}
}

#[tokio::test]
async fn jobs_can_wait_for_open_transactions_to_commit() {
    let app = app_default();
    let transactions = Arc::new(FakeTransactions::default());
    app.container
        .instance_arc::<dyn TransactionManager>(transactions.clone());

    // No transaction is open: dispatched immediately.
    ProcessPodcast { podcast_id: 1 }
        .dispatch()
        .after_commit()
        .await
        .unwrap();
    assert_eq!(size("array", "default").await, 1);

    transactions.open.store(true, Ordering::SeqCst);
    ProcessPodcast { podcast_id: 2 }
        .dispatch()
        .after_commit()
        .await
        .unwrap();
    ProcessPodcast { podcast_id: 3 }.dispatch().await.unwrap();
    assert_eq!(size("array", "default").await, 2);

    // The transaction commits...
    let callbacks = std::mem::take(&mut *transactions.callbacks.lock().unwrap());
    assert_eq!(callbacks.len(), 1);
    for callback in callbacks {
        callback().await;
    }
    assert_eq!(size("array", "default").await, 3);
}

#[tokio::test]
async fn queue_facade_pushes_jobs_directly() {
    let _app = app_default();

    Queue::push(ProcessPodcast { podcast_id: 1 }).await.unwrap();
    Queue::push_on("podcasts", ProcessPodcast { podcast_id: 2 })
        .await
        .unwrap();
    Queue::later(60, ProcessPodcast { podcast_id: 3 })
        .await
        .unwrap();
    Queue::later_on("podcasts", 60, ProcessPodcast { podcast_id: 4 })
        .await
        .unwrap();
    Queue::bulk(
        vec![
            Box::new(ProcessPodcast { podcast_id: 5 }),
            Box::new(EmailsJob),
        ],
        Some("bulk"),
    )
    .await
    .unwrap();

    assert_eq!(size("array", "default").await, 2);
    assert_eq!(size("array", "podcasts").await, 2);
    assert_eq!(size("array", "bulk").await, 2);
    assert_eq!(Queue::clear(Some("bulk")).await.unwrap(), 2);
}

#[tokio::test]
async fn bus_bulk_groups_jobs_by_connection_and_queue() {
    let _app = app_default();

    Bus::bulk(vec![
        Box::new(ProcessPodcast { podcast_id: 1 }),
        Box::new(EmailsJob),
        Box::new(EmailsJob),
    ])
    .await
    .unwrap();

    assert_eq!(size("array", "default").await, 1);
    assert_eq!(size("secondary", "emails").await, 2);
}

#[tokio::test]
async fn once_closures_run_a_single_time() {
    let _app = app_default();
    let event = String::from("OrderShipped");

    CallQueuedClosure::once(move || async move {
        record(format!("handled:{event}"));
        Err(illuminate_support::error::RuntimeException::new("listener failed").into())
    })
    .dispatch()
    .await
    .unwrap();

    illuminate_queue::Worker::make()
        .daemon(
            "array",
            "default",
            &illuminate_queue::WorkerOptions::new()
                .sleep(0.0)
                .stop_when_empty()
                .tries(2),
        )
        .await
        .unwrap();

    // The retry finds the closure already used.
    assert_eq!(recorded(), vec!["handled:OrderShipped"]);
    let failed = Queue::failed_jobs().await.unwrap();
    assert!(failed[0].exception.contains("may only run once"));
}
