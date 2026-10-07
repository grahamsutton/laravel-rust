//! The database batch repository: batches live in the `job_batches` table.

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;

use illuminate_config::Repository as Config;
use illuminate_container::Container;
use illuminate_database::{Builder, Connection, DatabaseManager, Operand, raw};
use illuminate_support::{Carbon, Result, Str, Value, ValueExt, json};

use super::batch::UpdatedBatchJobCounts;
use super::repository::{BatchRecord, BatchRepository, InMemoryBatchRepository};

/// How many rows a prune deletes per query.
const PRUNE_CHUNK: i64 = 1000;

/// The current UNIX timestamp (honouring `Carbon`'s test "now").
fn current_time() -> i64 {
    Carbon::now().timestamp()
}

/// An integer column of a row.
fn integer(row: &Value, column: &str) -> i64 {
    row.get(column)
        .and_then(ValueExt::to_i64_lossy)
        .unwrap_or(0)
}

/// A nullable UNIX timestamp column of a row.
fn timestamp(row: &Value, column: &str) -> Option<Carbon> {
    match row.get(column) {
        Some(Value::Null) | None => None,
        Some(value) => value
            .to_i64_lossy()
            .filter(|timestamp| *timestamp != 0)
            .map(Carbon::from_timestamp),
    }
}

/// Decode a JSON text column (drivers that decode JSON hand back values).
fn decode(value: Option<&Value>) -> Value {
    match value {
        Some(Value::String(json)) => serde_json::from_str(json).unwrap_or(Value::Null),
        Some(value) => value.clone(),
        None => Value::Null,
    }
}

/// The failed job ids stored in a row.
fn failed_job_ids(row: &Value) -> Vec<String> {
    match decode(row.get("failed_job_ids")) {
        Value::Array(ids) => ids.iter().map(ValueExt::to_string_lossy).collect(),
        _ => Vec::new(),
    }
}

/// A row of the `job_batches` table as a batch record.
fn to_record(row: &Value) -> BatchRecord {
    let options = match decode(row.get("options")) {
        Value::Null => json!({}),
        options => options,
    };
    BatchRecord {
        id: row
            .get("id")
            .map(ValueExt::to_string_lossy)
            .unwrap_or_default(),
        name: row
            .get("name")
            .map(ValueExt::to_string_lossy)
            .unwrap_or_default(),
        total_jobs: integer(row, "total_jobs").max(0) as u64,
        pending_jobs: integer(row, "pending_jobs").max(0) as u64,
        failed_jobs: integer(row, "failed_jobs").max(0) as u64,
        failed_job_ids: failed_job_ids(row),
        options,
        created_at: Carbon::from_timestamp(integer(row, "created_at")),
        cancelled_at: timestamp(row, "cancelled_at"),
        finished_at: timestamp(row, "finished_at"),
    }
}

/// The job counts of a batch, as read under lock.
struct Counts {
    pending_jobs: i64,
    failed_jobs: i64,
    failed_job_ids: Vec<String>,
}

/// Stores batches in the `job_batches` table, exactly like Laravel's
/// `DatabaseBatchRepository`:
///
/// ```php
/// Schema::create('job_batches', function (Blueprint $table) {
///     $table->string('id')->primary();
///     $table->string('name');
///     $table->integer('total_jobs');
///     $table->integer('pending_jobs');
///     $table->integer('failed_jobs');
///     $table->longText('failed_job_ids');
///     $table->mediumText('options')->nullable();
///     $table->integer('cancelled_at')->nullable();
///     $table->integer('created_at');
///     $table->integer('finished_at')->nullable();
/// });
/// ```
///
/// Job counts are updated inside a transaction with the batch's row locked,
/// so workers on every server keep them right. The options are stored as
/// JSON, and timestamps as UNIX timestamps.
///
/// ```
/// use illuminate_database::Connection;
/// use illuminate_queue::{BatchRepository, DatabaseBatchRepository};
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let connection = Connection::new("sqlite", json!({"driver": "sqlite", "database": ":memory:"}));
/// connection.get_schema_builder().create("job_batches", |table| {
///     table.string("id").primary();
///     table.string("name");
///     table.integer("total_jobs");
///     table.integer("pending_jobs");
///     table.integer("failed_jobs");
///     table.long_text("failed_job_ids");
///     table.medium_text("options").nullable();
///     table.integer("cancelled_at").nullable();
///     table.integer("created_at");
///     table.integer("finished_at").nullable();
/// }).await.unwrap();
///
/// let batches = DatabaseBatchRepository::new(connection, "job_batches");
/// let batch = batches.store("Import CSV", &json!({"queue": "imports"})).await.unwrap();
/// batches.increment_total_jobs(&batch.id, 2).await.unwrap();
///
/// let counts = batches.decrement_pending_jobs(&batch.id, "job-1").await.unwrap();
/// assert_eq!(counts.pending_jobs, 1);
///
/// let batch = batches.find(&batch.id).await.unwrap().unwrap();
/// assert_eq!((batch.total_jobs, batch.pending_jobs), (2, 1));
/// assert_eq!(batch.options["queue"], json!("imports"));
/// # });
/// ```
#[derive(Debug, Clone)]
pub struct DatabaseBatchRepository {
    connection: Connection,
    table: String,
}

