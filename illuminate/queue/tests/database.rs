//! The database driver, the `database-uuids` failed job provider and the
//! database batch repository, end to end on an in-memory SQLite database.

mod common;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use common::{TestApp, app_with, record, recorded};
use illuminate_container::ServiceProvider;
use illuminate_database::{Connection, DatabaseManager, DatabaseServiceProvider};
use illuminate_queue::contracts::Queue as QueueContract;
use illuminate_queue::{
    Batch, BatchRepository, Bus, DatabaseBatchRepository, DatabaseQueue,
    DatabaseUuidFailedJobProvider, Dispatchable, FailedJobProvider, InteractsWithQueue, Queue,
    QueueManager, ShouldQueue, Worker, WorkerOptions, async_trait,
};
use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Result, Value, json};
use serde::{Deserialize, Serialize};

// ----------------------------------------------------------------------
// Setup
// ----------------------------------------------------------------------

fn config() -> Value {
    json!({
        "database": {
            "default": "sqlite",
            "connections": {"sqlite": {"driver": "sqlite", "database": ":memory:"}},
        },
        "queue": {
            "default": "database",
            "connections": {
                "database": {
                    "driver": "database",
                    "connection": null,
                    "table": "jobs",
                    "queue": "default",
                    "retry_after": 90,
                    "after_commit": false,
                },
                "sync": {"driver": "sync"},
            },
            "batching": {"database": "sqlite", "table": "job_batches"},
            "failed": {"driver": "database-uuids", "database": "sqlite", "table": "failed_jobs"},
        },
        "cache": {
            "default": "array",
            "stores": {"array": {"driver": "array", "serialize": false}},
        },
    })
}

/// Create the tables of Laravel's `create_jobs_table` migration.
async fn create_tables(connection: &Connection) {
    let schema = connection.get_schema_builder();
    schema
        .create("jobs", |table| {
            table.id();
            table.string("queue").index();
            table.long_text("payload");
            table.unsigned_small_integer("attempts");
            table.unsigned_integer("reserved_at").nullable();
            table.unsigned_integer("available_at");
            table.unsigned_integer("created_at");
        })
        .await
        .unwrap();
    schema
        .create("job_batches", |table| {
            table.string("id").primary();
            table.string("name");
            table.integer("total_jobs");
            table.integer("pending_jobs");
            table.integer("failed_jobs");
            table.long_text("failed_job_ids");
            table.medium_text("options").nullable();
            table.integer("cancelled_at").nullable();
            table.integer("created_at");
            table.integer("finished_at").nullable();
        })
        .await
        .unwrap();
    schema
        .create("failed_jobs", |table| {
            table.id();
            table.string("uuid").unique();
            table.string("connection");
            table.string("queue");
            table.long_text("payload");
            table.long_text("exception");
            table.timestamp("failed_at").use_current();
            table.index(["connection", "queue", "failed_at"]);
        })
        .await
        .unwrap();
}

/// Boot an application whose queue, failed jobs and batches live in an
/// in-memory SQLite database.
async fn app() -> TestApp {
    let app = app_with(config());
    DatabaseServiceProvider.register(&app.container);
    create_tables(&db()).await;
    app
}

fn db() -> Connection {
    DatabaseManager::resolve().connection("sqlite")
}

/// The database queue connection.
fn queue() -> Arc<dyn QueueContract> {
    Queue::connection("database").unwrap()
}

async fn jobs() -> Vec<Value> {
    db().table("jobs")
        .order_by("id", "asc")
        .get()
        .await
        .unwrap()
        .all()
        .to_vec()
}

fn payload(uuid: &str) -> String {
    json!({"uuid": uuid, "displayName": "Test", "job": "x", "data": {}}).to_string()
}

/// Freeze "now" for this test's thread.
fn travel_to(timestamp: i64) {
    Carbon::set_thread_test_now(Some(Carbon::from_timestamp(timestamp)));
}

