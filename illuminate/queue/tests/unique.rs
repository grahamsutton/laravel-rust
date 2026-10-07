//! Unique jobs.

mod common;

use common::{app_default, record, recorded};
use illuminate_cache::Cache;
use illuminate_queue::events::UniqueJobSkipped;
use illuminate_queue::{
    Bus, Dispatchable, Queue, ShouldQueue, UniqueLock, Worker, WorkerOptions, async_trait,
};
use illuminate_support::Result;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct UpdateSearchIndex {
    product_id: u64,
    fail: bool,
}

#[async_trait]
impl ShouldQueue for UpdateSearchIndex {
    async fn handle(&self) -> Result<()> {
        // While processing, the lock is still held.
        let locked = Cache::lock(&UniqueLock::key(self), 0)?.is_locked().await?;
        record(format!("indexing:{} locked:{locked}", self.product_id));
        if self.fail {
            return Err(illuminate_support::error::RuntimeException::new("index down").into());
        }
        Ok(())
    }

    fn unique_id(&self) -> Option<String> {
        Some(self.product_id.to_string())
    }

    fn unique_for(&self) -> u64 {
        3600
    }
}

#[derive(Serialize, Deserialize)]
struct RebuildSitemap;

#[async_trait]
impl ShouldQueue for RebuildSitemap {
    async fn handle(&self) -> Result<()> {
        let locked = Cache::lock(&UniqueLock::key(self), 0)?.is_locked().await?;
        record(format!("rebuilding locked:{locked}"));
        Ok(())
    }

    fn unique_id(&self) -> Option<String> {
        Some(String::new())
    }

    fn unique_until_processing(&self) -> bool {
        true
    }
}

fn index(product_id: u64) -> UpdateSearchIndex {
    UpdateSearchIndex {
        product_id,
        fail: false,
    }
}

async fn work() {
    Worker::make()
        .daemon(
            "array",
            "default",
            &WorkerOptions::new().sleep(0.0).stop_when_empty(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn unique_jobs_are_only_queued_once() {
    let _app = app_default();
    let skipped = std::sync::Arc::new(std::sync::Mutex::new(0));
    let counter = skipped.clone();
    Queue::listen(move |_: &UniqueJobSkipped| *counter.lock().unwrap() += 1);

    index(1).dispatch().await.unwrap();
    index(1).dispatch().await.unwrap();
    index(2).dispatch().await.unwrap();

    assert_eq!(Queue::size(None).await.unwrap(), 2);
    assert_eq!(*skipped.lock().unwrap(), 1);

    let payload = Queue::default_connection()
        .unwrap()
        .pop(None)
        .await
        .unwrap()
        .unwrap();
    assert!(payload.payload()["data"]["uniqueLockOwner"].is_string());
    payload.release(0).await.unwrap();
}

#[tokio::test]
async fn the_lock_is_released_once_the_job_is_processed() {
    let _app = app_default();

    index(1).dispatch().await.unwrap();
    assert!(
        Cache::lock(&UniqueLock::key(&index(1)), 0)
            .unwrap()
            .is_locked()
            .await
            .unwrap()
    );

    Worker::make()
        .daemon(
            "array",
            "default",
            &WorkerOptions::new().sleep(0.0).stop_when_empty().tries(2),
        )
        .await
        .unwrap();

    assert_eq!(recorded(), vec!["indexing:1 locked:true"]);
    assert!(
        !Cache::lock(&UniqueLock::key(&index(1)), 0)
            .unwrap()
            .is_locked()
            .await
            .unwrap()
    );

    // Once processed, the job may be queued again.
    index(1).dispatch().await.unwrap();
    assert_eq!(Queue::size(None).await.unwrap(), 1);
}

#[tokio::test]
async fn the_lock_is_released_when_the_job_fails() {
    let _app = app_default();

    UpdateSearchIndex {
        product_id: 5,
        fail: true,
    }
    .dispatch()
    .await
    .unwrap();
    work().await;

    assert_eq!(recorded(), vec!["indexing:5 locked:true"]);
    assert!(
        !Cache::lock(&UniqueLock::key(&index(5)), 0)
            .unwrap()
            .is_locked()
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn unique_until_processing_releases_the_lock_before_running() {
    let _app = app_default();

    RebuildSitemap.dispatch().await.unwrap();
    RebuildSitemap.dispatch().await.unwrap();
    assert_eq!(Queue::size(None).await.unwrap(), 1);

    work().await;

    assert_eq!(recorded(), vec!["rebuilding locked:false"]);
}

#[tokio::test]
async fn bus_dispatch_bypasses_uniqueness() {
    let _app = app_default();

    index(1).dispatch().await.unwrap();
    Bus::dispatch(index(1)).await.unwrap();

    assert_eq!(Queue::size(None).await.unwrap(), 2);
}

#[tokio::test]
async fn unique_keys_include_the_job_name_and_id() {
    let _app = app_default();

    assert!(UniqueLock::key(&index(3)).starts_with("laravel_unique_job:"));
    assert!(UniqueLock::key(&index(3)).ends_with("UpdateSearchIndex:3"));
    assert!(UniqueLock::key(&RebuildSitemap).ends_with("RebuildSitemap:"));
}
