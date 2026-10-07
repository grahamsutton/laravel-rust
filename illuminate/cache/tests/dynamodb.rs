//! The `dynamodb` cache driver against a fake DynamoDB API served by
//! `Http::fake()`: the exact requests, the parsed responses, conditional
//! writes, locks, and the manager's configuration.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use illuminate_cache::{
    AwsException, Cache, CacheManager, CacheServiceProvider, DynamoDbClient, DynamoDbStore,
    LockProvider, Repository as CacheRepository, Store,
};
use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_http_client::{FakeResponse, Http, Request};
use illuminate_support::{Carbon, Value, json};

const NOW: i64 = 1_700_000_000;

/// Freeze "now" on this thread (the store and the fake share it).
struct Frozen;

impl Frozen {
    fn at(timestamp: i64) -> Self {
        Carbon::set_thread_test_now(Some(Carbon::from_timestamp(timestamp)));
        Self
    }

    fn travel(&self, seconds: i64) {
        let now = Carbon::now().add_seconds(seconds);
        Carbon::set_thread_test_now(Some(now));
    }
}

impl Drop for Frozen {
    fn drop(&mut self) {
        Carbon::set_thread_test_now(None);
    }
}

/// A tiny DynamoDB: one table keyed by the `key` attribute (or whatever
/// the `#key` / `Key` say), understanding exactly the expressions the
/// store sends.
#[derive(Clone, Default)]
struct FakeDynamo {
    items: Arc<Mutex<BTreeMap<String, Value>>>,
}

fn conditional_check_failed() -> FakeResponse {
    Http::response(
        json!({
            "__type": "com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException",
            "message": "The conditional request failed",
        }),
        400,
        &[],
    )
}

fn number(value: &Value) -> f64 {
    value["N"].as_str().unwrap().parse().unwrap()
}

impl FakeDynamo {
    fn install() -> Self {
        let fake = Self::default();
        let handler = fake.clone();
        Http::fake_using(move |request: &Request| handler.handle(request));
        fake
    }

    fn item(&self, key: &str) -> Option<Value> {
        self.items.lock().unwrap().get(key).cloned()
    }

