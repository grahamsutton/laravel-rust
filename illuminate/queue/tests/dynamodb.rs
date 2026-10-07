//! The DynamoDB failed job provider and batch repository against a fake
//! DynamoDB API served by `Http::fake()`.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use common::{TestApp, app_with, record, recorded};
use illuminate_cache::DynamoDbClient;
use illuminate_http_client::{FakeResponse, Http, Request};
use illuminate_queue::{
    Batch, BatchRepository, Bus, DynamoBatchRepository, DynamoDbFailedJobProvider, Envelope,
    FailedJobProvider, InteractsWithQueue, Queue, ShouldQueue, Worker, WorkerOptions, async_trait,
};
use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Result, Value, json};
use serde::{Deserialize, Serialize};

const NOW: i64 = 1_700_000_000;

// ----------------------------------------------------------------------
// A fake DynamoDB
// ----------------------------------------------------------------------

/// A table's items, by partition and sort key.
type Table = BTreeMap<(String, String), Value>;

/// DynamoDB tables keyed by `application` and a sort key (`uuid` or `id`),
/// understanding `SET` update expressions with `+`, `-` and `list_append`.
#[derive(Clone, Default)]
struct FakeDynamo {
    tables: Arc<Mutex<BTreeMap<String, Table>>>,
}

fn sort_key(item: &Value) -> String {
    item.get("uuid")
        .or(item.get("id"))
        .and_then(|key| key["S"].as_str())
        .unwrap()
        .to_string()
}

fn number(attribute: &Value) -> i64 {
    attribute["N"].as_str().unwrap().parse().unwrap()
}

/// Split `a = b, c = list_append(c, :d)` into its assignments.
fn assignments(expression: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut depth = 0;
    for c in expression.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(String::new());
                continue;
            }
            _ => {}
        }
        parts.last_mut().unwrap().push(c);
    }
    parts.iter().map(|part| part.trim().to_string()).collect()
}

impl FakeDynamo {
    fn install() -> Self {
        let fake = Self::default();
        let handler = fake.clone();
        Http::fake_using(move |request: &Request| handler.handle(request));
        fake
    }

    fn items(&self, table: &str) -> Vec<Value> {
        self.tables
            .lock()
            .unwrap()
            .get(table)
            .map(|items| items.values().cloned().collect())
            .unwrap_or_default()
    }

    fn handle(&self, request: &Request) -> FakeResponse {
        assert!(request.has_header("Authorization"), "requests are signed");
        let operation = request.header("X-Amz-Target")[0]
            .strip_prefix("DynamoDB_20120810.")
            .unwrap()
            .to_string();
        let input = request.data().clone();
        let table = input["TableName"].as_str().unwrap().to_string();
        let mut tables = self.tables.lock().unwrap();
        let items = tables.entry(table).or_default();
        let key = |key: &Value| {
            (
                key["application"]["S"].as_str().unwrap().to_string(),
                sort_key(key),
            )
        };

        match operation.as_str() {
            "PutItem" => {
                let item = input["Item"].clone();
                items.insert(key(&item), item);
                Http::response(json!({}), 200, &[])
            }
            "GetItem" => match items.get(&key(&input["Key"])) {
                Some(item) => Http::response(json!({"Item": item}), 200, &[]),
                None => Http::response(json!({}), 200, &[]),
            },
            "DeleteItem" => {
                items.remove(&key(&input["Key"]));
                Http::response(json!({}), 200, &[])
            }
            "Query" => {
                let values = &input["ExpressionAttributeValues"];
                let application = values[":application"]["S"].as_str().unwrap();
                let before = values
                    .get(":id")
                    .map(|id| id["S"].as_str().unwrap().to_string());
                let mut found: Vec<Value> = items
                    .iter()
                    .filter(|((app, sort), _)| {
                        app == application && before.as_ref().is_none_or(|before| sort < before)
                    })
                    .map(|(_, item)| item.clone())
                    .collect();
                if input["ScanIndexForward"] == false {
                    found.reverse();
                }
                if let Some(limit) = input["Limit"].as_u64() {
                    found.truncate(limit as usize);
                }
                Http::response(json!({"Items": found, "Count": found.len()}), 200, &[])
            }
            "UpdateItem" => {
                let names = input["ExpressionAttributeNames"].clone();
                let values = input["ExpressionAttributeValues"].clone();
                let Some(item) = items.get_mut(&key(&input["Key"])) else {
                    return Http::response(
                        json!({"__type": "ValidationException", "message": "missing"}),
                        400,
                        &[],
                    );
                };
                let expression = input["UpdateExpression"].as_str().unwrap();
                let name = |name: &str| match name.strip_prefix('#') {
                    Some(_) => names[name].as_str().unwrap().to_string(),
                    None => name.to_string(),
                };
                for assignment in assignments(expression.strip_prefix("SET ").unwrap()) {
                    let (target, value) = assignment.split_once(" = ").unwrap();
                    let new = if let Some(arguments) = value
                        .strip_prefix("list_append(")
                        .and_then(|rest| rest.strip_suffix(')'))
                    {
                        let (list, addition) = arguments.split_once(", ").unwrap();
                        let mut list = item[name(list)]["L"].as_array().unwrap().clone();
                        list.extend(values[addition]["L"].as_array().unwrap().clone());
                        json!({"L": list})
                    } else if let Some((attribute, operand)) = value.split_once(" + ") {
                        json!({"N": (number(&item[name(attribute)]) + number(&values[operand])).to_string()})
                    } else if let Some((attribute, operand)) = value.split_once(" - ") {
                        json!({"N": (number(&item[name(attribute)]) - number(&values[operand])).to_string()})
                    } else {
                        values[value].clone()
                    };
                    item[name(target)] = new;
                }
                let output = if input["ReturnValues"] == "ALL_NEW" {
                    json!({"Attributes": item})
                } else {
                    json!({})
                };
                Http::response(output, 200, &[])
            }
            other => panic!("Unexpected operation [{other}]"),
        }
    }
}

