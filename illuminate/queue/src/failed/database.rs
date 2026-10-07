//! The `database-uuids` failed job provider.

use async_trait::async_trait;

use illuminate_database::{Builder, Connection};
use illuminate_support::{Carbon, Result, Value, ValueExt, json};

use super::{FailedJob, FailedJobProvider, failed_job_id};

/// How many rows a prune deletes per query.
const PRUNE_CHUNK: i64 = 1000;

/// Keeps failed jobs in the `failed_jobs` table, identified by the job's
/// UUID (`queue.failed.driver` = `database-uuids`), exactly like Laravel's
/// `DatabaseUuidFailedJobProvider`:
///
/// ```php
/// Schema::create('failed_jobs', function (Blueprint $table) {
///     $table->id();
///     $table->string('uuid')->unique();
///     $table->text('connection');
///     $table->text('queue');
///     $table->longText('payload');
///     $table->longText('exception');
///     $table->timestamp('failed_at')->useCurrent();
/// });
/// ```
///
/// ```
/// use illuminate_database::Connection;
/// use illuminate_queue::{DatabaseUuidFailedJobProvider, FailedJobProvider};
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let connection = Connection::new("sqlite", json!({"driver": "sqlite", "database": ":memory:"}));
/// connection.get_schema_builder().create("failed_jobs", |table| {
///     table.id();
///     table.string("uuid").unique();
///     table.string("connection");
///     table.string("queue");
///     table.long_text("payload");
///     table.long_text("exception");
///     table.timestamp("failed_at").use_current();
/// }).await.unwrap();
///
/// let failer = DatabaseUuidFailedJobProvider::new(connection, "failed_jobs");
/// let payload = json!({"uuid": "9a4c2b1e", "displayName": "ProcessPodcast"}).to_string();
/// failer.log("database", "default", &payload, "RuntimeException: boom").await.unwrap();
///
/// let job = failer.find("9a4c2b1e").await.unwrap().unwrap();
/// assert_eq!(job.display_name(), "ProcessPodcast");
/// assert!(failer.forget("9a4c2b1e").await.unwrap());
/// # });
/// ```
#[derive(Debug, Clone)]
pub struct DatabaseUuidFailedJobProvider {
    connection: Connection,
    table: String,
}

impl DatabaseUuidFailedJobProvider {
    /// Store failed jobs in the given table of the given connection.
    pub fn new(connection: Connection, table: impl Into<String>) -> Self {
        Self {
            connection,
            table: table.into(),
        }
    }

    /// Get a new query builder instance for the table.
    pub fn get_table(&self) -> Builder {
        self.connection.table(self.table.as_str())
    }

    /// The name of the failed jobs table.
    pub fn table_name(&self) -> &str {
        &self.table
    }

    /// The database connection the failed jobs are stored on.
    pub fn get_connection(&self) -> &Connection {
        &self.connection
    }

    /// Delete the failed jobs of a queue, optionally only those that failed
    /// at least `hours` hours ago (`queue:flush --queue=`).
    pub async fn flush_queue(&self, hours: Option<u64>, queue: Option<&str>) -> Result<u64> {
        let mut query = self.get_table();
        if let Some(hours) = hours.filter(|hours| *hours > 0) {
            let before = Carbon::now().sub_hours(i64::try_from(hours).unwrap_or(i64::MAX));
            query = query.where_op("failed_at", "<=", before.to_date_time_string());
        }
        if let Some(queue) = queue {
            query = query.where_("queue", queue);
        }
        query.delete().await
    }
}

/// A string column of a row.
fn column(row: &Value, column: &str) -> String {
    match row.get(column) {
        Some(Value::Null) | None => String::new(),
        Some(value) => value.to_string_lossy(),
    }
}

/// Read a `failed_at` column (stored as `Y-m-d H:i:s`).
fn failed_at(row: &Value) -> Carbon {
    match row.get("failed_at") {
        Some(Value::String(date)) => Carbon::create_from_format("Y-m-d H:i:s", date)
            .or_else(|_| Carbon::parse(date))
            .unwrap_or_else(|_| Carbon::from_timestamp(0)),
        Some(value) => value
            .to_i64_lossy()
            .map_or_else(|| Carbon::from_timestamp(0), Carbon::from_timestamp),
        None => Carbon::from_timestamp(0),
    }
}

/// A row of the table as a failed job (its UUID is its id).
fn to_failed_job(row: &Value) -> FailedJob {
    FailedJob {
        id: column(row, "uuid"),
        connection: column(row, "connection"),
        queue: column(row, "queue"),
        payload: column(row, "payload"),
        exception: column(row, "exception"),
        failed_at: failed_at(row),
    }
}

#[async_trait]
impl FailedJobProvider for DatabaseUuidFailedJobProvider {
    async fn log(
        &self,
        connection: &str,
        queue: &str,
        payload: &str,
        exception: &str,
    ) -> Result<Option<String>> {
        let uuid = failed_job_id(payload);
        self.get_table()
            .insert(json!({
                "uuid": uuid,
                "connection": connection,
                "queue": queue,
                "payload": payload,
                "exception": exception,
                "failed_at": Carbon::now().to_date_time_string(),
            }))
            .await?;
        Ok(Some(uuid))
    }

    async fn all(&self) -> Result<Vec<FailedJob>> {
        Ok(self
            .get_table()
            .order_by("id", "desc")
            .get()
            .await?
            .into_iter()
            .map(|row| to_failed_job(&row))
            .collect())
    }

