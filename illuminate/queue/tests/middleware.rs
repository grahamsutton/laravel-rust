//! Job middleware.

mod common;

use std::sync::Arc;

use common::{app_default, record, recorded};
use illuminate_cache::facades::RateLimiter;
use illuminate_cache::{Cache, Limit};
use illuminate_queue::middleware::{
    FailOnException, JobMiddleware, Next, RateLimited, RateLimitsJobs, Release, Skip,
    ThrottlesExceptions, WithoutOverlapping,
};
use illuminate_queue::{
    Dispatchable, InteractsWithQueue, Queue, ShouldQueue, Worker, WorkerOptions, async_trait,
};
use illuminate_support::error::{InvalidArgumentException, RuntimeException};
use illuminate_support::{Error, Result};
use serde::{Deserialize, Serialize};

async fn work_once() -> bool {
    Worker::make()
        .work_once("array", "default", &WorkerOptions::new().tries(10))
        .await
        .unwrap()
}

async fn work() {
    Worker::make()
        .daemon(
            "array",
            "default",
            &WorkerOptions::new().sleep(0.0).stop_when_empty().tries(10),
        )
        .await
        .unwrap();
}

async fn delayed() -> u64 {
    Queue::default_connection()
        .unwrap()
        .delayed_size(None)
        .await
        .unwrap()
}