// ----------------------------------------------------------------------
// Setup
// ----------------------------------------------------------------------

fn client() -> DynamoDbClient {
    DynamoDbClient::from_config(
        &json!({"key": "AKIDEXAMPLE", "secret": "secret", "region": "us-east-1"}),
    )
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

/// Freeze "now" on this thread.
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

fn app() -> TestApp {
    let mut config = common::config();
    config["app"] = json!({"name": "Podcasts"});
    config["queue"]["failed"] = json!({
        "driver": "dynamodb",
        "key": "AKIDEXAMPLE",
        "secret": "secret",
        "region": "us-east-1",
        "table": "failed_jobs",
    });
    config["queue"]["batching"] = json!({
        "driver": "dynamodb",
        "key": "AKIDEXAMPLE",
        "secret": "secret",
        "region": "us-east-1",
        "table": "job_batches",
        "ttl": 3600,
        "ttl_attribute": "expires",
    });
    app_with(config)
}

fn payload(uuid: &str) -> String {
    json!({"uuid": uuid, "displayName": "ProcessPodcast", "job": "x", "data": {}}).to_string()
}

// ----------------------------------------------------------------------
// Failed jobs
// ----------------------------------------------------------------------

#[tokio::test]
async fn failed_jobs_are_logged_as_items() {
    let _container = common::app_default();
    let now = Frozen::at(NOW);
    let fake = FakeDynamo::install();
    let provider = DynamoDbFailedJobProvider::new(client(), "Podcasts", "failed_jobs");

    let id = provider
        .log(
            "sqs",
            "default",
            &payload("first"),
            "RuntimeException: Boom",
        )
        .await
        .unwrap();
    assert_eq!(id.as_deref(), Some("first"));
    let put = last_request();
    assert_eq!(target(&put), "DynamoDB_20120810.PutItem");
    assert_eq!(
        put.data().clone(),
        json!({
            "TableName": "failed_jobs",
            "Item": {
                "application": {"S": "Podcasts"},
                "uuid": {"S": "first"},
                "connection": {"S": "sqs"},
                "queue": {"S": "default"},
                "payload": {"S": payload("first")},
                "exception": {"S": "RuntimeException: Boom"},
                "failed_at": {"N": "1700000000"},
                "expires_at": {"N": "1700604800"},
            },
        })
    );
    assert_eq!(fake.items("failed_jobs").len(), 1);

    now.travel(60);
    provider
        .log("redis", "emails", &payload("second"), "Exception")
        .await
        .unwrap();

    let all = provider.all().await.unwrap();
    assert_eq!(
        last_request().data().clone(),
        json!({
            "TableName": "failed_jobs",
            "Select": "ALL_ATTRIBUTES",
            "KeyConditionExpression": "application = :application",
            "ExpressionAttributeValues": {":application": {"S": "Podcasts"}},
            "ScanIndexForward": false,
        })
    );
    assert_eq!(
        all.iter().map(|job| job.id.as_str()).collect::<Vec<_>>(),
        ["second", "first"]
    );
    assert_eq!(all[0].failed_at.timestamp(), NOW + 60);
    assert_eq!(all[1].exception, "RuntimeException: Boom");
    assert_eq!(provider.ids(Some("emails")).await.unwrap(), ["second"]);

    let found = provider.find("first").await.unwrap().unwrap();
    assert_eq!(
        last_request().data().clone(),
        json!({"TableName": "failed_jobs", "Key": {"application": {"S": "Podcasts"}, "uuid": {"S": "first"}}})
    );
    assert_eq!(found.connection, "sqs");
    assert_eq!(found.display_name(), "ProcessPodcast");
    assert!(provider.find("missing").await.unwrap().is_none());

    assert!(provider.forget("first").await.unwrap());
    assert_eq!(target(&last_request()), "DynamoDB_20120810.DeleteItem");
    assert_eq!(fake.items("failed_jobs").len(), 1);

    assert_eq!(
        provider.flush(None).await.unwrap_err().to_string(),
        "DynamoDb failed job storage may not be flushed. Please use DynamoDb's TTL features on your expires_at attribute."
    );
}

#[derive(Serialize, Deserialize)]
struct AlwaysFails;

#[async_trait]
impl ShouldQueue for AlwaysFails {
    async fn handle(&self) -> Result<()> {
        Err(RuntimeException::new("Boom").into())
    }
}

#[tokio::test]
async fn workers_log_failed_jobs_to_dynamodb_when_configured() {
    let _app = app();
    let fake = FakeDynamo::install();

    Queue::push(Envelope::new(AlwaysFails)).await.unwrap();
    Worker::make()
        .daemon(
            "array",
            "default",
            &WorkerOptions::new().sleep(0.0).stop_when_empty(),
        )
        .await
        .unwrap();

    let items = fake.items("failed_jobs");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["application"], json!({"S": "Podcasts"}));
    assert_eq!(items[0]["connection"], json!({"S": "array"}));
    assert!(
        items[0]["exception"]["S"]
            .as_str()
            .unwrap()
            .contains("Boom")
    );
    let failed = Queue::failed_jobs().await.unwrap();
    assert_eq!(failed.len(), 1);
}

