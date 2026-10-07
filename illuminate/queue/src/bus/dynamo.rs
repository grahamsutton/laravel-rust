//! The DynamoDB batch repository (`queue.batching.driver = "dynamodb"`).

use async_trait::async_trait;

use illuminate_cache::DynamoDbClient;
use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Map, Result, Str, Value, ValueExt, json};

use super::batch::UpdatedBatchJobCounts;
use super::repository::{BatchRecord, BatchRepository};

/// Turn a value into a DynamoDB attribute value (the AWS SDK's `Marshaler`).
///
/// ```
/// use illuminate_queue::bus::dynamo::{marshal_value, unmarshal_value};
/// use illuminate_support::json;
///
/// assert_eq!(marshal_value(&json!(3)), json!({"N": "3"}));
/// assert_eq!(marshal_value(&json!(["a"])), json!({"L": [{"S": "a"}]}));
/// assert_eq!(marshal_value(&json!(null)), json!({"NULL": true}));
/// assert_eq!(unmarshal_value(&json!({"M": {"total": {"N": "3"}}})), json!({"total": 3}));
/// ```
pub fn marshal_value(value: &Value) -> Value {
    match value {
        Value::Null => json!({"NULL": true}),
        Value::Bool(boolean) => json!({"BOOL": boolean}),
        Value::Number(number) => json!({"N": number.to_string()}),
        Value::String(string) => json!({"S": string}),
        Value::Array(items) => json!({"L": items.iter().map(marshal_value).collect::<Vec<_>>()}),
        Value::Object(map) => json!({"M": marshal_item(map)}),
    }
}

/// Turn an object into a DynamoDB item.
pub fn marshal_item(map: &Map<String, Value>) -> Value {
    Value::Object(
        map.iter()
            .map(|(key, value)| (key.clone(), marshal_value(value)))
            .collect(),
    )
}

/// Turn a DynamoDB attribute value back into a value.
pub fn unmarshal_value(attribute: &Value) -> Value {
    let Some((kind, value)) = attribute.as_object().and_then(|map| map.iter().next()) else {
        return Value::Null;
    };
    match kind.as_str() {
        "S" | "B" => value.clone(),
        "N" => {
            let number = value.as_str().unwrap_or_default();
            number
                .parse::<i64>()
                .map(Value::from)
                .or_else(|_| number.parse::<f64>().map(Value::from))
                .unwrap_or(Value::Null)
        }
        "BOOL" => Value::Bool(value.truthy()),
        "L" => Value::Array(
            value
                .as_array()
                .into_iter()
                .flatten()
                .map(unmarshal_value)
                .collect(),
        ),
        "M" => unmarshal_item(value),
        "SS" | "BS" => value.clone(),
        "NS" => Value::Array(
            value
                .as_array()
                .into_iter()
                .flatten()
                .map(|number| unmarshal_value(&json!({"N": number})))
                .collect(),
        ),
        _ => Value::Null,
    }
}

/// Turn a DynamoDB item back into an object.
pub fn unmarshal_item(item: &Value) -> Value {
    Value::Object(
        item.as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| (key.clone(), unmarshal_value(value)))
            .collect(),
    )
}

/// Stores batches in a DynamoDB table, exactly like Laravel's
/// `DynamoBatchRepository`: the partition key is `application` (the
/// application's name) and the sort key is `id` (the batch's ordered UUID).
///
/// With a `ttl`, every write sets the `ttl_attribute` to that many seconds
/// from now; enable DynamoDB's TTL on it to have old batches removed (the
/// repository itself never prunes).
///
/// ```json
/// "batching": {
///     "driver": "dynamodb",
///     "key": "AWS_ACCESS_KEY_ID",
///     "secret": "AWS_SECRET_ACCESS_KEY",
///     "region": "us-east-1",
///     "table": "job_batches",
///     "ttl": 86400,
///     "ttl_attribute": "ttl"
/// }
/// ```
#[derive(Debug, Clone)]
pub struct DynamoBatchRepository {
    dynamo: DynamoDbClient,
    application_name: String,
    table: String,
    ttl: Option<u64>,
    ttl_attribute: String,
}

/// The current UNIX timestamp (honouring `Carbon`'s test "now").
fn current_time() -> i64 {
    Carbon::now().timestamp()
}