async fn work(queues: &str) -> u64 {
    let worker = Worker::make();
    worker
        .daemon(
            "database",
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
struct ImportCsv {
    chunk: u64,
    fail: bool,
}

#[async_trait]
impl ShouldQueue for ImportCsv {
    async fn handle(&self) -> Result<()> {
        let batch = self.batch().await?.expect("the job belongs to a batch");
        record(format!("import:{} ({})", self.chunk, batch.name));
        if self.fail {
            return Err(RuntimeException::new(format!("chunk {} failed", self.chunk)).into());
        }
        Ok(())
    }
}

// ----------------------------------------------------------------------
// The driver
// ----------------------------------------------------------------------

#[tokio::test]
async fn the_database_driver_is_built_from_configuration() {
    let _app = app().await;
    let manager = illuminate_container::app::<QueueManager>();
    let connection = manager.connection(Some("database")).unwrap();
    assert_eq!(connection.connection_name(), "database");
    assert_eq!(connection.default_queue(), "default");
    assert!(!connection.dispatches_after_commit());

    let queue = DatabaseQueue::from_config(
        &json!({"driver": "database", "connection": "sqlite", "table": "jobs", "retry_after": 30, "queue": "emails", "after_commit": true}),
        "custom",
    );
    assert_eq!(queue.connection_name(), "custom");
    assert_eq!(queue.default_queue(), "emails");
    assert_eq!(queue.get_retry_after(), 30);
    assert_eq!(queue.get_table(), "jobs");
    assert!(queue.dispatches_after_commit());
    assert!(queue.get_database().same_as(&db()));

    let defaults = DatabaseQueue::from_config(&json!({"driver": "database"}), "database");
    assert_eq!(defaults.get_table(), "jobs");
    assert_eq!(defaults.get_retry_after(), 90);
    assert_eq!(defaults.default_queue(), "default");
}

#[tokio::test]
async fn jobs_are_pushed_reserved_and_deleted() {
    let _app = app().await;
    travel_to(1_700_000_000);
    let queue = queue();

    let id = queue
        .push_raw(payload("a"), Some("emails"), None)
        .await
        .unwrap()
        .unwrap();
    queue.push_raw(payload("b"), None, None).await.unwrap();

    let rows = jobs().await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["id"].to_string(), id);
    assert_eq!(rows[0]["queue"], json!("emails"));
    assert_eq!(rows[0]["attempts"], json!(0));
    assert_eq!(rows[0]["reserved_at"], Value::Null);
    assert_eq!(rows[0]["available_at"], json!(1_700_000_000));
    assert_eq!(rows[0]["created_at"], json!(1_700_000_000));
    assert_eq!(rows[1]["queue"], json!("default"));

    assert_eq!(queue.size(Some("emails")).await.unwrap(), 1);
    assert_eq!(queue.pending_size(Some("emails")).await.unwrap(), 1);

    travel_to(1_700_000_010);
    let job = queue.pop(Some("emails")).await.unwrap().unwrap();
    assert_eq!(job.job_id(), id);
    assert_eq!(job.uuid(), Some("a"));
    assert_eq!(job.attempts(), 1);
    assert_eq!(job.queue(), "emails");
    assert_eq!(job.connection_name(), "database");

    let row = &jobs().await[0];
    assert_eq!(row["reserved_at"], json!(1_700_000_010));
    assert_eq!(row["attempts"], json!(1));
    assert_eq!(queue.reserved_size(Some("emails")).await.unwrap(), 1);
    assert_eq!(queue.pending_size(Some("emails")).await.unwrap(), 0);
    assert!(queue.pop(Some("emails")).await.unwrap().is_none());

    job.delete().await.unwrap();
    assert!(job.is_deleted());
    assert_eq!(queue.size(Some("emails")).await.unwrap(), 0);
    assert_eq!(queue.size(None).await.unwrap(), 1);
    Carbon::set_thread_test_now(None);
}

#[tokio::test]
async fn delayed_jobs_wait_for_their_delay() {
    let _app = app().await;
    travel_to(1_700_000_000);
    let queue = queue();

    queue
        .push_raw(payload("later"), None, Some(Duration::from_secs(60)))
        .await
        .unwrap();
    queue.push_raw(payload("now"), None, None).await.unwrap();

    assert_eq!(jobs().await[0]["available_at"], json!(1_700_000_060));
    assert_eq!(queue.delayed_size(None).await.unwrap(), 1);
    assert_eq!(queue.pending_size(None).await.unwrap(), 1);
    assert_eq!(queue.pop(None).await.unwrap().unwrap().uuid(), Some("now"));
    assert!(queue.pop(None).await.unwrap().is_none());

    travel_to(1_700_000_059);
    assert!(queue.pop(None).await.unwrap().is_none());
    travel_to(1_700_000_060);
    assert_eq!(
        queue.pop(None).await.unwrap().unwrap().uuid(),
        Some("later")
    );
    Carbon::set_thread_test_now(None);
}

#[tokio::test]
async fn released_jobs_are_pushed_back_with_their_attempts() {
    let _app = app().await;
    travel_to(1_700_000_000);
    let queue = queue();
    queue
        .push_raw(payload("a"), Some("emails"), None)
        .await
        .unwrap();

    let job = queue.pop(Some("emails")).await.unwrap().unwrap();
    job.release(30).await.unwrap();
    assert!(job.is_released());

    // The reserved row was replaced by a fresh one, waiting 30 seconds.
    let rows = jobs().await;
    assert_eq!(rows.len(), 1);
    assert_ne!(rows[0]["id"].to_string(), job.job_id());
    assert_eq!(rows[0]["attempts"], json!(1));
    assert_eq!(rows[0]["reserved_at"], Value::Null);
    assert_eq!(rows[0]["available_at"], json!(1_700_000_030));
    assert_eq!(rows[0]["queue"], json!("emails"));
    assert_eq!(queue.delayed_size(Some("emails")).await.unwrap(), 1);
    assert_eq!(queue.reserved_size(Some("emails")).await.unwrap(), 0);
    assert!(queue.pop(Some("emails")).await.unwrap().is_none());

    travel_to(1_700_000_030);
    let job = queue.pop(Some("emails")).await.unwrap().unwrap();
    assert_eq!(job.attempts(), 2);
    assert_eq!(job.uuid(), Some("a"));
    job.delete().await.unwrap();
    assert_eq!(queue.size(Some("emails")).await.unwrap(), 0);
    Carbon::set_thread_test_now(None);
}

#[tokio::test]
async fn expired_reservations_are_handed_to_the_next_worker() {
    let _app = app().await;
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
    assert_eq!(jobs().await[0]["reserved_at"], json!(1_700_000_090));

    second.delete().await.unwrap();
    // Deleting a job that is already gone is fine.
    first.delete().await.unwrap();
    assert_eq!(queue.size(None).await.unwrap(), 0);
    Carbon::set_thread_test_now(None);
}

#[tokio::test]
async fn sizes_and_clearing_queues() {
    let _app = app().await;
    travel_to(1_700_000_000);
    let queue = DatabaseQueue::new(db(), "jobs").with_default_queue("low");

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
    assert_eq!(queue.total_size().await.unwrap(), 4);
    assert_eq!(queue.total_pending_size().await.unwrap(), 2);
    assert_eq!(queue.total_delayed_size().await.unwrap(), 1);
    assert_eq!(queue.total_reserved_size().await.unwrap(), 1);
    assert_eq!(
        queue
            .creation_time_of_oldest_pending_job(None)
            .await
            .unwrap(),
        Some(1_700_000_000)
    );
    let uuids: Vec<Value> = queue
        .payloads(None)
        .await
        .unwrap()
        .into_iter()
        .map(|payload| payload["uuid"].clone())
        .collect();
    assert_eq!(uuids, vec![json!("a"), json!("b"), json!("c")]);

    assert_eq!(queue.clear(None).await.unwrap(), 3);
    assert_eq!(queue.size(None).await.unwrap(), 0);
    assert_eq!(queue.size(Some("high")).await.unwrap(), 1);
    assert_eq!(
        queue
            .creation_time_of_oldest_pending_job(None)
            .await
            .unwrap(),
        None
    );
    Carbon::set_thread_test_now(None);
}

#[tokio::test]
async fn concurrent_pops_never_return_the_same_job() {
    let _app = app().await;
    let queue = queue();
    for uuid in ["a", "b", "c"] {
        queue.push_raw(payload(uuid), None, None).await.unwrap();
    }

    let (first, second, third, fourth) = tokio::join!(
        queue.pop(None),
        queue.pop(None),
        queue.pop(None),
        queue.pop(None)
    );
    let popped: Vec<_> = [first, second, third, fourth]
        .into_iter()
        .filter_map(|job| job.unwrap())
        .collect();
    let ids: HashSet<&str> = popped.iter().map(|job| job.job_id()).collect();
    assert_eq!(popped.len(), 3);
    assert_eq!(ids.len(), 3);
    assert!(popped.iter().all(|job| job.attempts() == 1));
    assert_eq!(queue.reserved_size(None).await.unwrap(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workers_on_separate_connections_share_the_jobs() {
    // A file database lets every worker hold its own connection; immediate
    // transactions make SQLite serialize them.
    let directory = tempfile::tempdir().unwrap();
    let connection = Connection::new(
        "sqlite",
        json!({
            "driver": "sqlite",
            "database": directory.path().join("queue.sqlite").to_string_lossy(),
            "transaction_mode": "IMMEDIATE",
            "busy_timeout": 10_000,
        }),
    );
    create_tables(&connection).await;
    let queue = DatabaseQueue::new(connection, "jobs");
    for index in 0..24 {
        queue
            .push_raw(payload(&format!("job-{index}")), None, None)
            .await
            .unwrap();
    }

    let mut workers = Vec::new();
    for _ in 0..4 {
        let queue = queue.clone();
        workers.push(tokio::spawn(async move {
            let mut processed = Vec::new();
            while let Some(job) = queue.pop(None).await.unwrap() {
                processed.push(job.uuid().unwrap().to_string());
                job.delete().await.unwrap();
            }
            processed
        }));
    }

    let mut processed = Vec::new();
    for worker in workers {
        processed.extend(worker.await.unwrap());
    }
    let unique: HashSet<&String> = processed.iter().collect();
    assert_eq!(processed.len(), 24);
    assert_eq!(unique.len(), 24);
    assert_eq!(queue.size(None).await.unwrap(), 0);
}

#[tokio::test]
async fn unreadable_payloads_are_failed() {
    let _app = app().await;
    let queue = queue();
    queue
        .push_raw("not json".to_string(), None, None)
        .await
        .unwrap();

    assert!(queue.pop(None).await.is_err());
    assert_eq!(queue.size(None).await.unwrap(), 0);

    let failed = Queue::failed_jobs().await.unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].payload, "not json");
    assert_eq!(failed[0].connection, "database");
}

// ----------------------------------------------------------------------
// The worker
// ----------------------------------------------------------------------

#[tokio::test]
async fn workers_process_jobs_from_the_database() {
    let _app = app().await;

    SendInvoice { id: 1 }.dispatch().await.unwrap();
    SendInvoice { id: 2 }.dispatch().await.unwrap();
    assert_eq!(Queue::size(None).await.unwrap(), 2);

    assert_eq!(work("default").await, 2);
    assert_eq!(
        recorded(),
        vec!["invoice:1 attempt:1", "invoice:2 attempt:1"]
    );
    assert!(jobs().await.is_empty());
}

#[tokio::test]
async fn workers_process_queues_in_priority_order() {
    let _app = app().await;

    SendInvoice { id: 1 }
        .dispatch()
        .on_queue("low")
        .await
        .unwrap();
    SendInvoice { id: 2 }
        .dispatch()
        .on_queue("high")
        .await
        .unwrap();
    SendInvoice { id: 3 }
        .dispatch()
        .on_queue("low")
        .await
        .unwrap();
    SendInvoice { id: 4 }
        .dispatch()
        .on_queue("high")
        .await
        .unwrap();

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
async fn failing_jobs_are_released_until_they_succeed() {
    let _app = app().await;

    FlakyJob.dispatch().await.unwrap();
    work("default").await;

    assert_eq!(recorded(), vec!["flaky attempt:1", "flaky attempt:2"]);
    assert!(jobs().await.is_empty());
    assert!(Queue::failed_jobs().await.unwrap().is_empty());
}

#[tokio::test]
async fn failed_jobs_are_logged_to_the_database_and_can_be_retried() {
    let _app = app().await;
    assert!(
        Queue::failer()
            .count(None, None)
            .await
            .is_ok_and(|count| count == 0)
    );

    ChargeCard { order_id: 1 }.dispatch().await.unwrap();
    ChargeCard { order_id: 2 }
        .dispatch()
        .on_queue("payments")
        .await
        .unwrap();
    work("default,payments").await;
    assert!(jobs().await.is_empty());

    // The failures are rows of the `failed_jobs` table...
    let rows = db()
        .table("failed_jobs")
        .order_by("id", "asc")
        .get()
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["connection"], json!("database"));
    assert_eq!(rows[0]["queue"], json!("default"));
    assert!(
        rows[0]["exception"]
            .as_str()
            .unwrap()
            .contains("Gateway down")
    );

    // ...listed newest first, identified by the job's UUID.
    let failed = Queue::failed_jobs().await.unwrap();
    assert_eq!(failed[0].queue, "payments");
    assert_eq!(failed[1].queue, "default");
    assert_eq!(failed[1].id, rows[0]["uuid"].as_str().unwrap());
    assert_eq!(failed[1].exception_message(), "Gateway down");
    assert_eq!(
        Queue::failer().count(Some("database"), None).await.unwrap(),
        2
    );

    // `queue:retry` pushes the job back onto its queue, attempts reset.
    record("gateway up");
    let id = failed[1].id.clone();
    assert!(Queue::retry_failed(&id).await.unwrap());
    assert!(Queue::find_failed(&id).await.unwrap().is_none());
    let rows = jobs().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["queue"], json!("default"));
    assert_eq!(rows[0]["attempts"], json!(0));

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

    // `queue:retry all` and `queue:flush`.
    assert_eq!(
        Queue::retry_all_failed(Some("payments")).await.unwrap(),
        vec![failed[0].id.clone()]
    );
    work("payments").await;
    assert!(Queue::failed_jobs().await.unwrap().is_empty());
    assert!(jobs().await.is_empty());
}

#[tokio::test]
async fn the_failer_is_the_database_provider() {
    let _app = app().await;
    let failer = Queue::failer();
    failer
        .log("database", "default", &payload("logged"), "Exception: boom")
        .await
        .unwrap();
    let row = db()
        .table("failed_jobs")
        .where_("uuid", "logged")
        .first()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row["exception"], json!("Exception: boom"));

    let custom = DatabaseUuidFailedJobProvider::new(db(), "failed_jobs");
    assert_eq!(custom.ids(None).await.unwrap(), vec!["logged"]);
    assert_eq!(custom.table_name(), "failed_jobs");
}