// ----------------------------------------------------------------------
// Batches
// ----------------------------------------------------------------------

#[tokio::test]
async fn batches_are_stored_as_items() {
    let _container = common::app_default();
    let _now = Frozen::at(NOW);
    let fake = FakeDynamo::install();
    let repository = DynamoBatchRepository::new(client(), "Podcasts", "job_batches");

    let batch = repository
        .store("Import", &json!({"queue": "imports"}))
        .await
        .unwrap();
    let sent = requests();
    let put = &sent[0];
    assert_eq!(target(put), "DynamoDB_20120810.PutItem");
    assert_eq!(
        put.data().clone(),
        json!({
            "TableName": "job_batches",
            "Item": {
                "application": {"S": "Podcasts"},
                "id": {"S": batch.id},
                "name": {"S": "Import"},
                "total_jobs": {"N": "0"},
                "pending_jobs": {"N": "0"},
                "failed_jobs": {"N": "0"},
                "failed_job_ids": {"L": []},
                "options": {"S": "{\"queue\":\"imports\"}"},
                "created_at": {"N": "1700000000"},
                "cancelled_at": {"NULL": true},
                "finished_at": {"NULL": true},
            },
        })
    );
    assert_eq!(target(&sent[1]), "DynamoDB_20120810.GetItem");
    assert_eq!(batch.name, "Import");
    assert_eq!(batch.options, json!({"queue": "imports"}));
    assert_eq!(batch.created_at.timestamp(), NOW);
    assert!(batch.finished_at.is_none());

    repository.increment_total_jobs(&batch.id, 3).await.unwrap();
    assert_eq!(
        last_request().data().clone(),
        json!({
            "TableName": "job_batches",
            "Key": {"application": {"S": "Podcasts"}, "id": {"S": batch.id}},
            "UpdateExpression": "SET total_jobs = total_jobs + :val, pending_jobs = pending_jobs + :val",
            "ExpressionAttributeValues": {":val": {"N": "3"}},
            "ReturnValues": "ALL_NEW",
        })
    );

    let counts = repository
        .decrement_pending_jobs(&batch.id, "job-1")
        .await
        .unwrap();
    assert_eq!((counts.pending_jobs, counts.failed_jobs), (2, 0));
    assert_eq!(
        last_request()["UpdateExpression"],
        "SET pending_jobs = pending_jobs - :inc"
    );

    let counts = repository
        .increment_failed_jobs(&batch.id, "job-2")
        .await
        .unwrap();
    assert_eq!((counts.pending_jobs, counts.failed_jobs), (2, 1));
    let update = last_request();
    assert_eq!(
        update["UpdateExpression"],
        "SET failed_jobs = failed_jobs + :inc, failed_job_ids = list_append(failed_job_ids, :jobId)"
    );
    assert_eq!(
        update["ExpressionAttributeValues"],
        json!({":jobId": {"L": [{"S": "job-2"}]}, ":inc": {"N": "1"}})
    );

    repository.mark_as_finished(&batch.id).await.unwrap();
    let finished = repository.find(&batch.id).await.unwrap().unwrap();
    assert_eq!(finished.total_jobs, 3);
    assert_eq!(finished.pending_jobs, 2);
    assert_eq!(finished.failed_job_ids, ["job-2"]);
    assert_eq!(finished.finished_at.unwrap().timestamp(), NOW);

    repository.cancel(&batch.id).await.unwrap();
    assert_eq!(
        last_request()["UpdateExpression"],
        "SET cancelled_at = :timestamp, finished_at = :timestamp"
    );
    assert!(
        repository
            .find(&batch.id)
            .await
            .unwrap()
            .unwrap()
            .cancelled_at
            .is_some()
    );

    assert!(repository.find("missing").await.unwrap().is_none());
    let reads: Vec<Request> = requests().into_iter().rev().take(2).collect();
    assert_eq!(
        reads[0]["ConsistentRead"], true,
        "a consistent read is attempted last"
    );
    assert!(reads[1].data().get("ConsistentRead").is_none());
    assert!(repository.find("  ").await.unwrap().is_none());

    repository.delete(&batch.id).await.unwrap();
    assert!(fake.items("job_batches").is_empty());
    assert_eq!(repository.prune(Carbon::now()).await.unwrap(), 0);
}