    fn handle(&self, request: &Request) -> FakeResponse {
        assert!(request.has_header("Authorization"), "requests are signed");
        assert!(request.has_header("X-Amz-Date"));
        assert!(request.has_header_value("Content-Type", "application/x-amz-json-1.0"));
        let target = request.header("X-Amz-Target")[0].clone();
        let operation = target.strip_prefix("DynamoDB_20120810.").unwrap();
        let input = request.data().clone();
        let mut items = self.items.lock().unwrap();
        let names = input["ExpressionAttributeNames"].clone();
        let values = input["ExpressionAttributeValues"].clone();
        let name = |placeholder: &str| names[placeholder].as_str().unwrap().to_string();
        let key_of = |key: &Value| -> String {
            let (_, attribute) = key.as_object().unwrap().iter().next().unwrap();
            attribute["S"].as_str().unwrap().to_string()
        };

        match operation {
            "GetItem" => {
                let key = key_of(&input["Key"]);
                match items.get(&key) {
                    Some(item) => Http::response(json!({"Item": item}), 200, &[]),
                    None => Http::response(json!({}), 200, &[]),
                }
            }
            "BatchGetItem" => {
                let (table, request) = input["RequestItems"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .next()
                    .unwrap();
                let found: Vec<Value> = request["Keys"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|key| items.get(&key_of(key)).cloned())
                    .collect();
                Http::response(
                    json!({"Responses": {table.as_str(): found}, "UnprocessedKeys": {}}),
                    200,
                    &[],
                )
            }
            "PutItem" => {
                let item = input["Item"].clone();
                let (_, key) = item.as_object().unwrap().iter().next().unwrap();
                let key = key["S"].as_str().unwrap().to_string();
                if let Some(condition) = input["ConditionExpression"].as_str() {
                    assert_eq!(
                        condition,
                        "attribute_not_exists(#key) OR #expires_at < :now"
                    );
                    let existing = items.get(&key);
                    let allowed = existing.is_none_or(|existing| {
                        number(&existing[name("#expires_at")]) < number(&values[":now"])
                    });
                    if !allowed {
                        return conditional_check_failed();
                    }
                }
                items.insert(key, item);
                Http::response(json!({}), 200, &[])
            }
            "BatchWriteItem" => {
                let (_, writes) = input["RequestItems"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .next()
                    .unwrap();
                for write in writes.as_array().unwrap() {
                    let item = write["PutRequest"]["Item"].clone();
                    let (_, key) = item.as_object().unwrap().iter().next().unwrap();
                    items.insert(key["S"].as_str().unwrap().to_string(), item);
                }
                Http::response(json!({"UnprocessedItems": {}}), 200, &[])
            }
            "UpdateItem" => {
                let key = key_of(&input["Key"]);
                let condition = input["ConditionExpression"].as_str().unwrap();
                let update = input["UpdateExpression"].as_str().unwrap();
                let Some(item) = items.get_mut(&key) else {
                    return conditional_check_failed();
                };
                let expiration = names
                    .get("#expires_at")
                    .or(names.get("#expiry"))
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string();
                if number(&item[&expiration]) <= number(&values[":now"]) {
                    return conditional_check_failed();
                }
                if condition.contains("#value = :owner") && item[name("#value")] != values[":owner"]
                {
                    return conditional_check_failed();
                }
                let attributes = match update {
                    "SET #value = #value + :amount" | "SET #value = #value - :amount" => {
                        let value = name("#value");
                        let amount = number(&values[":amount"]) as i64;
                        let current = number(&item[&value]) as i64;
                        let new = if update.contains('+') {
                            current + amount
                        } else {
                            current - amount
                        };
                        item[&value] = json!({"N": new.to_string()});
                        json!({value.as_str(): {"N": new.to_string()}})
                    }
                    "SET #expiry = :expiry" => {
                        item[&expiration] = values[":expiry"].clone();
                        json!({})
                    }
                    "SET #expires_at = :expires_at" => {
                        item[&expiration] = values[":expires_at"].clone();
                        json!({})
                    }
                    other => panic!("Unexpected update [{other}]"),
                };
                Http::response(json!({"Attributes": attributes}), 200, &[])
            }
            "DeleteItem" => {
                items.remove(&key_of(&input["Key"]));
                Http::response(json!({}), 200, &[])
            }
            other => panic!("Unexpected operation [{other}]"),
        }
    }
}

fn container() -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    (container, guard)
}

fn store() -> DynamoDbStore {
    let client = DynamoDbClient::from_config(&json!({
        "key": "AKIDEXAMPLE",
        "secret": "secret",
        "region": "eu-west-1",
    }));
    DynamoDbStore::new(client, "cache").with_prefix("laravel-cache-")
}

fn requests() -> Vec<Request> {
    Http::recorded()
        .into_vec()
        .into_iter()
        .map(|(request, _)| request)
        .collect()
}

fn target(request: &Request) -> String {
    request.header("X-Amz-Target")[0].clone()
}

#[tokio::test]
async fn items_are_written_and_read_with_the_exact_requests_laravel_sends() {
    let (_container, _guard) = container();
    let _now = Frozen::at(NOW);
    let fake = FakeDynamo::install();
    let store = store();

    assert!(store.put("name", json!("Taylor"), 60).await.unwrap());
    assert!(store.put("count", json!(42), 60).await.unwrap());

    let sent = requests();
    assert_eq!(sent.len(), 2);
    let put = &sent[0];
    assert_eq!(put.method(), "POST");
    assert_eq!(put.url(), "https://dynamodb.eu-west-1.amazonaws.com/");
    assert_eq!(target(put), "DynamoDB_20120810.PutItem");
    assert!(put.has_header_value("X-Amz-Date", "20231114T221320Z"));
    let authorization = put.header("Authorization")[0].clone();
    assert!(authorization.starts_with(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20231114/eu-west-1/dynamodb/aws4_request, SignedHeaders=content-type;host;x-amz-date;x-amz-target, Signature="
    ));
    assert_eq!(
        put.data().clone(),
        json!({
            "TableName": "cache",
            "Item": {
                "key": {"S": "laravel-cache-name"},
                "value": {"S": "\"Taylor\""},
                "expires_at": {"N": "1700000060"},
            },
        })
    );
    assert_eq!(sent[1]["Item"]["value"], json!({"N": "42"}));

    assert_eq!(store.get("name").await.unwrap(), Some(json!("Taylor")));
    assert_eq!(store.get("count").await.unwrap(), Some(json!(42)));
    assert_eq!(store.get("missing").await.unwrap(), None);
    let get = requests().pop().unwrap();
    assert_eq!(target(&get), "DynamoDB_20120810.GetItem");
    assert_eq!(
        get.data().clone(),
        json!({"TableName": "cache", "ConsistentRead": false, "Key": {"key": {"S": "laravel-cache-missing"}}})
    );

    // Items are expired once "now" reaches their expiration.
    _now.travel(60);
    assert_eq!(store.get("name").await.unwrap(), None);
    assert!(fake.item("laravel-cache-name").is_some());
}

#[tokio::test]
async fn many_items_are_read_and_written_in_batches() {
    let (_container, _guard) = container();
    let _now = Frozen::at(NOW);
    let _fake = FakeDynamo::install();
    let store = store();

    store
        .put_many(
            vec![
                ("a".to_string(), json!(1)),
                ("b".to_string(), json!({"nested": true})),
                ("a".to_string(), json!(2)),
            ],
            120,
        )
        .await
        .unwrap();
    let write = requests().pop().unwrap();
    assert_eq!(target(&write), "DynamoDB_20120810.BatchWriteItem");
    assert_eq!(
        write.data().clone(),
        json!({"RequestItems": {"cache": [
            {"PutRequest": {"Item": {"key": {"S": "laravel-cache-b"}, "value": {"S": "{\"nested\":true}"}, "expires_at": {"N": "1700000120"}}}},
            {"PutRequest": {"Item": {"key": {"S": "laravel-cache-a"}, "value": {"N": "2"}, "expires_at": {"N": "1700000120"}}}},
        ]}})
    );

    let values = store
        .many(&["a".to_string(), "missing".to_string(), "b".to_string()])
        .await
        .unwrap();
    assert_eq!(
        values,
        vec![
            ("a".to_string(), Some(json!(2))),
            ("missing".to_string(), None),
            ("b".to_string(), Some(json!({"nested": true}))),
        ]
    );
    let read = requests().pop().unwrap();
    assert_eq!(target(&read), "DynamoDB_20120810.BatchGetItem");
    assert_eq!(
        read.data().clone(),
        json!({"RequestItems": {"cache": {"ConsistentRead": false, "Keys": [
            {"key": {"S": "laravel-cache-a"}},
            {"key": {"S": "laravel-cache-missing"}},
            {"key": {"S": "laravel-cache-b"}},
        ]}}})
    );
    assert!(store.many(&[]).await.unwrap().is_empty());
}

#[tokio::test]
async fn add_is_a_conditional_put() {
    let (_container, _guard) = container();
    let now = Frozen::at(NOW);
    let _fake = FakeDynamo::install();
    let store = store();

    assert!(store.add("lock", json!("first"), 10).await.unwrap());
    let add = requests().pop().unwrap();
    assert_eq!(
        add.data().clone(),
        json!({
            "TableName": "cache",
            "Item": {
                "key": {"S": "laravel-cache-lock"},
                "value": {"S": "\"first\""},
                "expires_at": {"N": "1700000010"},
            },
            "ConditionExpression": "attribute_not_exists(#key) OR #expires_at < :now",
            "ExpressionAttributeNames": {"#key": "key", "#expires_at": "expires_at"},
            "ExpressionAttributeValues": {":now": {"N": "1700000000"}},
        })
    );

    assert!(!store.add("lock", json!("second"), 10).await.unwrap());
    assert_eq!(store.get("lock").await.unwrap(), Some(json!("first")));

    now.travel(11);
    assert!(store.add("lock", json!("third"), 10).await.unwrap());
    assert_eq!(store.get("lock").await.unwrap(), Some(json!("third")));
}

#[tokio::test]
async fn increments_are_atomic_updates() {
    let (_container, _guard) = container();
    let now = Frozen::at(NOW);
    let fake = FakeDynamo::install();
    let store = store();

    store.put("visits", json!(10), 60).await.unwrap();
    assert_eq!(store.increment("visits", 5).await.unwrap(), 15);
    let update = requests().pop().unwrap();
    assert_eq!(target(&update), "DynamoDB_20120810.UpdateItem");
    assert_eq!(
        update.data().clone(),
        json!({
            "TableName": "cache",
            "Key": {"key": {"S": "laravel-cache-visits"}},
            "ConditionExpression": "attribute_exists(#key) AND #expires_at > :now",
            "UpdateExpression": "SET #value = #value + :amount",
            "ExpressionAttributeNames": {"#key": "key", "#value": "value", "#expires_at": "expires_at"},
            "ExpressionAttributeValues": {":now": {"N": "1700000000"}, ":amount": {"N": "5"}},
            "ReturnValues": "UPDATED_NEW",
        })
    );

    assert_eq!(store.decrement("visits", 20).await.unwrap(), -5);
    assert_eq!(
        requests().pop().unwrap()["UpdateExpression"],
        "SET #value = #value - :amount"
    );
    assert_eq!(store.increment("visits", -1).await.unwrap(), -6);

    // Missing (and expired) items start at zero, and are kept forever.
    assert_eq!(store.increment("new", 3).await.unwrap(), 3);
    let stored = fake.item("laravel-cache-new").unwrap();
    assert_eq!(stored["value"], json!({"N": "3"}));
    assert!(number(&stored["expires_at"]) > (NOW + 5 * 365 * 86_400) as f64);

    now.travel(61);
    assert_eq!(store.increment("visits", 1).await.unwrap(), 1);
}

#[tokio::test]
async fn items_can_be_touched_forgotten_and_never_flushed() {
    let (_container, _guard) = container();
    let _now = Frozen::at(NOW);
    let fake = FakeDynamo::install();
    let store = store();

    store.put("name", json!("Taylor"), 60).await.unwrap();
    assert!(store.touch("name", 600).await.unwrap());
    assert_eq!(
        requests().pop().unwrap().data().clone(),
        json!({
            "TableName": "cache",
            "Key": {"key": {"S": "laravel-cache-name"}},
            "UpdateExpression": "SET #expiry = :expiry",
            "ConditionExpression": "attribute_exists(#key) AND #expiry > :now",
            "ExpressionAttributeNames": {"#key": "key", "#expiry": "expires_at"},
            "ExpressionAttributeValues": {":expiry": {"N": "1700000600"}, ":now": {"N": "1700000000"}},
        })
    );
    assert!(!store.touch("missing", 600).await.unwrap());

    assert!(store.forget("name").await.unwrap());
    let delete = requests().pop().unwrap();
    assert_eq!(target(&delete), "DynamoDB_20120810.DeleteItem");
    assert_eq!(
        delete.data().clone(),
        json!({"TableName": "cache", "Key": {"key": {"S": "laravel-cache-name"}}})
    );
    assert!(fake.item("laravel-cache-name").is_none());

    assert_eq!(
        store.flush().await.unwrap_err().to_string(),
        "DynamoDb does not support flushing an entire table. Please create a new table."
    );

    store.forever("forever", json!("ever")).await.unwrap();
    let expiration = number(&fake.item("laravel-cache-forever").unwrap()["expires_at"]);
    assert!(expiration > (NOW + 5 * 365 * 86_400) as f64);
}

#[tokio::test]
async fn aws_errors_are_reported() {
    let (_container, _guard) = container();
    Http::fake_using(|_| {
        Http::response(
            json!({
                "__type": "com.amazonaws.dynamodb.v20120810#ResourceNotFoundException",
                "message": "Requested resource not found",
            }),
            400,
            &[],
        )
    });

    let error = store().get("name").await.unwrap_err();
    let exception = error.downcast_ref::<AwsException>().unwrap();
    assert_eq!(exception.code, "ResourceNotFoundException");
    assert_eq!(exception.operation, "GetItem");
    assert_eq!(exception.status, 400);
    assert_eq!(exception.message, "Requested resource not found");

    let unsigned = DynamoDbStore::new(DynamoDbClient::new("us-east-1"), "cache");
    let error = unsigned.get("name").await.unwrap_err();
    assert!(AwsException::has_code(&error, "CredentialsException"));
}

#[tokio::test]
async fn locks_are_conditional_items() {
    let (_container, _guard) = container();
    let now = Frozen::at(NOW);
    let fake = FakeDynamo::install();
    let store = store();

    let lock = store.lock("reports", 10, Some("taylor".into()));
    assert_eq!(lock.name(), "reports");
    assert!(lock.get().await.unwrap());
    assert_eq!(
        fake.item("laravel-cache-reports").unwrap(),
        json!({"key": {"S": "laravel-cache-reports"}, "value": {"S": "\"taylor\""}, "expires_at": {"N": "1700000010"}})
    );

    let other = store.lock("reports", 10, Some("abigail".into()));
    assert!(!other.get().await.unwrap());
    assert!(lock.is_owned_by_current_process().await.unwrap());
    assert!(!other.is_owned_by_current_process().await.unwrap());
    assert!(other.is_locked().await.unwrap());
    assert!(!other.release().await.unwrap());

    assert!(lock.refresh(Some(30)).await.unwrap());
    let refresh = requests().pop().unwrap();
    assert_eq!(
        refresh.data().clone(),
        json!({
            "TableName": "cache",
            "Key": {"key": {"S": "laravel-cache-reports"}},
            "ConditionExpression": "attribute_exists(#key) AND #value = :owner AND #expires_at > :now",
            "UpdateExpression": "SET #expires_at = :expires_at",
            "ExpressionAttributeNames": {"#key": "key", "#value": "value", "#expires_at": "expires_at"},
            "ExpressionAttributeValues": {
                ":owner": {"S": "\"taylor\""},
                ":now": {"N": "1700000000"},
                ":expires_at": {"N": "1700000030"},
            },
        })
    );
    assert!(!other.refresh(Some(30)).await.unwrap());

    assert!(lock.release().await.unwrap());
    assert!(fake.item("laravel-cache-reports").is_none());

    // Locks without a duration are held for a day.
    let forever = store.lock("daily", 0, None);
    assert!(forever.acquire().await.unwrap());
    assert_eq!(
        fake.item("laravel-cache-daily").unwrap()["expires_at"],
        json!({"N": "1700086400"})
    );
    now.travel(86_401);
    assert!(store.lock("daily", 0, None).acquire().await.unwrap());

    let restored = store.restore_lock("daily", "someone");
    restored.force_release().await.unwrap();
    assert!(fake.item("laravel-cache-daily").is_none());
}

#[tokio::test]
async fn the_manager_builds_dynamodb_stores_from_configuration() {
    let (container, _guard) = container();
    let _now = Frozen::at(NOW);
    container.instance(Repository::new(json!({
        "app": {"name": "Laravel"},
        "cache": {
            "default": "dynamodb",
            "stores": {
                "dynamodb": {
                    "driver": "dynamodb",
                    "key": "AKIDEXAMPLE",
                    "secret": "secret",
                    "token": "session-token",
                    "region": "eu-central-1",
                    "table": "app_cache",
                    "endpoint": "http://localhost:8000",
                    "attributes": {"key": "id", "value": "payload", "expiration": "ttl"},
                },
                "defaults": {"driver": "dynamodb", "key": "AKIDEXAMPLE", "secret": "secret"},
            },
            "prefix": "app_",
        },
    })));
    CacheServiceProvider.register(&container);
    let _fake = FakeDynamo::install();

    Cache::put("name", "Taylor", 60).await.unwrap();
    let put = requests().pop().unwrap();
    assert_eq!(put.url(), "http://localhost:8000/");
    assert!(put.has_header_value("X-Amz-Security-Token", "session-token"));
    assert!(put.header("Authorization")[0].contains("/eu-central-1/dynamodb/aws4_request"));
    assert_eq!(
        put.data().clone(),
        json!({
            "TableName": "app_cache",
            "Item": {
                "id": {"S": "app_name"},
                "payload": {"S": "\"Taylor\""},
                "ttl": {"N": "1700000060"},
            },
        })
    );
    assert_eq!(Cache::get("name").await.unwrap(), Some(json!("Taylor")));
    assert!(Cache::lock("job", 5).unwrap().get().await.unwrap());

    let manager = container.make::<CacheManager>();
    let defaults: CacheRepository = manager.store("defaults").unwrap();
    assert_eq!(defaults.get_name(), Some("defaults"));
    defaults.put("x", 1, 60).await.unwrap();
    let put = requests().pop().unwrap();
    assert_eq!(put.url(), "https://dynamodb.us-east-1.amazonaws.com/");
    assert_eq!(put["TableName"], "cache");
    assert_eq!(
        put["Item"],
        json!({"key": {"S": "app_x"}, "value": {"N": "1"}, "expires_at": {"N": "1700000060"}})
    );
    assert!(!defaults.supports_tags());
}