// ----------------------------------------------------------------------
// Batches
// ----------------------------------------------------------------------

#[tokio::test]
async fn batches_are_stored_in_the_database_and_progressed_by_the_worker() {
    let _app = app().await;

    let batch = Bus::batch(vec![
        Box::new(ImportCsv {
            chunk: 1,
            fail: false,
        }),
        Box::new(ImportCsv {
            chunk: 2,
            fail: true,
        }),
        Box::new(ImportCsv {
            chunk: 3,
            fail: false,
        }),
    ])
    .name("Import CSV")
    .allow_failures()
    .then(|batch: Batch| async move {
        record(format!("then:{}", batch.processed_jobs()));
        Ok(())
    })
    .finally(|batch: Batch| async move {
        record(format!(
            "finally:{}/{}",
            batch.pending_jobs, batch.failed_jobs
        ));
        Ok(())
    })
    .dispatch()
    .await
    .unwrap();

    // The batch is a row of `job_batches`, its jobs rows of `jobs`.
    let row = db()
        .table("job_batches")
        .where_("id", batch.id.as_str())
        .first()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row["name"], json!("Import CSV"));
    assert_eq!(row["total_jobs"], json!(3));
    assert_eq!(row["pending_jobs"], json!(3));
    assert_eq!(row["failed_job_ids"], json!("[]"));
    assert!(row["options"].as_str().unwrap().starts_with('{'));
    assert_eq!(jobs().await.len(), 3);

    work("default").await;

    assert_eq!(
        recorded(),
        vec![
            "import:1 (Import CSV)",
            "import:2 (Import CSV)",
            "import:3 (Import CSV)",
            "finally:1/1",
        ]
    );

    let batch = Bus::find_batch(&batch.id).await.unwrap().unwrap();
    assert_eq!(batch.total_jobs, 3);
    assert_eq!(batch.pending_jobs, 1);
    assert_eq!(batch.failed_jobs, 1);
    assert_eq!(batch.failed_job_ids.len(), 1);
    assert!(batch.has_failures());
    // Every job ran once, but the failed one is still pending a retry.
    assert!(!batch.finished());

    let failed = Queue::find_failed(&batch.failed_job_ids[0])
        .await
        .unwrap()
        .unwrap();
    assert!(failed.exception.contains("chunk 2 failed"));

    let row = db()
        .table("job_batches")
        .where_("id", batch.id.as_str())
        .first()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row["finished_at"], Value::Null);
    assert_eq!(row["failed_jobs"], json!(1));
    assert_eq!(
        row["failed_job_ids"],
        json!(serde_json::to_string(&batch.failed_job_ids).unwrap())
    );
}