#[tokio::test]
async fn batches_are_listed_newest_first_and_expire_with_a_ttl() {
    let _container = common::app_default();
    let now = Frozen::at(NOW);
    let _fake = FakeDynamo::install();
    let repository = DynamoBatchRepository::new(client(), "Podcasts", "job_batches")
        .with_ttl(Some(3600), "expires");

    let first = repository.store("first", &json!({})).await.unwrap();
    now.travel(1);
    let second = repository.store("second", &json!({})).await.unwrap();
    assert_eq!(requests()[0]["Item"]["expires"], json!({"N": "1700003600"}));

    let listed = repository.get(10, None).await.unwrap();
    assert_eq!(
        last_request().data().clone(),
        json!({
            "TableName": "job_batches",
            "KeyConditionExpression": "application = :application",
            "ExpressionAttributeValues": {":application": {"S": "Podcasts"}},
            "Limit": 10,
            "ScanIndexForward": false,
        })
    );
    assert_eq!(
        listed
            .iter()
            .map(|batch| batch.name.as_str())
            .collect::<Vec<_>>(),
        ["second", "first"]
    );

    let older = repository.get(10, Some(&second.id)).await.unwrap();
    assert_eq!(
        last_request()["KeyConditionExpression"],
        "application = :application AND id < :id"
    );
    assert_eq!(older.len(), 1);
    assert_eq!(older[0].id, first.id);

    repository.increment_total_jobs(&first.id, 1).await.unwrap();
    let update = last_request();
    assert_eq!(
        update["UpdateExpression"],
        "SET total_jobs = total_jobs + :val, pending_jobs = pending_jobs + :val, #expires = :ttl"
    );
    assert_eq!(
        update["ExpressionAttributeNames"],
        json!({"#expires": "expires"})
    );
    assert_eq!(
        update["ExpressionAttributeValues"][":ttl"],
        json!({"N": "1700003601"})
    );
}

#[derive(Serialize, Deserialize)]
struct ImportChunk {
    chunk: u64,
}

#[async_trait]
impl ShouldQueue for ImportChunk {
    async fn handle(&self) -> Result<()> {
        let batch = self.batch().await?.expect("the job belongs to a batch");
        record(format!("import:{} ({})", self.chunk, batch.name));
        Ok(())
    }
}

#[tokio::test]
async fn batches_run_on_dynamodb_when_configured() {
    let _app = app();
    let fake = FakeDynamo::install();

    let batch = Bus::batch(vec![
        Box::new(ImportChunk { chunk: 1 }) as Box<dyn ShouldQueue>,
        Box::new(ImportChunk { chunk: 2 }),
    ])
    .name("Import CSV")
    .then(|batch: Batch| async move {
        record(format!("then:{}", batch.processed_jobs()));
        Ok(())
    })
    .dispatch()
    .await
    .unwrap();
    assert_eq!(batch.total_jobs, 2);

    Worker::make()
        .daemon(
            "array",
            "default",
            &WorkerOptions::new().sleep(0.0).stop_when_empty(),
        )
        .await
        .unwrap();

    assert_eq!(
        recorded(),
        vec!["import:1 (Import CSV)", "import:2 (Import CSV)", "then:2"]
    );
    let stored = fake.items("job_batches");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0]["application"], json!({"S": "Podcasts"}));
    assert_eq!(stored[0]["pending_jobs"], json!({"N": "0"}));
    assert!(stored[0]["finished_at"].get("N").is_some());
    assert!(stored[0]["expires"].get("N").is_some());
    let fresh = batch.fresh().await.unwrap().unwrap();
    assert!(fresh.finished());
}