/// A middleware that records around the job.
struct Around(&'static str);

#[async_trait]
impl JobMiddleware for Around {
    async fn handle(&self, job: &dyn ShouldQueue, next: Next<'_>) -> Result<()> {
        record(format!("before:{}", self.0));
        let result = next.run(job).await;
        record(format!("after:{}", self.0));
        result
    }
}

#[derive(Serialize, Deserialize)]
struct Wrapped;

#[async_trait]
impl ShouldQueue for Wrapped {
    async fn handle(&self) -> Result<()> {
        record("handle");
        Ok(())
    }

    fn middleware(&self) -> Vec<Arc<dyn JobMiddleware>> {
        vec![Arc::new(Around("outer")), Arc::new(Around("inner"))]
    }
}

#[tokio::test]
async fn middleware_wrap_the_job_in_order() {
    let _app = app_default();

    Wrapped.dispatch_sync().await.unwrap();

    assert_eq!(
        recorded(),
        vec![
            "before:outer",
            "before:inner",
            "handle",
            "after:inner",
            "after:outer"
        ]
    );
}

#[derive(Serialize, Deserialize)]
struct Backup {
    user_id: u64,
    vip: bool,
}

#[async_trait]
impl ShouldQueue for Backup {
    async fn handle(&self) -> Result<()> {
        record(format!("backup:{}", self.user_id));
        Ok(())
    }

    fn middleware(&self) -> Vec<Arc<dyn JobMiddleware>> {
        vec![Arc::new(RateLimited::new("backups"))]
    }
}

#[tokio::test]
async fn rate_limited_jobs_are_released() {
    let _app = app_default();
    RateLimiter::for_job("backups", |job: &Backup| {
        if job.vip {
            Limit::none()
        } else {
            Limit::per_hour(1).by(job.user_id)
        }
    });

    for _ in 0..2 {
        Backup {
            user_id: 1,
            vip: false,
        }
        .dispatch()
        .await
        .unwrap();
        Backup {
            user_id: 2,
            vip: true,
        }
        .dispatch()
        .await
        .unwrap();
        Backup {
            user_id: 2,
            vip: true,
        }
        .dispatch()
        .await
        .unwrap();
    }

    work().await;

    assert_eq!(
        recorded()
            .iter()
            .filter(|entry| *entry == "backup:1")
            .count(),
        1
    );
    assert_eq!(
        recorded()
            .iter()
            .filter(|entry| *entry == "backup:2")
            .count(),
        4
    );
    // The limited job waits for the limit to reset.
    assert_eq!(delayed().await, 1);
}

#[derive(Serialize, Deserialize)]
struct StrictBackup;

#[async_trait]
impl ShouldQueue for StrictBackup {
    async fn handle(&self) -> Result<()> {
        record("strict");
        Ok(())
    }

    fn middleware(&self) -> Vec<Arc<dyn JobMiddleware>> {
        vec![Arc::new(RateLimited::new("strict").dont_release())]
    }
}

#[tokio::test]
async fn rate_limited_jobs_can_be_deleted_instead() {
    let _app = app_default();
    RateLimiter::for_job("strict", |_: &StrictBackup| Limit::per_minute(1));

    StrictBackup.dispatch().await.unwrap();
    StrictBackup.dispatch().await.unwrap();
    work().await;

    assert_eq!(recorded(), vec!["strict"]);
    assert_eq!(Queue::size(None).await.unwrap(), 0);
}

#[derive(Serialize, Deserialize)]
struct UpdateCreditScore {
    user_id: u64,
}

#[async_trait]
impl ShouldQueue for UpdateCreditScore {
    async fn handle(&self) -> Result<()> {
        record("scored");
        Ok(())
    }

    fn middleware(&self) -> Vec<Arc<dyn JobMiddleware>> {
        vec![Arc::new(
            WithoutOverlapping::new(self.user_id).release_after(30),
        )]
    }
}

#[tokio::test]
async fn overlapping_jobs_are_released() {
    let _app = app_default();

    // Another worker is busy with this user...
    let key = WithoutOverlapping::new(7).get_lock_key(&UpdateCreditScore { user_id: 7 });
    let lock = Cache::lock(&key, 60).unwrap();
    assert!(lock.get().await.unwrap());

    UpdateCreditScore { user_id: 7 }.dispatch().await.unwrap();
    UpdateCreditScore { user_id: 8 }.dispatch().await.unwrap();
    work().await;

    assert_eq!(recorded(), vec!["scored"]);
    assert_eq!(delayed().await, 1);

    // The lock is released after the job runs.
    lock.release().await.unwrap();
    assert!(!Cache::lock(&key, 0).unwrap().is_locked().await.unwrap());
}

#[test]
fn overlap_keys_may_be_shared() {
    let job = UpdateCreditScore { user_id: 1 };
    assert_eq!(
        WithoutOverlapping::new("status:aws")
            .shared()
            .get_lock_key(&job),
        "laravel-queue-overlap:status:aws"
    );
    assert!(
        WithoutOverlapping::new(1)
            .get_lock_key(&job)
            .ends_with("UpdateCreditScore:1")
    );
}

#[derive(Serialize, Deserialize)]
struct CallsFlakyApi {
    error: String,
}

#[async_trait]
impl ShouldQueue for CallsFlakyApi {
    async fn handle(&self) -> Result<()> {
        record(format!("attempt:{}", self.attempts()));
        match self.error.as_str() {
            "invalid" => Err(InvalidArgumentException::new("customer deleted").into()),
            "" => Ok(()),
            message => Err(RuntimeException::new(message.to_string()).into()),
        }
    }

    async fn failed(&self, error: &Error) -> Result<()> {
        record(format!("failed:{error}"));
        Ok(())
    }

    fn middleware(&self) -> Vec<Arc<dyn JobMiddleware>> {
        vec![Arc::new(
            ThrottlesExceptions::new(2, 600)
                .by("flaky-api")
                .delete_when_error::<InvalidArgumentException>(),
        )]
    }
}

#[tokio::test]
async fn exceptions_are_throttled() {
    let _app = app_default();

    CallsFlakyApi {
        error: "timeout".into(),
    }
    .dispatch()
    .await
    .unwrap();

    // Each exception releases the job (immediately, with no backoff)...
    assert!(work_once().await);
    assert!(work_once().await);
    // ...until the threshold is hit: the job is then released until the
    // throttle decays.
    assert!(work_once().await);

    assert_eq!(recorded(), vec!["attempt:1", "attempt:2"]);
    assert_eq!(delayed().await, 1);
    assert!(
        RateLimiter::too_many_attempts("laravel_throttles_exceptions:flaky-api", 2)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn throttled_jobs_can_be_deleted_on_specific_exceptions() {
    let _app = app_default();

    CallsFlakyApi {
        error: "invalid".into(),
    }
    .dispatch()
    .await
    .unwrap();
    work().await;

    assert_eq!(recorded(), vec!["attempt:1"]);
    assert_eq!(Queue::size(None).await.unwrap(), 0);
    assert!(Queue::failed_jobs().await.unwrap().is_empty());
}

#[derive(Serialize, Deserialize)]
struct Conditional {
    skip: bool,
    release: bool,
}

#[async_trait]
impl ShouldQueue for Conditional {
    async fn handle(&self) -> Result<()> {
        record("ran");
        Ok(())
    }

    fn middleware(&self) -> Vec<Arc<dyn JobMiddleware>> {
        vec![
            Arc::new(Skip::when(self.skip)),
            Arc::new(Release::when(self.release, 45)),
        ]
    }
}

#[tokio::test]
async fn jobs_can_be_skipped_or_released() {
    let _app = app_default();

    Conditional {
        skip: true,
        release: false,
    }
    .dispatch()
    .await
    .unwrap();
    Conditional {
        skip: false,
        release: true,
    }
    .dispatch()
    .await
    .unwrap();
    Conditional {
        skip: false,
        release: false,
    }
    .dispatch()
    .await
    .unwrap();

    work().await;

    assert_eq!(recorded(), vec!["ran"]);
    assert_eq!(delayed().await, 1);
    assert_eq!(Queue::size(None).await.unwrap(), 1);
}

#[derive(Serialize, Deserialize)]
struct SyncChatHistory {
    revoked: bool,
}

#[async_trait]
impl ShouldQueue for SyncChatHistory {
    async fn handle(&self) -> Result<()> {
        record(format!("attempt:{}", self.attempts()));
        if self.revoked {
            return Err(InvalidArgumentException::new("permission revoked").into());
        }
        Err(RuntimeException::new("chat server down").into())
    }

    async fn failed(&self, error: &Error) -> Result<()> {
        record(format!("failed:{error}"));
        Ok(())
    }

    fn tries(&self) -> Option<u32> {
        Some(3)
    }

    fn middleware(&self) -> Vec<Arc<dyn JobMiddleware>> {
        vec![Arc::new(FailOnException::for_error::<
            InvalidArgumentException,
        >())]
    }
}

#[tokio::test]
async fn specific_exceptions_fail_the_job_immediately() {
    let _app = app_default();

    SyncChatHistory { revoked: true }.dispatch().await.unwrap();
    SyncChatHistory { revoked: false }.dispatch().await.unwrap();
    work().await;

    assert_eq!(
        recorded(),
        vec![
            "attempt:1",
            "failed:permission revoked",
            "attempt:1",
            "attempt:2",
            "attempt:3",
            "failed:chat server down",
        ]
    );
    assert_eq!(Queue::failed_jobs().await.unwrap().len(), 2);
}
