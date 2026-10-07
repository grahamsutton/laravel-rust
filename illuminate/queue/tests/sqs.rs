//! The `sqs` driver against a fake SQS API served by `Http::fake()`: the
//! exact requests, the parsed responses, delays, releases, batches, FIFO
//! queues, overflow storage, and the worker loop.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{TestApp, app_with, record, recorded};
use illuminate_http_client::{FakeResponse, Http, Request};
use illuminate_queue::contracts::Queue as QueueContract;
use illuminate_queue::events::JobQueued;
use illuminate_queue::{
    Bus, Dispatchable, Envelope, InteractsWithQueue, Queue, QueueManager, ShouldQueue, SqsQueue,
    Worker, WorkerOptions, async_trait,
};
use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Result, Value, json};
use serde::{Deserialize, Serialize};

const PREFIX: &str = "https://sqs.us-east-1.amazonaws.com/123456789012";
const NOW: i64 = 1_700_000_000;

// ----------------------------------------------------------------------
// A fake SQS
// ----------------------------------------------------------------------

#[derive(Clone)]
struct Message {
    id: String,
    body: String,
    receives: u32,
    visible_at: i64,
    receipt: Option<String>,
    group: Option<String>,
}

#[derive(Default)]
struct State {
    queues: HashMap<String, Vec<Message>>,
    next_id: u64,
}

/// SQS queues held in memory, with visibility timeouts of 30 seconds.
#[derive(Clone, Default)]
struct FakeSqs {
    state: Arc<Mutex<State>>,
}

impl FakeSqs {
    fn install() -> Self {
        let fake = Self::default();
        let handler = fake.clone();
        Http::fake_using(move |request: &Request| handler.handle(request));
        fake
    }

    fn messages(&self, queue: &str) -> Vec<Message> {
        self.state
            .lock()
            .unwrap()
            .queues
            .get(&format!("{PREFIX}/{queue}"))
            .cloned()
            .unwrap_or_default()
    }

