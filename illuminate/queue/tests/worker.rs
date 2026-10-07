//! The queue worker: attempts, backoff, failures, timeouts and stopping.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{app_default, record, record_events, recorded};
use illuminate_queue::events::{
    JobFailed, JobProcessed, JobReleasedAfterException, WorkerStopping,
};
use illuminate_queue::{
    Dispatchable, InteractsWithQueue, MaxAttemptsExceededException, Queue, ShouldQueue,
    TimeoutExceededException, UnknownJobException, Worker, WorkerOptions, WorkerStopReason,
    async_trait,
};
use illuminate_support::{Carbon, Error, Result, json};
use serde::{Deserialize, Serialize};

fn options() -> WorkerOptions {
    WorkerOptions::new().sleep(0.0).stop_when_empty()
}

async fn work() -> Worker {
    let worker = Worker::make();
    worker.daemon("array", "default", &options()).await.unwrap();
    worker
}

#[derive(Serialize, Deserialize)]
struct SendReport {
    id: u64,
}

#[async_trait]
impl ShouldQueue for SendReport {
    async fn handle(&self) -> Result<()> {
        record(format!("report:{}", self.id));
        Ok(())
    }
}

/// Fails until it has been attempted `succeed_on` times.
#[derive(Serialize, Deserialize)]
struct Flaky {
    succeed_on: u32,
    tries: Option<u32>,
    backoff: Vec<u64>,
}

#[async_trait]
impl ShouldQueue for Flaky {
    async fn handle(&self) -> Result<()> {
        let attempt = self.attempts();
        record(format!("attempt:{attempt}"));
        if attempt < self.succeed_on {
            return Err(illuminate_support::error::RuntimeException::new(format!(
                "Attempt {attempt} failed"
            ))
            .into());
        }
        Ok(())
    }

    async fn failed(&self, error: &Error) -> Result<()> {
        record(format!("failed:{error}"));
        Ok(())
    }

    fn tries(&self) -> Option<u32> {
        self.tries
    }

    fn backoff(&self) -> Vec<u64> {
        self.backoff.clone()
    }
}

#[tokio::test]
async fn the_worker_processes_jobs_until_the_queue_is_empty() {
    let _app = app_default();
    let events = record_events();

    SendReport { id: 1 }.dispatch().await.unwrap();
    SendReport { id: 2 }.dispatch().await.unwrap();

    let worker = Worker::make();
    let reason = worker.daemon("array", "default", &options()).await.unwrap();

    assert_eq!(reason, WorkerStopReason::QueueEmpty);
    assert_eq!(reason.exit_code(), 0);
    assert_eq!(worker.jobs_processed(), 2);
    assert_eq!(recorded(), vec!["report:1", "report:2"]);
    assert_eq!(Queue::size(None).await.unwrap(), 0);

    let events = events.lock().unwrap().clone();
    let worker_events: Vec<&str> = events
        .iter()
        .map(String::as_str)
        .filter(|event| !event.starts_with("JobQueu"))
        .collect();
    assert_eq!(
        worker_events,
        vec![
            "WorkerStarting",
            "Looping",
            "JobPopping",
            "JobPopped",
            "JobProcessing",
            "JobProcessed",
            "JobAttempted",
            "Looping",
            "JobPopping",
            "JobPopped",
            "JobProcessing",
            "JobProcessed",
            "JobAttempted",
            "Looping",
            "JobPopping",
            "WorkerIdle",
            "WorkerStopping",
        ]
    );
}

#[tokio::test]
async fn queues_are_worked_in_priority_order() {
    let _app = app_default();

    SendReport { id: 1 }
        .dispatch()
        .on_queue("low")
        .await
        .unwrap();
    SendReport { id: 2 }
        .dispatch()
        .on_queue("high")
        .await
        .unwrap();
    SendReport { id: 3 }
        .dispatch()
        .on_queue("low")
        .await
        .unwrap();
    SendReport { id: 4 }
        .dispatch()
        .on_queue("high")
        .await
        .unwrap();

    Worker::make()
        .daemon("array", "high,low", &options())
        .await
        .unwrap();

    assert_eq!(
        recorded(),
        vec!["report:2", "report:4", "report:1", "report:3"]
    );
}