    async fn find(&self, id: &str) -> Result<Option<FailedJob>> {
        Ok(self
            .get_table()
            .where_("uuid", id)
            .first()
            .await?
            .map(|row| to_failed_job(&row)))
    }

    async fn forget(&self, id: &str) -> Result<bool> {
        Ok(self.get_table().where_("uuid", id).delete().await? > 0)
    }

    async fn ids(&self, queue: Option<&str>) -> Result<Vec<String>> {
        let mut query = self.get_table();
        if let Some(queue) = queue {
            query = query.where_("queue", queue);
        }
        Ok(query
            .order_by("id", "desc")
            .pluck("uuid")
            .await?
            .into_iter()
            .map(|uuid| uuid.to_string_lossy())
            .collect())
    }

    async fn flush(&self, hours: Option<u64>) -> Result<()> {
        self.flush_queue(hours, None).await.map(drop)
    }

    async fn prune(&self, before: Carbon) -> Result<u64> {
        let query = self
            .get_table()
            .where_op("failed_at", "<", before.to_date_time_string());
        let mut total = 0;
        loop {
            let deleted = query.clone().limit(PRUNE_CHUNK).delete().await?;
            total += deleted;
            if deleted == 0 {
                return Ok(total);
            }
        }
    }

    async fn count(&self, connection: Option<&str>, queue: Option<&str>) -> Result<u64> {
        let mut query = self.get_table();
        if let Some(connection) = connection.filter(|connection| !connection.is_empty()) {
            query = query.where_("connection", connection);
        }
        if let Some(queue) = queue.filter(|queue| !queue.is_empty()) {
            query = query.where_("queue", queue);
        }
        Ok(query.count().await?.max(0) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn provider() -> DatabaseUuidFailedJobProvider {
        let connection = Connection::new(
            "sqlite",
            json!({"driver": "sqlite", "database": ":memory:"}),
        );
        connection
            .get_schema_builder()
            .create("failed_jobs", |table| {
                table.id();
                table.string("uuid").unique();
                table.string("connection");
                table.string("queue");
                table.long_text("payload");
                table.long_text("exception");
                table.timestamp("failed_at").use_current();
                table.index(["connection", "queue", "failed_at"]);
            })
            .await
            .unwrap();
        DatabaseUuidFailedJobProvider::new(connection, "failed_jobs")
    }

    fn payload(uuid: &str) -> String {
        json!({"uuid": uuid, "displayName": "ProcessPodcast", "data": {"commandName": "app::ProcessPodcast"}})
            .to_string()
    }

    #[tokio::test]
    async fn it_logs_lists_and_forgets_failed_jobs() {
        let provider = provider().await;
        let id = provider
            .log(
                "database",
                "default",
                &payload("a"),
                "RuntimeException: boom\n#0 trace",
            )
            .await
            .unwrap();
        assert_eq!(id.as_deref(), Some("a"));
        provider
            .log("redis", "emails", &payload("b"), "Exception")
            .await
            .unwrap();

        // Newest first, identified by their UUIDs.
        assert_eq!(provider.ids(None).await.unwrap(), vec!["b", "a"]);
        assert_eq!(provider.ids(Some("default")).await.unwrap(), vec!["a"]);
        let all = provider.all().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "b");
        assert_eq!(all[0].connection, "redis");

        let job = provider.find("a").await.unwrap().unwrap();
        assert_eq!(job.queue, "default");
        assert_eq!(job.display_name(), "ProcessPodcast");
        assert_eq!(job.exception_message(), "RuntimeException: boom");
        assert!(job.failed_at.diff_in_seconds(&Carbon::now()).abs() <= 2);
        assert!(provider.find("missing").await.unwrap().is_none());

        assert_eq!(provider.count(None, None).await.unwrap(), 2);
        assert_eq!(provider.count(Some("redis"), None).await.unwrap(), 1);
        assert_eq!(
            provider
                .count(Some("redis"), Some("default"))
                .await
                .unwrap(),
            0
        );

        assert!(provider.forget("a").await.unwrap());
        assert!(!provider.forget("a").await.unwrap());
        assert_eq!(provider.ids(None).await.unwrap(), vec!["b"]);
    }

    #[tokio::test]
    async fn it_flushes_and_prunes_old_failed_jobs() {
        let provider = provider().await;
        let now = Carbon::now();

        Carbon::set_thread_test_now(Some(now.sub_hours(48)));
        provider
            .log("database", "default", &payload("old"), "E")
            .await
            .unwrap();
        Carbon::set_thread_test_now(Some(now.sub_hours(2)));
        provider
            .log("database", "emails", &payload("recent"), "E")
            .await
            .unwrap();
        Carbon::set_thread_test_now(Some(now));
        provider
            .log("database", "default", &payload("new"), "E")
            .await
            .unwrap();

        // `queue:flush --hours=24` only deletes the jobs older than a day.
        provider.flush(Some(24)).await.unwrap();
        assert_eq!(provider.ids(None).await.unwrap(), vec!["new", "recent"]);

        assert_eq!(provider.flush_queue(None, Some("emails")).await.unwrap(), 1);
        assert_eq!(provider.ids(None).await.unwrap(), vec!["new"]);

        assert_eq!(provider.prune(now.sub_hours(1)).await.unwrap(), 0);
        assert_eq!(provider.prune(now.add_seconds(5)).await.unwrap(), 1);
        assert!(provider.all().await.unwrap().is_empty());

        provider
            .log("database", "default", &payload("x"), "E")
            .await
            .unwrap();
        provider.flush(None).await.unwrap();
        assert!(provider.all().await.unwrap().is_empty());
        Carbon::set_thread_test_now(None);
    }
}
