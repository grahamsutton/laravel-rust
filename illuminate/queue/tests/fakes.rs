//! The queue and bus fakes.

mod common;

use std::time::Duration;

use common::{app_default, record, recorded};
use illuminate_queue::{
    Bus, CallQueuedClosure, Dispatchable, InteractsWithQueue, PendingBatch, Queue, QueuedJob,
    ShouldQueue, async_trait, dispatch_closure, with_job,
};
use illuminate_support::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct ShipOrder {
    order_id: u64,
}

#[async_trait]
impl ShouldQueue for ShipOrder {
    async fn handle(&self) -> Result<()> {
        record(format!("shipped:{}", self.order_id));
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct RecordShipment;

#[async_trait]
impl ShouldQueue for RecordShipment {
    async fn handle(&self) -> Result<()> {
        record("recorded");
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct UpdateInventory;

#[async_trait]
impl ShouldQueue for UpdateInventory {
    async fn handle(&self) -> Result<()> {
        record("inventory");
        Ok(())
    }
}

#[tokio::test]
async fn the_queue_fake_records_pushed_jobs() {
    let _app = app_default();
    Queue::fake();

    Queue::assert_nothing_pushed();

    ShipOrder { order_id: 1 }.dispatch().await.unwrap();
    ShipOrder { order_id: 2 }
        .dispatch()
        .on_queue("shipping")
        .await
        .unwrap();
    RecordShipment
        .dispatch()
        .delay(Duration::from_secs(60))
        .await
        .unwrap();

    Queue::assert_pushed::<ShipOrder>();
    Queue::assert_pushed_with::<ShipOrder>(|job| job.order_id == 2);
    Queue::assert_pushed_times::<ShipOrder>(2);
    Queue::assert_pushed_once::<RecordShipment>();
    Queue::assert_pushed_on::<ShipOrder>("shipping");
    Queue::assert_pushed_on_with::<ShipOrder>("shipping", |job| job.order_id == 2);
    Queue::assert_pushed_on::<ShipOrder>("default");
    Queue::assert_not_pushed::<UpdateInventory>();
    Queue::assert_not_pushed_with::<ShipOrder>(|job| job.order_id == 3);
    Queue::assert_pushed_without_chain::<ShipOrder>();
    Queue::assert_closure_not_pushed();
    Queue::assert_count(3);

    let pushed = Queue::pushed::<ShipOrder>();
    assert_eq!(*pushed[1], ShipOrder { order_id: 2 });
    let fake = Queue::fake_instance();
    let delayed = fake.pushed_matching::<RecordShipment>(|_, pushed| pushed.delay.is_some());
    assert_eq!(delayed[0].delay, Some(Duration::from_secs(60)));

    // Nothing actually ran.
    assert!(recorded().is_empty());
}

#[tokio::test]
async fn the_queue_fake_records_chains_and_closures() {
    let _app = app_default();
    Queue::fake();

    Bus::chain(vec![
        Box::new(ShipOrder { order_id: 1 }),
        Box::new(RecordShipment),
        Box::new(UpdateInventory),
    ])
    .dispatch()
    .await
    .unwrap();
    dispatch_closure(|| async { Ok(()) }).await.unwrap();

    Queue::assert_pushed_with_chain::<ShipOrder, (RecordShipment, UpdateInventory)>();
    Queue::assert_closure_pushed();
    Queue::assert_pushed::<CallQueuedClosure>();
}

#[tokio::test]
#[should_panic(expected = "The expected [")]
async fn missing_pushes_fail_the_assertion() {
    let _app = app_default();
    Queue::fake();

    Queue::assert_pushed::<ShipOrder>();
}

#[tokio::test]
#[should_panic(expected = "Expected 2 jobs to be pushed, but found 1 instead.")]
async fn wrong_counts_fail_the_assertion() {
    let _app = app_default();
    Queue::fake();
    ShipOrder { order_id: 1 }.dispatch().await.unwrap();

    Queue::assert_count(2);
}

#[tokio::test]
#[should_panic(expected = "were pushed unexpectedly")]
async fn unexpected_pushes_fail_the_assertion() {
    let _app = app_default();
    Queue::fake();
    ShipOrder { order_id: 1 }.dispatch().await.unwrap();

    Queue::assert_nothing_pushed();
}

#[tokio::test]
async fn only_some_jobs_may_be_faked() {
    let _app = app_default();
    Queue::fake().only::<ShipOrder>();

    ShipOrder { order_id: 1 }.dispatch().await.unwrap();
    RecordShipment
        .dispatch()
        .on_connection("sync")
        .await
        .unwrap();

    Queue::assert_pushed::<ShipOrder>();
    Queue::assert_not_pushed::<RecordShipment>();
    assert_eq!(recorded(), vec!["recorded"]);
}

#[tokio::test]
async fn some_jobs_may_be_excluded_from_the_fake() {
    let _app = app_default();
    Queue::fake().except::<RecordShipment>();

    ShipOrder { order_id: 1 }.dispatch().await.unwrap();
    RecordShipment
        .dispatch()
        .on_connection("sync")
        .await
        .unwrap();

    Queue::assert_count(1);
    assert_eq!(recorded(), vec!["recorded"]);
}

#[tokio::test]
async fn the_bus_fake_records_dispatches() {
    let _app = app_default();
    Bus::fake();

    Bus::assert_nothing_dispatched();

    ShipOrder { order_id: 1 }.dispatch().await.unwrap();
    ShipOrder { order_id: 2 }.dispatch_sync().await.unwrap();
    RecordShipment.dispatch_after_response().await.unwrap();

    Bus::assert_dispatched::<ShipOrder>();
    Bus::assert_dispatched_with::<ShipOrder>(|job| job.order_id == 2);
    Bus::assert_dispatched_times::<ShipOrder>(2);
    Bus::assert_dispatched_sync::<ShipOrder>();
    Bus::assert_dispatched_sync_with::<ShipOrder>(|job| job.order_id == 2);
    Bus::assert_dispatched_sync_times::<ShipOrder>(1);
    Bus::assert_not_dispatched_sync::<RecordShipment>();
    Bus::assert_dispatched_after_response::<RecordShipment>();
    Bus::assert_dispatched_after_response_times::<RecordShipment>(1);
    Bus::assert_not_dispatched_after_response::<ShipOrder>();
    Bus::assert_dispatched_once::<RecordShipment>();
    Bus::assert_not_dispatched::<UpdateInventory>();
    Bus::assert_not_dispatched_with::<ShipOrder>(|job| job.order_id == 9);
    Bus::assert_dispatched_without_chain::<ShipOrder>();
    Bus::assert_nothing_batched();

    assert_eq!(Bus::dispatched::<ShipOrder>().len(), 1);
    assert!(recorded().is_empty());
}

#[tokio::test]
async fn the_bus_fake_records_chains() {
    let _app = app_default();
    Bus::fake();

    Bus::chain(vec![
        Box::new(ShipOrder { order_id: 1 }),
        Box::new(RecordShipment),
        Box::new(UpdateInventory),
    ])
    .dispatch()
    .await
    .unwrap();

    Bus::assert_chained::<(ShipOrder, RecordShipment, UpdateInventory)>();
    Bus::assert_chained_jobs(vec![
        Box::new(ShipOrder { order_id: 1 }),
        Box::new(RecordShipment),
        Box::new(UpdateInventory),
    ]);
}

#[tokio::test]
#[should_panic(expected = "The expected chain was not dispatched")]
async fn chains_with_different_data_fail_the_assertion() {
    let _app = app_default();
    Bus::fake();

    Bus::chain(vec![
        Box::new(ShipOrder { order_id: 1 }),
        Box::new(RecordShipment),
    ])
    .dispatch()
    .await
    .unwrap();

    Bus::assert_chained_jobs(vec![
        Box::new(ShipOrder { order_id: 2 }),
        Box::new(RecordShipment),
    ]);
}

#[tokio::test]
async fn the_bus_fake_records_batches() {
    let _app = app_default();
    Bus::fake();

    let batch = Bus::batch(vec![
        Box::new(ShipOrder { order_id: 1 }),
        Box::new(ShipOrder { order_id: 2 }),
    ])
    .name("Ship")
    .dispatch()
    .await
    .unwrap();

    assert_eq!(batch.total_jobs, 2);
    assert_eq!(
        Bus::find_batch(&batch.id).await.unwrap().unwrap().name,
        "Ship"
    );

    Bus::assert_batched(|batch: &PendingBatch| {
        batch.name == "Ship"
            && batch.jobs.len() == 2
            && batch.has_job_matching::<ShipOrder>(|job| job.order_id == 2)
    });
    Bus::assert_batch_count(1);
    Bus::assert_nothing_dispatched();
    assert!(recorded().is_empty());
}

#[tokio::test]
#[should_panic(expected = "The expected batch was not dispatched.")]
async fn missing_batches_fail_the_assertion() {
    let _app = app_default();
    Bus::fake();

    Bus::assert_batched(|_| true);
}

#[tokio::test]
async fn the_bus_fake_may_fake_only_some_jobs() {
    let _app = app_default();
    Bus::fake().except::<RecordShipment>();

    ShipOrder { order_id: 1 }.dispatch_sync().await.unwrap();
    RecordShipment.dispatch_sync().await.unwrap();

    Bus::assert_dispatched_sync::<ShipOrder>();
    Bus::assert_not_dispatched::<RecordShipment>();
    assert_eq!(recorded(), vec!["recorded"]);
}

#[derive(Serialize, Deserialize)]
struct ProcessPodcast;

#[async_trait]
impl ShouldQueue for ProcessPodcast {
    async fn handle(&self) -> Result<()> {
        if self.attempts() > 1 {
            return self.fail("Too many attempts.").await;
        }
        self.release(30).await
    }
}

#[tokio::test]
async fn job_queue_interactions_can_be_tested() {
    let _app = app_default();

    let job = QueuedJob::fake();
    with_job(job.clone(), ProcessPodcast.handle())
        .await
        .unwrap();
    job.assert_released(Some(30));
    job.assert_not_deleted();
    job.assert_not_failed();

    // Outside a worker the interactions do nothing.
    ProcessPodcast.handle().await.unwrap();
}

#[tokio::test]
async fn the_bus_fake_and_queue_fake_work_together() {
    let _app = app_default();
    Queue::fake();

    // Without a bus fake, dispatches reach the (fake) queue.
    ShipOrder { order_id: 1 }.dispatch().await.unwrap();
    Queue::assert_pushed::<ShipOrder>();

    Bus::fake();
    ShipOrder { order_id: 2 }.dispatch().await.unwrap();
    Bus::assert_dispatched_with::<ShipOrder>(|job| job.order_id == 2);
    Queue::assert_count(1);
}