#[tokio::test]
async fn failing_jobs_are_retried_until_they_succeed() {
    let _app = app_default();

    Flaky {
        succeed_on: 3,
        tries: Some(3),
        backoff: vec![],
    }
    .dispatch()
    .await
    .unwrap();

    work().await;

    assert_eq!(recorded(), vec!["attempt:1", "attempt:2", "attempt:3"]);
    assert!(Queue::failed_jobs().await.unwrap().is_empty());
}

#[tokio::test]
async fn jobs_that_run_out_of_attempts_are_failed_and_logged() {
    let _app = app_default();
    let failures = Arc::new(Mutex::new(Vec::new()));
    let log = failures.clone();
    Queue::failing(move |event: &JobFailed| {
        log.lock().unwrap().push(format!(
            "{} on {}",
            event.job.resolve_name(),
            event.connection_name
        ));
    });

    Flaky {
        succeed_on: 10,
        tries: Some(2),
        backoff: vec![],
    }
    .dispatch()
    .await
    .unwrap();

    work().await;

    assert_eq!(
        recorded(),
        vec!["attempt:1", "attempt:2", "failed:Attempt 2 failed"]
    );
    assert_eq!(*failures.lock().unwrap(), vec!["Flaky on array"]);

    let failed = Queue::failed_jobs().await.unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].connection, "array");
    assert_eq!(failed[0].queue, "default");
    assert_eq!(failed[0].display_name(), "Flaky");
    assert!(failed[0].exception.contains("Attempt 2 failed"));
    let payload = failed[0].payload_value();
    assert_eq!(failed[0].id, payload["uuid"].as_str().unwrap());
    assert_eq!(Queue::size(None).await.unwrap(), 0);
}

#[tokio::test]
async fn the_worker_tries_option_applies_when_jobs_dont_say() {
    let _app = app_default();

    Flaky {
        succeed_on: 10,
        tries: None,
        backoff: vec![],
    }
    .dispatch()
    .await
    .unwrap();

    Worker::make()
        .daemon("array", "default", &options().tries(3))
        .await
        .unwrap();

    assert_eq!(
        recorded(),
        vec![
            "attempt:1",
            "attempt:2",
            "attempt:3",
            "failed:Attempt 3 failed"
        ]
    );
}

#[tokio::test]
async fn failed_jobs_are_released_with_their_backoff() {
    let _app = app_default();
    let backoffs = Arc::new(Mutex::new(Vec::new()));
    let log = backoffs.clone();
    Queue::listen(move |event: &JobReleasedAfterException| log.lock().unwrap().push(event.backoff));

    Flaky {
        succeed_on: 10,
        tries: Some(5),
        backoff: vec![30, 60],
    }
    .dispatch()
    .await
    .unwrap();

    let worker = Worker::make();
    assert!(
        worker
            .work_once("array", "default", &options())
            .await
            .unwrap()
    );

    let queue = Queue::default_connection().unwrap();
    assert_eq!(queue.delayed_size(None).await.unwrap(), 1);
    assert!(
        !worker
            .work_once("array", "default", &options())
            .await
            .unwrap()
    );
    assert_eq!(*backoffs.lock().unwrap(), vec![30]);
}

#[tokio::test]
async fn backoff_is_calculated_per_attempt() {
    let _app = app_default();
    let worker = Worker::make();

    let job = illuminate_queue::QueuedJob::new(
        "1",
        json!({"uuid": "x", "backoff": "1,5,10", "data": {}}).to_string(),
        2,
        "array",
        "default",
        Arc::new(illuminate_queue::NullBackend),
    )
    .unwrap();
    assert_eq!(worker.calculate_backoff(&job, &options()), 5);

    let job = illuminate_queue::QueuedJob::new(
        "1",
        json!({"uuid": "x", "backoff": "1,5,10", "data": {}}).to_string(),
        7,
        "array",
        "default",
        Arc::new(illuminate_queue::NullBackend),
    )
    .unwrap();
    assert_eq!(worker.calculate_backoff(&job, &options()), 10);

    let job = illuminate_queue::QueuedJob::new(
        "1",
        json!({"uuid": "x", "backoff": null, "data": {}}).to_string(),
        1,
        "array",
        "default",
        Arc::new(illuminate_queue::NullBackend),
    )
    .unwrap();
    assert_eq!(
        worker.calculate_backoff(&job, &options().backoff(vec![3])),
        3
    );
}

