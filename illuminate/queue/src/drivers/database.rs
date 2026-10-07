//! The `database` driver: jobs wait in a database table.
//!
//! Jobs are rows of the `jobs` table, exactly like Laravel's
//! `DatabaseQueue`:
//!
//! ```php
//! Schema::create('jobs', function (Blueprint $table) {
//!     $table->id();
//!     $table->string('queue')->index();
//!     $table->longText('payload');
//!     $table->unsignedTinyInteger('attempts');
//!     $table->unsignedInteger('reserved_at')->nullable();
//!     $table->unsignedInteger('available_at');
//!     $table->unsignedInteger('created_at');
//! });
//! ```
//!
//! Timestamps are UNIX timestamps (seconds). A worker reserves a job by
//! stamping `reserved_at` inside a transaction (locking the row with
//! `FOR UPDATE SKIP LOCKED` where the database supports it), and a job that
//! stays reserved for longer than `retry_after` seconds is handed to the
//! next worker that asks.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use illuminate_database::query::Lock;
use illuminate_database::{Builder, Connection, DatabaseManager, Driver};
use illuminate_support::{Carbon, Error, Result, Value, ValueExt, json};

use crate::contracts::Queue;
use crate::queued_job::{JobBackend, QueuedJob};

/// How many seconds a reserved job may run before it is handed to another
/// worker, when the connection doesn't say (`retry_after`).
pub const DEFAULT_RETRY_AFTER: u64 = 90;

/// The current UNIX timestamp (honouring `Carbon`'s test "now").
fn current_time() -> i64 {
    Carbon::now().timestamp()
}

/// The UNIX timestamp a job pushed with the given delay becomes available
/// at. Partial seconds round up, so a delayed job never runs early.
fn available_at(delay: Duration) -> i64 {
    let seconds = delay.as_secs() + u64::from(delay.subsec_nanos() > 0);
    current_time().saturating_add(i64::try_from(seconds).unwrap_or(i64::MAX))
}

/// A job id as a query binding: numeric ids are bound as integers.
fn id_binding(id: &str) -> Value {
    id.parse::<i64>()
        .map(Value::from)
        .unwrap_or_else(|_| Value::from(id))
}

/// A row of the jobs table (Laravel's `DatabaseJobRecord`).
#[derive(Debug, Clone)]
struct JobRecord {
    id: String,
    payload: String,
    attempts: u32,
}

impl JobRecord {
    fn from_row(row: &Value) -> Self {
        let id = match row.get("id") {
            Some(Value::Null) | None => String::new(),
            Some(id) => id.to_string_lossy(),
        };
        Self {
            id,
            payload: row
                .get("payload")
                .map(ValueExt::to_string_lossy)
                .unwrap_or_default(),
            attempts: row
                .get("attempts")
                .and_then(ValueExt::to_i64_lossy)
                .unwrap_or(0)
                .clamp(0, i64::from(u32::MAX)) as u32,
        }
    }
}

/// The storage shared by a database queue and the jobs it hands out.
struct Store {
    connection: Connection,
    table: String,
    lock_for_popping: Mutex<Option<Lock>>,
}

impl Store {
    /// A query builder for the jobs table.
    fn table(&self) -> Builder {
        self.connection.table(self.table.as_str())
    }

    /// Insert a job, returning its id (Laravel's `pushToDatabase`).
    async fn push_to_database(
        &self,
        queue: &str,
        payload: &str,
        delay: Duration,
        attempts: u32,
    ) -> Result<i64> {
        self.table()
            .insert_get_id(json!({
                "queue": queue,
                "attempts": attempts,
                "reserved_at": null,
                "available_at": available_at(delay),
                "created_at": current_time(),
                "payload": payload,
            }))
            .await
    }

