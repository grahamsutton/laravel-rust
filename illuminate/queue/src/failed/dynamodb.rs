//! The DynamoDB failed job provider (`queue.failed.driver = "dynamodb"`).

use async_trait::async_trait;

use illuminate_cache::DynamoDbClient;
use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Result, Value, ValueExt, json};

use super::{FailedJob, FailedJobProvider, failed_job_id};

/// Keeps failed jobs in a DynamoDB table, exactly like Laravel's
/// `DynamoDbFailedJobProvider`: the table's partition key is `application`
/// (the application's name) and its sort key is `uuid` (the job's UUID).
///
/// Every failed job gets an `expires_at` attribute a week after it failed:
/// enable DynamoDB's TTL on it to have old failures removed, since failed
/// jobs stored in DynamoDB can't be flushed.
///
/// ```json
/// "failed": {
///     "driver": "dynamodb",
///     "key": "AWS_ACCESS_KEY_ID",
///     "secret": "AWS_SECRET_ACCESS_KEY",
///     "region": "us-east-1",
///     "table": "failed_jobs"
/// }
/// ```
#[derive(Debug, Clone)]
pub struct DynamoDbFailedJobProvider {
    dynamo: DynamoDbClient,
    application_name: String,
    table: String,
}

/// A string attribute of an item.
fn string(item: &Value, attribute: &str) -> String {
    item[attribute]["S"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A failed job read from an item.
fn to_failed_job(item: &Value) -> FailedJob {
    let failed_at = item["failed_at"]["N"]
        .as_str()
        .and_then(|timestamp| timestamp.parse::<i64>().ok())
        .unwrap_or(0);
    FailedJob {
        id: string(item, "uuid"),
        connection: string(item, "connection"),
        queue: string(item, "queue"),
        payload: string(item, "payload"),
        exception: string(item, "exception"),
        failed_at: Carbon::from_timestamp(failed_at),
    }
}

impl DynamoDbFailedJobProvider {
    /// Create a provider storing the failed jobs of the named application
    /// in the given table.
    pub fn new(
        dynamo: DynamoDbClient,
        application_name: impl Into<String>,
        table: impl Into<String>,
    ) -> Self {
        Self {
            dynamo,
            application_name: application_name.into(),
            table: table.into(),
        }
    }

    /// The DynamoDB client.
    pub fn get_client(&self) -> &DynamoDbClient {
        &self.dynamo
    }

    /// The name of the table.
    pub fn get_table(&self) -> &str {
        &self.table
    }

    /// The application whose failed jobs are stored.
    pub fn get_application_name(&self) -> &str {
        &self.application_name
    }

    fn key(&self, id: &str) -> Value {
        json!({"application": {"S": self.application_name}, "uuid": {"S": id}})
    }
}

#[async_trait]
impl FailedJobProvider for DynamoDbFailedJobProvider {
    async fn log(
        &self,
        connection: &str,
        queue: &str,
        payload: &str,
        exception: &str,
    ) -> Result<Option<String>> {
        let id = failed_job_id(payload);
        let failed_at = Carbon::now();
        self.dynamo
            .put_item(json!({
                "TableName": self.table,
                "Item": {
                    "application": {"S": self.application_name},
                    "uuid": {"S": id},
                    "connection": {"S": connection},
                    "queue": {"S": queue},
                    "payload": {"S": payload},
                    "exception": {"S": exception},
                    "failed_at": {"N": failed_at.timestamp().to_string()},
                    "expires_at": {"N": failed_at.add_week().timestamp().to_string()},
                },
            }))
            .await?;
        Ok(Some(id))
    }

    /// Every failed job of the application, newest first (following the
    /// query's pages).
    async fn all(&self) -> Result<Vec<FailedJob>> {
        let mut items: Vec<Value> = Vec::new();
        let mut start: Option<Value> = None;
        loop {
            let mut input = json!({
                "TableName": self.table,
                "Select": "ALL_ATTRIBUTES",
                "KeyConditionExpression": "application = :application",
                "ExpressionAttributeValues": {
                    ":application": {"S": self.application_name},
                },
                "ScanIndexForward": false,
            });
            if let Some(start) = start.take() {
                input["ExclusiveStartKey"] = start;
            }
            let output = self.dynamo.query(input).await?;
            items.extend(output["Items"].as_array().cloned().unwrap_or_default());
            match output.get("LastEvaluatedKey") {
                Some(key) if key.as_object().is_some_and(|key| !key.is_empty()) => {
                    start = Some(key.clone());
                }
                _ => break,
            }
        }

        let mut jobs: Vec<FailedJob> = items.iter().map(to_failed_job).collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.failed_at.timestamp()));
        Ok(jobs)
    }

    async fn find(&self, id: &str) -> Result<Option<FailedJob>> {
        let output = self
            .dynamo
            .get_item(json!({"TableName": self.table, "Key": self.key(id)}))
            .await?;
        Ok(output
            .get("Item")
            .filter(|item| item.as_object().is_some_and(|item| !item.is_empty()))
            .map(to_failed_job))
    }

    async fn forget(&self, id: &str) -> Result<bool> {
        self.dynamo
            .delete_item(json!({"TableName": self.table, "Key": self.key(id)}))
            .await?;
        Ok(true)
    }

    /// DynamoDB failed jobs can't be flushed: like Laravel, this always fails.
    async fn flush(&self, _hours: Option<u64>) -> Result<()> {
        Err(RuntimeException::new(
            "DynamoDb failed job storage may not be flushed. Please use DynamoDb's TTL features on your expires_at attribute.",
        )
        .into())
    }
}

/// Build the provider from the `queue.failed` configuration (Laravel's
/// `dynamoFailedJobProvider`).
pub(crate) fn from_config(failed: &Value, application_name: String) -> DynamoDbFailedJobProvider {
    let table = failed
        .get("table")
        .filter(|table| !table.is_blank())
        .map(ValueExt::to_string_lossy)
        .unwrap_or_else(|| "failed_jobs".to_string());
    DynamoDbFailedJobProvider::new(DynamoDbClient::from_config(failed), application_name, table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::Map;

    #[test]
    fn items_become_failed_jobs() {
        let mut item = Map::new();
        for (key, value) in [
            ("uuid", "abc"),
            ("connection", "sqs"),
            ("queue", "default"),
            ("payload", "{}"),
            ("exception", "Boom"),
        ] {
            item.insert(key.to_string(), json!({"S": value}));
        }
        item.insert("failed_at".into(), json!({"N": "1700000000"}));
        let job = to_failed_job(&Value::Object(item));
        assert_eq!(job.id, "abc");
        assert_eq!(job.connection, "sqs");
        assert_eq!(job.exception, "Boom");
        assert_eq!(job.failed_at.timestamp(), 1_700_000_000);
    }
}