impl DynamoBatchRepository {
    /// Create a repository storing the named application's batches in the
    /// given table.
    pub fn new(
        dynamo: DynamoDbClient,
        application_name: impl Into<String>,
        table: impl Into<String>,
    ) -> Self {
        Self {
            dynamo,
            application_name: application_name.into(),
            table: table.into(),
            ttl: None,
            ttl_attribute: "ttl".to_string(),
        }
    }

    /// Expire batches the given number of seconds after their last update,
    /// in the given attribute.
    pub fn with_ttl(mut self, ttl: Option<u64>, attribute: impl Into<String>) -> Self {
        self.ttl = ttl;
        self.ttl_attribute = attribute.into();
        self
    }

    /// Build the repository from the `queue.batching` configuration
    /// (Laravel's `BusServiceProvider`).
    pub fn from_config(batching: &Value, application_name: impl Into<String>) -> Self {
        let table = batching
            .get("table")
            .filter(|table| !table.is_blank())
            .map(ValueExt::to_string_lossy)
            .unwrap_or_else(|| "job_batches".to_string());
        let ttl = batching
            .get("ttl")
            .and_then(ValueExt::to_i64_lossy)
            .map(|ttl| ttl.max(0) as u64);
        let ttl_attribute = batching
            .get("ttl_attribute")
            .filter(|attribute| !attribute.is_blank())
            .map(ValueExt::to_string_lossy)
            .unwrap_or_else(|| "ttl".to_string());
        Self::new(
            DynamoDbClient::from_config(batching),
            application_name,
            table,
        )
        .with_ttl(ttl, ttl_attribute)
    }

    /// The DynamoDB client.
    pub fn get_dynamo_client(&self) -> &DynamoDbClient {
        &self.dynamo
    }

    /// The name of the table.
    pub fn get_table(&self) -> &str {
        &self.table
    }

    /// Create the table (with on-demand billing), and turn on TTL when the
    /// repository has one.
    pub async fn create_aws_dynamo_table(&self) -> Result<()> {
        self.dynamo
            .create_table(json!({
                "TableName": self.table,
                "AttributeDefinitions": [
                    {"AttributeName": "application", "AttributeType": "S"},
                    {"AttributeName": "id", "AttributeType": "S"},
                ],
                "KeySchema": [
                    {"AttributeName": "application", "KeyType": "HASH"},
                    {"AttributeName": "id", "KeyType": "RANGE"},
                ],
                "BillingMode": "PAY_PER_REQUEST",
            }))
            .await?;
        if self.ttl.is_some() {
            self.dynamo
                .update_time_to_live(json!({
                    "TableName": self.table,
                    "TimeToLiveSpecification": {
                        "AttributeName": self.ttl_attribute,
                        "Enabled": true,
                    },
                }))
                .await?;
        }
        Ok(())
    }

    /// Delete the table.
    pub async fn delete_aws_dynamo_table(&self) -> Result<()> {
        self.dynamo
            .delete_table(json!({"TableName": self.table}))
            .await?;
        Ok(())
    }

    fn key(&self, batch_id: &str) -> Value {
        json!({"application": {"S": self.application_name}, "id": {"S": batch_id}})
    }

    /// The expiration timestamp a write sets, when the repository has a TTL.
    fn expiry_time(&self) -> Option<String> {
        self.ttl.map(|ttl| {
            current_time()
                .saturating_add(i64::try_from(ttl).unwrap_or(i64::MAX))
                .to_string()
        })
    }

    /// Update a batch with the given `SET` clauses, adding the TTL.
    async fn update(
        &self,
        batch_id: &str,
        set: &str,
        mut values: Map<String, Value>,
        return_values: bool,
    ) -> Result<Value> {
        let mut input = Map::new();
        input.insert("TableName".into(), json!(self.table));
        input.insert("Key".into(), self.key(batch_id));
        let mut expression = format!("SET {set}");
        if let Some(expiry) = self.expiry_time() {
            expression.push_str(&format!(", #{0} = :ttl", self.ttl_attribute));
            values.insert(":ttl".into(), json!({"N": expiry}));
            input.insert(
                "ExpressionAttributeNames".into(),
                json!({format!("#{}", self.ttl_attribute): self.ttl_attribute}),
            );
        }
        input.insert("UpdateExpression".into(), json!(expression));
        input.insert("ExpressionAttributeValues".into(), Value::Object(values));
        if return_values {
            input.insert("ReturnValues".into(), json!("ALL_NEW"));
        }
        self.dynamo.update_item(Value::Object(input)).await
    }