    /// Get the lock used to reserve jobs: `FOR UPDATE SKIP LOCKED` on
    /// MySQL 8.0.1+, MariaDB 10.6+ and PostgreSQL 9.5+, and a plain
    /// `FOR UPDATE` everywhere else (SQLite has no row locks: the
    /// transaction serializes workers).
    async fn lock_for_popping(&self) -> Lock {
        if let Some(lock) = self.lock_for_popping.lock().unwrap().clone() {
            return lock;
        }

        let driver = self.connection.driver();
        let version = match self.connection.get_config("version") {
            Value::Null => {
                if driver == Driver::Sqlite {
                    Some(String::new())
                } else {
                    self.server_version().await
                }
            }
            version => Some(version.to_string_lossy()),
        };

        // When the version can't be determined yet, try again next time.
        let Some(version) = version else {
            return Lock::Update;
        };

        let lock = if supports_skip_locked(driver, &version) {
            Lock::Raw("for update skip locked".to_string())
        } else {
            Lock::Update
        };
        *self.lock_for_popping.lock().unwrap() = Some(lock.clone());
        lock
    }

    /// The database server's version string.
    async fn server_version(&self) -> Option<String> {
        let query = match self.connection.driver() {
            Driver::Postgres => "show server_version",
            _ => "select version()",
        };
        self.connection
            .scalar(query, ())
            .await
            .ok()
            .map(|version| version.to_string_lossy())
    }

    /// Find the next job that is available (or whose reservation expired),
    /// locking its row.
    async fn next_available_job(
        &self,
        queue: &str,
        lock: Lock,
        retry_after: u64,
    ) -> Result<Option<JobRecord>> {
        let now = current_time();
        let expiration = Carbon::now()
            .sub_seconds(i64::try_from(retry_after).unwrap_or(i64::MAX))
            .timestamp();

        Ok(self
            .table()
            .lock(lock)
            .where_("queue", queue)
            .where_group(|query| {
                query
                    .where_group(|query| {
                        query
                            .where_null("reserved_at")
                            .where_op("available_at", "<=", now)
                    })
                    .or_where_group(|query| query.where_op("reserved_at", "<=", expiration))
            })
            .order_by("id", "asc")
            .first()
            .await?
            .map(|row| JobRecord::from_row(&row)))
    }

    /// Mark the job as reserved, counting the attempt.
    async fn mark_job_as_reserved(&self, mut record: JobRecord) -> Result<JobRecord> {
        record.attempts = record.attempts.saturating_add(1);
        self.table()
            .where_("id", id_binding(&record.id))
            .update(json!({
                "reserved_at": current_time(),
                "attempts": record.attempts,
            }))
            .await?;
        Ok(record)
    }

    /// Delete a reserved job (Laravel's `deleteReserved`).
    async fn delete_reserved(&self, id: &str) -> Result<()> {
        self.connection
            .transaction(|| async {
                if self
                    .table()
                    .lock_for_update()
                    .find(id_binding(id))
                    .await?
                    .is_some()
                {
                    self.table().where_("id", id_binding(id)).delete().await?;
                }
                Ok(())
            })
            .await
    }

    /// Delete a reserved job and push it back onto the queue with its
    /// attempts (Laravel's `deleteAndRelease`).
    async fn delete_and_release(&self, job: &QueuedJob, delay: Duration) -> Result<()> {
        self.connection
            .transaction(|| async {
                let id = id_binding(job.job_id());
                if self
                    .table()
                    .lock_for_update()
                    .find(id.clone())
                    .await?
                    .is_some()
                {
                    self.table().where_("id", id).delete().await?;
                }
                self.push_to_database(job.queue(), job.raw_body(), delay, job.attempts())
                    .await?;
                Ok(())
            })
            .await
    }
}

#[async_trait]
impl JobBackend for Store {
    async fn delete(&self, job: &QueuedJob) -> Result<()> {
        self.delete_reserved(job.job_id()).await
    }

    async fn release(&self, job: &QueuedJob, delay: Duration) -> Result<()> {
        self.delete_and_release(job, delay).await
    }
}

/// Determine if the database can skip rows locked by other workers
/// (Laravel's `getLockForPopping`).
fn supports_skip_locked(driver: Driver, version: &str) -> bool {
    let (engine, version) = if version.contains("MariaDB") {
        let version = version.split_once("5.5.5-").map_or(version, |(_, v)| v);
        ("mariadb", version.split('-').next().unwrap_or_default())
    } else if version.contains("vitess") || version.contains("PlanetScale") {
        ("vitess", version.split('-').next().unwrap_or_default())
    } else {
        (driver.name(), version)
    };

    let at_least = |minimum: &str| version_at_least(version, minimum);
    match engine {
        "mysql" => at_least("8.0.1"),
        "mariadb" => at_least("10.6.0"),
        "pgsql" => at_least("9.5"),
        "vitess" => at_least("19.0"),
        _ => false,
    }
}

