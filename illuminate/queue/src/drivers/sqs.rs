//! The `sqs` driver: jobs are Amazon SQS messages.
//!
//! Exactly like Laravel's `SqsQueue`, a job's payload is the message body,
//! a queue name is turned into a queue URL with the connection's `prefix`
//! and `suffix`, attempts come from the message's `ApproximateReceiveCount`,
//! releasing a job changes its message's visibility timeout, and SQS takes
//! care of handing a job whose worker died to the next worker (its
//! *visibility timeout* plays the part of `retry_after`).
//!
//! ```json
//! "sqs": {
//!     "driver": "sqs",
//!     "key": "AWS_ACCESS_KEY_ID",
//!     "secret": "AWS_SECRET_ACCESS_KEY",
//!     "prefix": "https://sqs.us-east-1.amazonaws.com/your-account-id",
//!     "queue": "default",
//!     "suffix": null,
//!     "region": "us-east-1",
//!     "after_commit": false,
//!     "overflow": {
//!         "enabled": false,
//!         "store": null,
//!         "always": false,
//!         "delete_after_processing": true,
//!         "flush_on_clear": false
//!     }
//! }
//! ```
//!
//! The SQS API is called through the `Http` client (the JSON protocol,
//! signed with AWS Signature Version 4), so `Http::fake()` works in tests.
//! Set `endpoint` to use LocalStack or ElasticMQ.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use indexmap::IndexMap;

use illuminate_cache::Cache;
use illuminate_cache::aws::AwsClient;
use illuminate_support::error::RuntimeException;
use illuminate_support::{Map, Result, Str, Value, ValueExt, json};

use crate::contracts::{Queue, enqueue_using};
use crate::delay::ceil_seconds;
use crate::envelope::Envelope;
use crate::events::{self, JobQueued, JobQueueing};
use crate::payload::{create_payload_value, encode_payload};
use crate::queued_job::{JobBackend, QueuedJob};

/// The largest message SQS accepts, in bytes (1 MB).
pub const MAX_SQS_PAYLOAD_SIZE: usize = 1_048_576;

/// The most messages a `SendMessageBatch` request may carry.
pub const MAX_MESSAGES_PER_BATCH: usize = 10;

/// The longest SQS lets a message be delayed: 15 minutes.
pub const MAX_DELAY_SECONDS: u64 = 900;

/// The longest SQS lets a message stay invisible: 12 hours.
pub const MAX_VISIBILITY_TIMEOUT: u64 = 43_200;

/// The cache key prefix of payloads stored in the overflow store.
pub const EXTENDED_PAYLOAD_CACHE_PREFIX: &str = "laravel:sqs-payloads:";

/// A client for the [Amazon SQS API](https://docs.aws.amazon.com/AWSSimpleQueueService/latest/APIReference/),
/// with one method per operation the queue uses. Inputs and outputs are the
/// API's JSON documents, just like the AWS SDK's arrays.
#[derive(Debug, Clone)]
pub struct SqsClient {
    client: AwsClient,
}

impl SqsClient {
    /// The prefix of every SQS `X-Amz-Target` header.
    pub const TARGET_PREFIX: &'static str = "AmazonSQS";

    /// Create a client for the given region.
    pub fn new(region: impl Into<String>) -> Self {
        Self::from_aws(AwsClient::new("sqs", Self::TARGET_PREFIX, region))
    }

    /// Create a client from a connection's configuration: `key`, `secret`,
    /// `token`, `region` and `endpoint`, with Laravel's 60 second timeout.
    pub fn from_config(config: &Value) -> Self {
        Self::from_aws(AwsClient::from_config("sqs", Self::TARGET_PREFIX, config).with_timeout(60))
    }

    /// Wrap an existing AWS client.
    pub fn from_aws(client: AwsClient) -> Self {
        Self { client }
    }

    /// The underlying AWS client.
    pub fn aws(&self) -> &AwsClient {
        &self.client
    }

