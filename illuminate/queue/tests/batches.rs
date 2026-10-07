//! Job batches.

mod common;

use std::sync::Arc;

use common::{app_default, record, recorded};
use illuminate_queue::events::{BatchCanceled, BatchDispatched, BatchFinished};
use illuminate_queue::middleware::{JobMiddleware, SkipIfBatchCancelled};
use illuminate_queue::{
    Batch, BatchRepository, Bus, InteractsWithQueue, Queue, ShouldQueue, Worker, WorkerOptions,
    async_trait,
};
use illuminate_support::{Error, Result, json};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct ImportCsv {
    chunk: u64,
    fail: bool,
}

impl ImportCsv {
    fn ok(chunk: u64) -> Box<ImportCsv> {
        Box::new(ImportCsv { chunk, fail: false })
    }

    fn failing(chunk: u64) -> Box<ImportCsv> {
        Box::new(ImportCsv { chunk, fail: true })
    }
}

#[async_trait]
impl ShouldQueue for ImportCsv {
    async fn handle(&self) -> Result<()> {
        let batch = self.batch().await?.expect("the job belongs to a batch");
        record(format!("import:{} ({})", self.chunk, batch.name));
        if self.fail {
            return Err(illuminate_support::error::RuntimeException::new(format!(
                "chunk {} failed",
                self.chunk
            ))
            .into());
        }
        Ok(())
    }

