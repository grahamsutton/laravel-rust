//! The `redis` driver, end to end against a real `redis-server` started for
//! this test binary: the queue's keys and Lua scripts, reservations,
//! releases, blocking pops, and the worker loop.

mod common;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{TestApp, app_with, record, recorded};
use illuminate_cache::Limit;
use illuminate_cache::facades::RateLimiter;
use illuminate_queue::contracts::Queue as QueueContract;
use illuminate_queue::events::JobQueued;
use illuminate_queue::middleware::{JobMiddleware, RateLimitedWithRedis, RateLimitsJobs};
use illuminate_queue::{
    Dispatchable, InteractsWithQueue, Queue, QueueManager, RedisQueue, ShouldQueue, Worker,
    WorkerOptions, async_trait,
};
use illuminate_redis::testing::RedisServer;
use illuminate_redis::{Connection, Redis};
use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Result, Value, json};
use serde::{Deserialize, Serialize};

// ----------------------------------------------------------------------
// Setup
// ----------------------------------------------------------------------

struct RedisApp {
    _app: TestApp,
    _server: Arc<RedisServer>,
}

/// Boot an application whose default queue connection is `redis`, on a
/// fresh database of the shared server, with keys prefixed by `prefix`.
fn app_with_prefix(prefix: &str, connection: Value) -> RedisApp {
    let server = RedisServer::shared();
    let mut config = common::config();
    let mut redis = server.config();
    redis["options"]["prefix"] = json!(prefix);
    config["database"] = json!({"redis": redis});
    config["queue"]["default"] = json!("redis");
    config["queue"]["connections"]["redis"] = connection;
    RedisApp {
        _app: app_with(config),
        _server: server,
    }
}

fn app() -> RedisApp {
    app_with_prefix(
        "",
        json!({
            "driver": "redis",
            "connection": "default",
            "queue": "default",
            "retry_after": 90,
            "block_for": null,
            "after_commit": false,
        }),
    )
}

/// The redis queue connection.
fn queue() -> Arc<dyn QueueContract> {
    Queue::connection("redis").unwrap()
}

/// The queue's Redis connection.
fn redis() -> Connection {
    Redis::connection(None).unwrap()
}

fn payload(uuid: &str) -> String {
    json!({"uuid": uuid, "displayName": "Test", "job": "x", "data": {}}).to_string()
}

fn decode(payload: &str) -> Value {
    serde_json::from_str(payload).unwrap()
}

/// Freeze "now" for this test's thread.
fn travel_to(timestamp: i64) {
    Carbon::set_thread_test_now(Some(Carbon::from_timestamp(timestamp)));
}

async fn work(queues: &str) -> u64 {
    let worker = Worker::make();
    worker
        .daemon(
            "redis",
            queues,
            &WorkerOptions::new().sleep(0.0).stop_when_empty(),
        )
        .await
        .unwrap();
    worker.jobs_processed()
}

// ----------------------------------------------------------------------
// Jobs
// ----------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct SendInvoice {
    id: u64,
}