#[tokio::test]
async fn a_failed_job_cancels_its_database_batch() {
    let _app = app().await;

    let batch = Bus::batch(vec![
        Box::new(ImportCsv {
            chunk: 1,
            fail: true,
        }),
        Box::new(ImportCsv {
            chunk: 2,
            fail: false,
        }),
    ])
    .catch(
        |_batch: Batch, error: Arc<illuminate_support::Error>| async move {
            record(format!("catch:{error}"));
            Ok(())
        },
    )
    .dispatch()
    .await
    .unwrap();

    work("default").await;

    let batch = Bus::find_batch(&batch.id).await.unwrap().unwrap();
    assert!(batch.cancelled());
    assert_eq!(batch.failed_jobs, 1);
    assert!(recorded().contains(&"catch:chunk 1 failed".to_string()));
}

#[tokio::test]
async fn the_batch_repository_is_bound_from_configuration() {
    let _app = app().await;
    let repository = illuminate_container::app::<dyn BatchRepository>();
    let batch = repository.store("Direct", &json!({})).await.unwrap();
    assert!(
        db().table("job_batches")
            .where_("id", batch.id.as_str())
            .exists()
            .await
            .unwrap()
    );

    // Without batching configuration, batches stay in memory.
    let mut config = config();
    config["queue"].as_object_mut().unwrap().remove("batching");
    let memory = app_with(config);
    DatabaseServiceProvider.register(&memory.container);
    let repository = illuminate_container::app::<dyn BatchRepository>();
    // This database has no `job_batches` table, so only memory can store it.
    assert!(repository.store("Memory", &json!({})).await.is_ok());
    drop(memory);

    let explicit = DatabaseBatchRepository::new(db(), "job_batches");
    assert_eq!(explicit.table_name(), "job_batches");
}