    fn handle(&self, request: &Request) -> FakeResponse {
        assert!(request.has_header("Authorization"), "requests are signed");
        assert!(request.has_header("X-Amz-Date"));
        assert!(request.has_header_value("Content-Type", "application/x-amz-json-1.0"));
        assert_eq!(request.url(), "https://sqs.us-east-1.amazonaws.com/");
        let operation = request.header("X-Amz-Target")[0]
            .strip_prefix("AmazonSQS.")
            .unwrap()
            .to_string();
        let input = request.data().clone();
        let url = input["QueueUrl"].as_str().unwrap().to_string();
        let now = Carbon::now().timestamp();
        let mut state = self.state.lock().unwrap();

        let send = |state: &mut State, entry: &Value| -> String {
            state.next_id += 1;
            let id = format!("message-{}", state.next_id);
            let delay = entry["DelaySeconds"].as_i64().unwrap_or(0);
            state.queues.entry(url.clone()).or_default().push(Message {
                id: id.clone(),
                body: entry["MessageBody"].as_str().unwrap().to_string(),
                receives: 0,
                visible_at: now + delay,
                receipt: None,
                group: entry["MessageGroupId"].as_str().map(String::from),
            });
            id
        };

        match operation.as_str() {
            "SendMessage" => {
                let id = send(&mut state, &input);
                Http::response(
                    json!({"MD5OfMessageBody": "md5", "MessageId": id}),
                    200,
                    &[],
                )
            }
            "SendMessageBatch" => {
                let mut successful = Vec::new();
                let mut failed = Vec::new();
                for entry in input["Entries"].as_array().unwrap() {
                    if entry["MessageBody"].as_str().unwrap().contains("Rejected") {
                        failed.push(json!({"Id": entry["Id"], "Code": "InvalidMessageContents", "Message": "Nope", "SenderFault": true}));
                        continue;
                    }
                    let id = send(&mut state, entry);
                    successful.push(
                        json!({"Id": entry["Id"], "MessageId": id, "MD5OfMessageBody": "md5"}),
                    );
                }
                Http::response(
                    json!({"Successful": successful, "Failed": failed}),
                    200,
                    &[],
                )
            }
            "ReceiveMessage" => {
                let messages = state.queues.entry(url).or_default();
                let Some(message) = messages
                    .iter_mut()
                    .find(|message| message.visible_at <= now)
                else {
                    return Http::response(json!({}), 200, &[]);
                };
                message.receives += 1;
                message.visible_at = now + 30;
                let receipt = format!("receipt-{}-{}", message.id, message.receives);
                message.receipt = Some(receipt.clone());
                Http::response(
                    json!({"Messages": [{
                        "MessageId": message.id,
                        "ReceiptHandle": receipt,
                        "MD5OfBody": "md5",
                        "Body": message.body,
                        "Attributes": {"ApproximateReceiveCount": message.receives.to_string()},
                    }]}),
                    200,
                    &[],
                )
            }
            "DeleteMessage" => {
                let receipt = input["ReceiptHandle"].as_str().unwrap();
                state
                    .queues
                    .entry(url)
                    .or_default()
                    .retain(|message| message.receipt.as_deref() != Some(receipt));
                Http::response(json!({}), 200, &[])
            }
            "ChangeMessageVisibility" => {
                let receipt = input["ReceiptHandle"].as_str().unwrap();
                let timeout = input["VisibilityTimeout"].as_i64().unwrap();
                for message in state.queues.entry(url).or_default() {
                    if message.receipt.as_deref() == Some(receipt) {
                        message.visible_at = now + timeout;
                    }
                }
                Http::response(json!({}), 200, &[])
            }
            "GetQueueAttributes" => {
                let messages = state.queues.entry(url).or_default();
                let count = |predicate: &dyn Fn(&Message) -> bool| {
                    messages
                        .iter()
                        .filter(|message| predicate(message))
                        .count()
                        .to_string()
                };
                let mut attributes = serde_json::Map::new();
                for name in input["AttributeNames"].as_array().unwrap() {
                    let name = name.as_str().unwrap();
                    let value = match name {
                        "ApproximateNumberOfMessages" => count(&|m| m.visible_at <= now),
                        "ApproximateNumberOfMessagesDelayed" => {
                            count(&|m| m.visible_at > now && m.receives == 0)
                        }
                        "ApproximateNumberOfMessagesNotVisible" => {
                            count(&|m| m.visible_at > now && m.receives > 0)
                        }
                        other => panic!("Unexpected attribute [{other}]"),
                    };
                    attributes.insert(name.to_string(), json!(value));
                }
                Http::response(json!({"Attributes": attributes}), 200, &[])
            }
            "PurgeQueue" => {
                state.queues.remove(&url);
                Http::response(json!({}), 200, &[])
            }
            other => panic!("Unexpected operation [{other}]"),
        }
    }
}

// ----------------------------------------------------------------------
// Setup
// ----------------------------------------------------------------------

fn connection_config() -> Value {
    json!({
        "driver": "sqs",
        "key": "AKIDEXAMPLE",
        "secret": "secret",
        "prefix": PREFIX,
        "queue": "default",
        "suffix": null,
        "region": "us-east-1",
        "after_commit": false,
    })
}

fn app_with_connection(connection: Value) -> TestApp {
    let mut config = common::config();
    config["queue"]["default"] = json!("sqs");
    config["queue"]["connections"]["sqs"] = connection;
    config["cache"]["stores"]["payloads"] = json!({"driver": "array"});
    app_with(config)
}

fn app() -> TestApp {
    app_with_connection(connection_config())
}

fn queue() -> Arc<dyn QueueContract> {
    Queue::connection("sqs").unwrap()
}

/// Freeze "now" on this thread (the queue and the fake share it).
struct Frozen;

impl Frozen {
    fn at(timestamp: i64) -> Self {
        Carbon::set_thread_test_now(Some(Carbon::from_timestamp(timestamp)));
        Self
    }

    fn travel(&self, seconds: i64) {
        Carbon::set_thread_test_now(Some(Carbon::now().add_seconds(seconds)));
    }
}

impl Drop for Frozen {
    fn drop(&mut self) {
        Carbon::set_thread_test_now(None);
    }
}

fn requests() -> Vec<Request> {
    Http::recorded()
        .into_vec()
        .into_iter()
        .map(|(request, _)| request)
        .collect()
}

fn last_request() -> Request {
    requests().pop().unwrap()
}