impl DatabaseBatchRepository {
    /// Store batches in the given table of the given connection.
    pub fn new(connection: Connection, table: impl Into<String>) -> Self {
        Self {
            connection,
            table: table.into(),
        }
    }

    /// Get the underlying database connection.
    pub fn get_connection(&self) -> &Connection {
        &self.connection
    }

    /// Set the underlying database connection.
    pub fn set_connection(&mut self, connection: Connection) {
        self.connection = connection;
    }

    /// The name of the batches table.
    pub fn table_name(&self) -> &str {
        &self.table
    }

    /// Execute the given callback within a storage specific transaction.
    pub async fn transaction<F, Fut, T>(&self, callback: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.connection.transaction(callback).await
    }

    fn table(&self) -> Builder {
        self.connection.table(self.table.as_str())
    }

    fn batch(&self, batch_id: &str) -> Builder {
        self.table().where_("id", batch_id)
    }

    /// Update the batch's job counts with its row locked (Laravel's
    /// `updateAtomicValues`).
    async fn update_atomic_values(
        &self,
        batch_id: &str,
        callback: impl FnOnce(Counts) -> Counts + Send,
    ) -> Result<UpdatedBatchJobCounts> {
        self.connection
            .transaction(|| async {
                let Some(row) = self.batch(batch_id).lock_for_update().first().await? else {
                    return Ok(UpdatedBatchJobCounts::default());
                };
                let counts = callback(Counts {
                    pending_jobs: integer(&row, "pending_jobs"),
                    failed_jobs: integer(&row, "failed_jobs"),
                    failed_job_ids: failed_job_ids(&row),
                });
                self.batch(batch_id)
                    .update(json!({
                        "pending_jobs": counts.pending_jobs,
                        "failed_jobs": counts.failed_jobs,
                        "failed_job_ids": serde_json::to_string(&counts.failed_job_ids)?,
                    }))
                    .await?;
                Ok(UpdatedBatchJobCounts::new(
                    counts.pending_jobs.max(0) as u64,
                    counts.failed_jobs.max(0) as u64,
                ))
            })
            .await
    }

    /// Delete the rows matched by the query a thousand at a time.
    async fn prune_query(&self, query: Builder) -> Result<u64> {
        let mut total = 0;
        loop {
            let deleted = query.clone().limit(PRUNE_CHUNK).delete().await?;
            total += deleted;
            if deleted == 0 {
                return Ok(total);
            }
        }
    }
}

#[async_trait]
impl BatchRepository for DatabaseBatchRepository {
    async fn get(&self, limit: usize, before: Option<&str>) -> Result<Vec<BatchRecord>> {
        let mut query = self
            .table()
            .order_by_desc("id")
            .limit(i64::try_from(limit).unwrap_or(i64::MAX));
        if let Some(before) = before.filter(|before| !before.is_empty()) {
            query = query.where_op("id", "<", before);
        }
        Ok(query
            .get()
            .await?
            .into_iter()
            .map(|row| to_record(&row))
            .collect())
    }

    async fn find(&self, batch_id: &str) -> Result<Option<BatchRecord>> {
        Ok(self
            .batch(batch_id)
            .use_write_pdo()
            .first()
            .await?
            .map(|row| to_record(&row)))
    }

    async fn store(&self, name: &str, options: &Value) -> Result<BatchRecord> {
        let id = Str::ordered_uuid().to_string();
        let created_at = current_time();
        self.table()
            .insert(json!({
                "id": id,
                "name": name,
                "total_jobs": 0,
                "pending_jobs": 0,
                "failed_jobs": 0,
                "failed_job_ids": "[]",
                "options": serde_json::to_string(options)?,
                "created_at": created_at,
                "cancelled_at": null,
                "finished_at": null,
            }))
            .await?;

        match self.find(&id).await? {
            Some(record) => Ok(record),
            None => Ok(BatchRecord {
                id,
                name: name.to_string(),
                total_jobs: 0,
                pending_jobs: 0,
                failed_jobs: 0,
                failed_job_ids: Vec::new(),
                options: options.clone(),
                created_at: Carbon::from_timestamp(created_at),
                cancelled_at: None,
                finished_at: None,
            }),
        }
    }

