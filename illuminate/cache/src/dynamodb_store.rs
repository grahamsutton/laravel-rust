//! The DynamoDB cache store and its locks.
//!
//! Items live in a DynamoDB table — `cache` by default — exactly like
//! Laravel's `DynamoDbStore`: each item has a string partition key holding
//! the prefixed cache key, a value, and a numeric expiration timestamp:
//!
//! ```json
//! {"key": {"S": "laravel-cache-name"}, "value": {"S": "\"Taylor\""}, "expires_at": {"N": "1700000600"}}
//! ```
//!
//! Numbers are stored as DynamoDB numbers (so increments happen in
//! DynamoDB), and every other value as JSON text — where Laravel would use
//! PHP's `serialize`. Enable [TTL](https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/TTL.html)
//! on the expiration attribute to have DynamoDB remove expired items.
//!
//! ```json
//! "dynamodb": {
//!     "driver": "dynamodb",
//!     "key": "AWS_ACCESS_KEY_ID",
//!     "secret": "AWS_SECRET_ACCESS_KEY",
//!     "region": "us-east-1",
//!     "table": "cache",
//!     "endpoint": null,
//!     "attributes": {"key": "key", "value": "value", "expiration": "expires_at"}
//! }
//! ```

use std::sync::Arc;

use async_trait::async_trait;

use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Map, Result, Value, json};

use crate::aws::{AwsClient, AwsException};
use crate::lock::{Lock, LockDriver, LockInfo};
use crate::store::{LockProvider, Store};

/// The error code DynamoDB returns when a condition expression fails.
const CONDITIONAL_CHECK_FAILED: &str = "ConditionalCheckFailedException";

/// The most keys a `BatchGetItem` request may read.
const BATCH_GET_LIMIT: usize = 100;

/// The most items a `BatchWriteItem` request may write.
const BATCH_WRITE_LIMIT: usize = 25;

/// How long a lock without an explicit duration is held: one day.
const DEFAULT_LOCK_SECONDS: u64 = 86_400;

/// A client for the [DynamoDB API](https://docs.aws.amazon.com/amazondynamodb/latest/APIReference/),
/// with one method per operation the framework uses. Inputs and outputs are
/// the API's JSON documents, just like the AWS SDK's arrays.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_cache::DynamoDbClient;
/// use illuminate_container::Container;
/// use illuminate_http_client::Http;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let _guard = Container::set_local_instance(Arc::new(Container::new()));
/// Http::fake_using(|_| Http::response(json!({"Item": {"key": {"S": "a"}}}), 200, &[]));
///
/// let dynamo = DynamoDbClient::from_config(&json!({"key": "AKID", "secret": "secret", "region": "us-east-1"}));
/// let result = dynamo
///     .get_item(json!({"TableName": "cache", "Key": {"key": {"S": "a"}}}))
///     .await
///     .unwrap();
///
/// assert_eq!(result["Item"]["key"]["S"], "a");
/// # });
/// ```
#[derive(Debug, Clone)]
pub struct DynamoDbClient {
    client: AwsClient,
}

impl DynamoDbClient {
    /// The prefix of every DynamoDB `X-Amz-Target` header.
    pub const TARGET_PREFIX: &'static str = "DynamoDB_20120810";

    /// Create a client for the given region (configure credentials on the
    /// underlying [`AwsClient`] with [`DynamoDbClient::from_aws`]).
    pub fn new(region: impl Into<String>) -> Self {
        Self::from_aws(AwsClient::new("dynamodb", Self::TARGET_PREFIX, region))
    }

    /// Create a client from configuration: `key`, `secret`, `token`,
    /// `region` and `endpoint` (Laravel's `newDynamodbClient`).
    pub fn from_config(config: &Value) -> Self {
        Self::from_aws(AwsClient::from_config(
            "dynamodb",
            Self::TARGET_PREFIX,
            config,
        ))
    }

    /// Wrap an existing AWS client.
    pub fn from_aws(client: AwsClient) -> Self {
        Self { client }
    }

    /// The underlying AWS client.
    pub fn aws(&self) -> &AwsClient {
        &self.client
    }

    /// Call any DynamoDB operation.
    pub async fn call(&self, operation: &str, input: Value) -> Result<Value> {
        self.client.call(operation, input).await
    }

    /// The `GetItem` operation.
    pub async fn get_item(&self, input: Value) -> Result<Value> {
        self.call("GetItem", input).await
    }

    /// The `BatchGetItem` operation.
    pub async fn batch_get_item(&self, input: Value) -> Result<Value> {
        self.call("BatchGetItem", input).await
    }

