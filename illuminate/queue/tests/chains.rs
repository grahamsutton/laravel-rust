//! Job chains.

mod common;

use std::time::Duration;

use common::{app_default, record, recorded};
use illuminate_queue::events::JobQueued;
use illuminate_queue::{
    Bus, Dispatchable, InteractsWithQueue, Queue, ShouldQueue, Worker, WorkerOptions, async_trait,
};
use illuminate_support::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Step {
    name: String,
    fail: bool,
}

impl Step {
    fn ok(name: &str) -> Box<Step> {
        Box::new(Step {
            name: name.into(),
            fail: false,
        })
    }

    fn failing(name: &str) -> Box<Step> {
        Box::new(Step {
            name: name.into(),
            fail: true,
        })
    }
}

#[async_trait]
impl ShouldQueue for Step {
    async fn handle(&self) -> Result<()> {
        let queue = self
            .job()
            .map(|job| job.queue().to_string())
            .unwrap_or_default();
        record(format!("{}@{queue}", self.name));
        if self.fail {
            return Err(illuminate_support::error::RuntimeException::new(format!(
                "{} broke",
                self.name
            ))
            .into());
        }
        Ok(())
    }

    async fn failed(&self, error: &Error) -> Result<()> {
        record(format!("failed:{error}"));
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct Extends;

#[async_trait]
impl ShouldQueue for Extends {
    async fn handle(&self) -> Result<()> {
        record("extends");
        self.prepend_to_chain(Step {
            name: "prepended".into(),
            fail: false,
        });
        self.append_to_chain(Step {
            name: "appended".into(),
            fail: false,
        });
        Ok(())
    }
}

async fn work() {
    Worker::make()
        .daemon(
            "array",
            "default,podcasts",
            &WorkerOptions::new().sleep(0.0).stop_when_empty(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn chained_jobs_run_in_order() {
    let _app = app_default();

    let payloads = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = payloads.clone();
    Queue::listen(move |event: &JobQueued| log.lock().unwrap().push(event.payload.clone()));

    Bus::chain(vec![
        Step::ok("first"),
        Step::ok("second"),
        Step::ok("third"),
    ])
    .dispatch()
    .await
    .unwrap();

    // Only the first job is on the queue; it carries the rest of the chain.
    assert_eq!(Queue::size(None).await.unwrap(), 1);
    let payload: illuminate_support::Value =
        serde_json::from_str(&payloads.lock().unwrap()[0]).unwrap();
    let chained = payload["data"]["chained"].as_array().unwrap();
    assert_eq!(chained.len(), 2);
    assert_eq!(chained[0]["command"]["name"], "second");

    work().await;

    assert_eq!(
        recorded(),
        vec!["first@default", "second@default", "third@default"]
    );
}

#[tokio::test]
async fn a_failure_stops_the_chain_and_calls_catch() {
    let _app = app_default();

    Bus::chain(vec![
        Step::ok("first"),
        Step::failing("second"),
        Step::ok("third"),
    ])
    .catch(|error| async move {
        record(format!("catch:{error}"));
        Ok(())
    })
    .dispatch()
    .await
    .unwrap();

    work().await;

    assert_eq!(
        recorded(),
        vec![
            "first@default",
            "second@default",
            "catch:second broke",
            "failed:second broke",
        ]
    );
    assert_eq!(Queue::failed_jobs().await.unwrap().len(), 1);
}

#[tokio::test]
async fn chains_can_run_on_a_connection_and_queue() {
    let _app = app_default();

    Bus::chain(vec![Step::ok("first"), Step::ok("second")])
        .on_queue("podcasts")
        .dispatch()
        .await
        .unwrap();

    assert_eq!(Queue::size(Some("podcasts")).await.unwrap(), 1);
    work().await;
    assert_eq!(recorded(), vec!["first@podcasts", "second@podcasts"]);
}

#[tokio::test]
async fn chains_run_synchronously_on_the_sync_connection() {
    let _app = app_default();

    Bus::chain(vec![Step::ok("first"), Step::ok("second")])
        .on_connection("sync")
        .dispatch()
        .await
        .unwrap();

    assert_eq!(recorded(), vec!["first@default", "second@default"]);
}

#[tokio::test]
async fn jobs_can_extend_their_chain() {
    let _app = app_default();

    Bus::chain(vec![Box::new(Extends), Step::ok("last")])
        .dispatch()
        .await
        .unwrap();
    work().await;

    assert_eq!(
        recorded(),
        vec![
            "extends",
            "prepended@default",
            "last@default",
            "appended@default"
        ]
    );
}

#[tokio::test]
async fn with_chain_starts_a_chain_from_a_job() {
    let _app = app_default();

    Step {
        name: "first".into(),
        fail: false,
    }
    .with_chain(vec![Step::ok("second")])
    .prepend(Step {
        name: "zero".into(),
        fail: false,
    })
    .append(Step {
        name: "third".into(),
        fail: false,
    })
    .dispatch()
    .await
    .unwrap();
    work().await;

    assert_eq!(
        recorded(),
        vec![
            "zero@default",
            "first@default",
            "second@default",
            "third@default"
        ]
    );
}

#[tokio::test]
async fn pending_dispatch_can_carry_a_chain() {
    let _app = app_default();

    Step {
        name: "first".into(),
        fail: false,
    }
    .dispatch()
    .chain(vec![Step::ok("second")])
    .await
    .unwrap();
    work().await;

    assert_eq!(recorded(), vec!["first@default", "second@default"]);
}

#[tokio::test]
async fn delayed_chains_delay_the_first_job() {
    let _app = app_default();

    Bus::chain(vec![Step::ok("first"), Step::ok("second")])
        .delay(Duration::from_secs(60))
        .dispatch()
        .await
        .unwrap();

    let queue = Queue::default_connection().unwrap();
    assert_eq!(queue.delayed_size(None).await.unwrap(), 1);
}

#[derive(Serialize, Deserialize)]
struct NotifyAdmin;

#[async_trait]
impl ShouldQueue for NotifyAdmin {
    async fn handle(&self) -> Result<()> {
        record("admin notified");
        Ok(())
    }
}

#[tokio::test]
async fn catch_callbacks_may_dispatch_jobs() {
    let _app = app_default();

    Bus::chain(vec![Step::failing("first"), Step::ok("second")])
        .catch_dispatch(NotifyAdmin)
        .dispatch()
        .await
        .unwrap();
    work().await;

    assert_eq!(
        recorded(),
        vec!["first@default", "failed:first broke", "admin notified"]
    );
}

#[tokio::test]
async fn nested_chains_are_flattened() {
    let _app = app_default();

    Bus::chain(vec![
        Step::ok("first"),
        Box::new(Bus::chain(vec![Step::ok("second"), Step::ok("third")])),
    ])
    .dispatch()
    .await
    .unwrap();
    work().await;

    assert_eq!(
        recorded(),
        vec!["first@default", "second@default", "third@default"]
    );
}

#[tokio::test]
async fn empty_chains_do_nothing() {
    let _app = app_default();

    Bus::chain(Vec::new()).dispatch().await.unwrap();
    Bus::chain(vec![Step::ok("skipped")])
        .dispatch_if(false)
        .await
        .unwrap();

    assert_eq!(Queue::size(None).await.unwrap(), 0);
}