#[derive(Serialize, Deserialize)]
struct Sleepy {
    millis: u64,
    fail_on_timeout: bool,
}

#[async_trait]
impl ShouldQueue for Sleepy {
    async fn handle(&self) -> Result<()> {
        tokio::time::sleep(Duration::from_millis(self.millis)).await;
        record("finished");
        Ok(())
    }

    async fn failed(&self, error: &Error) -> Result<()> {
        record(format!(
            "failed:{}",
            if error.is::<TimeoutExceededException>() {
                "timeout"
            } else {
                "other"
            }
        ));
        Ok(())
    }

    fn fail_on_timeout(&self) -> bool {
        self.fail_on_timeout
    }

    fn tries(&self) -> Option<u32> {
        Some(2)
    }
}

#[tokio::test]
async fn jobs_that_time_out_are_released_and_then_failed() {
    let _app = app_default();
    let events = record_events();

    Sleepy {
        millis: 2_000,
        fail_on_timeout: false,
    }
    .dispatch()
    .await
    .unwrap();

    let options = options().timeout_duration(Duration::from_millis(50));
    let worker = Worker::make();
    worker.daemon("array", "default", &options).await.unwrap();

    assert_eq!(worker.jobs_processed(), 2);
    assert_eq!(recorded(), vec!["failed:timeout"]);
    let events = events.lock().unwrap().clone();
    assert_eq!(
        events
            .iter()
            .filter(|event| *event == "JobTimedOut")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| *event == "JobReleasedAfterException")
            .count(),
        1
    );
    let failed = Queue::failed_jobs().await.unwrap();
    assert!(failed[0].exception.contains("has timed out"));
}

#[tokio::test]
async fn jobs_may_fail_on_the_first_timeout() {
    let _app = app_default();

    Sleepy {
        millis: 2_000,
        fail_on_timeout: true,
    }
    .dispatch()
    .await
    .unwrap();

    let worker = Worker::make();
    worker
        .daemon(
            "array",
            "default",
            &options().timeout_duration(Duration::from_millis(50)),
        )
        .await
        .unwrap();

    assert_eq!(worker.jobs_processed(), 1);
    assert_eq!(recorded(), vec!["failed:timeout"]);
}

#[derive(Serialize, Deserialize)]
struct TimeBoxed;

#[async_trait]
impl ShouldQueue for TimeBoxed {
    async fn handle(&self) -> Result<()> {
        record("ran");
        Ok(())
    }

    async fn failed(&self, error: &Error) -> Result<()> {
        if error.is::<MaxAttemptsExceededException>() {
            record("expired");
        }
        Ok(())
    }

    fn retry_until(&self) -> Option<Carbon> {
        Some(Carbon::now().sub_seconds(10))
    }
}

#[tokio::test]
async fn jobs_past_their_retry_until_are_failed() {
    let _app = app_default();

    TimeBoxed.dispatch().await.unwrap();

    // Simulate a second attempt: retry_until takes precedence over tries.
    let queue = Queue::default_connection().unwrap();
    let job = queue.pop(None).await.unwrap().unwrap();
    job.release(0).await.unwrap();

    work().await;

    assert_eq!(recorded(), vec!["expired"]);
}

#[derive(Serialize, Deserialize)]
struct AlwaysThrows;

#[async_trait]
impl ShouldQueue for AlwaysThrows {
    async fn handle(&self) -> Result<()> {
        record("threw");
        Err(illuminate_support::error::RuntimeException::new("nope").into())
    }

    fn tries(&self) -> Option<u32> {
        Some(10)
    }

    fn max_exceptions(&self) -> Option<u32> {
        Some(2)
    }
}

