use std::sync::atomic::{AtomicUsize, Ordering};

use illuminate_foundation::Application;
use illuminate_foundation::testing::TestApp;
use illuminate_queue::{Dispatchable, Queue, ShouldQueue, async_trait};
use illuminate_routing::Route;
use illuminate_support::{Result, error::bail};
use serde::{Deserialize, Serialize};

static PROCESSED: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProcessPodcast {
    id: u64,
}

illuminate_queue::register_job!(ProcessPodcast);

#[async_trait]
impl ShouldQueue for ProcessPodcast {
    async fn handle(&self) -> Result<()> {
        PROCESSED.fetch_add(self.id as usize, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FailingJob;

illuminate_queue::register_job!(FailingJob);

#[async_trait]
impl ShouldQueue for FailingJob {
    async fn handle(&self) -> Result<()> {
        bail!("The podcast could not be processed.")
    }
}

fn test_app(driver: &str) -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let builder = Application::configure_detached(dir.path()).with_routing(|routing| {
        routing.web(|| {
            Route::get("/podcasts/{id}", |illuminate_routing::Path(id): illuminate_routing::Path<u64>| async move {
                ProcessPodcast { id }.dispatch_after_response().await?;
                Ok::<_, illuminate_support::Error>("Queued")
            });
        });
    });
    let app = TestApp::new(builder);
    app.app().override_config("queue.default", driver);
    app.app().override_config("queue.connections.array", illuminate_support::json!({"driver": "array"}));
    (app, dir)
}

#[tokio::test]
async fn jobs_run_after_the_response_is_sent() {
    let (mut app, _dir) = test_app("sync");
    let before = PROCESSED.load(Ordering::SeqCst);

    app.get("/podcasts/1000").await.assert_ok().assert_see("Queued");

    assert!(PROCESSED.load(Ordering::SeqCst) >= before + 1000);
}

/// The `failed_jobs` table from Laravel's default jobs migration.
async fn create_failed_jobs_table() {
    illuminate_database::Schema::create("failed_jobs", |table| {
        table.id();
        table.string("uuid").unique();
        table.string("connection");
        table.string("queue");
        table.long_text("payload");
        table.long_text("exception");
        table.timestamp("failed_at").use_current();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn workers_process_queued_jobs() {
    let (app, _dir) = test_app("array");
    create_failed_jobs_table().await;

    ProcessPodcast { id: 7 }.dispatch().await.unwrap();
    assert_eq!(Queue::size(None).await.unwrap(), 1);

    app.artisan("queue:work --once")
        .expects_output_to_contain("ProcessPodcast")
        .expects_output_to_contain("RUNNING")
        .expects_output_to_contain("DONE")
        .assert_successful()
        .await;
    assert_eq!(Queue::size(None).await.unwrap(), 0);

    FailingJob.dispatch().await.unwrap();
    app.artisan("queue:work --once")
        .expects_output_to_contain("FailingJob")
        .expects_output_to_contain("FAIL")
        .assert_successful()
        .await;

    let failed = Queue::failed_jobs().await.unwrap();
    assert_eq!(failed.len(), 1);
    let id = failed[0].id.clone();

    app.artisan("queue:failed")
        .expects_output_to_contain(&id)
        .expects_output_to_contain("FailingJob")
        .assert_successful()
        .await;

    app.artisan("queue:retry all")
        .expects_output_to_contain("Pushing failed queue jobs back onto the queue.")
        .assert_successful()
        .await;
    assert_eq!(Queue::size(None).await.unwrap(), 1);
    assert!(Queue::failed_jobs().await.unwrap().is_empty());

    app.artisan("queue:clear --force")
        .expects_output_to_contain("Cleared 1 job from the [default] queue.")
        .assert_successful()
        .await;

    app.artisan("queue:forget missing-id")
        .expects_output_to_contain("No failed job matches the given ID [missing-id].")
        .assert_failed()
        .await;
}

#[tokio::test]
async fn jobs_can_be_generated() {
    let (app, dir) = test_app("sync");

    app.artisan("make:job ProcessPodcast")
        .expects_output_to_contain("Job [app/jobs/process_podcast.rs] created successfully.")
        .assert_successful()
        .await;

    let job = std::fs::read_to_string(dir.path().join("app/jobs/process_podcast.rs")).unwrap();
    assert!(job.contains("impl ShouldQueue for ProcessPodcast"));
    assert!(job.contains("laravel::register_job!(ProcessPodcast);"));
}

static SHIPPED: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone)]
struct OrderShipped {
    order_id: usize,
}

struct SendShipmentNotification;

#[async_trait]
impl illuminate_events::Listener<OrderShipped> for SendShipmentNotification {
    async fn handle(&self, event: &OrderShipped) -> Result<()> {
        SHIPPED.fetch_add(event.order_id, Ordering::SeqCst);
        Ok(())
    }
}

impl illuminate_events::ShouldQueue for SendShipmentNotification {}

#[tokio::test]
async fn queued_listeners_are_pushed_onto_the_queue() {
    let (app, _dir) = test_app("array");
    illuminate_events::Event::listen_queued(SendShipmentNotification);

    illuminate_events::Event::dispatch(OrderShipped { order_id: 42 }).await.unwrap();

    assert_eq!(Queue::size(None).await.unwrap(), 1);
    assert_eq!(SHIPPED.load(Ordering::SeqCst), 0);

    app.artisan("queue:work --once")
        .expects_output_to_contain("SendShipmentNotification")
        .assert_successful()
        .await;

    assert_eq!(SHIPPED.load(Ordering::SeqCst), 42);
}

#[tokio::test]
async fn after_commit_jobs_wait_for_the_transaction() {
    let (_app, _dir) = test_app("array");
    use illuminate_database::DB;

    DB::transaction(|| async {
        ProcessPodcast { id: 1 }.dispatch().after_commit().await?;
        assert_eq!(Queue::size(None).await?, 0, "the job waits for the commit");
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(Queue::size(None).await.unwrap(), 1);

    let result: Result<()> = DB::transaction(|| async {
        ProcessPodcast { id: 2 }.dispatch().after_commit().await?;
        bail!("Something went wrong.")
    })
    .await;
    assert!(result.is_err());
    assert_eq!(Queue::size(None).await.unwrap(), 1, "rolled back jobs are never pushed");

    // Outside a transaction, the job is pushed right away.
    ProcessPodcast { id: 3 }.dispatch().after_commit().await.unwrap();
    assert_eq!(Queue::size(None).await.unwrap(), 2);
}

#[tokio::test]
async fn batches_can_be_retried_and_pruned() {
    let (app, _dir) = test_app("array");
    illuminate_database::Schema::create("job_batches", |table| {
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

    app.artisan("queue:retry-batch missing-batch")
        .expects_output_to_contain("Unable to find a batch with ID [missing-batch].")
        .assert_failed()
        .await;

    app.artisan("queue:prune-batches --unfinished=72")
        .expects_output_to_contain("0 entries deleted.")
        .expects_output_to_contain("0 unfinished entries deleted.")
        .assert_successful()
        .await;
}

static SEEN_TRACE: std::sync::Mutex<Option<illuminate_support::Value>> = std::sync::Mutex::new(None);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TraceJob;

illuminate_queue::register_job!(TraceJob);

#[async_trait]
impl ShouldQueue for TraceJob {
    async fn handle(&self) -> Result<()> {
        let job = illuminate_queue::current_job().unwrap();
        assert_eq!(job.payload()["illuminate:log:context"]["data"]["trace_id"], "8f5b0a");
        *SEEN_TRACE.lock().unwrap() = illuminate_log::Context::get("trace_id");
        Ok(())
    }
}

#[tokio::test]
async fn context_travels_with_queued_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let builder = Application::configure_detached(dir.path()).with_routing(|routing| {
        routing.web(|| {
            Route::get("/import", || async {
                illuminate_log::Context::add("trace_id", "8f5b0a");
                TraceJob.dispatch().await?;
                Ok::<_, illuminate_support::Error>("Queued")
            });
        });
    });
    let mut app = TestApp::new(builder);
    app.app().override_config("queue.default", "array");
    app.app().override_config("queue.connections.array", illuminate_support::json!({"driver": "array"}));

    app.get("/import").await.assert_ok();
    assert!(illuminate_log::Context::missing("trace_id"), "the request's context stays with the request");
    app.artisan("queue:work --once").assert_successful().await;

    assert_eq!(*SEEN_TRACE.lock().unwrap(), Some(illuminate_support::json!("8f5b0a")));
    assert!(illuminate_log::Context::missing("trace_id"), "the worker forgets the job's context");
}

#[tokio::test]
async fn scheduled_events_lock_through_the_cache() {
    use illuminate_console::scheduling::{EventMutex, SchedulingMutex};
    use illuminate_foundation::scheduling::{CacheEventMutex, CacheSchedulingMutex};

    let (_app, _dir) = test_app("array");
    let event = illuminate_console::Schedule::command("inspire").without_overlapping();

    assert!(CacheEventMutex.create(&event).await);
    assert!(!CacheEventMutex.create(&event).await, "a second run overlaps");
    assert!(CacheEventMutex.exists(&event).await);
    CacheEventMutex.forget(&event).await;
    assert!(!CacheEventMutex.exists(&event).await);

    let now = illuminate_support::Carbon::now();
    assert!(CacheSchedulingMutex.create(&event, &now).await);
    assert!(!CacheSchedulingMutex.create(&event, &now).await, "another server already ran it this minute");
}

#[tokio::test]
async fn jobs_can_be_scheduled() {
    use illuminate_foundation::scheduling::ScheduleJobs;

    let (app, _dir) = test_app("array");
    illuminate_console::Schedule::job(ProcessPodcast { id: 5 }).every_minute();

    app.artisan("schedule:run")
        .expects_output_to_contain("ProcessPodcast")
        .assert_successful()
        .await;

    assert_eq!(Queue::size(None).await.unwrap(), 1);
}
