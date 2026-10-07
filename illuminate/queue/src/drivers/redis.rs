//! The `redis` driver: jobs wait in Redis lists.
//!
//! Exactly like Laravel's `RedisQueue`, each queue is a handful of keys:
//!
//! - `queues:{name}` — a list of the jobs ready to run, oldest first;
//! - `queues:{name}:delayed` — a sorted set of delayed (and released) jobs,
//!   scored by the UNIX timestamp they become available at;
//! - `queues:{name}:reserved` — a sorted set of the jobs being processed,
//!   scored by the time their reservation expires (`retry_after`);
//! - `queues:{name}:notify` — a list with an entry per ready job, which
//!   workers block on when `block_for` is set.
//!
//! Every operation is an atomic Lua script, so any number of workers on any
//! number of servers can share a queue: each job is handed to exactly one of
//! them, and a job whose worker died is handed to the next worker once its
//! reservation expires.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use illuminate_redis::{Connection, Redis, RedisManager};
use illuminate_support::{Carbon, Map, Result, Str, Value, ValueExt, json};

use crate::contracts::{Queue, enqueue_with};
use crate::envelope::Envelope;
use crate::queued_job::{JobBackend, QueuedJob};

/// How many seconds a reserved job may run before it is handed to another
/// worker, when the connection doesn't say (`retry_after`).
pub const DEFAULT_RETRY_AFTER: u64 = 60;

/// The current UNIX timestamp (honouring `Carbon`'s test "now").
fn current_time() -> i64 {
    Carbon::now().timestamp()
}

/// The UNIX timestamp a job delayed by `delay` becomes available at.
/// Partial seconds round up, so a delayed job never runs early.
fn available_at(delay: Duration) -> i64 {
    let seconds = delay.as_secs() + u64::from(delay.subsec_nanos() > 0);
    current_time().saturating_add(i64::try_from(seconds).unwrap_or(i64::MAX))
}

/// The Lua scripts behind the queue (Laravel's `Illuminate\Queue\LuaScripts`).
pub mod scripts {
    /// Get the size of a queue.
    ///
    /// KEYS[1] - The name of the primary queue
    /// KEYS[2] - The name of the "delayed" queue
    /// KEYS[3] - The name of the "reserved" queue
    pub const SIZE: &str = r#"
return redis.call('llen', KEYS[1]) + redis.call('zcard', KEYS[2]) + redis.call('zcard', KEYS[3])
"#;

    /// Push a job onto a queue.
    ///
    /// KEYS[1] - The queue to push the job onto, for example: queues:foo
    /// KEYS[2] - The notification list for the queue we are pushing jobs onto, for example: queues:foo:notify
    /// ARGV[1] - The job payload
    pub const PUSH: &str = r#"
-- Push the job onto the queue...
redis.call('rpush', KEYS[1], ARGV[1])
-- Push a notification onto the "notify" queue...
redis.call('rpush', KEYS[2], 1)
"#;

    /// Push a delayed job onto a queue.
    ///
    /// KEYS[1] - The delayed queue to push the job onto, for example: queues:foo:delayed
    /// ARGV[1] - The UNIX timestamp at which the job should become available
    /// ARGV[2] - The job payload
    pub const LATER: &str = r#"
-- Push the job onto the delayed queue...
redis.call('zadd', KEYS[1], ARGV[1], ARGV[2])
"#;