    /// Call any SQS operation.
    pub async fn call(&self, operation: &str, input: Value) -> Result<Value> {
        self.client.call(operation, input).await
    }

    /// The `SendMessage` operation.
    pub async fn send_message(&self, input: Value) -> Result<Value> {
        self.call("SendMessage", input).await
    }

    /// The `SendMessageBatch` operation.
    pub async fn send_message_batch(&self, input: Value) -> Result<Value> {
        self.call("SendMessageBatch", input).await
    }

    /// The `ReceiveMessage` operation.
    pub async fn receive_message(&self, input: Value) -> Result<Value> {
        self.call("ReceiveMessage", input).await
    }

    /// The `DeleteMessage` operation.
    pub async fn delete_message(&self, input: Value) -> Result<Value> {
        self.call("DeleteMessage", input).await
    }

    /// The `ChangeMessageVisibility` operation.
    pub async fn change_message_visibility(&self, input: Value) -> Result<Value> {
        self.call("ChangeMessageVisibility", input).await
    }

    /// The `GetQueueAttributes` operation.
    pub async fn get_queue_attributes(&self, input: Value) -> Result<Value> {
        self.call("GetQueueAttributes", input).await
    }

    /// The `PurgeQueue` operation.
    pub async fn purge_queue(&self, input: Value) -> Result<Value> {
        self.call("PurgeQueue", input).await
    }
}

/// Where payloads too large for SQS are kept (the connection's `overflow`
/// options): a cache store holds the payload and SQS carries a pointer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverflowStorage {
    /// Whether large payloads are stored in the cache at all.
    pub enabled: bool,
    /// The cache store holding the payloads (the default store when `None`).
    pub store: Option<String>,
    /// Store every payload, whatever its size.
    pub always: bool,
    /// Forget a job's payload once the job is deleted.
    pub delete_after_processing: bool,
    /// Flush the whole store when the queue is cleared.
    pub flush_on_clear: bool,
}

impl OverflowStorage {
    /// Read the `overflow` options.
    pub fn from_config(config: &Value) -> Self {
        let flag = |key: &str| config.get(key).is_some_and(ValueExt::truthy);
        Self {
            enabled: flag("enabled"),
            store: config
                .get("store")
                .filter(|store| !store.is_blank())
                .map(ValueExt::to_string_lossy),
            always: flag("always"),
            delete_after_processing: flag("delete_after_processing"),
            flush_on_clear: flag("flush_on_clear"),
        }
    }

    fn cache(&self) -> Result<illuminate_cache::Repository> {
        Cache::driver(self.store.as_deref())
    }

    /// The overflow pointer a message body holds, if any.
    fn pointer(&self, body: &str) -> Option<String> {
        if !self.enabled || body.is_empty() {
            return None;
        }
        serde_json::from_str::<Value>(body)
            .ok()?
            .get("@pointer")?
            .as_str()
            .map(String::from)
    }
}

/// Determine if a queue name is a full URL (PHP's `FILTER_VALIDATE_URL`).
fn is_url(queue: &str) -> bool {
    queue.split_once("://").is_some_and(|(scheme, rest)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            && !rest.is_empty()
            && !rest.starts_with('/')
    })
}