#[async_trait]
impl ShouldQueue for SendInvoice {
    async fn handle(&self) -> Result<()> {
        record(format!("invoice:{} attempt:{}", self.id, self.attempts()));
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct FlakyJob;

#[async_trait]
impl ShouldQueue for FlakyJob {
    async fn handle(&self) -> Result<()> {
        record(format!("flaky attempt:{}", self.attempts()));
        if self.attempts() < 2 {
            return Err(RuntimeException::new("Try again").into());
        }
        Ok(())
    }

    fn tries(&self) -> Option<u32> {
        Some(3)
    }
}

#[derive(Serialize, Deserialize)]
struct ChargeCard {
    order_id: u64,
}

#[async_trait]
impl ShouldQueue for ChargeCard {
    async fn handle(&self) -> Result<()> {
        record(format!(
            "charge:{} attempt:{}",
            self.order_id,
            self.attempts()
        ));
        if !recorded().iter().any(|entry| entry == "gateway up") {
            return Err(RuntimeException::new("Gateway down").into());
        }
        Ok(())
    }
}

/// A job whose data survives being released: empty lists stay lists and
/// big numbers stay exact.
#[derive(Serialize, Deserialize)]
struct ImportContacts {
    contact_ids: Vec<u64>,
    account_id: u64,
    ratio: f64,
}

#[async_trait]
impl ShouldQueue for ImportContacts {
    async fn handle(&self) -> Result<()> {
        record(format!(
            "import {:?} {} {} attempt:{}",
            self.contact_ids,
            self.account_id,
            self.ratio,
            self.attempts()
        ));
        if self.attempts() < 3 {
            self.release(0).await?;
        }
        Ok(())
    }

    fn tries(&self) -> Option<u32> {
        Some(5)
    }
}

// ----------------------------------------------------------------------
// The driver
// ----------------------------------------------------------------------

#[tokio::test]
async fn the_redis_driver_is_built_from_configuration() {
    let _app = app();
    let manager = illuminate_container::app::<QueueManager>();
    let connection = manager.connection(Some("redis")).unwrap();
    assert_eq!(connection.connection_name(), "redis");
    assert_eq!(connection.default_queue(), "default");
    assert!(!connection.dispatches_after_commit());

    let queue = RedisQueue::from_config(
        &json!({
            "driver": "redis",
            "connection": "cache",
            "queue": "emails",
            "retry_after": 30,
            "block_for": 5,
            "after_commit": true,
            "migration_batch_size": 100,
        }),
        "custom",
    )
    .unwrap();
    assert_eq!(queue.connection_name(), "custom");
    assert_eq!(queue.default_queue(), "emails");
    assert_eq!(queue.get_retry_after(), Some(30));
    assert_eq!(queue.get_block_for(), Some(Duration::from_secs(5)));
    assert!(queue.dispatches_after_commit());
    assert_eq!(queue.get_connection().unwrap().name(), "cache");
    assert_eq!(queue.get_queue(None), "queues:emails");
    assert_eq!(queue.get_queue(Some("high")), "queues:high");

    let defaults = RedisQueue::from_config(&json!({"driver": "redis"}), "redis").unwrap();
    assert_eq!(defaults.get_retry_after(), Some(60));
    assert_eq!(defaults.get_block_for(), None);
    assert_eq!(defaults.default_queue(), "default");
    assert_eq!(defaults.get_connection().unwrap().name(), "default");
    assert!(format!("{defaults:?}").contains("RedisQueue"));

    let fractional = RedisQueue::from_config(&json!({"block_for": "0.5"}), "redis").unwrap();
    assert_eq!(fractional.get_block_for(), Some(Duration::from_millis(500)));
}

#[tokio::test]
async fn jobs_are_pushed_reserved_and_deleted() {
    let _app = app();
    travel_to(1_700_000_000);
    let queue = queue();
    let redis = redis();

    let id = queue
        .push_raw(payload("a"), Some("emails"), None)
        .await
        .unwrap()
        .unwrap();
    queue.push_raw(payload("b"), None, None).await.unwrap();

    // The job waits in the `queues:emails` list, with an id and its attempts...
    let waiting = redis.lrange("queues:emails", 0, -1).await.unwrap();
    assert_eq!(waiting.len(), 1);
    let pushed = decode(&waiting[0]);
    assert_eq!(pushed["id"], json!(id));
    assert_eq!(pushed["uuid"], json!("a"));
    assert_eq!(pushed["attempts"], json!(0));
    assert_eq!(id.len(), 32);
    // ...and a worker blocking on the queue is notified.
    assert_eq!(redis.llen("queues:emails:notify").await.unwrap(), 1);

    assert_eq!(queue.size(Some("emails")).await.unwrap(), 1);
    assert_eq!(queue.pending_size(Some("emails")).await.unwrap(), 1);

    travel_to(1_700_000_010);
    let job = queue.pop(Some("emails")).await.unwrap().unwrap();
    assert_eq!(job.job_id(), id);
    assert_eq!(job.uuid(), Some("a"));
    assert_eq!(job.attempts(), 1);
    assert_eq!(job.queue(), "emails");
    assert_eq!(job.connection_name(), "redis");
    assert_eq!(decode(job.raw_body())["attempts"], json!(0));

    // The reserved copy counts the attempt, scored by when it expires.
    let reserved = redis
        .query::<Vec<(String, i64)>>("zrange", ("queues:emails:reserved", 0, -1, "WITHSCORES"))
        .await
        .unwrap();
    assert_eq!(reserved.len(), 1);
    assert_eq!(decode(&reserved[0].0)["attempts"], json!(1));
    assert_eq!(decode(&reserved[0].0)["id"], json!(id));
    assert_eq!(reserved[0].1, 1_700_000_100);
    assert_eq!(redis.llen("queues:emails").await.unwrap(), 0);
    assert_eq!(redis.llen("queues:emails:notify").await.unwrap(), 0);

    assert_eq!(queue.reserved_size(Some("emails")).await.unwrap(), 1);
    assert_eq!(queue.pending_size(Some("emails")).await.unwrap(), 0);
    assert_eq!(queue.size(Some("emails")).await.unwrap(), 1);
    assert!(queue.pop(Some("emails")).await.unwrap().is_none());

    job.delete().await.unwrap();
    assert!(job.is_deleted());
    assert_eq!(queue.size(Some("emails")).await.unwrap(), 0);
    assert_eq!(queue.size(None).await.unwrap(), 1);
    Carbon::set_thread_test_now(None);
}

#[tokio::test]
async fn delayed_jobs_wait_for_their_delay() {
    let _app = app();
    travel_to(1_700_000_000);
    let queue = queue();
    let redis = redis();

    queue
        .push_raw(payload("later"), None, Some(Duration::from_secs(60)))
        .await
        .unwrap();
    queue.push_raw(payload("now"), None, None).await.unwrap();

    let delayed = redis
        .query::<Vec<(String, i64)>>("zrange", ("queues:default:delayed", 0, -1, "WITHSCORES"))
        .await
        .unwrap();
    assert_eq!(delayed[0].1, 1_700_000_060);
    assert_eq!(queue.delayed_size(None).await.unwrap(), 1);
    assert_eq!(queue.pending_size(None).await.unwrap(), 1);
    assert_eq!(queue.pop(None).await.unwrap().unwrap().uuid(), Some("now"));
    assert!(queue.pop(None).await.unwrap().is_none());

    travel_to(1_700_000_059);
    assert!(queue.pop(None).await.unwrap().is_none());
    travel_to(1_700_000_060);
    let job = queue.pop(None).await.unwrap().unwrap();
    assert_eq!(job.uuid(), Some("later"));
    assert_eq!(job.attempts(), 1);
    assert_eq!(queue.delayed_size(None).await.unwrap(), 0);
    Carbon::set_thread_test_now(None);
}

#[tokio::test]
async fn released_jobs_keep_their_attempts() {
    let _app = app();
    travel_to(1_700_000_000);
    let queue = queue();
    let redis = redis();
    queue
        .push_raw(payload("a"), Some("emails"), None)
        .await
        .unwrap();

    let job = queue.pop(Some("emails")).await.unwrap().unwrap();
    job.release(30).await.unwrap();
    assert!(job.is_released());

    // The reserved copy moved to the delayed queue, available in 30 seconds.
    let delayed = redis
        .query::<Vec<(String, i64)>>("zrange", ("queues:emails:delayed", 0, -1, "WITHSCORES"))
        .await
        .unwrap();
    assert_eq!(delayed.len(), 1);
    assert_eq!(decode(&delayed[0].0)["attempts"], json!(1));
    assert_eq!(delayed[0].1, 1_700_000_030);
    assert_eq!(queue.delayed_size(Some("emails")).await.unwrap(), 1);
    assert_eq!(queue.reserved_size(Some("emails")).await.unwrap(), 0);
    assert!(queue.pop(Some("emails")).await.unwrap().is_none());

    travel_to(1_700_000_030);
    let job = queue.pop(Some("emails")).await.unwrap().unwrap();
    assert_eq!(job.attempts(), 2);
    assert_eq!(job.uuid(), Some("a"));
    job.release(0).await.unwrap();

    let job = queue.pop(Some("emails")).await.unwrap().unwrap();
    assert_eq!(job.attempts(), 3);
    job.delete().await.unwrap();
    assert_eq!(queue.size(Some("emails")).await.unwrap(), 0);
    Carbon::set_thread_test_now(None);
}

#[tokio::test]
async fn expired_reservations_are_handed_to_the_next_worker() {
    let _app = app();
    travel_to(1_700_000_000);
    let queue = queue();
    queue.push_raw(payload("a"), None, None).await.unwrap();

    let first = queue.pop(None).await.unwrap().unwrap();
    assert_eq!(first.attempts(), 1);

    // The first worker is still within `retry_after` (90 seconds)...
    travel_to(1_700_000_089);
    assert!(queue.pop(None).await.unwrap().is_none());

    // ...until it isn't: the job is reserved again, counting the attempt.
    travel_to(1_700_000_090);
    let second = queue.pop(None).await.unwrap().unwrap();
    assert_eq!(second.job_id(), first.job_id());
    assert_eq!(second.attempts(), 2);
    assert_eq!(queue.reserved_size(None).await.unwrap(), 1);

    second.delete().await.unwrap();
    // Deleting a job that is already gone is fine.
    first.delete().await.unwrap();
    assert_eq!(queue.size(None).await.unwrap(), 0);
    Carbon::set_thread_test_now(None);
}

#[tokio::test]
async fn reservations_never_expire_without_retry_after() {
    let _app = app();
    travel_to(1_700_000_000);
    let queue = RedisQueue::new(Redis::manager().unwrap()).with_retry_after(None);
    queue.push_raw(payload("a"), None, None).await.unwrap();
    assert!(queue.pop(None).await.unwrap().is_some());

    travel_to(1_800_000_000);
    assert!(queue.pop(None).await.unwrap().is_none());
    assert_eq!(queue.reserved_size(None).await.unwrap(), 1);
    Carbon::set_thread_test_now(None);
}

#[tokio::test]
async fn sizes_payloads_and_clearing_queues() {
    let _app = app();
    travel_to(1_700_000_000);
    let queue = RedisQueue::new(Redis::manager().unwrap()).with_default_queue("low");

    queue.push_raw(payload("a"), None, None).await.unwrap();
    queue.push_raw(payload("b"), None, None).await.unwrap();
    queue
        .push_raw(payload("c"), None, Some(Duration::from_secs(10)))
        .await
        .unwrap();
    queue
        .push_raw(payload("d"), Some("high"), None)
        .await
        .unwrap();
    let _reserved = queue.pop(None).await.unwrap().unwrap();

    assert_eq!(queue.size(None).await.unwrap(), 3);
    assert_eq!(queue.pending_size(None).await.unwrap(), 1);
    assert_eq!(queue.delayed_size(None).await.unwrap(), 1);
    assert_eq!(queue.reserved_size(None).await.unwrap(), 1);
    assert_eq!(queue.queue_names().await.unwrap(), ["high", "low"]);
    assert_eq!(queue.total_size().await.unwrap(), 4);
    assert_eq!(queue.total_pending_size().await.unwrap(), 2);
    assert_eq!(queue.total_delayed_size().await.unwrap(), 1);
    assert_eq!(queue.total_reserved_size().await.unwrap(), 1);

    let uuids = |payloads: Vec<Value>| -> Vec<Value> {
        payloads
            .into_iter()
            .map(|payload| payload["uuid"].clone())
            .collect()
    };
    assert_eq!(uuids(queue.pending_jobs(None).await.unwrap()), [json!("b")]);
    assert_eq!(uuids(queue.delayed_jobs(None).await.unwrap()), [json!("c")]);
    assert_eq!(
        uuids(queue.reserved_jobs(None).await.unwrap()),
        [json!("a")]
    );
    assert_eq!(
        queue
            .creation_time_of_oldest_pending_job(None)
            .await
            .unwrap(),
        None
    );

    queue
        .push_raw(
            json!({"uuid": "e", "createdAt": 1_700_000_000, "data": {}}).to_string(),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        queue
            .creation_time_of_oldest_pending_job(Some("low"))
            .await
            .unwrap(),
        None,
        "the oldest pending job has no `createdAt`"
    );
    redis().lpop("queues:low").await.unwrap();
    assert_eq!(
        queue
            .creation_time_of_oldest_pending_job(None)
            .await
            .unwrap(),
        Some(1_700_000_000)
    );

    assert_eq!(queue.clear(None).await.unwrap(), 3);
    assert_eq!(queue.size(None).await.unwrap(), 0);
    assert_eq!(redis().exists("queues:low:notify").await.unwrap(), 0);
    assert_eq!(queue.size(Some("high")).await.unwrap(), 1);
    assert_eq!(queue.queue_names().await.unwrap(), ["high"]);
    Carbon::set_thread_test_now(None);
}

#[tokio::test]
async fn concurrent_pops_never_return_the_same_job() {
    let _app = app();
    let queue = queue();
    for index in 0..50 {
        queue
            .push_raw(payload(&format!("job-{index}")), None, None)
            .await
            .unwrap();
    }

    let mut workers = Vec::new();
    for _ in 0..8 {
        let queue = queue.clone();
        workers.push(tokio::spawn(async move {
            let mut popped = Vec::new();
            while let Some(job) = queue.pop(None).await.unwrap() {
                assert_eq!(job.attempts(), 1);
                popped.push(job.uuid().unwrap().to_string());
                job.delete().await.unwrap();
            }
            popped
        }));
    }

    let mut popped = Vec::new();
    for worker in workers {
        popped.extend(worker.await.unwrap());
    }
    let unique: HashSet<&String> = popped.iter().collect();
    assert_eq!(popped.len(), 50);
    assert_eq!(unique.len(), 50);
    assert_eq!(queue.size(None).await.unwrap(), 0);

    // Two pops racing for one job: exactly one wins.
    queue
        .push_raw(payload("contested"), None, None)
        .await
        .unwrap();
    let (first, second) = tokio::join!(queue.pop(None), queue.pop(None));
    let winners: Vec<_> = [first.unwrap(), second.unwrap()]
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(winners.len(), 1);
}

#[tokio::test]
async fn keys_carry_the_connection_prefix() {
    let _app = app_with_prefix(
        "laravel-database-",
        json!({"driver": "redis", "queue": "default"}),
    );
    let queue = queue();
    queue.push_raw(payload("a"), None, None).await.unwrap();

    let raw = Connection::new(
        "raw",
        illuminate_redis::ConnectionConfig {
            prefix: String::new(),
            ..redis().config().clone()
        },
    )
    .unwrap();
    assert_eq!(
        raw.llen("laravel-database-queues:default").await.unwrap(),
        1
    );
    assert_eq!(raw.llen("queues:default").await.unwrap(), 0);

    let job = queue.pop(None).await.unwrap().unwrap();
    assert_eq!(
        raw.zcard("laravel-database-queues:default:reserved")
            .await
            .unwrap(),
        1
    );
    job.delete().await.unwrap();
    assert_eq!(
        raw.zcard("laravel-database-queues:default:reserved")
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn pushed_jobs_carry_their_id() {
    let _app = app();
    let queued = Arc::new(Mutex::new(Vec::new()));
    let log = queued.clone();
    Queue::listen(move |event: &JobQueued| {
        log.lock()
            .unwrap()
            .push((event.id.clone(), event.payload.clone()));
    });

    SendInvoice { id: 1 }.dispatch().await.unwrap();
    SendInvoice { id: 2 }
        .dispatch()
        .delay(Duration::from_secs(30))
        .await
        .unwrap();

    let queued = queued.lock().unwrap().clone();
    assert_eq!(queued.len(), 2);
    for (id, payload) in &queued {
        let payload = decode(payload);
        assert_eq!(Some(payload["id"].as_str().unwrap().to_string()), *id);
        assert_eq!(payload["attempts"], json!(0));
    }

    let waiting = redis().lrange("queues:default", 0, -1).await.unwrap();
    assert_eq!(
        decode(&waiting[0])["id"],
        json!(queued[0].0.clone().unwrap())
    );
    assert!(waiting[0].ends_with(r#""attempts":0}"#));
    assert_eq!(Queue::size(None).await.unwrap(), 2);
}

#[tokio::test]
async fn unreadable_payloads_are_failed_instead_of_lost() {
    let _app = app();
    let queue = queue();
    redis().rpush("queues:default", "not json").await.unwrap();

    assert!(queue.pop(None).await.is_err());
    assert_eq!(queue.size(None).await.unwrap(), 0);

    let failed = Queue::failed_jobs().await.unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].payload, "not json");
    assert_eq!(failed[0].connection, "redis");
}

// ----------------------------------------------------------------------
// Blocking pops
// ----------------------------------------------------------------------

#[tokio::test]
async fn block_for_waits_for_jobs_to_arrive() {
    let _app = app();
    let queue =
        RedisQueue::new(Redis::manager().unwrap()).with_block_for(Some(Duration::from_secs(5)));

    let pusher = {
        let queue = queue.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            queue.push_raw(payload("late"), None, None).await.unwrap();
        })
    };

    let started = Instant::now();
    let job = queue.pop(None).await.unwrap().unwrap();
    assert_eq!(job.uuid(), Some("late"));
    assert!(started.elapsed() >= Duration::from_millis(150));
    assert!(started.elapsed() < Duration::from_secs(4));
    pusher.await.unwrap();

    // Nothing arrives: the pop gives up after `block_for`.
    let queue = queue.with_block_for(Some(Duration::from_millis(200)));
    let started = Instant::now();
    assert!(queue.pop(None).await.unwrap().is_none());
    assert!(started.elapsed() >= Duration::from_millis(150));
}

#[tokio::test]
async fn only_the_first_queue_blocks() {
    let _app = app();
    let queue =
        RedisQueue::new(Redis::manager().unwrap()).with_block_for(Some(Duration::from_millis(500)));
    for uuid in ["a", "b"] {
        queue
            .push_raw(payload(uuid), Some("low"), None)
            .await
            .unwrap();
    }

    // The empty primary queue blocks, the secondary one never does...
    let started = Instant::now();
    assert!(queue.pop_at(Some("high"), 0).await.unwrap().is_none());
    assert!(started.elapsed() >= Duration::from_millis(400));
    assert_eq!(
        queue.pop_at(Some("low"), 1).await.unwrap().unwrap().uuid(),
        Some("a")
    );

    // ...and while the secondary queue has jobs, the primary doesn't block.
    let started = Instant::now();
    assert!(queue.pop_at(Some("high"), 0).await.unwrap().is_none());
    assert!(started.elapsed() < Duration::from_millis(300));
    assert_eq!(
        queue.pop_at(Some("low"), 1).await.unwrap().unwrap().uuid(),
        Some("b")
    );
}

// ----------------------------------------------------------------------
// The worker
// ----------------------------------------------------------------------

#[tokio::test]
async fn workers_process_jobs_from_redis() {
    let _app = app();

    SendInvoice { id: 1 }.dispatch().await.unwrap();
    SendInvoice { id: 2 }.dispatch().await.unwrap();
    assert_eq!(Queue::size(None).await.unwrap(), 2);

    assert_eq!(work("default").await, 2);
    assert_eq!(
        recorded(),
        vec!["invoice:1 attempt:1", "invoice:2 attempt:1"]
    );
    assert_eq!(Queue::size(None).await.unwrap(), 0);
}

#[tokio::test]
async fn workers_process_queues_in_priority_order() {
    let _app = app();

    for (id, queue) in [(1, "low"), (2, "high"), (3, "low"), (4, "high")] {
        SendInvoice { id }.dispatch().on_queue(queue).await.unwrap();
    }

    assert_eq!(work("high,low").await, 4);
    assert_eq!(
        recorded(),
        vec![
            "invoice:2 attempt:1",
            "invoice:4 attempt:1",
            "invoice:1 attempt:1",
            "invoice:3 attempt:1",
        ]
    );
}

#[tokio::test]
async fn delayed_jobs_are_processed_once_due() {
    let _app = app();

    SendInvoice { id: 1 }
        .dispatch()
        .delay(Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(
        Queue::connection("redis")
            .unwrap()
            .delayed_size(None)
            .await
            .unwrap(),
        1
    );
    assert_eq!(work("default").await, 0, "not due yet");

    let started = Instant::now();
    while work("default").await == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "never processed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(recorded(), vec!["invoice:1 attempt:1"]);
}

#[tokio::test]
async fn failing_jobs_are_released_until_they_succeed() {
    let _app = app();

    FlakyJob.dispatch().await.unwrap();
    work("default").await;

    assert_eq!(recorded(), vec!["flaky attempt:1", "flaky attempt:2"]);
    assert_eq!(Queue::size(None).await.unwrap(), 0);
    assert!(Queue::failed_jobs().await.unwrap().is_empty());
}

#[tokio::test]
async fn job_data_survives_being_released() {
    let _app = app();

    ImportContacts {
        contact_ids: Vec::new(),
        account_id: 123_456_789_012_345_678,
        ratio: 0.1,
    }
    .dispatch()
    .await
    .unwrap();
    work("default").await;

    assert_eq!(
        recorded(),
        vec![
            "import [] 123456789012345678 0.1 attempt:1",
            "import [] 123456789012345678 0.1 attempt:2",
            "import [] 123456789012345678 0.1 attempt:3",
        ]
    );
    assert!(Queue::failed_jobs().await.unwrap().is_empty());
}

#[tokio::test]
async fn failed_jobs_are_logged_and_can_be_retried() {
    let _app = app();

    ChargeCard { order_id: 1 }.dispatch().await.unwrap();
    ChargeCard { order_id: 2 }
        .dispatch()
        .on_queue("payments")
        .await
        .unwrap();
    work("default,payments").await;
    assert_eq!(Queue::size(None).await.unwrap(), 0);
    assert_eq!(Queue::size(Some("payments")).await.unwrap(), 0);

    let failed = Queue::failed_jobs().await.unwrap();
    assert_eq!(failed.len(), 2);
    assert!(failed.iter().all(|job| job.connection == "redis"));
    assert!(failed.iter().any(|job| job.queue == "payments"));
    let default = failed.iter().find(|job| job.queue == "default").unwrap();
    assert_eq!(default.exception_message(), "Gateway down");

    // `queue:retry` pushes the job back onto its queue, attempts reset.
    record("gateway up");
    assert!(Queue::retry_failed(&default.id).await.unwrap());
    let waiting = redis().lrange("queues:default", 0, -1).await.unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(decode(&waiting[0])["attempts"], json!(0));

    work("default,payments").await;
    assert_eq!(
        recorded(),
        vec![
            "charge:1 attempt:1",
            "charge:2 attempt:1",
            "gateway up",
            "charge:1 attempt:1",
        ]
    );
    assert_eq!(Queue::failed_jobs().await.unwrap().len(), 1);
}

#[tokio::test]
async fn bulk_pushes_every_job() {
    let _app = app();
    let queue = queue();
    let jobs = vec![
        illuminate_queue::Envelope::new(SendInvoice { id: 1 }),
        illuminate_queue::Envelope::new(SendInvoice { id: 2 }),
    ];
    queue.bulk(&jobs, Some("invoices")).await.unwrap();
    assert_eq!(queue.size(Some("invoices")).await.unwrap(), 2);
    assert_eq!(work("invoices").await, 2);
}

#[tokio::test]
async fn workers_wait_for_jobs_with_block_for() {
    let _app = app_with_prefix(
        "",
        json!({"driver": "redis", "queue": "default", "block_for": 2}),
    );

    let dispatcher = tokio::spawn(illuminate_container::Container::scope_current(async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        SendInvoice { id: 7 }.dispatch().await.unwrap();
    }));

    let started = Instant::now();
    let worker = Worker::make();
    assert!(
        worker
            .work_once("redis", "default", &WorkerOptions::new().sleep(0.0))
            .await
            .unwrap()
    );
    assert!(started.elapsed() < Duration::from_millis(1900));
    dispatcher.await.unwrap();
    assert_eq!(recorded(), vec!["invoice:7 attempt:1"]);
}

#[derive(Serialize, Deserialize)]
struct Backup {
    user_id: u64,
    strict: bool,
}

#[async_trait]
impl ShouldQueue for Backup {
    async fn handle(&self) -> Result<()> {
        record(format!("backup:{}", self.user_id));
        Ok(())
    }

    fn middleware(&self) -> Vec<Arc<dyn JobMiddleware>> {
        let limited = RateLimitedWithRedis::new("backups");
        if self.strict {
            vec![Arc::new(limited.dont_release())]
        } else {
            vec![Arc::new(limited)]
        }
    }
}

#[tokio::test]
async fn jobs_are_rate_limited_with_redis() {
    let _app = app();
    RateLimiter::for_job("backups", |job: &Backup| {
        if job.user_id == 0 {
            Limit::none()
        } else {
            Limit::per_minute(1).by(job.user_id)
        }
    });

    for (user_id, strict) in [
        (1, false),
        (1, false),
        (2, false),
        (0, false),
        (0, false),
        (2, true),
    ] {
        Backup { user_id, strict }.dispatch().await.unwrap();
    }
    assert_eq!(work("default").await, 6);

    // One backup per user per minute: the second one for user 1 is released
    // until the window resets, the strict one for user 2 is dropped.
    assert_eq!(
        recorded(),
        vec!["backup:1", "backup:2", "backup:0", "backup:0"]
    );
    let delayed = redis()
        .query::<Vec<(String, i64)>>("zrange", ("queues:default:delayed", 0, -1, "WITHSCORES"))
        .await
        .unwrap();
    assert_eq!(delayed.len(), 1);
    let wait = delayed[0].1 - Carbon::now().timestamp();
    assert!((60..=64).contains(&wait), "{wait}");
    assert_eq!(Queue::size(None).await.unwrap(), 1);
    assert!(Queue::failed_jobs().await.unwrap().is_empty());
}