#[tokio::test]
async fn max_exceptions_fails_the_job_early() {
    let _app = app_default();

    AlwaysThrows.dispatch().await.unwrap();
    work().await;

    assert_eq!(recorded(), vec!["threw", "threw"]);
    assert_eq!(Queue::failed_jobs().await.unwrap().len(), 1);
}

#[derive(Serialize, Deserialize)]
struct ReleasesItself;

#[async_trait]
impl ShouldQueue for ReleasesItself {
    async fn handle(&self) -> Result<()> {
        record(format!("attempt:{}", self.attempts()));
        if self.attempts() < 2 {
            return self.release(0).await;
        }
        Ok(())
    }

    fn tries(&self) -> Option<u32> {
        Some(3)
    }
}

#[derive(Serialize, Deserialize)]
struct FailsItself;

#[async_trait]
impl ShouldQueue for FailsItself {
    async fn handle(&self) -> Result<()> {
        record("handling");
        self.fail("Giving up.").await
    }

    async fn failed(&self, error: &Error) -> Result<()> {
        record(format!("failed:{error}"));
        Ok(())
    }

    fn tries(&self) -> Option<u32> {
        Some(5)
    }
}

#[derive(Serialize, Deserialize)]
struct DeletesItself;

#[async_trait]
impl ShouldQueue for DeletesItself {
    async fn handle(&self) -> Result<()> {
        record("deleting");
        self.delete().await
    }
}

#[tokio::test]
async fn jobs_can_release_fail_and_delete_themselves() {
    let _app = app_default();
    let events = record_events();

    ReleasesItself.dispatch().await.unwrap();
    FailsItself.dispatch().await.unwrap();
    DeletesItself.dispatch().await.unwrap();
    work().await;

    assert_eq!(
        recorded(),
        vec![
            "attempt:1",
            "handling",
            "failed:Giving up.",
            "deleting",
            "attempt:2",
        ]
    );
    assert_eq!(Queue::failed_jobs().await.unwrap().len(), 1);
    assert_eq!(Queue::size(None).await.unwrap(), 0);
    assert!(events.lock().unwrap().contains(&"JobReleased".to_string()));
}

#[tokio::test]
async fn unknown_jobs_are_failed_with_a_clear_error() {
    let _app = app_default();

    let payload = json!({
        "uuid": "f6a3c1d2-0000-0000-0000-000000000000",
        "displayName": "App\\Jobs\\Missing",
        "job": "Illuminate\\Queue\\CallQueuedHandler@call",
        "maxTries": null,
        "data": {"commandName": "App\\Jobs\\Missing", "command": {}, "batchId": null},
        "attempts": 0,
    });
    Queue::push_raw(payload.to_string(), None).await.unwrap();

    work().await;

    let failed = Queue::failed_jobs().await.unwrap();
    assert_eq!(failed.len(), 1);
    assert!(
        failed[0]
            .exception
            .contains("Unable to resolve the queued job [App\\Jobs\\Missing]")
    );
    assert_eq!(Queue::size(None).await.unwrap(), 0);
    let _ = UnknownJobException {
        name: String::new(),
    };
}

#[tokio::test]
async fn the_worker_stops_after_max_jobs() {
    let _app = app_default();
    let stops = Arc::new(Mutex::new(Vec::new()));
    let log = stops.clone();
    Queue::stopping(move |event: &WorkerStopping| {
        log.lock()
            .unwrap()
            .push((event.reason, event.jobs_processed, event.status));
    });

    for id in 0..3 {
        SendReport { id }.dispatch().await.unwrap();
    }

    let worker = Worker::make();
    let reason = worker
        .daemon(
            "array",
            "default",
            &WorkerOptions::new().sleep(0.0).max_jobs(2),
        )
        .await
        .unwrap();

    assert_eq!(reason, WorkerStopReason::MaxJobsExceeded);
    assert_eq!(reason.description(), "Maximum jobs exceeded");
    assert_eq!(recorded(), vec!["report:0", "report:1"]);
    assert_eq!(
        *stops.lock().unwrap(),
        vec![(WorkerStopReason::MaxJobsExceeded, 2, 0)]
    );
}