    async fn increment_total_jobs(&self, batch_id: &str, amount: u64) -> Result<()> {
        self.connection
            .transaction(|| async {
                if self
                    .batch(batch_id)
                    .lock_for_update()
                    .first()
                    .await?
                    .is_none()
                {
                    return Ok(());
                }
                self.batch(batch_id)
                    .update(vec![
                        (
                            "total_jobs",
                            Operand::from(raw(format!("total_jobs + {amount}"))),
                        ),
                        (
                            "pending_jobs",
                            Operand::from(raw(format!("pending_jobs + {amount}"))),
                        ),
                        ("finished_at", Operand::from(Value::Null)),
                    ])
                    .await?;
                Ok(())
            })
            .await
    }

    async fn decrement_pending_jobs(
        &self,
        batch_id: &str,
        job_id: &str,
    ) -> Result<UpdatedBatchJobCounts> {
        self.update_atomic_values(batch_id, |mut counts| {
            counts.pending_jobs -= 1;
            counts.failed_job_ids.retain(|id| id != job_id);
            counts
        })
        .await
    }

    async fn increment_failed_jobs(
        &self,
        batch_id: &str,
        job_id: &str,
    ) -> Result<UpdatedBatchJobCounts> {
        self.update_atomic_values(batch_id, |mut counts| {
            counts.failed_jobs += 1;
            if !counts.failed_job_ids.iter().any(|id| id == job_id) {
                counts.failed_job_ids.push(job_id.to_string());
            }
            counts
        })
        .await
    }

    async fn mark_as_finished(&self, batch_id: &str) -> Result<()> {
        self.batch(batch_id)
            .update(json!({"finished_at": current_time()}))
            .await?;
        Ok(())
    }

    async fn cancel(&self, batch_id: &str) -> Result<()> {
        let now = current_time();
        self.batch(batch_id)
            .update(json!({"cancelled_at": now, "finished_at": now}))
            .await?;
        Ok(())
    }

    async fn delete(&self, batch_id: &str) -> Result<()> {
        self.batch(batch_id).delete().await?;
        Ok(())
    }

    async fn prune(&self, before: Carbon) -> Result<u64> {
        self.prune_query(self.table().where_not_null("finished_at").where_op(
            "finished_at",
            "<",
            before.timestamp(),
        ))
        .await
    }

    async fn prune_unfinished(&self, before: Carbon) -> Result<u64> {
        self.prune_query(self.table().where_null("finished_at").where_op(
            "created_at",
            "<",
            before.timestamp(),
        ))
        .await
    }

    async fn prune_cancelled(&self, before: Carbon) -> Result<u64> {
        self.prune_query(self.table().where_not_null("cancelled_at").where_op(
            "created_at",
            "<",
            before.timestamp(),
        ))
        .await
    }
}