    fn middleware(&self) -> Vec<Arc<dyn JobMiddleware>> {
        vec![Arc::new(SkipIfBatchCancelled)]
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
async fn batches_run_their_callbacks() {
    let _app = app_default();
    let dispatched = Arc::new(std::sync::Mutex::new(0));
    let finished = Arc::new(std::sync::Mutex::new(0));
    let (d, f) = (dispatched.clone(), finished.clone());
    Queue::listen(move |_: &BatchDispatched| *d.lock().unwrap() += 1);
    Queue::listen(move |_: &BatchFinished| *f.lock().unwrap() += 1);

    let batch = Bus::batch(vec![ImportCsv::ok(1), ImportCsv::ok(2), ImportCsv::ok(3)])
        .name("Import CSV")
        .before(|batch: Batch| async move {
            record(format!("before:{}", batch.total_jobs));
            Ok(())
        })
        .progress(|batch: Batch| async move {
            record(format!("progress:{}%", batch.progress()));
            Ok(())
        })
        .then(|batch: Batch| async move {
            record(format!("then:{}", batch.processed_jobs()));
            Ok(())
        })
        .catch(|_batch: Batch, error: Arc<Error>| async move {
            record(format!("catch:{error}"));
            Ok(())
        })
        .finally(|batch: Batch| async move {
            record(format!("finally:{}", batch.finished()));
            Ok(())
        })
        .dispatch()
        .await
        .unwrap();

    assert_eq!(batch.name, "Import CSV");
    assert_eq!(batch.total_jobs, 3);
    assert_eq!(batch.pending_jobs, 3);
    assert_eq!(batch.progress(), 0);
    assert!(!batch.finished());
    assert_eq!(*dispatched.lock().unwrap(), 1);

    work().await;

    assert_eq!(
        recorded(),
        vec![
            "before:0",
            "import:1 (Import CSV)",
            "progress:33%",
            "import:2 (Import CSV)",
            "progress:67%",
            "import:3 (Import CSV)",
            "progress:100%",
            "then:3",
            "finally:true",
        ]
    );

    let batch = Bus::find_batch(&batch.id).await.unwrap().unwrap();
    assert_eq!(batch.pending_jobs, 0);
    assert_eq!(batch.progress(), 100);
    assert!(batch.finished());
    assert!(!batch.cancelled());
    assert_eq!(*finished.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_failed_job_cancels_the_batch() {
    let _app = app_default();
    let cancelled = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = cancelled.clone();
    Queue::listen(move |event: &BatchCanceled| {
        log.lock().unwrap().push(
            event
                .exception
                .as_ref()
                .map(|error| error.to_string())
                .unwrap_or_default(),
        )
    });

    let batch = Bus::batch(vec![
        ImportCsv::failing(1),
        ImportCsv::ok(2),
        ImportCsv::ok(3),
    ])
    .name("Import")
    .then(|_: Batch| async {
        record("then");
        Ok(())
    })
    .catch(|batch: Batch, error: Arc<Error>| async move {
        record(format!("catch:{error}:{}", batch.cancelled()));
        Ok(())
    })
    .finally(|batch: Batch| async move {
        record(format!("finally:{}", batch.failed_jobs));
        Ok(())
    })
    .dispatch()
    .await
    .unwrap();

    work().await;

    // The remaining jobs skip themselves once the batch is cancelled.
    assert_eq!(
        recorded(),
        vec![
            "import:1 (Import)",
            "catch:chunk 1 failed:true",
            "finally:1"
        ]
    );
    assert_eq!(*cancelled.lock().unwrap(), vec!["chunk 1 failed"]);

    let batch = batch.fresh().await.unwrap().unwrap();
    assert!(batch.cancelled());
    assert!(batch.has_failures());
    assert_eq!(batch.failed_jobs, 1);
    assert_eq!(batch.failed_job_ids.len(), 1);
    assert_eq!(batch.pending_jobs, 1);
}

#[tokio::test]
async fn batches_may_allow_failures() {
    let _app = app_default();

    let batch = Bus::batch(vec![ImportCsv::failing(1), ImportCsv::ok(2)])
        .allow_failures_with(|_: Batch, error: Arc<Error>| async move {
            record(format!("failure:{error}"));
            Ok(())
        })
        .then(|_: Batch| async {
            record("then");
            Ok(())
        })
        .catch(|_: Batch, _: Arc<Error>| async {
            record("catch");
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
    assert!(batch.allows_failures());

    work().await;

    assert_eq!(
        recorded(),
        vec![
            "import:1 ()",
            "failure:chunk 1 failed",
            "catch",
            "import:2 ()",
            "finally:1/1",
        ]
    );
    let batch = batch.fresh().await.unwrap().unwrap();
    assert!(!batch.cancelled());
    assert!(!batch.finished());
}

#[tokio::test]
async fn batches_can_be_cancelled() {
    let _app = app_default();

    let batch = Bus::batch(vec![ImportCsv::ok(1), ImportCsv::ok(2)])
        .dispatch()
        .await
        .unwrap();
    batch.cancel().await.unwrap();
    assert!(batch.fresh().await.unwrap().unwrap().canceled());

    work().await;

    assert!(recorded().is_empty());
}

#[derive(Serialize, Deserialize)]
struct LoadImportBatch;

#[async_trait]
impl ShouldQueue for LoadImportBatch {
    async fn handle(&self) -> Result<()> {
        record("loading");
        let batch = self.batch().await?.unwrap();
        batch
            .add(vec![ImportCsv::ok(10), ImportCsv::ok(11)])
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn jobs_can_add_jobs_to_their_batch() {
    let _app = app_default();

    let batch = Bus::batch(vec![Box::new(LoadImportBatch)])
        .name("Contacts")
        .then(|batch: Batch| async move {
            record(format!("then:{}", batch.total_jobs));
            Ok(())
        })
        .dispatch()
        .await
        .unwrap();

    work().await;

    assert_eq!(
        recorded(),
        vec![
            "loading",
            "import:10 (Contacts)",
            "import:11 (Contacts)",
            "then:3",
        ]
    );
    assert!(batch.fresh().await.unwrap().unwrap().finished());
}

#[tokio::test]
async fn batches_may_contain_chains() {
    let _app = app_default();

    let batch = Bus::batch(vec![
        Box::new(Bus::chain(vec![ImportCsv::ok(1), ImportCsv::ok(2)])),
        Box::new(Bus::chain(vec![ImportCsv::ok(3), ImportCsv::ok(4)])),
    ])
    .then(|batch: Batch| async move {
        record(format!("then:{}", batch.total_jobs));
        Ok(())
    })
    .dispatch()
    .await
    .unwrap();

    assert_eq!(batch.total_jobs, 4);
    // Only the first job of each chain is queued.
    assert_eq!(Queue::size(None).await.unwrap(), 2);

    work().await;

    assert_eq!(
        recorded(),
        vec![
            "import:1 ()",
            "import:3 ()",
            "import:2 ()",
            "import:4 ()",
            "then:4",
        ]
    );
}

#[derive(Serialize, Deserialize)]
struct Announce {
    message: String,
}

#[async_trait]
impl ShouldQueue for Announce {
    async fn handle(&self) -> Result<()> {
        record(self.message.clone());
        Ok(())
    }
}

#[tokio::test]
async fn chains_may_contain_batches() {
    let _app = app_default();

    Bus::chain(vec![
        Box::new(Announce {
            message: "start".into(),
        }),
        Box::new(Bus::batch(vec![ImportCsv::ok(1), ImportCsv::ok(2)]).name("Releases")),
        Box::new(Announce {
            message: "done".into(),
        }),
    ])
    .dispatch()
    .await
    .unwrap();

    work().await;

    assert_eq!(
        recorded(),
        vec![
            "start",
            "import:1 (Releases)",
            "import:2 (Releases)",
            "done",
        ]
    );
}

#[tokio::test]
async fn batch_callbacks_may_dispatch_jobs() {
    let _app = app_default();

    Bus::batch(vec![ImportCsv::ok(1)])
        .then_dispatch(Announce {
            message: "then job".into(),
        })
        .finally_dispatch(Announce {
            message: "finally job".into(),
        })
        .dispatch()
        .await
        .unwrap();

    work().await;

    assert_eq!(recorded(), vec!["import:1 ()", "then job", "finally job"]);
}

#[tokio::test]
async fn batches_serialize_like_laravel() {
    let _app = app_default();

    let batch = Bus::batch(vec![ImportCsv::ok(1), ImportCsv::ok(2)])
        .name("Import")
        .on_queue("imports")
        .dispatch()
        .await
        .unwrap();

    let value = serde_json::to_value(&batch).unwrap();
    assert_eq!(value["id"], json!(batch.id));
    assert_eq!(value["name"], json!("Import"));
    assert_eq!(value["totalJobs"], json!(2));
    assert_eq!(value["pendingJobs"], json!(2));
    assert_eq!(value["processedJobs"], json!(0));
    assert_eq!(value["progress"], json!(0));
    assert_eq!(value["failedJobs"], json!(0));
    assert_eq!(value["options"]["queue"], json!("imports"));
    assert_eq!(value["options"]["allowFailures"], json!(false));
    assert!(value["cancelledAt"].is_null());
    assert_eq!(Queue::size(Some("imports")).await.unwrap(), 2);
}

#[tokio::test]
async fn batches_run_immediately_on_the_sync_connection() {
    let _app = app_default();

    let batch = Bus::batch(vec![ImportCsv::ok(1), ImportCsv::ok(2)])
        .on_connection("sync")
        .then(|_: Batch| async {
            record("then");
            Ok(())
        })
        .dispatch()
        .await
        .unwrap();

    assert_eq!(recorded(), vec!["import:1 ()", "import:2 ()", "then"]);
    assert!(batch.finished());
}

#[tokio::test]
async fn batches_can_be_dispatched_after_the_response() {
    let _app = app_default();

    let batch = Bus::batch(vec![ImportCsv::ok(1)])
        .dispatch_after_response()
        .await
        .unwrap();
    assert_eq!(batch.total_jobs, 0);
    assert_eq!(Queue::size(None).await.unwrap(), 0);

    illuminate_queue::DeferredCallbacks::current()
        .invoke()
        .await;

    assert_eq!(batch.fresh().await.unwrap().unwrap().total_jobs, 1);
    assert_eq!(Queue::size(None).await.unwrap(), 1);
}

#[tokio::test]
async fn batches_can_be_listed_and_pruned() {
    let app = app_default();

    let first = Bus::batch(vec![]).name("first").dispatch().await.unwrap();
    let second = Bus::batch(vec![]).name("second").dispatch().await.unwrap();

    let repository = app.container.make::<dyn BatchRepository>();
    let listed = repository.get(10, None).await.unwrap();
    assert_eq!(listed[0].id, second.id);
    assert_eq!(listed[1].id, first.id);

    first.delete().await.unwrap();
    assert!(Bus::find_batch(&first.id).await.unwrap().is_none());
}

#[derive(Serialize, Deserialize)]
struct ChargeCard {
    order_id: u64,
}

#[async_trait]
impl ShouldQueue for ChargeCard {
    async fn handle(&self) -> Result<()> {
        record(format!("charge:{}", self.order_id));
        if !common::recorded().iter().any(|entry| entry == "gateway up") {
            return Err(illuminate_support::error::RuntimeException::new("Gateway down").into());
        }
        Ok(())
    }
}

#[tokio::test]
async fn failed_batch_jobs_can_be_retried() {
    let _app = app_default();

    let batch = Bus::batch(vec![
        Box::new(ChargeCard { order_id: 1 }),
        Box::new(ChargeCard { order_id: 2 }),
    ])
    .allow_failures()
    .then(|_: Batch| async {
        record("then");
        Ok(())
    })
    .dispatch()
    .await
    .unwrap();
    work().await;

    let failed = batch.fresh().await.unwrap().unwrap();
    assert_eq!(failed.failed_jobs, 2);
    assert_eq!(failed.failed_job_ids.len(), 2);

    // `queue:retry-batch {id}`
    record("gateway up");
    let retried = Queue::retry_batch(&batch.id).await.unwrap();
    assert_eq!(retried.len(), 2);
    work().await;

    let batch = batch.fresh().await.unwrap().unwrap();
    assert_eq!(batch.pending_jobs, 0);
    assert!(batch.failed_job_ids.is_empty());
    assert!(batch.finished());
    assert_eq!(recorded().last().unwrap(), "then");
    assert!(Queue::retry_batch("missing").await.is_err());
}