    /// Pop the next job off of a queue, reserving it.
    ///
    /// The reserved copy counts the attempt. Payloads written by this driver
    /// end with their `attempts` key, which is incremented in place: decoding
    /// and re-encoding them with `cjson` (as Laravel does for its
    /// PHP-serialized commands) would turn the job's empty arrays into
    /// objects and round its large numbers. Other payloads take Laravel's
    /// route, and one that isn't JSON at all is reserved as it is, so the
    /// worker can fail it instead of losing it.
    ///
    /// KEYS[1] - The queue to pop jobs from, for example: queues:foo
    /// KEYS[2] - The queue to place reserved jobs on, for example: queues:foo:reserved
    /// KEYS[3] - The notify queue
    /// ARGV[1] - The time at which the reserved job will expire
    pub const POP: &str = r#"
-- Pop the first job off of the queue...
local job = redis.call('lpop', KEYS[1])
local reserved = false

if(job ~= false) then
    -- Increment the attempt count and place job on the reserved queue...
    local attempts = string.match(job, '"attempts":(%d+)}$')
    if attempts then
        reserved = string.sub(job, 1, #job - #attempts - 1) .. (tonumber(attempts) + 1) .. '}'
    else
        local ok, decoded = pcall(cjson.decode, job)
        if ok and type(decoded) == 'table' then
            decoded['attempts'] = (tonumber(decoded['attempts']) or 0) + 1
            reserved = cjson.encode(decoded)
        else
            reserved = job
        end
    end
    redis.call('zadd', KEYS[2], ARGV[1], reserved)
    redis.call('lpop', KEYS[3])
end

return {job, reserved}
"#;

    /// Release a reserved job back onto the delayed queue.
    ///
    /// KEYS[1] - The "delayed" queue we release jobs onto, for example: queues:foo:delayed
    /// KEYS[2] - The queue the jobs are currently on, for example: queues:foo:reserved
    /// ARGV[1] - The raw payload of the job to add to the "delayed" queue
    /// ARGV[2] - The UNIX timestamp at which the job should become available
    pub const RELEASE: &str = r#"
-- Remove the job from the current queue...
redis.call('zrem', KEYS[2], ARGV[1])

-- Add the job onto the "delayed" queue...
redis.call('zadd', KEYS[1], ARGV[2], ARGV[1])

return true
"#;

    /// Migrate the expired jobs of a sorted set back onto the queue.
    ///
    /// KEYS[1] - The queue we are removing jobs from, for example: queues:foo:reserved
    /// KEYS[2] - The queue we are moving jobs to, for example: queues:foo
    /// KEYS[3] - The notification list for the queue we are moving jobs to, for example queues:foo:notify
    /// ARGV[1] - The current UNIX timestamp
    /// ARGV[2] - The most jobs to migrate at once (negative for all of them)
    pub const MIGRATE_EXPIRED_JOBS: &str = r#"
-- Get all of the jobs with an expired "score"...
local val = redis.call('zrangebyscore', KEYS[1], '-inf', ARGV[1], 'limit', 0, ARGV[2])

-- If we have values in the array, we will remove them from the first queue
-- and add them onto the destination queue in chunks of 100, which moves
-- all of the appropriate jobs onto the destination queue very safely.
if(next(val) ~= nil) then
    redis.call('zremrangebyrank', KEYS[1], 0, #val - 1)

    for i = 1, #val, 100 do
        redis.call('rpush', KEYS[2], unpack(val, i, math.min(i+99, #val)))
        -- Push a notification for every job that was migrated...
        for j = i, math.min(i+99, #val) do
            redis.call('rpush', KEYS[3], 1)
        end
    end
end

return val
"#;

    /// Remove every job from a queue.
    ///
    /// KEYS[1] - The name of the primary queue
    /// KEYS[2] - The name of the "delayed" queue
    /// KEYS[3] - The name of the "reserved" queue
    /// KEYS[4] - The name of the "notify" queue
    pub const CLEAR: &str = r#"
local size = redis.call('llen', KEYS[1]) + redis.call('zcard', KEYS[2]) + redis.call('zcard', KEYS[3])
redis.call('del', KEYS[1], KEYS[2], KEYS[3], KEYS[4])
return size
"#;
}

/// Prepare a payload for Redis: give it an `id` and an `attempts` count
/// when it has none, and make `attempts` its last key, so the pop script
/// can count attempts without re-encoding the payload. Returns the payload
/// and its id.
fn prepare_payload(payload: String) -> (String, Option<String>) {
    let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(&payload) else {
        return (payload, None);
    };
    let id = match map.get("id").and_then(Value::as_str) {
        Some(id) => id.to_string(),
        None => {
            let id = Str::random(32);
            map.insert("id".to_string(), Value::from(id.clone()));
            id
        }
    };
    let attempts = map
        .remove("attempts")
        .and_then(|attempts| attempts.to_i64_lossy())
        .unwrap_or(0)
        .max(0);
    map.insert("attempts".to_string(), Value::from(attempts));
    (Value::Object(map).to_string(), Some(id))
}

/// A queue backed by Redis (`QUEUE_CONNECTION=redis`).
///
/// ```no_run
/// use illuminate_queue::RedisQueue;
/// use illuminate_queue::contracts::Queue;
/// use illuminate_redis::Redis;
///
/// # async fn example() -> illuminate_support::Result<()> {
/// let queue = RedisQueue::new(Redis::manager()?).with_retry_after(Some(90));
///
/// queue.push_raw(r#"{"uuid":"1","job":"x","data":{}}"#.to_string(), Some("emails"), None).await?;
/// assert_eq!(queue.size(Some("emails")).await?, 1);
///
/// let job = queue.pop(Some("emails")).await?.unwrap();
/// assert_eq!(job.attempts(), 1);
/// job.delete().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct RedisQueue {
    redis: Arc<RedisManager>,
    name: String,
    connection: Option<String>,
    default_queue: String,
    retry_after: Option<u64>,
    block_for: Option<Duration>,
    after_commit: bool,
    migration_batch_size: i64,
    secondary_queue_had_job: Arc<AtomicBool>,
}

impl std::fmt::Debug for RedisQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisQueue")
            .field("name", &self.name)
            .field("connection", &self.connection)
            .field("default_queue", &self.default_queue)
            .field("retry_after", &self.retry_after)
            .field("block_for", &self.block_for)
            .finish_non_exhaustive()
    }
}

impl RedisQueue {
    /// Create a queue on the default Redis connection.
    pub fn new(redis: Arc<RedisManager>) -> Self {
        Self {
            redis,
            name: "redis".to_string(),
            connection: None,
            default_queue: "default".to_string(),
            retry_after: Some(DEFAULT_RETRY_AFTER),
            block_for: None,
            after_commit: false,
            migration_batch_size: -1,
            secondary_queue_had_job: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a queue from a `queue.connections.*` entry (Laravel's
    /// `RedisConnector`): `connection` names the Redis connection (the
    /// `default` one when null), `queue` defaults to `default`,
    /// `retry_after` to 60 seconds, `block_for` to not blocking, and
    /// `migration_batch_size` to migrating every expired job at once.
    ///
    /// Redis connections are resolved through the container's
    /// [`RedisManager`].
    pub fn from_config(config: &Value, name: &str) -> Result<Self> {
        let string = |key: &str| match config.get(key) {
            Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
            _ => None,
        };
        let block_for = config
            .get("block_for")
            .filter(|value| !value.is_null())
            .and_then(|value| {
                value
                    .as_f64()
                    .or_else(|| value.to_string_lossy().parse().ok())
            })
            .map(|seconds| Duration::from_secs_f64(seconds.max(0.0)));
        let retry_after = config
            .get("retry_after")
            .and_then(ValueExt::to_i64_lossy)
            .map_or(DEFAULT_RETRY_AFTER, |seconds| seconds.max(0) as u64);

        let mut queue = Self::new(Redis::manager()?)
            .with_connection_name(name)
            .with_default_queue(string("queue").unwrap_or_else(|| "default".to_string()))
            .with_retry_after(Some(retry_after))
            .with_block_for(block_for)
            .with_after_commit(config.get("after_commit").is_some_and(ValueExt::truthy))
            .with_migration_batch_size(
                config
                    .get("migration_batch_size")
                    .and_then(ValueExt::to_i64_lossy)
                    .unwrap_or(-1),
            );
        queue.connection = string("connection");
        Ok(queue)
    }

    /// Set the name of the queue connection.
    pub fn with_connection_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Keep the jobs on the given Redis connection.
    pub fn with_connection(mut self, connection: impl Into<String>) -> Self {
        self.connection = Some(connection.into());
        self
    }

    /// Set the default queue name.
    pub fn with_default_queue(mut self, queue: impl Into<String>) -> Self {
        self.default_queue = queue.into();
        self
    }

    /// Set how many seconds a reserved job may run before it is handed to
    /// another worker (`None` never hands it over).
    pub fn with_retry_after(mut self, seconds: Option<u64>) -> Self {
        self.retry_after = seconds;
        self
    }

    /// Wait up to the given time for a job to arrive when the queue is
    /// empty, instead of returning right away (`Some(Duration::ZERO)` waits
    /// forever).
    pub fn with_block_for(mut self, block_for: Option<Duration>) -> Self {
        self.block_for = block_for;
        self
    }

    /// Dispatch jobs after open database transactions commit.
    pub fn with_after_commit(mut self, after_commit: bool) -> Self {
        self.after_commit = after_commit;
        self
    }

    /// Set how many expired jobs are migrated back onto a queue at once
    /// (negative migrates all of them).
    pub fn with_migration_batch_size(mut self, size: i64) -> Self {
        self.migration_batch_size = size;
        self
    }

    /// The Redis connection the jobs live on.
    pub fn get_connection(&self) -> Result<Connection> {
        self.redis.connection(self.connection.as_deref())
    }

    /// The Redis manager.
    pub fn get_redis(&self) -> &Arc<RedisManager> {
        &self.redis
    }

    /// How many seconds a reserved job may run before it is handed to
    /// another worker.
    pub fn get_retry_after(&self) -> Option<u64> {
        self.retry_after
    }

    /// How long a pop waits for a job to arrive.
    pub fn get_block_for(&self) -> Option<Duration> {
        self.block_for
    }

    /// The Redis key of a queue (`queues:{name}`), the default queue when `None`.
    pub fn get_queue(&self, queue: Option<&str>) -> String {
        format!("queues:{}", self.queue_name(queue))
    }

    fn queue_name<'a>(&'a self, queue: Option<&'a str>) -> &'a str {
        match queue {
            Some(queue) if !queue.is_empty() => queue,
            _ => &self.default_queue,
        }
    }

    // ------------------------------------------------------------------
    // Inspecting the queues
    // ------------------------------------------------------------------

    /// The names of every queue on the connection.
    pub async fn queue_names(&self) -> Result<Vec<String>> {
        let mut names: Vec<String> = Vec::new();
        for key in self.get_connection()?.keys("queues:*").await? {
            let name = Str::between(&key, "queues:", ":");
            let name = name.trim_matches(|c| c == '{' || c == '}').to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    /// The number of jobs across every queue.
    pub async fn total_size(&self) -> Result<u64> {
        let mut total = 0;
        for name in self.queue_names().await? {
            total += self.size(Some(&name)).await?;
        }
        Ok(total)
    }

    /// The number of pending jobs across every queue.
    pub async fn total_pending_size(&self) -> Result<u64> {
        let mut total = 0;
        for name in self.queue_names().await? {
            total += self.pending_size(Some(&name)).await?;
        }
        Ok(total)
    }

    /// The number of delayed jobs across every queue.
    pub async fn total_delayed_size(&self) -> Result<u64> {
        let mut total = 0;
        for name in self.queue_names().await? {
            total += self.delayed_size(Some(&name)).await?;
        }
        Ok(total)
    }

    /// The number of reserved jobs across every queue.
    pub async fn total_reserved_size(&self) -> Result<u64> {
        let mut total = 0;
        for name in self.queue_names().await? {
            total += self.reserved_size(Some(&name)).await?;
        }
        Ok(total)
    }

    /// The payloads of the jobs ready to run, oldest first.
    pub async fn pending_jobs(&self, queue: Option<&str>) -> Result<Vec<Value>> {
        let payloads = self
            .get_connection()?
            .lrange(self.get_queue(queue), 0, -1)
            .await?;
        Ok(decode_all(payloads))
    }

    /// The payloads of the delayed jobs, soonest first.
    pub async fn delayed_jobs(&self, queue: Option<&str>) -> Result<Vec<Value>> {
        let key = format!("{}:delayed", self.get_queue(queue));
        Ok(decode_all(self.get_connection()?.zrange(key, 0, -1).await?))
    }

    /// The payloads of the jobs being processed.
    pub async fn reserved_jobs(&self, queue: Option<&str>) -> Result<Vec<Value>> {
        let key = format!("{}:reserved", self.get_queue(queue));
        Ok(decode_all(self.get_connection()?.zrange(key, 0, -1).await?))
    }

    /// The `createdAt` timestamp of the oldest job ready to run.
    pub async fn creation_time_of_oldest_pending_job(
        &self,
        queue: Option<&str>,
    ) -> Result<Option<i64>> {
        let payload = self
            .get_connection()?
            .lindex(self.get_queue(queue), 0)
            .await?;
        Ok(payload
            .and_then(|payload| serde_json::from_str::<Value>(&payload).ok())
            .and_then(|payload| payload.get("createdAt").and_then(ValueExt::to_i64_lossy)))
    }

    // ------------------------------------------------------------------
    // Moving jobs around
    // ------------------------------------------------------------------

    /// Move the jobs of a sorted set whose time has come (delayed jobs that
    /// are due, reservations that expired) onto a queue, returning them.
    pub async fn migrate_expired_jobs(&self, from: &str, to: &str) -> Result<Vec<String>> {
        let migrated = self
            .get_connection()?
            .eval(
                scripts::MIGRATE_EXPIRED_JOBS,
                (from, to, format!("{to}:notify")),
                (current_time(), self.migration_batch_size),
            )
            .await?;
        Ok(strings(migrated))
    }

    /// Migrate any delayed or expired jobs onto the queue.
    async fn migrate(&self, queue: &str) -> Result<()> {
        self.migrate_expired_jobs(&format!("{queue}:delayed"), queue)
            .await?;
        if self.retry_after.is_some() {
            self.migrate_expired_jobs(&format!("{queue}:reserved"), queue)
                .await?;
        }
        Ok(())
    }

    /// Reserve the next job, waiting for one (`block_for`) when allowed.
    /// Returns the job's payload and its reserved copy.
    async fn retrieve_next_job(
        &self,
        queue: &str,
        block: bool,
    ) -> Result<Option<(String, String)>> {
        let connection = self.get_connection()?;
        let reservation = available_at(Duration::from_secs(self.retry_after.unwrap_or(0)));
        let next = connection
            .eval(
                scripts::POP,
                (
                    queue,
                    format!("{queue}:reserved"),
                    format!("{queue}:notify"),
                ),
                (reservation,),
            )
            .await?;

        let mut next = strings_or_nulls(next);
        let reserved = next.pop().flatten();
        let job = next.pop().flatten();

        if let (Some(job), Some(reserved)) = (job, reserved) {
            return Ok(Some((job, reserved)));
        }

        if let Some(block_for) = self.block_for
            && block
            && connection
                .blpop(format!("{queue}:notify"), block_for.as_secs_f64())
                .await?
                .is_some()
        {
            return Box::pin(self.retrieve_next_job(queue, false)).await;
        }
        Ok(None)
    }

    /// Delete a reserved job.
    pub async fn delete_reserved(&self, queue: &str, reserved: &str) -> Result<()> {
        let key = format!("{}:reserved", self.get_queue(Some(queue)));
        self.get_connection()?.zrem(key, (reserved,)).await?;
        Ok(())
    }

    /// Move a reserved job back onto the delayed queue, available after the
    /// given delay. Its attempts are kept.
    pub async fn delete_and_release(
        &self,
        queue: &str,
        reserved: &str,
        delay: Duration,
    ) -> Result<()> {
        let queue = self.get_queue(Some(queue));
        self.get_connection()?
            .eval(
                scripts::RELEASE,
                (format!("{queue}:delayed"), format!("{queue}:reserved")),
                (reserved, available_at(delay)),
            )
            .await?;
        Ok(())
    }
}

/// The strings of a list reply.
fn strings(value: Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|item| item.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    }
}

/// The strings (or nulls) of a list reply.
fn strings_or_nulls(value: Value) -> Vec<Option<String>> {
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(|item| item.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    }
}

fn decode_all(payloads: Vec<String>) -> Vec<Value> {
    payloads
        .iter()
        .filter_map(|payload| serde_json::from_str(payload).ok())
        .collect()
}

/// Add the job's `id` to a payload created by the queue.
fn add_id(payload: &mut Value) {
    if let Value::Object(map) = payload {
        let mut ordered = Map::new();
        ordered.insert("id".to_string(), json!(Str::random(32)));
        ordered.extend(std::mem::take(map));
        *map = ordered;
    }
}

#[async_trait]
impl Queue for RedisQueue {
    fn connection_name(&self) -> &str {
        &self.name
    }

    fn default_queue(&self) -> &str {
        &self.default_queue
    }

    fn dispatches_after_commit(&self) -> bool {
        self.after_commit
    }

    async fn size(&self, queue: Option<&str>) -> Result<u64> {
        let queue = self.get_queue(queue);
        let size = self
            .get_connection()?
            .eval(
                scripts::SIZE,
                (
                    queue.as_str(),
                    format!("{queue}:delayed"),
                    format!("{queue}:reserved"),
                ),
                (),
            )
            .await?;
        Ok(size.to_i64_lossy().unwrap_or(0).max(0) as u64)
    }

    async fn pending_size(&self, queue: Option<&str>) -> Result<u64> {
        let size = self.get_connection()?.llen(self.get_queue(queue)).await?;
        Ok(size.max(0) as u64)
    }

    async fn delayed_size(&self, queue: Option<&str>) -> Result<u64> {
        let key = format!("{}:delayed", self.get_queue(queue));
        Ok(self.get_connection()?.zcard(key).await?.max(0) as u64)
    }

    async fn reserved_size(&self, queue: Option<&str>) -> Result<u64> {
        let key = format!("{}:reserved", self.get_queue(queue));
        Ok(self.get_connection()?.zcard(key).await?.max(0) as u64)
    }

    async fn push(&self, job: &Envelope, queue: Option<&str>) -> Result<Option<String>> {
        enqueue_with(self, job, queue, None, add_id).await
    }

    async fn later(
        &self,
        delay: Duration,
        job: &Envelope,
        queue: Option<&str>,
    ) -> Result<Option<String>> {
        enqueue_with(self, job, queue, Some(delay), add_id).await
    }

    async fn push_raw(
        &self,
        payload: String,
        queue: Option<&str>,
        delay: Option<Duration>,
    ) -> Result<Option<String>> {
        let (payload, id) = prepare_payload(payload);
        let queue = self.get_queue(queue);
        let connection = self.get_connection()?;

        match delay.filter(|delay| !delay.is_zero()) {
            Some(delay) => {
                connection
                    .eval(
                        scripts::LATER,
                        (format!("{queue}:delayed"),),
                        (available_at(delay), payload),
                    )
                    .await?;
            }
            None => {
                connection
                    .eval(
                        scripts::PUSH,
                        (queue.as_str(), format!("{queue}:notify")),
                        (payload,),
                    )
                    .await?;
            }
        }
        Ok(id)
    }

    async fn pop(&self, queue: Option<&str>) -> Result<Option<QueuedJob>> {
        self.pop_at(queue, 0).await
    }

    async fn pop_at(&self, queue: Option<&str>, index: usize) -> Result<Option<QueuedJob>> {
        let name = self.queue_name(queue).to_string();
        let prefixed = self.get_queue(Some(&name));
        self.migrate(&prefixed).await?;

        let block = !self.secondary_queue_had_job.load(Ordering::SeqCst) && index == 0;
        let next = self.retrieve_next_job(&prefixed, block).await?;

        if index == 0 {
            self.secondary_queue_had_job.store(false, Ordering::SeqCst);
        }

        let Some((job, reserved)) = next else {
            return Ok(None);
        };
        if index > 0 {
            self.secondary_queue_had_job.store(true, Ordering::SeqCst);
        }

        let decoded: Value = serde_json::from_str(&job).unwrap_or(Value::Null);
        let id = decoded
            .get("id")
            .map(ValueExt::to_string_lossy)
            .unwrap_or_default();
        let attempts = decoded
            .get("attempts")
            .and_then(ValueExt::to_i64_lossy)
            .unwrap_or(0)
            .clamp(0, i64::from(u32::MAX - 1)) as u32
            + 1;

        let backend: Arc<dyn JobBackend> = Arc::new(ReservedJob {
            queue: self.clone(),
            name: name.clone(),
            reserved: reserved.clone(),
        });
        match QueuedJob::new(id, job, attempts, &self.name, &name, backend) {
            Ok(job) => Ok(Some(job)),
            Err(error) => {
                // A payload that can't be read can never be processed.
                self.delete_reserved(&name, &reserved).await?;
                let logged = crate::failed::failer()
                    .log(&self.name, &name, &reserved, &format!("{error:?}"))
                    .await;
                if let Err(error) = logged {
                    crate::report(&error);
                }
                Err(error)
            }
        }
    }

    async fn clear(&self, queue: Option<&str>) -> Result<u64> {
        let queue = self.get_queue(queue);
        let cleared = self
            .get_connection()?
            .eval(
                scripts::CLEAR,
                (
                    queue.as_str(),
                    format!("{queue}:delayed"),
                    format!("{queue}:reserved"),
                    format!("{queue}:notify"),
                ),
                (),
            )
            .await?;
        Ok(cleared.to_i64_lossy().unwrap_or(0).max(0) as u64)
    }
}

/// A job reserved from a Redis queue (Laravel's `RedisJob`): its reserved
/// copy is what gets deleted or released.
struct ReservedJob {
    queue: RedisQueue,
    name: String,
    reserved: String,
}

#[async_trait]
impl JobBackend for ReservedJob {
    async fn delete(&self, _job: &QueuedJob) -> Result<()> {
        self.queue.delete_reserved(&self.name, &self.reserved).await
    }

    async fn release(&self, _job: &QueuedJob, delay: Duration) -> Result<()> {
        self.queue
            .delete_and_release(&self.name, &self.reserved, delay)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payloads_get_an_id_and_end_with_their_attempts() {
        let (payload, id) = prepare_payload(r#"{"uuid":"1","attempts":2,"data":{}}"#.to_string());
        let id = id.unwrap();
        assert_eq!(id.len(), 32);
        assert!(payload.ends_with(r#""attempts":2}"#), "{payload}");
        let decoded: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(decoded["id"], json!(id));
        assert_eq!(decoded["uuid"], json!("1"));

        let (payload, id) = prepare_payload(r#"{"id":"abc","uuid":"1"}"#.to_string());
        assert_eq!(id.as_deref(), Some("abc"));
        assert!(payload.ends_with(r#""attempts":0}"#), "{payload}");

        let (payload, id) = prepare_payload("not json".to_string());
        assert_eq!(payload, "not json");
        assert_eq!(id, None);
    }

    #[test]
    fn ids_lead_the_payload() {
        let mut payload = json!({"uuid": "1", "attempts": 0});
        add_id(&mut payload);
        let keys: Vec<&String> = payload.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["id", "uuid", "attempts"]);

        let mut scalar = json!("x");
        add_id(&mut scalar);
        assert_eq!(scalar, json!("x"));
    }

    #[test]
    fn delays_round_up_to_whole_seconds() {
        Carbon::set_thread_test_now(Some(Carbon::from_timestamp(1_000)));
        assert_eq!(available_at(Duration::ZERO), 1_000);
        assert_eq!(available_at(Duration::from_millis(1)), 1_001);
        assert_eq!(available_at(Duration::from_secs(60)), 1_060);
        Carbon::set_thread_test_now(None);
    }

    #[test]
    fn replies_are_read_as_strings() {
        assert_eq!(strings(json!(["a", 1, "b"])), ["a", "b"]);
        assert!(strings(json!(null)).is_empty());
        assert_eq!(
            strings_or_nulls(json!(["a", null])),
            [Some("a".to_string()), None]
        );
        assert!(strings_or_nulls(json!(1)).is_empty());
        assert_eq!(decode_all(vec!["{}".into(), "nope".into()]), [json!({})]);
    }
}