/// Build the batch repository for the container: batches are stored in the
/// `queue.batching.table` table (`job_batches`) of the
/// `queue.batching.database` connection when batching is configured and a
/// [`DatabaseManager`] is bound, and in memory otherwise.
pub(crate) fn make_batch_repository(container: &Container) -> Arc<dyn BatchRepository> {
    let config = container
        .try_make::<Config>()
        .unwrap_or_else(|_| Arc::new(Config::empty()));
    let batching = config.get("queue.batching");
    let configured = batching.get("database").is_some() || batching.get("table").is_some();

    match container.try_make::<DatabaseManager>() {
        Ok(database) if configured => {
            let connection = match batching.get("database") {
                Some(Value::String(name)) if !name.is_empty() => database.connection(name),
                _ => database.default_connection(),
            };
            let table = match batching.get("table") {
                Some(Value::String(table)) if !table.is_empty() => table.clone(),
                _ => "job_batches".to_string(),
            };
            Arc::new(DatabaseBatchRepository::new(connection, table))
        }
        _ => Arc::new(InMemoryBatchRepository::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn repository() -> DatabaseBatchRepository {
        let connection = Connection::new(
            "sqlite",
            json!({"driver": "sqlite", "database": ":memory:"}),
        );
        connection
            .get_schema_builder()
            .create("job_batches", |table| {
                table.string("id").primary();
                table.string("name");
                table.integer("total_jobs");
                table.integer("pending_jobs");
                table.integer("failed_jobs");
                table.long_text("failed_job_ids");
                table.medium_text("options").nullable();
                table.integer("cancelled_at").nullable();
                table.integer("created_at");
                table.integer("finished_at").nullable();
            })
            .await
            .unwrap();
        DatabaseBatchRepository::new(connection, "job_batches")
    }

    #[tokio::test]
    async fn it_stores_batches_and_tracks_their_job_counts() {
        let repository = repository().await;
        let batch = repository
            .store(
                "Import",
                &json!({"queue": "imports", "allowFailures": true}),
            )
            .await
            .unwrap();
        assert_eq!(batch.name, "Import");
        assert_eq!(batch.total_jobs, 0);
        assert_eq!(batch.options["queue"], json!("imports"));
        assert!(batch.finished_at.is_none());

        repository.increment_total_jobs(&batch.id, 3).await.unwrap();
        let counts = repository
            .decrement_pending_jobs(&batch.id, "a")
            .await
            .unwrap();
        assert_eq!((counts.pending_jobs, counts.failed_jobs), (2, 0));

        let counts = repository
            .increment_failed_jobs(&batch.id, "b")
            .await
            .unwrap();
        assert_eq!((counts.pending_jobs, counts.failed_jobs), (2, 1));
        repository
            .increment_failed_jobs(&batch.id, "b")
            .await
            .unwrap();

        let record = repository.find(&batch.id).await.unwrap().unwrap();
        assert_eq!(record.total_jobs, 3);
        assert_eq!(record.pending_jobs, 2);
        assert_eq!(record.failed_jobs, 2);
        assert_eq!(record.failed_job_ids, vec!["b".to_string()]);

        // A retried job that succeeds is no longer a failed job.
        let counts = repository
            .decrement_pending_jobs(&batch.id, "b")
            .await
            .unwrap();
        assert_eq!(counts.pending_jobs, 1);
        let record = repository.find(&batch.id).await.unwrap().unwrap();
        assert!(record.failed_job_ids.is_empty());

        // Unknown batches have nothing to count.
        assert_eq!(
            repository
                .decrement_pending_jobs("missing", "a")
                .await
                .unwrap(),
            UpdatedBatchJobCounts::default()
        );
        repository.increment_total_jobs("missing", 1).await.unwrap();
    }

    #[tokio::test]
    async fn it_finishes_cancels_and_deletes_batches() {
        let repository = repository().await;
        let batch = repository.store("Import", &json!({})).await.unwrap();

        repository.mark_as_finished(&batch.id).await.unwrap();
        assert!(
            repository
                .find(&batch.id)
                .await
                .unwrap()
                .unwrap()
                .finished_at
                .is_some()
        );

        // Adding jobs re-opens the batch.
        repository.increment_total_jobs(&batch.id, 1).await.unwrap();
        let record = repository.find(&batch.id).await.unwrap().unwrap();
        assert!(record.finished_at.is_none());
        assert_eq!((record.total_jobs, record.pending_jobs), (1, 1));

        repository.cancel(&batch.id).await.unwrap();
        let record = repository.find(&batch.id).await.unwrap().unwrap();
        assert!(record.cancelled_at.is_some() && record.finished_at.is_some());

        repository.delete(&batch.id).await.unwrap();
        assert!(repository.find(&batch.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn it_lists_and_prunes_batches() {
        let repository = repository().await;
        let first = repository.store("first", &json!({})).await.unwrap();
        let second = repository.store("second", &json!({})).await.unwrap();
        let third = repository.store("third", &json!({})).await.unwrap();

        let listed = repository.get(10, None).await.unwrap();
        let names: Vec<&str> = listed.iter().map(|batch| batch.name.as_str()).collect();
        assert_eq!(names, vec!["third", "second", "first"]);
        assert_eq!(repository.get(1, None).await.unwrap()[0].id, third.id);
        let before: Vec<String> = repository
            .get(10, Some(&second.id))
            .await
            .unwrap()
            .into_iter()
            .map(|batch| batch.id)
            .collect();
        assert_eq!(before, vec![first.id.clone()]);

        repository.mark_as_finished(&first.id).await.unwrap();
        repository.cancel(&second.id).await.unwrap();

        let later = Carbon::now().add_seconds(5);
        assert_eq!(
            repository
                .prune_cancelled(Carbon::now().sub_hours(1))
                .await
                .unwrap(),
            0
        );
        assert_eq!(repository.prune_cancelled(later).await.unwrap(), 1);
        assert_eq!(repository.prune(later).await.unwrap(), 1);
        assert_eq!(repository.prune_unfinished(later).await.unwrap(), 1);
        assert!(repository.get(10, None).await.unwrap().is_empty());
    }

    #[test]
    fn rows_are_read_leniently() {
        let record = to_record(&json!({
            "id": "9a", "name": "Import", "total_jobs": "3", "pending_jobs": -1,
            "failed_jobs": 1, "failed_job_ids": "[\"x\"]", "options": null,
            "created_at": 1_700_000_000, "cancelled_at": null, "finished_at": 1_700_000_100,
        }));
        assert_eq!(record.total_jobs, 3);
        assert_eq!(record.pending_jobs, 0);
        assert_eq!(record.failed_job_ids, vec!["x".to_string()]);
        assert_eq!(record.options, json!({}));
        assert_eq!(record.created_at.timestamp(), 1_700_000_000);
        assert!(record.cancelled_at.is_none());
        assert_eq!(record.finished_at.unwrap().timestamp(), 1_700_000_100);
    }
}