/// Compare the leading `major.minor.patch` of a version string.
fn version_at_least(version: &str, minimum: &str) -> bool {
    fn parts(version: &str) -> Vec<u64> {
        let start = version
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(version.len());
        version[start..]
            .split(|c: char| !c.is_ascii_digit() && c != '.')
            .next()
            .unwrap_or_default()
            .split('.')
            .map_while(|part| part.parse().ok())
            .collect()
    }

    let (version, minimum) = (parts(version), parts(minimum));
    if version.is_empty() {
        return false;
    }
    let length = version.len().max(minimum.len());
    let pad = |parts: &[u64]| -> Vec<u64> {
        (0..length)
            .map(|i| parts.get(i).copied().unwrap_or(0))
            .collect()
    };
    pad(&version) >= pad(&minimum)
}

/// A queue backed by a database table (`QUEUE_CONNECTION=database`).
///
/// Every worker, on every server, sees the same jobs: workers reserve the
/// next available job inside a transaction, so a job is only ever handed
/// to one of them. A job whose worker died is released to another worker
/// once it has been reserved for `retry_after` seconds.
///
/// SQLite has no row locks: when several workers share a SQLite database
/// file, set the connection's `transaction_mode` to `IMMEDIATE` (and a
/// `busy_timeout`) so their transactions wait for each other instead of
/// failing with "database is locked".
///
/// ```
/// use illuminate_database::Connection;
/// use illuminate_queue::DatabaseQueue;
/// use illuminate_queue::contracts::Queue;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let connection = Connection::new("sqlite", json!({"driver": "sqlite", "database": ":memory:"}));
/// connection.get_schema_builder().create("jobs", |table| {
///     table.id();
///     table.string("queue").index();
///     table.long_text("payload");
///     table.unsigned_small_integer("attempts");
///     table.unsigned_integer("reserved_at").nullable();
///     table.unsigned_integer("available_at");
///     table.unsigned_integer("created_at");
/// }).await.unwrap();
///
/// let queue = DatabaseQueue::new(connection, "jobs").with_retry_after(90);
/// queue.push_raw(r#"{"uuid":"1","job":"x","data":{}}"#.to_string(), Some("emails"), None).await.unwrap();
/// assert_eq!(queue.size(Some("emails")).await.unwrap(), 1);
///
/// let job = queue.pop(Some("emails")).await.unwrap().unwrap();
/// assert_eq!(job.attempts(), 1);
/// assert_eq!(queue.reserved_size(Some("emails")).await.unwrap(), 1);
///
/// job.delete().await.unwrap();
/// assert_eq!(queue.size(Some("emails")).await.unwrap(), 0);
/// # });
/// ```
#[derive(Clone)]
pub struct DatabaseQueue {
    name: String,
    default_queue: String,
    retry_after: u64,
    after_commit: bool,
    store: Arc<Store>,
}

impl DatabaseQueue {
    /// Create a queue keeping its jobs in the given table of the given
    /// database connection.
    pub fn new(connection: Connection, table: impl Into<String>) -> Self {
        Self {
            name: "database".to_string(),
            default_queue: "default".to_string(),
            retry_after: DEFAULT_RETRY_AFTER,
            after_commit: false,
            store: Arc::new(Store {
                connection,
                table: table.into(),
                lock_for_popping: Mutex::new(None),
            }),
        }
    }

