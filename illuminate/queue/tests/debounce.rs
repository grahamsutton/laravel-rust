//! Debounced jobs.

mod common;

use std::sync::{Arc, Mutex};

use common::{app_default, record, recorded};
use illuminate_cache::Cache;
use illuminate_queue::events::{JobDebounced, JobQueued};
use illuminate_queue::{
    DebounceFor, DebounceLock, Dispatchable, Queue, ShouldQueue, Worker, WorkerOptions, async_trait,
};
use illuminate_support::Result;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct UpdateSearchIndex {
    product_id: u64,
    version: u64,
}

#[async_trait]
impl ShouldQueue for UpdateSearchIndex {
    async fn handle(&self) -> Result<()> {
        record(format!("indexing:{} v{}", self.product_id, self.version));
        Ok(())
    }

    fn debounce_for(&self) -> Option<DebounceFor> {
        Some(DebounceFor::new(30))
    }

    fn debounce_id(&self) -> String {
        self.product_id.to_string()
    }
}

#[derive(Serialize, Deserialize)]
struct SyncInventory;

#[async_trait]
impl ShouldQueue for SyncInventory {
    async fn handle(&self) -> Result<()> {
        Ok(())
    }

    fn debounce_for(&self) -> Option<DebounceFor> {
        Some(DebounceFor::new(30).max_wait(0))
    }
}

#[derive(Serialize, Deserialize)]
struct UniqueAndDebounced;

#[async_trait]
impl ShouldQueue for UniqueAndDebounced {
    async fn handle(&self) -> Result<()> {
        Ok(())
    }

    fn debounce_for(&self) -> Option<DebounceFor> {
        Some(30.into())
    }

    fn unique_id(&self) -> Option<String> {
        Some(String::new())
    }
}

fn index(product_id: u64, version: u64) -> UpdateSearchIndex {
    UpdateSearchIndex { product_id, version }
}

async fn work() {
    Worker::make()
        .daemon("array", "default", &WorkerOptions::new().sleep(0.0).stop_when_empty())
        .await
        .unwrap();
}

/// The delay, in seconds, of every job pushed onto the queue.
fn delays() -> Arc<Mutex<Vec<u64>>> {
    let delays = Arc::new(Mutex::new(Vec::new()));
    let log = delays.clone();
    Queue::listen(move |event: &JobQueued| log.lock().unwrap().push(event.delay.unwrap_or_default().as_secs()));
    delays
}

#[tokio::test]
async fn only_the_latest_dispatch_runs() {
    let _app = app_default();
    let debounced = Arc::new(Mutex::new(Vec::new()));
    let log = debounced.clone();
    Queue::listen(move |event: &JobDebounced| {
        let job = event.command.job().as_any().downcast_ref::<UpdateSearchIndex>().unwrap();
        log.lock().unwrap().push(job.version);
    });

    for version in 1..=3 {
        index(1, version).dispatch().without_delay().await.unwrap();
    }
    index(2, 1).dispatch().without_delay().await.unwrap();
    work().await;

    assert_eq!(recorded(), vec!["indexing:1 v3", "indexing:2 v1"]);
    assert_eq!(*debounced.lock().unwrap(), vec![1, 2]);
    assert_eq!(Queue::size(None).await.unwrap(), 0);
}

#[tokio::test]
async fn debounced_jobs_are_delayed_by_default() {
    let _app = app_default();
    let delays = delays();

    index(1, 1).dispatch().await.unwrap();
    index(1, 2).dispatch().delay(5).await.unwrap();

    assert_eq!(*delays.lock().unwrap(), vec![30, 5]);
}

#[tokio::test]
async fn the_owner_token_travels_with_the_payload() {
    let _app = app_default();

    index(1, 1).dispatch().without_delay().await.unwrap();

    let job = Queue::default_connection().unwrap().pop(None).await.unwrap().unwrap();
    let owner = job.payload()["data"]["debounceOwner"].as_str().unwrap().to_string();
    assert_eq!(owner.len(), 40);
    assert_eq!(
        DebounceLock::new(None).current_owner(&index(1, 1)).await.unwrap(),
        Some(owner)
    );
    job.delete().await.unwrap();
}

#[tokio::test]
async fn jobs_run_when_their_token_is_gone() {
    let _app = app_default();

    index(1, 1).dispatch().without_delay().await.unwrap();
    Cache::forget(&DebounceLock::key(&index(1, 1))).await.unwrap();
    work().await;

    assert_eq!(recorded(), vec!["indexing:1 v1"]);
}

#[tokio::test]
async fn the_max_wait_forces_the_job_to_run() {
    let _app = app_default();
    let delays = delays();

    SyncInventory.dispatch().await.unwrap();
    SyncInventory.dispatch().await.unwrap();
    SyncInventory.dispatch().await.unwrap();

    // The second dispatch had waited long enough: it runs right away, and
    // the wait starts over.
    assert_eq!(*delays.lock().unwrap(), vec![30, 0, 30]);
}

#[tokio::test]
async fn debounced_jobs_cannot_be_unique() {
    let _app = app_default();

    let error = UniqueAndDebounced.dispatch().await.unwrap_err();

    assert_eq!(error.to_string(), "A debounced job cannot also implement ShouldBeUnique.");
    assert_eq!(Queue::size(None).await.unwrap(), 0);
}

#[tokio::test]
async fn the_token_may_be_released() {
    let _app = app_default();
    let lock = DebounceLock::new(None);
    let job = index(1, 1);

    let acquired = lock.acquire(&job, None, None).await.unwrap();
    assert!(!acquired.max_wait_exceeded);

    lock.release(&job, Some("someone-else")).await.unwrap();
    assert_eq!(lock.current_owner(&job).await.unwrap(), Some(acquired.owner.clone()));

    lock.release(&job, Some(&acquired.owner)).await.unwrap();
    assert_eq!(lock.current_owner(&job).await.unwrap(), None);
}
