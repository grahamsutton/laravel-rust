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

#[tokio::test]
async fn workers_process_queued_jobs() {
    let (app, _dir) = test_app("array");

    ProcessPodcast { id: 7 }.dispatch().await.unwrap();
    assert_eq!(Queue::size(None).await.unwrap(), 1);

    app.artisan("queue:work --once")
        .expects_output_to_contain("ProcessPodcast")
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