    /// Create a queue from a `queue.connections.*` entry (Laravel's
    /// `DatabaseConnector`): `connection` names the database connection
    /// (the default one when null), `table` defaults to `jobs`, `queue` to
    /// `default` and `retry_after` to 90 seconds.
    ///
    /// The database connection is resolved through the container's
    /// [`DatabaseManager`].
    pub fn from_config(config: &Value, name: &str) -> Self {
        let database = DatabaseManager::resolve();
        let connection = match config.get("connection") {
            Some(Value::String(connection)) if !connection.is_empty() => {
                database.connection(connection)
            }
            _ => database.default_connection(),
        };
        let string = |key: &str, default: &str| -> String {
            match config.get(key) {
                Some(Value::String(value)) if !value.is_empty() => value.clone(),
                _ => default.to_string(),
            }
        };
        let retry_after = config
            .get("retry_after")
            .and_then(ValueExt::to_i64_lossy)
            .map_or(DEFAULT_RETRY_AFTER, |seconds| seconds.max(0) as u64);

        Self::new(connection, string("table", "jobs"))
            .with_connection_name(name)
            .with_default_queue(string("queue", "default"))
            .with_retry_after(retry_after)
            .with_after_commit(config.get("after_commit").is_some_and(ValueExt::truthy))
    }

    /// Set the name of the queue connection.
    pub fn with_connection_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the default queue name.
    pub fn with_default_queue(mut self, queue: impl Into<String>) -> Self {
        self.default_queue = queue.into();
        self
    }

    /// Set how many seconds a reserved job may run before it is handed to
    /// another worker.
    pub fn with_retry_after(mut self, seconds: u64) -> Self {
        self.retry_after = seconds;
        self
    }

    /// Dispatch jobs after open database transactions commit.
    pub fn with_after_commit(mut self, after_commit: bool) -> Self {
        self.after_commit = after_commit;
        self
    }

    /// Get the underlying database connection.
    pub fn get_database(&self) -> &Connection {
        &self.store.connection
    }

    /// Get the name of the jobs table.
    pub fn get_table(&self) -> &str {
        &self.store.table
    }

    /// Get the number of seconds a reserved job may run before it is
    /// released.
    pub fn get_retry_after(&self) -> u64 {
        self.retry_after
    }

    /// Get the queue or return the default.
    pub fn get_queue<'a>(&'a self, queue: Option<&'a str>) -> &'a str {
        match queue {
            Some(queue) if !queue.is_empty() => queue,
            _ => &self.default_queue,
        }
    }

    fn table(&self) -> Builder {
        self.store.table()
    }

    fn pending(&self, query: Builder) -> Builder {
        query
            .where_null("reserved_at")
            .where_op("available_at", "<=", current_time())
    }

    fn delayed(&self, query: Builder) -> Builder {
        query
            .where_null("reserved_at")
            .where_op("available_at", ">", current_time())
    }

    /// The number of jobs across every queue.
    pub async fn total_size(&self) -> Result<u64> {
        count(self.table()).await
    }

    /// The number of pending jobs across every queue.
    pub async fn total_pending_size(&self) -> Result<u64> {
        count(self.pending(self.table())).await
    }

    /// The number of delayed jobs across every queue.
    pub async fn total_delayed_size(&self) -> Result<u64> {
        count(self.delayed(self.table())).await
    }

    /// The number of reserved jobs across every queue.
    pub async fn total_reserved_size(&self) -> Result<u64> {
        count(self.table().where_not_null("reserved_at")).await
    }

    /// The UNIX timestamp the oldest pending job became available at.
    pub async fn creation_time_of_oldest_pending_job(
        &self,
        queue: Option<&str>,
    ) -> Result<Option<i64>> {
        let query = self.table().where_("queue", self.get_queue(queue));
        Ok(self
            .pending(query)
            .oldest_by("available_at")
            .value("available_at")
            .await?
            .and_then(|value| value.to_i64_lossy()))
    }

    /// The payloads of the jobs waiting on the queue (including delayed
    /// and reserved jobs), oldest first.
    pub async fn payloads(&self, queue: Option<&str>) -> Result<Vec<Value>> {
        Ok(self
            .table()
            .where_("queue", self.get_queue(queue))
            .order_by("id", "asc")
            .pluck("payload")
            .await?
            .into_iter()
            .filter_map(|payload| serde_json::from_str(&payload.to_string_lossy()).ok())
            .collect())
    }

    /// A payload that can't be read can never be processed: fail it, like
    /// Laravel does, by deleting it and logging it as a failed job.
    async fn fail_unreadable_job(&self, queue: &str, record: &JobRecord, error: &Error) {
        if let Err(error) = self.store.delete_reserved(&record.id).await {
            crate::report(&error);
        }
        let logged = crate::failed::failer()
            .log(&self.name, queue, &record.payload, &format!("{error:?}"))
            .await;
        if let Err(error) = logged {
            crate::report(&error);
        }
    }
}