    /// The `PutItem` operation.
    pub async fn put_item(&self, input: Value) -> Result<Value> {
        self.call("PutItem", input).await
    }

    /// The `BatchWriteItem` operation.
    pub async fn batch_write_item(&self, input: Value) -> Result<Value> {
        self.call("BatchWriteItem", input).await
    }

    /// The `UpdateItem` operation.
    pub async fn update_item(&self, input: Value) -> Result<Value> {
        self.call("UpdateItem", input).await
    }

    /// The `DeleteItem` operation.
    pub async fn delete_item(&self, input: Value) -> Result<Value> {
        self.call("DeleteItem", input).await
    }

    /// The `Query` operation.
    pub async fn query(&self, input: Value) -> Result<Value> {
        self.call("Query", input).await
    }

    /// The `CreateTable` operation.
    pub async fn create_table(&self, input: Value) -> Result<Value> {
        self.call("CreateTable", input).await
    }

    /// The `UpdateTimeToLive` operation.
    pub async fn update_time_to_live(&self, input: Value) -> Result<Value> {
        self.call("UpdateTimeToLive", input).await
    }

    /// The `DeleteTable` operation.
    pub async fn delete_table(&self, input: Value) -> Result<Value> {
        self.call("DeleteTable", input).await
    }
}

/// The current UNIX timestamp (honouring `Carbon::set_test_now`).
fn current_time() -> i64 {
    Carbon::now().timestamp()
}

/// The DynamoDB type of a value: numbers are `N`, everything else `S`.
fn attribute_type(value: &Value) -> &'static str {
    match value {
        Value::Number(_) => "N",
        _ => "S",
    }
}