#[tokio::test]
async fn the_worker_stops_gracefully_when_asked() {
    let _app = app_default();

    let worker = Arc::new(Worker::make());
    let handle = worker.handle();

    let running = {
        let worker = worker.clone();
        async move {
            worker
                .daemon("array", "default", &WorkerOptions::new().sleep(10.0))
                .await
        }
    };
    let stopper = async {
        tokio::time::sleep(Duration::from_millis(30)).await;
        handle.quit();
    };

    let (reason, _) = tokio::join!(running, stopper);
    assert_eq!(reason.unwrap(), WorkerStopReason::Interrupted);
    assert!(handle.should_quit());
}

#[tokio::test]
async fn the_worker_stops_after_max_time() {
    let _app = app_default();

    let mut options = WorkerOptions::new().sleep(0.01);
    options.max_time = Duration::from_millis(50);

    let reason = Worker::make()
        .daemon("array", "default", &options)
        .await
        .unwrap();
    assert_eq!(reason, WorkerStopReason::MaxTimeExceeded);
}

#[tokio::test]
async fn the_worker_restarts_when_signalled() {
    let _app = app_default();

    let worker = Arc::new(Worker::make());
    let running = {
        let worker = worker.clone();
        async move {
            worker
                .daemon("array", "default", &WorkerOptions::new().sleep(0.01))
                .await
        }
    };
    let restart = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        // `queue:restart` stores a new timestamp in the cache.
        illuminate_cache::Cache::forever("illuminate:queue:restart", 1)
            .await
            .unwrap();
    };

    let (reason, _) = tokio::join!(running, restart);
    assert_eq!(reason.unwrap(), WorkerStopReason::ReceivedRestartSignal);
}

#[tokio::test]
async fn paused_queues_are_skipped() {
    let _app = app_default();
    let events = record_events();

    SendReport { id: 1 }
        .dispatch()
        .on_queue("paused")
        .await
        .unwrap();
    SendReport { id: 2 }.dispatch().await.unwrap();

    Queue::pause("array", "paused").await.unwrap();
    assert!(Queue::is_paused("array", "paused").await.unwrap());

    Worker::make()
        .daemon("array", "paused,default", &options())
        .await
        .unwrap();
    assert_eq!(recorded(), vec!["report:2"]);
    assert!(
        events
            .lock()
            .unwrap()
            .contains(&"WorkerQueuePaused".to_string())
    );

    Queue::resume("array", "paused").await.unwrap();
    Worker::make()
        .daemon("array", "paused,default", &options())
        .await
        .unwrap();
    assert_eq!(recorded(), vec!["report:2", "report:1"]);
}

#[tokio::test]
async fn maintenance_mode_pauses_the_worker_unless_forced() {
    let _app = app_default();

    SendReport { id: 1 }.dispatch().await.unwrap();

    let worker = Worker::make().down_for_maintenance_using(|| true);
    let mut options = options();
    options.max_time = Duration::from_millis(10);
    options.sleep = Duration::from_millis(5);
    options.stop_when_empty = false;
    worker.daemon("array", "default", &options).await.unwrap();
    assert!(recorded().is_empty());

    worker
        .daemon("array", "default", &self::options().force())
        .await
        .unwrap();
    assert_eq!(recorded(), vec!["report:1"]);
}

#[tokio::test]
async fn processed_events_carry_the_duration() {
    let _app = app_default();
    let durations = Arc::new(Mutex::new(Vec::new()));
    let log = durations.clone();
    Queue::after(move |event: &JobProcessed| log.lock().unwrap().push(event.duration.is_some()));

    SendReport { id: 1 }.dispatch().await.unwrap();
    work().await;

    assert_eq!(*durations.lock().unwrap(), vec![true]);
}

#[tokio::test]
async fn run_next_job_processes_a_single_job() {
    let _app = app_default();

    SendReport { id: 1 }.dispatch().await.unwrap();
    SendReport { id: 2 }.dispatch().await.unwrap();

    Worker::make()
        .run_next_job("array", "default", &options())
        .await
        .unwrap();

    assert_eq!(recorded(), vec!["report:1"]);
    assert_eq!(Queue::size(None).await.unwrap(), 1);
}