async fn count(query: Builder) -> Result<u64> {
    Ok(query.count().await?.max(0) as u64)
}

#[async_trait]
impl Queue for DatabaseQueue {
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
        count(self.table().where_("queue", self.get_queue(queue))).await
    }

    async fn pending_size(&self, queue: Option<&str>) -> Result<u64> {
        count(self.pending(self.table().where_("queue", self.get_queue(queue)))).await
    }

    async fn delayed_size(&self, queue: Option<&str>) -> Result<u64> {
        count(self.delayed(self.table().where_("queue", self.get_queue(queue)))).await
    }

    async fn reserved_size(&self, queue: Option<&str>) -> Result<u64> {
        count(
            self.table()
                .where_("queue", self.get_queue(queue))
                .where_not_null("reserved_at"),
        )
        .await
    }

    async fn push_raw(
        &self,
        payload: String,
        queue: Option<&str>,
        delay: Option<Duration>,
    ) -> Result<Option<String>> {
        let id = self
            .store
            .push_to_database(
                self.get_queue(queue),
                &payload,
                delay.unwrap_or_default(),
                0,
            )
            .await?;
        Ok(Some(id.to_string()))
    }

    async fn pop(&self, queue: Option<&str>) -> Result<Option<QueuedJob>> {
        let queue = self.get_queue(queue).to_string();
        let lock = self.store.lock_for_popping().await;
        let store = &self.store;

        let record = store
            .connection
            .transaction(|| async {
                match store
                    .next_available_job(&queue, lock, self.retry_after)
                    .await?
                {
                    Some(record) => store.mark_job_as_reserved(record).await.map(Some),
                    None => Ok(None),
                }
            })
            .await?;

        let Some(record) = record else {
            return Ok(None);
        };

        let backend: Arc<dyn JobBackend> = self.store.clone();
        match QueuedJob::new(
            record.id.clone(),
            record.payload.clone(),
            record.attempts,
            &self.name,
            &queue,
            backend,
        ) {
            Ok(job) => Ok(Some(job)),
            Err(error) => {
                self.fail_unreadable_job(&queue, &record, &error).await;
                Err(error)
            }
        }
    }

    async fn clear(&self, queue: Option<&str>) -> Result<u64> {
        self.table()
            .where_("queue", self.get_queue(queue))
            .delete()
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_locked_is_used_where_supported() {
        assert!(supports_skip_locked(Driver::MySql, "8.0.36"));
        assert!(supports_skip_locked(Driver::MySql, "8.0.1"));
        assert!(!supports_skip_locked(Driver::MySql, "8.0.0"));
        assert!(!supports_skip_locked(Driver::MySql, "5.7.44-log"));
        assert!(supports_skip_locked(
            Driver::MySql,
            "8.4.0-0ubuntu0.24.04.1"
        ));
        assert!(supports_skip_locked(
            Driver::MySql,
            "5.5.5-10.11.6-MariaDB-1:10.11.6+maria~ubu2204"
        ));
        assert!(supports_skip_locked(Driver::MariaDb, "10.6.17-MariaDB"));
        assert!(!supports_skip_locked(Driver::MariaDb, "10.5.24-MariaDB"));
        assert!(supports_skip_locked(
            Driver::Postgres,
            "16.2 (Debian 16.2-1.pgdg120+2)"
        ));
        assert!(supports_skip_locked(Driver::Postgres, "9.5"));
        assert!(!supports_skip_locked(Driver::Postgres, "9.4.26"));
        assert!(supports_skip_locked(Driver::MySql, "19.0.4-vitess"));
        assert!(!supports_skip_locked(Driver::MySql, "18.0.2-vitess"));
        assert!(!supports_skip_locked(Driver::Sqlite, "3.45.0"));
        assert!(!supports_skip_locked(Driver::MySql, ""));
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
    fn job_ids_are_bound_as_integers() {
        assert_eq!(id_binding("42"), json!(42));
        assert_eq!(id_binding("abc"), json!("abc"));
    }
}