/// A queue backed by Amazon SQS (`QUEUE_CONNECTION=sqs`).
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_http_client::Http;
/// use illuminate_queue::SqsQueue;
/// use illuminate_queue::contracts::Queue;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let _guard = Container::set_local_instance(Arc::new(Container::new()));
/// Http::fake_using(|_| Http::response(json!({"MessageId": "5fea7756-0ea4-451a-a703-a558b933e274"}), 200, &[]));
///
/// let queue = SqsQueue::from_config(&json!({
///     "key": "AKID",
///     "secret": "secret",
///     "prefix": "https://sqs.us-east-1.amazonaws.com/123456789012",
///     "queue": "default",
///     "region": "us-east-1",
/// }), "sqs");
///
/// assert_eq!(queue.get_queue(Some("emails")), "https://sqs.us-east-1.amazonaws.com/123456789012/emails");
///
/// let id = queue.push_raw(r#"{"uuid":"1","job":"x","data":{}}"#.into(), Some("emails"), None).await.unwrap();
/// assert_eq!(id.as_deref(), Some("5fea7756-0ea4-451a-a703-a558b933e274"));
///
/// Http::assert_sent(|request| {
///     request.has_header_value("X-Amz-Target", "AmazonSQS.SendMessage")
///         && request["QueueUrl"] == "https://sqs.us-east-1.amazonaws.com/123456789012/emails"
/// });
/// # });
/// ```
#[derive(Debug, Clone)]
pub struct SqsQueue {
    sqs: SqsClient,
    name: String,
    default_queue: String,
    prefix: String,
    suffix: String,
    after_commit: bool,
    overflow: OverflowStorage,
}

impl SqsQueue {
    /// Create a queue on the given client.
    pub fn new(sqs: SqsClient, default_queue: impl Into<String>) -> Self {
        Self {
            sqs,
            name: "sqs".to_string(),
            default_queue: default_queue.into(),
            prefix: String::new(),
            suffix: String::new(),
            after_commit: false,
            overflow: OverflowStorage::default(),
        }
    }

    /// Create a queue from a `queue.connections.*` entry (Laravel's
    /// `SqsConnector`): `key`, `secret`, `token`, `region`, `endpoint`,
    /// `prefix`, `queue` (default `default`), `suffix`, `after_commit` and
    /// `overflow`.
    pub fn from_config(config: &Value, name: &str) -> Self {
        let string = |key: &str| {
            config
                .get(key)
                .filter(|value| !value.is_blank())
                .map(ValueExt::to_string_lossy)
        };
        Self::new(
            SqsClient::from_config(config),
            string("queue").unwrap_or_else(|| "default".to_string()),
        )
        .with_connection_name(name)
        .with_prefix(string("prefix").unwrap_or_default())
        .with_suffix(string("suffix").unwrap_or_default())
        .with_after_commit(config.get("after_commit").is_some_and(ValueExt::truthy))
        .with_overflow(OverflowStorage::from_config(
            config.get("overflow").unwrap_or(&Value::Null),
        ))
    }

