//! Failed jobs (retrying, forgetting, flushing), failover connections and
//! encrypted jobs.

mod common;

use std::sync::{Arc, Mutex};

use common::{app_default, app_with, config, record, recorded};
use illuminate_queue::events::QueueFailedOver;
use illuminate_queue::{
    Dispatchable, InteractsWithQueue, JobEncrypter, Queue, ShouldQueue, Worker, WorkerOptions,
    async_trait,
};
use illuminate_support::{Result, json};
use serde::{Deserialize, Serialize};

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
        let gateway_up = common::recorded().iter().any(|entry| entry == "gateway up");
        if !gateway_up {
            return Err(illuminate_support::error::RuntimeException::new("Gateway down").into());
        }
        Ok(())
    }
}

async fn work() {
    Worker::make()
        .daemon(
            "array",
            "default,payments",
            &WorkerOptions::new().sleep(0.0).stop_when_empty(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn failed_jobs_can_be_listed_and_retried() {
    let _app = app_default();

    ChargeCard { order_id: 1 }.dispatch().await.unwrap();
    ChargeCard { order_id: 2 }
        .dispatch()
        .on_queue("payments")
        .await
        .unwrap();
    work().await;

    // Newest failures first.
    let failed = Queue::failed_jobs().await.unwrap();
    assert_eq!(failed.len(), 2);
    assert_eq!(failed[0].queue, "payments");
    assert_eq!(failed[1].queue, "default");
    assert_eq!(failed[1].exception_message(), "Gateway down");

    // The gateway comes back up; `queue:retry {id}`...
    record("gateway up");
    let id = failed[1].id.clone();
    assert!(Queue::retry_failed(&id).await.unwrap());
    assert!(!Queue::retry_failed("missing").await.unwrap());
    assert!(Queue::find_failed(&id).await.unwrap().is_none());

    work().await;

    // ...and the job runs again, its attempts reset.
    assert_eq!(
        recorded(),
        vec![
            "charge:1 attempt:1",
            "charge:2 attempt:1",
            "gateway up",
            "charge:1 attempt:1",
        ]
    );

    // `queue:retry --queue=payments`
    let retried = Queue::retry_all_failed(Some("payments")).await.unwrap();
    assert_eq!(retried.len(), 1);
    work().await;
    assert_eq!(recorded().last().unwrap(), "charge:2 attempt:1");
    assert!(Queue::failed_jobs().await.unwrap().is_empty());
}

#[tokio::test]
async fn failed_jobs_can_be_forgotten_and_flushed() {
    let _app = app_default();

    for order_id in 1..=3 {
        ChargeCard { order_id }.dispatch().await.unwrap();
    }
    work().await;

    let failed = Queue::failed_jobs().await.unwrap();
    assert_eq!(failed.len(), 3);

    assert!(Queue::forget_failed(&failed[0].id).await.unwrap());
    assert_eq!(Queue::failed_jobs().await.unwrap().len(), 2);

    // `queue:flush --hours=1` keeps recent failures...
    Queue::flush_failed(Some(1)).await.unwrap();
    assert_eq!(Queue::failed_jobs().await.unwrap().len(), 2);

    // ...`queue:flush` removes them all.
    Queue::flush_failed(None).await.unwrap();
    assert!(Queue::failed_jobs().await.unwrap().is_empty());
}

#[tokio::test]
async fn failed_jobs_can_be_discarded() {
    let mut config = config();
    config["queue"]["failed"] = json!({"driver": "null"});
    let _app = app_with(config);

    ChargeCard { order_id: 1 }.dispatch().await.unwrap();
    work().await;

    assert!(Queue::failed_jobs().await.unwrap().is_empty());
    assert_eq!(Queue::size(None).await.unwrap(), 0);
}

#[derive(Serialize, Deserialize)]
struct Notify;

#[async_trait]
impl ShouldQueue for Notify {
    async fn handle(&self) -> Result<()> {
        record("notified");
        Ok(())
    }
}

#[tokio::test]
async fn failover_connections_try_the_next_connection() {
    let mut config = config();
    config["queue"]["default"] = json!("failover");
    config["queue"]["connections"]["failover"] = json!({
        "driver": "failover",
        "connections": ["broken", "array"],
    });
    config["queue"]["connections"]["broken"] = json!({"driver": "redis"});
    let _app = app_with(config);

    let failovers = Arc::new(Mutex::new(Vec::new()));
    let log = failovers.clone();
    Queue::listen(move |event: &QueueFailedOver| {
        log.lock().unwrap().push(format!(
            "{} -> {}",
            event.failed_connection, event.exception
        ));
    });

    Notify.dispatch().await.unwrap();

    assert_eq!(
        *failovers.lock().unwrap(),
        vec!["broken -> No connector for [redis]."]
    );
    assert_eq!(
        Queue::connection("array")
            .unwrap()
            .size(None)
            .await
            .unwrap(),
        1
    );
}

/// A toy encrypter: reverses the string.
struct Reverser;

impl JobEncrypter for Reverser {
    fn encrypt(&self, value: &str) -> Result<String> {
        Ok(value.chars().rev().collect())
    }

    fn decrypt(&self, payload: &str) -> Result<String> {
        Ok(payload.chars().rev().collect())
    }
}

#[derive(Serialize, Deserialize)]
struct SendSecret {
    secret: String,
}

#[async_trait]
impl ShouldQueue for SendSecret {
    async fn handle(&self) -> Result<()> {
        record(format!("secret:{}", self.secret));
        Ok(())
    }

    fn should_be_encrypted(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn encrypted_jobs_hide_their_data() {
    let app = app_default();
    app.container
        .instance_arc::<dyn JobEncrypter>(Arc::new(Reverser));
    let payloads = Arc::new(Mutex::new(Vec::new()));
    let log = payloads.clone();
    Queue::listen(move |event: &illuminate_queue::events::JobQueued| {
        log.lock().unwrap().push(event.payload.clone())
    });

    SendSecret {
        secret: "hunter2".into(),
    }
    .dispatch()
    .await
    .unwrap();

    let payload = payloads.lock().unwrap()[0].clone();
    assert!(!payload.contains("hunter2"));
    assert!(payload.contains(r#""encrypted":true"#));

    work().await;
    assert_eq!(recorded(), vec!["secret:hunter2"]);
}