/// Encode a value for DynamoDB: numbers as they are, everything else as JSON.
fn serialize(value: &Value) -> String {
    match value {
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

/// The attribute value of a cache value (`{"N": "42"}`, `{"S": "\"Taylor\""}`).
fn attribute(value: &Value) -> Value {
    json!({attribute_type(value): serialize(value)})
}

/// Decode a stored value: integers, then floats, then JSON — anything else
/// (written by another client, say) comes back as a string.
fn unserialize(raw: &str) -> Value {
    if let Ok(integer) = raw.parse::<i64>() {
        return Value::from(integer);
    }
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

/// The raw string or number held by an attribute value.
fn raw_attribute(attribute: &Value) -> Option<&str> {
    attribute
        .get("S")
        .or_else(|| attribute.get("N"))
        .and_then(Value::as_str)
}

/// A cache store backed by a DynamoDB table (`CACHE_STORE=dynamodb`).
///
/// ```
/// use std::sync::Arc;
/// use illuminate_cache::{DynamoDbClient, DynamoDbStore, Repository};
/// use illuminate_container::Container;
/// use illuminate_http_client::Http;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let _guard = Container::set_local_instance(Arc::new(Container::new()));
/// Http::fake_using(|_| Http::response(json!({}), 200, &[]));
///
/// let client = DynamoDbClient::from_config(&json!({"key": "AKID", "secret": "secret"}));
/// let cache = Repository::new(DynamoDbStore::new(client, "cache").with_prefix("laravel-cache-"));
///
/// cache.put("name", "Taylor", 600).await.unwrap();
///
/// Http::assert_sent(|request| {
///     request.has_header_value("X-Amz-Target", "DynamoDB_20120810.PutItem")
///         && request["Item"]["key"]["S"] == "laravel-cache-name"
///         && request["Item"]["value"]["S"] == "\"Taylor\""
/// });
/// # });
/// ```
#[derive(Debug, Clone)]
pub struct DynamoDbStore {
    dynamo: DynamoDbClient,
    table: String,
    key_attribute: String,
    value_attribute: String,
    expiration_attribute: String,
    prefix: String,
}

impl DynamoDbStore {
    /// Create a store keeping its items in the given table, with Laravel's
    /// attribute names: `key`, `value` and `expires_at`.
    pub fn new(dynamo: DynamoDbClient, table: impl Into<String>) -> Self {
        Self {
            dynamo,
            table: table.into(),
            key_attribute: "key".to_string(),
            value_attribute: "value".to_string(),
            expiration_attribute: "expires_at".to_string(),
            prefix: String::new(),
        }
    }

    /// Use different attribute names for the key, the value and the
    /// expiration timestamp (the `attributes` option).
    pub fn with_attributes(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
        expiration: impl Into<String>,
    ) -> Self {
        self.key_attribute = key.into();
        self.value_attribute = value.into();
        self.expiration_attribute = expiration.into();
        self
    }

    /// Prefix every key with the given string.
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Set the cache key prefix.
    pub fn set_prefix(&mut self, prefix: impl Into<String>) {
        self.prefix = prefix.into();
    }

    /// The DynamoDB client.
    pub fn get_client(&self) -> &DynamoDbClient {
        &self.dynamo
    }

    /// The name of the table.
    pub fn get_table(&self) -> &str {
        &self.table
    }

    /// The names of the key, value and expiration attributes.
    pub fn get_attributes(&self) -> (&str, &str, &str) {
        (
            &self.key_attribute,
            &self.value_attribute,
            &self.expiration_attribute,
        )
    }

    fn prefixed(&self, key: &str) -> String {
        format!("{}{key}", self.prefix)
    }

    fn key(&self, key: &str) -> Value {
        json!({self.key_attribute.as_str(): {"S": self.prefixed(key)}})
    }

    /// The item stored for a key.
    fn item(&self, key: &str, value: &Value, expiration: i64) -> Value {
        json!({
            self.key_attribute.as_str(): {"S": self.prefixed(key)},
            self.value_attribute.as_str(): attribute(value),
            self.expiration_attribute.as_str(): {"N": expiration.to_string()},
        })
    }

    /// The expiration timestamp of an item stored for `seconds` (`0` is
    /// forever, which Laravel stores as five years' worth of timestamp).
    fn expiration(&self, seconds: u64) -> i64 {
        let seconds = if seconds == 0 {
            Carbon::now().add_years(5).timestamp().max(0) as u64
        } else {
            seconds
        };
        current_time().saturating_add(i64::try_from(seconds).unwrap_or(i64::MAX))
    }

    /// Determine if the item is expired.
    fn is_expired(&self, item: &Value, now: i64) -> bool {
        item.get(&self.expiration_attribute)
            .and_then(|expiration| expiration.get("N"))
            .and_then(Value::as_str)
            .and_then(|expiration| expiration.parse::<f64>().ok())
            .is_some_and(|expiration| now as f64 >= expiration)
    }

    /// The value of an item, unless it is expired.
    fn value_of(&self, item: &Value, now: i64) -> Option<Value> {
        if self.is_expired(item, now) {
            return None;
        }
        item.get(&self.value_attribute)
            .and_then(raw_attribute)
            .map(unserialize)
    }

    /// Run an update guarded by a condition, returning `None` when the
    /// condition fails.
    async fn conditionally(&self, operation: &str, input: Value) -> Result<Option<Value>> {
        match self.dynamo.call(operation, input).await {
            Ok(output) => Ok(Some(output)),
            Err(error) if AwsException::has_code(&error, CONDITIONAL_CHECK_FAILED) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Add `amount` to a fresh item's value, returning the new value, or
    /// `None` when the item is missing or expired.
    async fn update_value(&self, key: &str, amount: i64) -> Result<Option<i64>> {
        let (operator, amount) = if amount < 0 {
            ("-", amount.unsigned_abs())
        } else {
            ("+", amount as u64)
        };
        let output = self
            .conditionally(
                "UpdateItem",
                json!({
                    "TableName": self.table,
                    "Key": self.key(key),
                    "ConditionExpression": "attribute_exists(#key) AND #expires_at > :now",
                    "UpdateExpression": format!("SET #value = #value {operator} :amount"),
                    "ExpressionAttributeNames": {
                        "#key": self.key_attribute,
                        "#value": self.value_attribute,
                        "#expires_at": self.expiration_attribute,
                    },
                    "ExpressionAttributeValues": {
                        ":now": {"N": current_time().to_string()},
                        ":amount": {"N": amount.to_string()},
                    },
                    "ReturnValues": "UPDATED_NEW",
                }),
            )
            .await?;
        Ok(output.map(|output| {
            output["Attributes"][&self.value_attribute]["N"]
                .as_str()
                .and_then(|value| {
                    value
                        .parse::<i64>()
                        .ok()
                        .or_else(|| value.parse::<f64>().ok().map(|value| value as i64))
                })
                .unwrap_or(0)
        }))
    }

    /// Atomically refresh the expiration of a key if it holds the expected
    /// owner (what [`DynamoDbLock`] refreshes with).
    pub async fn refresh_if_owned(&self, key: &str, owner: &Value, seconds: u64) -> Result<bool> {
        let refreshed = self
            .conditionally(
                "UpdateItem",
                json!({
                    "TableName": self.table,
                    "Key": self.key(key),
                    "ConditionExpression": "attribute_exists(#key) AND #value = :owner AND #expires_at > :now",
                    "UpdateExpression": "SET #expires_at = :expires_at",
                    "ExpressionAttributeNames": {
                        "#key": self.key_attribute,
                        "#value": self.value_attribute,
                        "#expires_at": self.expiration_attribute,
                    },
                    "ExpressionAttributeValues": {
                        ":owner": attribute(owner),
                        ":now": {"N": current_time().to_string()},
                        ":expires_at": {"N": self.expiration(seconds).to_string()},
                    },
                }),
            )
            .await?;
        Ok(refreshed.is_some())
    }
}

#[async_trait]
impl Store for DynamoDbStore {
    async fn get(&self, key: &str) -> Result<Option<Value>> {
        let output = self
            .dynamo
            .get_item(json!({
                "TableName": self.table,
                "ConsistentRead": false,
                "Key": self.key(key),
            }))
            .await?;
        Ok(match output.get("Item") {
            Some(item) if item.is_object() => self.value_of(item, current_time()),
            _ => None,
        })
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<(String, Option<Value>)>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let mut unique: Vec<String> = Vec::with_capacity(keys.len());
        for key in keys {
            let prefixed = self.prefixed(key);
            if !unique.contains(&prefixed) {
                unique.push(prefixed);
            }
        }

        let now = current_time();
        let mut found: Map<String, Value> = Map::new();
        for chunk in unique.chunks(BATCH_GET_LIMIT) {
            let keys: Vec<Value> = chunk
                .iter()
                .map(|key| json!({self.key_attribute.as_str(): {"S": key}}))
                .collect();
            let output = self
                .dynamo
                .batch_get_item(json!({
                    "RequestItems": {
                        self.table.as_str(): {"ConsistentRead": false, "Keys": keys},
                    },
                }))
                .await?;
            let items = output["Responses"][&self.table]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for item in items {
                if let Some(key) = item[&self.key_attribute]["S"].as_str() {
                    let value = self.value_of(&item, now).unwrap_or(Value::Null);
                    found.insert(key.to_string(), value);
                }
            }
        }

        Ok(keys
            .iter()
            .map(|key| {
                let value = found
                    .get(&self.prefixed(key))
                    .filter(|value| !value.is_null())
                    .cloned();
                (key.clone(), value)
            })
            .collect())
    }

    async fn put(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        self.dynamo
            .put_item(json!({
                "TableName": self.table,
                "Item": self.item(key, &value, self.expiration(seconds)),
            }))
            .await?;
        Ok(true)
    }

    async fn put_many(&self, values: Vec<(String, Value)>, seconds: u64) -> Result<bool> {
        if values.is_empty() {
            return Ok(true);
        }
        // A batch may not write the same item twice: the last value wins.
        let mut unique: Vec<(String, Value)> = Vec::with_capacity(values.len());
        for (key, value) in values {
            unique.retain(|(existing, _)| *existing != key);
            unique.push((key, value));
        }

        let expiration = self.expiration(seconds);
        for chunk in unique.chunks(BATCH_WRITE_LIMIT) {
            let requests: Vec<Value> = chunk
                .iter()
                .map(|(key, value)| json!({"PutRequest": {"Item": self.item(key, value, expiration)}}))
                .collect();
            self.dynamo
                .batch_write_item(json!({"RequestItems": {self.table.as_str(): requests}}))
                .await?;
        }
        Ok(true)
    }

    /// Store the item unless a fresh one exists, atomically, with a
    /// conditional `PutItem`.
    async fn add(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        let added = self
            .conditionally(
                "PutItem",
                json!({
                    "TableName": self.table,
                    "Item": self.item(key, &value, self.expiration(seconds)),
                    "ConditionExpression": "attribute_not_exists(#key) OR #expires_at < :now",
                    "ExpressionAttributeNames": {
                        "#key": self.key_attribute,
                        "#expires_at": self.expiration_attribute,
                    },
                    "ExpressionAttributeValues": {
                        ":now": {"N": current_time().to_string()},
                    },
                }),
            )
            .await?;
        Ok(added.is_some())
    }

    /// Increment the item with an `UpdateItem`. Where Laravel returns
    /// `false` for a missing (or expired) item, the item starts at zero
    /// and is stored forever, like every other store.
    async fn increment(&self, key: &str, value: i64) -> Result<i64> {
        for _ in 0..2 {
            if let Some(updated) = self.update_value(key, value).await? {
                return Ok(updated);
            }
            if self.add(key, json!(value), 0).await? {
                return Ok(value);
            }
        }
        Err(RuntimeException::new(format!("Unable to increment the cache item [{key}].")).into())
    }

    async fn forever(&self, key: &str, value: Value) -> Result<bool> {
        self.put(key, value, 0).await
    }

    async fn touch(&self, key: &str, seconds: u64) -> Result<bool> {
        let touched = self
            .conditionally(
                "UpdateItem",
                json!({
                    "TableName": self.table,
                    "Key": self.key(key),
                    "UpdateExpression": "SET #expiry = :expiry",
                    "ConditionExpression": "attribute_exists(#key) AND #expiry > :now",
                    "ExpressionAttributeNames": {
                        "#key": self.key_attribute,
                        "#expiry": self.expiration_attribute,
                    },
                    "ExpressionAttributeValues": {
                        ":expiry": {"N": self.expiration(seconds).to_string()},
                        ":now": {"N": current_time().to_string()},
                    },
                }),
            )
            .await?;
        Ok(touched.is_some())
    }

    async fn forget(&self, key: &str) -> Result<bool> {
        self.dynamo
            .delete_item(json!({"TableName": self.table, "Key": self.key(key)}))
            .await?;
        Ok(true)
    }

    /// DynamoDB can't flush a table: like Laravel, this always fails.
    async fn flush(&self) -> Result<bool> {
        Err(RuntimeException::new(
            "DynamoDb does not support flushing an entire table. Please create a new table.",
        )
        .into())
    }

    fn get_prefix(&self) -> String {
        self.prefix.clone()
    }

    fn lock_provider(&self) -> Option<&dyn LockProvider> {
        Some(self)
    }
}

impl LockProvider for DynamoDbStore {
    fn lock(&self, name: &str, seconds: u64, owner: Option<String>) -> Lock {
        Lock::new(
            Arc::new(DynamoDbLock::new(self.clone())),
            name,
            seconds,
            owner,
        )
    }
}

/// Locks stored as items of the cache table (Laravel's `DynamoDbLock`):
/// acquired with a conditional put, held for a day when no duration is
/// given, and released only by their owner.
#[derive(Debug, Clone)]
pub struct DynamoDbLock {
    dynamo: DynamoDbStore,
}

impl DynamoDbLock {
    /// Create a lock driver keeping its locks in the given store.
    pub fn new(dynamo: DynamoDbStore) -> Self {
        Self { dynamo }
    }

    fn lock_seconds(seconds: u64) -> u64 {
        if seconds > 0 {
            seconds
        } else {
            DEFAULT_LOCK_SECONDS
        }
    }
}

#[async_trait]
impl LockDriver for DynamoDbLock {
    async fn acquire(&self, lock: &LockInfo) -> Result<bool> {
        self.dynamo
            .add(
                &lock.name,
                json!(lock.owner),
                Self::lock_seconds(lock.seconds),
            )
            .await
    }

    async fn release(&self, lock: &LockInfo) -> Result<bool> {
        if self.current_owner(lock).await?.as_deref() == Some(lock.owner.as_str()) {
            return self.dynamo.forget(&lock.name).await;
        }
        Ok(false)
    }

    async fn force_release(&self, lock: &LockInfo) -> Result<()> {
        self.dynamo.forget(&lock.name).await?;
        Ok(())
    }

    async fn current_owner(&self, lock: &LockInfo) -> Result<Option<String>> {
        Ok(self.dynamo.get(&lock.name).await?.map(|owner| match owner {
            Value::String(owner) => owner,
            other => other.to_string(),
        }))
    }

    async fn refresh(&self, lock: &LockInfo, seconds: u64) -> Result<bool> {
        self.dynamo
            .refresh_if_owned(&lock.name, &json!(lock.owner), Self::lock_seconds(seconds))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_encoded_like_laravel() {
        assert_eq!(attribute(&json!(42)), json!({"N": "42"}));
        assert_eq!(attribute(&json!(1.5)), json!({"N": "1.5"}));
        assert_eq!(attribute(&json!("Taylor")), json!({"S": "\"Taylor\""}));
        assert_eq!(attribute(&json!({"a": [1]})), json!({"S": "{\"a\":[1]}"}));

        for value in [
            json!(42),
            json!(-7),
            json!(1.5),
            json!("42"),
            json!("Taylor"),
            json!(true),
            json!(null),
            json!([1, 2]),
        ] {
            assert_eq!(unserialize(&serialize(&value)), value);
        }
        assert_eq!(unserialize("not json"), json!("not json"));
    }
}