    /// Set the name of the queue connection.
    pub fn with_connection_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the queue URL prefix (`https://sqs.us-east-1.amazonaws.com/your-account-id`).
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Set the suffix added to every queue name (handy per environment).
    pub fn with_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = suffix.into();
        self
    }

    /// Dispatch jobs after open database transactions commit.
    pub fn with_after_commit(mut self, after_commit: bool) -> Self {
        self.after_commit = after_commit;
        self
    }

    /// Keep payloads too large for SQS in a cache store.
    pub fn with_overflow(mut self, overflow: OverflowStorage) -> Self {
        self.overflow = overflow;
        self
    }

    /// The SQS client.
    pub fn get_sqs(&self) -> &SqsClient {
        &self.sqs
    }

    /// The overflow storage options.
    pub fn get_overflow(&self) -> &OverflowStorage {
        &self.overflow
    }

    fn queue_name<'a>(&'a self, queue: Option<&'a str>) -> &'a str {
        match queue {
            Some(queue) if !queue.is_empty() => queue,
            _ => &self.default_queue,
        }
    }

    /// The URL of a queue (the default queue when `None`): full URLs are
    /// used as they are, and names get the prefix and suffix — FIFO queues
    /// keep their `.fifo` at the very end.
    ///
    /// ```
    /// use illuminate_queue::{SqsClient, SqsQueue};
    ///
    /// let queue = SqsQueue::new(SqsClient::new("us-east-1"), "default")
    ///     .with_prefix("https://sqs.us-east-1.amazonaws.com/123456789012/")
    ///     .with_suffix("-production");
    ///
    /// assert_eq!(queue.get_queue(None), "https://sqs.us-east-1.amazonaws.com/123456789012/default-production");
    /// assert_eq!(queue.get_queue(Some("orders.fifo")), "https://sqs.us-east-1.amazonaws.com/123456789012/orders-production.fifo");
    /// assert_eq!(queue.get_queue(Some("https://sqs.eu-west-1.amazonaws.com/1/other")), "https://sqs.eu-west-1.amazonaws.com/1/other");
    /// ```
    pub fn get_queue(&self, queue: Option<&str>) -> String {
        let queue = self.queue_name(queue);
        if is_url(queue) {
            return queue.to_string();
        }
        let prefix = self.prefix.trim_end_matches('/');
        match queue.strip_suffix(".fifo") {
            Some(name) => format!("{prefix}/{}.fifo", Str::finish(name, &self.suffix)),
            None => format!("{prefix}/{}", Str::finish(queue, &self.suffix)),
        }
    }

    /// The `SendMessage` options for a job (Laravel's `getQueueableOptions`):
    /// the delay (`DelaySeconds`, at most 15 minutes, never on FIFO
    /// queues), the message group (the job's, or the queue's name on FIFO
    /// queues) and, on FIFO queues, the deduplication id (the job's, or a
    /// fresh ordered UUID).
    pub fn get_queueable_options(
        &self,
        job: Option<&Envelope>,
        queue: Option<&str>,
        payload: &str,
        delay: Option<Duration>,
    ) -> Map<String, Value> {
        let queue = self.queue_name(queue);
        let is_fifo = queue.ends_with(".fifo");
        let mut options = Map::new();

        if let Some(delay) = delay.filter(|delay| !delay.is_zero())
            && !is_fifo
        {
            options.insert(
                "DelaySeconds".into(),
                json!(ceil_seconds(delay).min(MAX_DELAY_SECONDS)),
            );
        }

        if job.is_none() && !is_fifo {
            return options;
        }

        let mut group = job.and_then(|job| job.job().message_group());
        if is_fifo && group.is_none() {
            group = Some(queue.to_string());
        }
        if let Some(group) = group.filter(|group| !group.is_empty()) {
            options.insert("MessageGroupId".into(), json!(group));
        }

        if is_fifo {
            let deduplication_id = job
                .and_then(|job| job.job().deduplication_id(payload, queue))
                .unwrap_or_else(|| Str::ordered_uuid().to_string());
            if !deduplication_id.is_empty() {
                options.insert("MessageDeduplicationId".into(), json!(deduplication_id));
            }
        }

        options
    }

    /// Determine if a payload should be kept in the overflow store.
    fn will_overflow(&self, payload: &str) -> bool {
        self.overflow.enabled && (self.overflow.always || payload.len() >= MAX_SQS_PAYLOAD_SIZE)
    }

    /// Keep the payload in the overflow store, returning the pointer SQS carries.
    async fn overflow(&self, payload: String) -> Result<String> {
        let uuid = serde_json::from_str::<Value>(&payload)
            .ok()
            .and_then(|decoded| {
                decoded
                    .get("uuid")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .unwrap_or_else(|| Str::uuid().to_string());
        let path = format!("{EXTENDED_PAYLOAD_CACHE_PREFIX}{uuid}");
        self.overflow
            .cache()?
            .forever(&path, Value::String(payload))
            .await?;
        Ok(json!({"@pointer": path}).to_string())
    }

    /// The message body for a payload.
    async fn message_body(&self, payload: String) -> Result<String> {
        if self.will_overflow(&payload) {
            return self.overflow(payload).await;
        }
        Ok(payload)
    }

    /// Send a payload with the given options, returning the message id.
    async fn send(
        &self,
        payload: String,
        queue: Option<&str>,
        options: Map<String, Value>,
    ) -> Result<Option<String>> {
        let mut input = Map::new();
        input.insert("QueueUrl".into(), json!(self.get_queue(queue)));
        input.insert(
            "MessageBody".into(),
            json!(self.message_body(payload).await?),
        );
        input.extend(options);
        let output = self.sqs.send_message(Value::Object(input)).await?;
        Ok(output
            .get("MessageId")
            .filter(|id| !id.is_null())
            .map(ValueExt::to_string_lossy))
    }

    /// Fetch an attribute count of a queue.
    async fn attributes(&self, queue: Option<&str>, names: &[&str]) -> Result<u64> {
        let output = self
            .sqs
            .get_queue_attributes(json!({
                "QueueUrl": self.get_queue(queue),
                "AttributeNames": names,
            }))
            .await?;
        Ok(names
            .iter()
            .map(|name| {
                output["Attributes"]
                    .get(*name)
                    .and_then(ValueExt::to_i64_lossy)
                    .unwrap_or(0)
                    .max(0) as u64
            })
            .sum())
    }

    /// The number of jobs across every queue (not supported by SQS: always zero).
    pub async fn total_size(&self) -> Result<u64> {
        Ok(0)
    }

    /// The creation time of the oldest pending job (not supported by SQS).
    pub async fn creation_time_of_oldest_pending_job(
        &self,
        _queue: Option<&str>,
    ) -> Result<Option<i64>> {
        Ok(None)
    }

    /// Send a group of jobs with `SendMessageBatch`, ten messages (and at
    /// most 1 MB) per request, firing the queueing events around them.
    async fn send_batched_messages(&self, jobs: &[Envelope], queue: Option<&str>) -> Result<()> {
        let queue_name = self.queue_name(queue).to_string();
        let url = self.get_queue(queue);

        let mut entries: Vec<(Value, usize)> = Vec::with_capacity(jobs.len());
        let mut messages: Vec<(Envelope, String, Option<Duration>)> =
            Vec::with_capacity(jobs.len());
        for (id, job) in jobs.iter().enumerate() {
            let delay = job.get_delay().filter(|delay| !delay.is_zero());
            let payload = create_payload_value(job, &self.name, &queue_name, delay)?;
            let payload = encode_payload(job, &queue_name, &payload)?;

            events::dispatch(JobQueueing {
                connection_name: self.name.clone(),
                queue: queue_name.clone(),
                job: job.clone(),
                payload: payload.clone(),
                delay,
            });

            let mut entry = Map::new();
            entry.insert("Id".into(), json!(id.to_string()));
            let body = self.message_body(payload.clone()).await?;
            let size = body.len();
            entry.insert("MessageBody".into(), json!(body));
            entry.extend(self.get_queueable_options(Some(job), Some(&queue_name), &payload, delay));
            entries.push((Value::Object(entry), size));
            messages.push((job.clone(), payload, delay));
        }

        for chunk in chunk_batch_entries(entries) {
            let count = chunk.len();
            let output = self
                .sqs
                .send_message_batch(json!({"QueueUrl": url, "Entries": chunk}))
                .await?;

            for success in output["Successful"].as_array().into_iter().flatten() {
                let Some((job, payload, delay)) = success["Id"]
                    .as_str()
                    .and_then(|id| id.parse::<usize>().ok())
                    .and_then(|id| messages.get(id))
                else {
                    continue;
                };
                events::dispatch(JobQueued {
                    connection_name: self.name.clone(),
                    queue: queue_name.clone(),
                    id: success.get("MessageId").map(ValueExt::to_string_lossy),
                    job: job.clone(),
                    payload: payload.clone(),
                    delay: *delay,
                });
            }

            if let Some(failed) = output["Failed"]
                .as_array()
                .filter(|failed| !failed.is_empty())
            {
                let failure = &failed[0];
                return Err(RuntimeException::new(format!(
                    "SQS SendMessageBatch rejected [{}] of [{count}] messages. First failure [{}]: {}",
                    failed.len(),
                    failure
                        .get("Code")
                        .map(ValueExt::to_string_lossy)
                        .unwrap_or_else(|| "Unknown".to_string()),
                    failure
                        .get("Message")
                        .map(ValueExt::to_string_lossy)
                        .unwrap_or_default(),
                ))
                .into());
            }
        }
        Ok(())
    }

    /// A message whose payload can't be read can never be processed:
    /// delete it and log it as a failed job.
    async fn discard(
        &self,
        url: &str,
        receipt_handle: &str,
        body: &str,
        error: &illuminate_support::Error,
    ) {
        let deleted = self
            .sqs
            .delete_message(json!({"QueueUrl": url, "ReceiptHandle": receipt_handle}))
            .await;
        if let Err(error) = deleted {
            crate::report(&error);
        }
        let logged = crate::failed::failer()
            .log(&self.name, url, body, &format!("{error:?}"))
            .await;
        if let Err(error) = logged {
            crate::report(&error);
        }
    }
}

/// Chunk batch entries by the 10 message and 1 MB limits of `SendMessageBatch`.
fn chunk_batch_entries(entries: Vec<(Value, usize)>) -> Vec<Vec<Value>> {
    let mut chunks = Vec::new();
    let mut current: Vec<Value> = Vec::new();
    let mut bytes = 0;
    for (entry, size) in entries {
        let too_many = current.len() >= MAX_MESSAGES_PER_BATCH;
        let too_big = bytes + size > MAX_SQS_PAYLOAD_SIZE;
        if !current.is_empty() && (too_many || too_big) {
            chunks.push(std::mem::take(&mut current));
            bytes = 0;
        }
        current.push(entry);
        bytes += size;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[async_trait]
impl Queue for SqsQueue {
    fn connection_name(&self) -> &str {
        &self.name
    }

    fn default_queue(&self) -> &str {
        &self.default_queue
    }

    fn dispatches_after_commit(&self) -> bool {
        self.after_commit
    }

    /// The approximate number of visible, delayed and in-flight messages.
    async fn size(&self, queue: Option<&str>) -> Result<u64> {
        self.attributes(
            queue,
            &[
                "ApproximateNumberOfMessages",
                "ApproximateNumberOfMessagesDelayed",
                "ApproximateNumberOfMessagesNotVisible",
            ],
        )
        .await
    }

    async fn pending_size(&self, queue: Option<&str>) -> Result<u64> {
        self.attributes(queue, &["ApproximateNumberOfMessages"])
            .await
    }

    async fn delayed_size(&self, queue: Option<&str>) -> Result<u64> {
        self.attributes(queue, &["ApproximateNumberOfMessagesDelayed"])
            .await
    }

    async fn reserved_size(&self, queue: Option<&str>) -> Result<u64> {
        self.attributes(queue, &["ApproximateNumberOfMessagesNotVisible"])
            .await
    }

    async fn push(&self, job: &Envelope, queue: Option<&str>) -> Result<Option<String>> {
        enqueue_using(
            self,
            job,
            queue,
            None,
            |_| {},
            |payload: String, name: String, _delay: Option<Duration>| async move {
                let options = self.get_queueable_options(Some(job), Some(&name), &payload, None);
                self.send(payload, Some(&name), options).await
            },
        )
        .await
    }

    async fn later(
        &self,
        delay: Duration,
        job: &Envelope,
        queue: Option<&str>,
    ) -> Result<Option<String>> {
        enqueue_using(
            self,
            job,
            queue,
            Some(delay),
            |_| {},
            |payload: String, name: String, delay: Option<Duration>| async move {
                let options = self.get_queueable_options(Some(job), Some(&name), &payload, delay);
                self.send(payload, Some(&name), options).await
            },
        )
        .await
    }

    /// Push several jobs with `SendMessageBatch`, a batch per queue.
    async fn bulk(&self, jobs: &[Envelope], queue: Option<&str>) -> Result<()> {
        let mut groups: IndexMap<Option<String>, Vec<Envelope>> = IndexMap::new();
        for job in jobs {
            let queue = queue.or(job.queue_name()).map(String::from);
            groups.entry(queue).or_default().push(job.clone());
        }
        for (queue, jobs) in groups {
            self.send_batched_messages(&jobs, queue.as_deref()).await?;
        }
        Ok(())
    }

    async fn push_raw(
        &self,
        payload: String,
        queue: Option<&str>,
        delay: Option<Duration>,
    ) -> Result<Option<String>> {
        let options = self.get_queueable_options(None, queue, &payload, delay);
        self.send(payload, queue, options).await
    }

    async fn pop(&self, queue: Option<&str>) -> Result<Option<QueuedJob>> {
        let url = self.get_queue(queue);
        let output = self
            .sqs
            .receive_message(json!({
                "QueueUrl": url,
                "AttributeNames": ["ApproximateReceiveCount"],
            }))
            .await?;
        let Some(message) = output["Messages"]
            .as_array()
            .and_then(|messages| messages.first())
        else {
            return Ok(None);
        };

        let id = message["MessageId"].to_string_lossy();
        let receipt_handle = message["ReceiptHandle"].to_string_lossy();
        let body = message["Body"].as_str().unwrap_or_default().to_string();
        let attempts = message["Attributes"]["ApproximateReceiveCount"]
            .to_i64_lossy()
            .unwrap_or(1)
            .clamp(0, i64::from(u32::MAX)) as u32;

        let pointer = self.overflow.pointer(&body);
        let raw_body = match &pointer {
            Some(pointer) => match self.overflow.cache()?.get(pointer).await? {
                Some(Value::String(payload)) => payload,
                Some(other) => other.to_string(),
                None => String::new(),
            },
            None => body.clone(),
        };

        let backend: Arc<dyn JobBackend> = Arc::new(SqsJob {
            sqs: self.sqs.clone(),
            queue_url: url.clone(),
            receipt_handle: receipt_handle.clone(),
            overflow: self.overflow.clone(),
            pointer,
        });
        match QueuedJob::new(id, raw_body, attempts, &self.name, &url, backend) {
            Ok(job) => Ok(Some(job)),
            Err(error) => {
                self.discard(&url, &receipt_handle, &body, &error).await;
                Err(error)
            }
        }
    }

    /// Purge the queue, returning how many jobs it held. SQS may take up
    /// to a minute to delete them all.
    async fn clear(&self, queue: Option<&str>) -> Result<u64> {
        let size = self.size(queue).await?;
        self.sqs
            .purge_queue(json!({"QueueUrl": self.get_queue(queue)}))
            .await?;
        if self.overflow.enabled && self.overflow.flush_on_clear {
            self.overflow.cache()?.flush().await?;
        }
        Ok(size)
    }
}

/// A message received from SQS (Laravel's `SqsJob`): deleted by its
/// receipt handle, and released by changing its visibility timeout.
struct SqsJob {
    sqs: SqsClient,
    queue_url: String,
    receipt_handle: String,
    overflow: OverflowStorage,
    pointer: Option<String>,
}

#[async_trait]
impl JobBackend for SqsJob {
    async fn delete(&self, _job: &QueuedJob) -> Result<()> {
        self.sqs
            .delete_message(json!({
                "QueueUrl": self.queue_url,
                "ReceiptHandle": self.receipt_handle,
            }))
            .await?;
        if self.overflow.delete_after_processing
            && let Some(pointer) = &self.pointer
        {
            self.overflow.cache()?.forget(pointer).await?;
        }
        Ok(())
    }

    async fn release(&self, _job: &QueuedJob, delay: Duration) -> Result<()> {
        self.sqs
            .change_message_visibility(json!({
                "QueueUrl": self.queue_url,
                "ReceiptHandle": self.receipt_handle,
                "VisibilityTimeout": ceil_seconds(delay).min(MAX_VISIBILITY_TIMEOUT),
            }))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_recognized() {
        assert!(is_url("https://sqs.us-east-1.amazonaws.com/1/default"));
        assert!(is_url("http://localhost:4566/000000000000/default"));
        assert!(!is_url("default"));
        assert!(!is_url("emails.fifo"));
        assert!(!is_url("://nope"));
        assert!(!is_url("http:///nope"));
    }

    #[test]
    fn queue_urls_follow_laravel() {
        let queue = SqsQueue::new(SqsClient::new("us-east-1"), "default")
            .with_prefix("https://sqs.us-east-1.amazonaws.com/123456789012");
        assert_eq!(
            queue.get_queue(None),
            "https://sqs.us-east-1.amazonaws.com/123456789012/default"
        );
        assert_eq!(
            queue.get_queue(Some("")),
            "https://sqs.us-east-1.amazonaws.com/123456789012/default"
        );

        let queue = queue.with_suffix("-staging");
        assert_eq!(
            queue.get_queue(Some("emails-staging")),
            "https://sqs.us-east-1.amazonaws.com/123456789012/emails-staging"
        );
        assert_eq!(
            queue.get_queue(Some("orders.fifo")),
            "https://sqs.us-east-1.amazonaws.com/123456789012/orders-staging.fifo"
        );
    }

    #[test]
    fn options_follow_the_queue_type() {
        let queue = SqsQueue::new(SqsClient::new("us-east-1"), "default");
        let options = queue.get_queueable_options(None, None, "{}", Some(Duration::from_secs(30)));
        assert_eq!(Value::Object(options), json!({"DelaySeconds": 30}));

        let options =
            queue.get_queueable_options(None, None, "{}", Some(Duration::from_secs(3600)));
        assert_eq!(Value::Object(options), json!({"DelaySeconds": 900}));

        let options = queue.get_queueable_options(
            None,
            Some("orders.fifo"),
            "{}",
            Some(Duration::from_secs(30)),
        );
        assert_eq!(options["MessageGroupId"], json!("orders.fifo"));
        assert_eq!(
            options["MessageDeduplicationId"].as_str().unwrap().len(),
            36
        );
        assert!(!options.contains_key("DelaySeconds"));
    }

    #[test]
    fn batches_respect_the_count_and_size_limits() {
        let entries: Vec<(Value, usize)> = (0..23).map(|i| (json!(i), 10)).collect();
        let chunks = chunk_batch_entries(entries);
        assert_eq!(
            chunks.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![10, 10, 3]
        );

        let entries = vec![(json!(0), 600_000), (json!(1), 600_000), (json!(2), 100)];
        let chunks = chunk_batch_entries(entries);
        assert_eq!(chunks.iter().map(Vec::len).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn overflow_options_are_read_from_configuration() {
        let overflow = OverflowStorage::from_config(&json!({
            "enabled": true,
            "store": "dynamodb",
            "always": false,
            "delete_after_processing": true,
            "flush_on_clear": false,
        }));
        assert_eq!(
            overflow,
            OverflowStorage {
                enabled: true,
                store: Some("dynamodb".into()),
                always: false,
                delete_after_processing: true,
                flush_on_clear: false,
            }
        );
        assert_eq!(
            overflow.pointer(r#"{"@pointer":"laravel:sqs-payloads:1"}"#),
            Some("laravel:sqs-payloads:1".to_string())
        );
        assert_eq!(overflow.pointer(r#"{"uuid":"1"}"#), None);
        assert_eq!(
            OverflowStorage::default().pointer(r#"{"@pointer":"x"}"#),
            None
        );
    }
}