    /// The job counts of an updated batch.
    fn counts(output: &Value) -> UpdatedBatchJobCounts {
        let values = unmarshal_item(&output["Attributes"]);
        let count = |key: &str| values[key].to_i64_lossy().unwrap_or(0).max(0) as u64;
        UpdatedBatchJobCounts::new(count("pending_jobs"), count("failed_jobs"))
    }

    /// A stored batch as a record.
    fn to_record(item: &Value) -> BatchRecord {
        let batch = unmarshal_item(item);
        let integer = |key: &str| batch[key].to_i64_lossy().unwrap_or(0);
        let timestamp = |key: &str| {
            batch[key]
                .to_i64_lossy()
                .filter(|timestamp| *timestamp != 0)
                .map(Carbon::from_timestamp)
        };
        let options = match &batch["options"] {
            Value::String(options) => serde_json::from_str(options).unwrap_or(Value::Null),
            other => other.clone(),
        };
        BatchRecord {
            id: batch["id"].to_string_lossy(),
            name: batch["name"].to_string_lossy(),
            total_jobs: integer("total_jobs").max(0) as u64,
            pending_jobs: integer("pending_jobs").max(0) as u64,
            failed_jobs: integer("failed_jobs").max(0) as u64,
            failed_job_ids: batch["failed_job_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .map(ValueExt::to_string_lossy)
                .collect(),
            options: if options.is_null() {
                json!({})
            } else {
                options
            },
            created_at: Carbon::from_timestamp(integer("created_at")),
            cancelled_at: timestamp("cancelled_at"),
            finished_at: timestamp("finished_at"),
        }
    }

    async fn get_item(&self, batch_id: &str, consistent: bool) -> Result<Option<Value>> {
        let mut input = json!({"TableName": self.table, "Key": self.key(batch_id)});
        if consistent {
            input["ConsistentRead"] = json!(true);
        }
        let output = self.dynamo.get_item(input).await?;
        Ok(output
            .get("Item")
            .filter(|item| item.as_object().is_some_and(|item| !item.is_empty()))
            .cloned())
    }
}

#[async_trait]
impl BatchRepository for DynamoBatchRepository {
    async fn get(&self, limit: usize, before: Option<&str>) -> Result<Vec<BatchRecord>> {
        let mut values = json!({":application": {"S": self.application_name}});
        let condition = match before.filter(|before| !before.is_empty()) {
            Some(before) => {
                values[":id"] = json!({"S": before});
                "application = :application AND id < :id"
            }
            None => "application = :application",
        };
        let output = self
            .dynamo
            .query(json!({
                "TableName": self.table,
                "KeyConditionExpression": condition,
                "ExpressionAttributeValues": values,
                "Limit": limit,
                "ScanIndexForward": false,
            }))
            .await?;
        Ok(output["Items"]
            .as_array()
            .into_iter()
            .flatten()
            .map(Self::to_record)
            .collect())
    }

    /// Find a batch, retrying with a consistent read when an eventually
    /// consistent one finds nothing.
    async fn find(&self, batch_id: &str) -> Result<Option<BatchRecord>> {
        if batch_id.trim().is_empty() {
            return Ok(None);
        }
        let item = match self.get_item(batch_id, false).await? {
            Some(item) => Some(item),
            None => self.get_item(batch_id, true).await?,
        };
        Ok(item.as_ref().map(Self::to_record))
    }

    async fn store(&self, name: &str, options: &Value) -> Result<BatchRecord> {
        let id = Str::ordered_uuid().to_string();
        let mut batch = Map::new();
        batch.insert("application".into(), json!(self.application_name));
        batch.insert("id".into(), json!(id));
        batch.insert("name".into(), json!(name));
        batch.insert("total_jobs".into(), json!(0));
        batch.insert("pending_jobs".into(), json!(0));
        batch.insert("failed_jobs".into(), json!(0));
        batch.insert("failed_job_ids".into(), json!([]));
        batch.insert("options".into(), json!(serde_json::to_string(options)?));
        batch.insert("created_at".into(), json!(current_time()));
        batch.insert("cancelled_at".into(), Value::Null);
        batch.insert("finished_at".into(), Value::Null);
        if let Some(expiry) = self.expiry_time() {
            batch.insert(
                self.ttl_attribute.clone(),
                json!(expiry.parse::<i64>().unwrap_or(0)),
            );
        }

        self.dynamo
            .put_item(json!({"TableName": self.table, "Item": marshal_item(&batch)}))
            .await?;

        self.find(&id).await?.ok_or_else(|| {
            RuntimeException::new(format!("Unable to find the stored batch [{id}].")).into()
        })
    }