fn target(request: &Request) -> String {
    request.header("X-Amz-Target")[0].clone()
}

fn decode(payload: &str) -> Value {
    serde_json::from_str(payload).unwrap()
}

async fn work() -> u64 {
    let worker = Worker::make();
    worker
        .daemon(
            "sqs",
            "default",
            &WorkerOptions::new().sleep(0.0).stop_when_empty(),
        )
        .await
        .unwrap();
    worker.jobs_processed()
}

// ----------------------------------------------------------------------
// Jobs
// ----------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct SendInvoice {
    id: u64,
}

#[async_trait]
impl ShouldQueue for SendInvoice {
    async fn handle(&self) -> Result<()> {
        record(format!("invoice:{} attempt:{}", self.id, self.attempts()));
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct FlakyJob;

#[async_trait]
impl ShouldQueue for FlakyJob {
    async fn handle(&self) -> Result<()> {
        record(format!("flaky attempt:{}", self.attempts()));
        if self.attempts() < 2 {
            return Err(RuntimeException::new("Try again").into());
        }
        Ok(())
    }

    fn tries(&self) -> Option<u32> {
        Some(3)
    }
}

#[derive(Serialize, Deserialize)]
struct ProcessOrder {
    customer: u64,
}

#[async_trait]
impl ShouldQueue for ProcessOrder {
    async fn handle(&self) -> Result<()> {
        Ok(())
    }

    fn message_group(&self) -> Option<String> {
        Some(format!("customer-{}", self.customer))
    }

    fn deduplication_id(&self, _payload: &str, queue: &str) -> Option<String> {
        Some(format!("order-{}-{queue}", self.customer))
    }
}

// ----------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------

#[tokio::test]
async fn jobs_are_sent_as_messages() {
    let _app = app();
    let _now = Frozen::at(NOW);
    let fake = FakeSqs::install();
    let ids = Arc::new(Mutex::new(Vec::new()));
    let queued = ids.clone();
    Queue::listen(move |event: &JobQueued| queued.lock().unwrap().push(event.id.clone()));

    SendInvoice { id: 1 }.dispatch().await.unwrap();

    let send = last_request();
    assert_eq!(send.method(), "POST");
    assert_eq!(target(&send), "AmazonSQS.SendMessage");
    assert!(send.has_header_value("X-Amz-Date", "20231114T221320Z"));
    assert!(send.header("Authorization")[0].starts_with(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20231114/us-east-1/sqs/aws4_request, SignedHeaders=content-type;host;x-amz-date;x-amz-target, Signature="
    ));
    let input = send.data().clone();
    assert_eq!(
        input.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["QueueUrl", "MessageBody"]
    );
    assert_eq!(input["QueueUrl"], format!("{PREFIX}/default"));
    let payload = decode(input["MessageBody"].as_str().unwrap());
    assert_eq!(payload["displayName"], "SendInvoice");
    assert_eq!(payload["data"]["command"], json!({"id": 1}));
    assert_eq!(payload["attempts"], 0);

    assert_eq!(*ids.lock().unwrap(), vec![Some("message-1".to_string())]);
    assert_eq!(fake.messages("default").len(), 1);

    let id = queue()
        .push_raw(
            r#"{"uuid":"raw","job":"x","data":{}}"#.into(),
            Some("emails"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(id.as_deref(), Some("message-2"));
    assert_eq!(
        last_request().data().clone(),
        json!({"QueueUrl": format!("{PREFIX}/emails"), "MessageBody": r#"{"uuid":"raw","job":"x","data":{}}"#})
    );
}

#[tokio::test]
async fn delayed_jobs_use_delay_seconds_up_to_fifteen_minutes() {
    let _app = app();
    let now = Frozen::at(NOW);
    let fake = FakeSqs::install();

    SendInvoice { id: 1 }
        .dispatch()
        .delay(Duration::from_secs(60))
        .await
        .unwrap();
    let input = last_request().data().clone();
    assert_eq!(input["DelaySeconds"], 60);
    assert_eq!(decode(input["MessageBody"].as_str().unwrap())["delay"], 60);

    SendInvoice { id: 2 }.dispatch().delay(3600).await.unwrap();
    assert_eq!(last_request()["DelaySeconds"], 900);

    queue()
        .push_raw("{}".into(), None, Some(Duration::from_millis(1500)))
        .await
        .unwrap();
    assert_eq!(last_request()["DelaySeconds"], 2);

    assert_eq!(fake.messages("default").len(), 3);
    assert!(queue().pop(None).await.unwrap().is_none());
    assert_eq!(queue().delayed_size(None).await.unwrap(), 3);

    now.travel(60);
    let job = queue().pop(None).await.unwrap().unwrap();
    assert_eq!(decode(job.raw_body())["data"]["command"], json!({"id": 1}));
}

#[tokio::test]
async fn popped_jobs_carry_their_receive_count() {
    let _app = app();
    let now = Frozen::at(NOW);
    let _fake = FakeSqs::install();
    SendInvoice { id: 7 }.dispatch().await.unwrap();

    let job = queue().pop(None).await.unwrap().unwrap();
    let receive = last_request();
    assert_eq!(target(&receive), "AmazonSQS.ReceiveMessage");
    assert_eq!(
        receive.data().clone(),
        json!({"QueueUrl": format!("{PREFIX}/default"), "AttributeNames": ["ApproximateReceiveCount"]})
    );
    assert_eq!(job.job_id(), "message-1");
    assert_eq!(job.attempts(), 1);
    assert_eq!(job.queue(), format!("{PREFIX}/default"));
    assert_eq!(job.connection_name(), "sqs");
    assert_eq!(job.resolve_name(), "SendInvoice");

    // While it is invisible, nobody else gets it.
    assert!(queue().pop(None).await.unwrap().is_none());
    assert_eq!(queue().reserved_size(None).await.unwrap(), 1);

    // Releasing it changes its visibility timeout...
    job.release(10).await.unwrap();
    let release = last_request();
    assert_eq!(target(&release), "AmazonSQS.ChangeMessageVisibility");
    assert_eq!(
        release.data().clone(),
        json!({
            "QueueUrl": format!("{PREFIX}/default"),
            "ReceiptHandle": "receipt-message-1-1",
            "VisibilityTimeout": 10,
        })
    );
    assert!(queue().pop(None).await.unwrap().is_none());

    // ...and the next receive counts another attempt.
    now.travel(10);
    let job = queue().pop(None).await.unwrap().unwrap();
    assert_eq!(job.attempts(), 2);

    job.delete().await.unwrap();
    let delete = last_request();
    assert_eq!(target(&delete), "AmazonSQS.DeleteMessage");
    assert_eq!(
        delete.data().clone(),
        json!({"QueueUrl": format!("{PREFIX}/default"), "ReceiptHandle": "receipt-message-1-2"})
    );
    assert_eq!(queue().size(None).await.unwrap(), 0);
}

#[tokio::test]
async fn sizes_come_from_the_queue_attributes() {
    let _app = app();
    let _now = Frozen::at(NOW);
    let _fake = FakeSqs::install();
    for id in 0..3 {
        SendInvoice { id }.dispatch().await.unwrap();
    }
    SendInvoice { id: 9 }.dispatch().delay(30).await.unwrap();
    let _reserved = queue().pop(None).await.unwrap().unwrap();

    assert_eq!(queue().size(None).await.unwrap(), 4);
    assert_eq!(
        last_request().data().clone(),
        json!({
            "QueueUrl": format!("{PREFIX}/default"),
            "AttributeNames": [
                "ApproximateNumberOfMessages",
                "ApproximateNumberOfMessagesDelayed",
                "ApproximateNumberOfMessagesNotVisible",
            ],
        })
    );
    assert_eq!(queue().pending_size(None).await.unwrap(), 2);
    assert_eq!(
        last_request()["AttributeNames"],
        json!(["ApproximateNumberOfMessages"])
    );
    assert_eq!(queue().delayed_size(None).await.unwrap(), 1);
    assert_eq!(queue().reserved_size(None).await.unwrap(), 1);
    assert_eq!(Queue::size(Some("emails")).await.unwrap(), 0);
}

#[tokio::test]
async fn clearing_a_queue_purges_it() {
    let _app = app();
    let fake = FakeSqs::install();
    for id in 0..3 {
        SendInvoice { id }.dispatch().await.unwrap();
    }

    assert_eq!(queue().clear(None).await.unwrap(), 3);
    let sent = requests();
    let purge = sent.last().unwrap();
    assert_eq!(target(purge), "AmazonSQS.PurgeQueue");
    assert_eq!(
        purge.data().clone(),
        json!({"QueueUrl": format!("{PREFIX}/default")})
    );
    assert_eq!(
        target(&sent[sent.len() - 2]),
        "AmazonSQS.GetQueueAttributes"
    );
    assert!(fake.messages("default").is_empty());
}

#[tokio::test]
async fn the_worker_processes_releases_and_deletes_messages() {
    let _app = app();
    let fake = FakeSqs::install();

    SendInvoice { id: 1 }.dispatch().await.unwrap();
    FlakyJob.dispatch().await.unwrap();

    work().await;
    assert_eq!(
        recorded(),
        vec!["invoice:1 attempt:1", "flaky attempt:1", "flaky attempt:2"]
    );
    assert!(fake.messages("default").is_empty());

    let targets: Vec<String> = requests().iter().map(target).collect();
    assert!(targets.contains(&"AmazonSQS.ChangeMessageVisibility".to_string()));
    assert_eq!(
        targets
            .iter()
            .filter(|target| *target == "AmazonSQS.DeleteMessage")
            .count(),
        2
    );
}

#[tokio::test]
async fn unreadable_messages_are_deleted_and_logged_as_failed() {
    let _app = app();
    let fake = FakeSqs::install();
    queue()
        .push_raw("not json".into(), None, None)
        .await
        .unwrap();

    assert!(queue().pop(None).await.is_err());
    assert!(fake.messages("default").is_empty());
    let failed = illuminate_queue::failed::failer().all().await.unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].payload, "not json");
    assert_eq!(failed[0].queue, format!("{PREFIX}/default"));
}

#[tokio::test]
async fn fifo_queues_get_message_groups_and_deduplication_ids() {
    let _app = app_with_connection({
        let mut config = connection_config();
        config["suffix"] = json!("-production");
        config
    });
    let fake = FakeSqs::install();

    SendInvoice { id: 1 }
        .dispatch()
        .on_queue("invoices.fifo")
        .await
        .unwrap();
    let input = last_request().data().clone();
    assert_eq!(
        input["QueueUrl"],
        format!("{PREFIX}/invoices-production.fifo")
    );
    assert_eq!(input["MessageGroupId"], "invoices.fifo");
    assert_eq!(input["MessageDeduplicationId"].as_str().unwrap().len(), 36);

    ProcessOrder { customer: 7 }
        .dispatch()
        .on_queue("orders.fifo")
        .delay(30)
        .await
        .unwrap();
    let input = last_request().data().clone();
    assert_eq!(input["MessageGroupId"], "customer-7");
    assert_eq!(input["MessageDeduplicationId"], "order-7-orders.fifo");
    assert!(
        input.get("DelaySeconds").is_none(),
        "FIFO queues can't delay messages"
    );

    // On standard queues, a message group enables fair queueing.
    ProcessOrder { customer: 8 }.dispatch().await.unwrap();
    let input = last_request().data().clone();
    assert_eq!(input["QueueUrl"], format!("{PREFIX}/default-production"));
    assert_eq!(input["MessageGroupId"], "customer-8");
    assert!(input.get("MessageDeduplicationId").is_none());
    assert_eq!(
        fake.messages("default-production")[0].group.as_deref(),
        Some("customer-8")
    );
}

#[tokio::test]
async fn bulk_pushes_use_send_message_batch() {
    let _app = app();
    let fake = FakeSqs::install();
    let ids = Arc::new(Mutex::new(Vec::new()));
    let queued = ids.clone();
    Queue::listen(move |event: &JobQueued| queued.lock().unwrap().push(event.id.clone()));

    let jobs: Vec<Box<dyn ShouldQueue>> = (0..12)
        .map(|id| Box::new(SendInvoice { id }) as Box<dyn ShouldQueue>)
        .collect();
    Bus::bulk(jobs).await.unwrap();

    let batches: Vec<Request> = requests()
        .into_iter()
        .filter(|request| target(request) == "AmazonSQS.SendMessageBatch")
        .collect();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0]["QueueUrl"], format!("{PREFIX}/default"));
    let entries = batches[0]["Entries"].as_array().unwrap().clone();
    assert_eq!(entries.len(), 10);
    assert_eq!(entries[0]["Id"], "0");
    assert_eq!(
        decode(entries[0]["MessageBody"].as_str().unwrap())["data"]["command"],
        json!({"id": 0})
    );
    assert_eq!(batches[1]["Entries"].as_array().unwrap().len(), 2);
    assert_eq!(batches[1]["Entries"][1]["Id"], "11");
    assert_eq!(fake.messages("default").len(), 12);
    assert_eq!(ids.lock().unwrap().len(), 12);
    assert_eq!(ids.lock().unwrap()[11].as_deref(), Some("message-12"));

    #[derive(Serialize, Deserialize)]
    struct Rejected;

    #[async_trait]
    impl ShouldQueue for Rejected {
        async fn handle(&self) -> Result<()> {
            Ok(())
        }
    }

    let jobs = [
        Envelope::new(SendInvoice { id: 1 }),
        Envelope::new(Rejected),
    ];
    let error = queue().bulk(&jobs, Some("emails")).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "SQS SendMessageBatch rejected [1] of [2] messages. First failure [InvalidMessageContents]: Nope"
    );
    assert_eq!(fake.messages("emails").len(), 1);
}

#[tokio::test]
async fn large_payloads_overflow_into_the_cache() {
    let _app = app_with_connection({
        let mut config = connection_config();
        config["overflow"] = json!({
            "enabled": true,
            "store": "payloads",
            "always": true,
            "delete_after_processing": true,
            "flush_on_clear": true,
        });
        config
    });
    let fake = FakeSqs::install();

    SendInvoice { id: 3 }.dispatch().await.unwrap();
    let body = fake.messages("default")[0].body.clone();
    let pointer = decode(&body)["@pointer"].as_str().unwrap().to_string();
    assert!(pointer.starts_with("laravel:sqs-payloads:"));
    let cache = illuminate_cache::Cache::store("payloads").unwrap();
    assert!(cache.has(&pointer).await.unwrap());

    let job = queue().pop(None).await.unwrap().unwrap();
    assert_eq!(decode(job.raw_body())["data"]["command"], json!({"id": 3}));
    job.delete().await.unwrap();
    assert!(!cache.has(&pointer).await.unwrap());

    SendInvoice { id: 4 }.dispatch().await.unwrap();
    let body = fake.messages("default")[0].body.clone();
    let pointer = decode(&body)["@pointer"].as_str().unwrap().to_string();
    assert!(cache.has(&pointer).await.unwrap());
    queue().clear(None).await.unwrap();
    assert!(!cache.has(&pointer).await.unwrap());
}

#[tokio::test]
async fn connections_are_built_from_configuration() {
    let _app = app_with_connection(json!({
        "driver": "sqs",
        "key": "AKIDEXAMPLE",
        "secret": "secret",
        "token": "session-token",
        "prefix": "http://localhost:4566/000000000000/",
        "queue": "jobs",
        "suffix": "-local",
        "region": "eu-west-1",
        "endpoint": "http://localhost:4566",
        "after_commit": true,
    }));
    Http::fake_using(|_| Http::response(json!({"MessageId": "1"}), 200, &[]));

    let connection = queue();
    assert_eq!(connection.connection_name(), "sqs");
    assert_eq!(connection.default_queue(), "jobs");
    assert!(connection.dispatches_after_commit());

    connection.push_raw("{}".into(), None, None).await.unwrap();
    let send = last_request();
    assert_eq!(send.url(), "http://localhost:4566/");
    assert_eq!(
        send["QueueUrl"],
        "http://localhost:4566/000000000000/jobs-local"
    );
    assert!(send.has_header_value("X-Amz-Security-Token", "session-token"));
    assert!(send.header("Authorization")[0].contains("/eu-west-1/sqs/aws4_request"));

    let manager = illuminate_container::app::<QueueManager>();
    assert!(manager.connected(Some("sqs")));

    let queue = SqsQueue::from_config(&json!({"key": "k", "secret": "s"}), "other");
    assert_eq!(queue.get_queue(None), "/default");
}