    async fn increment_total_jobs(&self, batch_id: &str, amount: u64) -> Result<()> {
        let mut values = Map::new();
        values.insert(":val".into(), json!({"N": amount.to_string()}));
        self.update(
            batch_id,
            "total_jobs = total_jobs + :val, pending_jobs = pending_jobs + :val",
            values,
            true,
        )
        .await?;
        Ok(())
    }

    async fn decrement_pending_jobs(
        &self,
        batch_id: &str,
        _job_id: &str,
    ) -> Result<UpdatedBatchJobCounts> {
        let mut values = Map::new();
        values.insert(":inc".into(), json!({"N": "1"}));
        let output = self
            .update(batch_id, "pending_jobs = pending_jobs - :inc", values, true)
            .await?;
        Ok(Self::counts(&output))
    }

    async fn increment_failed_jobs(
        &self,
        batch_id: &str,
        job_id: &str,
    ) -> Result<UpdatedBatchJobCounts> {
        let mut values = Map::new();
        values.insert(":jobId".into(), marshal_value(&json!([job_id])));
        values.insert(":inc".into(), json!({"N": "1"}));
        let output = self
            .update(
                batch_id,
                "failed_jobs = failed_jobs + :inc, failed_job_ids = list_append(failed_job_ids, :jobId)",
                values,
                true,
            )
            .await?;
        Ok(Self::counts(&output))
    }

    async fn mark_as_finished(&self, batch_id: &str) -> Result<()> {
        let mut values = Map::new();
        values.insert(
            ":timestamp".into(),
            json!({"N": current_time().to_string()}),
        );
        self.update(batch_id, "finished_at = :timestamp", values, false)
            .await?;
        Ok(())
    }

    async fn cancel(&self, batch_id: &str) -> Result<()> {
        let mut values = Map::new();
        values.insert(
            ":timestamp".into(),
            json!({"N": current_time().to_string()}),
        );
        self.update(
            batch_id,
            "cancelled_at = :timestamp, finished_at = :timestamp",
            values,
            false,
        )
        .await?;
        Ok(())
    }

    async fn delete(&self, batch_id: &str) -> Result<()> {
        self.dynamo
            .delete_item(json!({"TableName": self.table, "Key": self.key(batch_id)}))
            .await?;
        Ok(())
    }

    /// Batches stored in DynamoDB expire with the table's TTL instead.
    async fn prune(&self, _before: Carbon) -> Result<u64> {
        Ok(0)
    }

    /// Batches stored in DynamoDB expire with the table's TTL instead.
    async fn prune_unfinished(&self, _before: Carbon) -> Result<u64> {
        Ok(0)
    }

    /// Batches stored in DynamoDB expire with the table's TTL instead.
    async fn prune_cancelled(&self, _before: Carbon) -> Result<u64> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_marshaled_like_the_aws_sdk() {
        let item = json!({
            "id": "1",
            "total": 3,
            "ratio": 1.5,
            "ids": ["a", "b"],
            "empty": [],
            "nested": {"ok": true},
            "missing": null,
        });
        let marshaled = marshal_item(item.as_object().unwrap());
        assert_eq!(
            marshaled,
            json!({
                "id": {"S": "1"},
                "total": {"N": "3"},
                "ratio": {"N": "1.5"},
                "ids": {"L": [{"S": "a"}, {"S": "b"}]},
                "empty": {"L": []},
                "nested": {"M": {"ok": {"BOOL": true}}},
                "missing": {"NULL": true},
            })
        );
        assert_eq!(unmarshal_item(&marshaled), item);
        assert_eq!(unmarshal_value(&json!({"NS": ["1", "2"]})), json!([1, 2]));
        assert_eq!(unmarshal_value(&json!("garbage")), Value::Null);
    }
}
