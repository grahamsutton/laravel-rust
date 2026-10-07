//! Failed jobs: where jobs go when they run out of attempts.
//!
//! Workers log every job that fails to the [`FailedJobProvider`] bound in
//! the container, so it may be listed (`queue:failed`), retried
//! (`queue:retry`), forgotten (`queue:forget`) or flushed (`queue:flush`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use illuminate_config::Repository as Config;
use illuminate_container::{Container, try_app};
use illuminate_database::DatabaseManager;
use illuminate_support::{Carbon, Result, Value, ValueExt};

mod database;

pub use database::DatabaseUuidFailedJobProvider;

/// A failed job record (a row of Laravel's `failed_jobs` table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailedJob {
    /// The failed job's id (the job's UUID).
    pub id: String,
    /// The connection the job ran on.
    pub connection: String,
    /// The queue the job ran on.
    pub queue: String,
    /// The job's raw payload.
    pub payload: String,
    /// The exception the job failed with.
    pub exception: String,
    /// When the job failed.
    pub failed_at: Carbon,
}

impl FailedJob {
    /// The decoded payload.
    pub fn payload_value(&self) -> Value {
        serde_json::from_str(&self.payload).unwrap_or(Value::Null)
    }

    /// The job's display name (what `queue:failed` shows as its class).
    pub fn display_name(&self) -> String {
        let payload = self.payload_value();
        payload
            .get("displayName")
            .and_then(Value::as_str)
            .or_else(|| {
                payload
                    .get("data")
                    .and_then(|data| data.get("commandName"))
                    .and_then(Value::as_str)
            })
            .unwrap_or_default()
            .to_string()
    }

    /// The first line of the exception (what `queue:failed` shows).
    pub fn exception_message(&self) -> &str {
        self.exception.lines().next().unwrap_or_default()
    }
}

/// Stores failed jobs (Laravel's `FailedJobProviderInterface`).
///
/// Only `log`, `all`, `find` and `forget` are required; the rest are built
/// on top of them (override them when your storage can do better).
#[async_trait]
pub trait FailedJobProvider: Send + Sync + 'static {
    /// Log a failed job, returning its id.
    async fn log(
        &self,
        connection: &str,
        queue: &str,
        payload: &str,
        exception: &str,
    ) -> Result<Option<String>>;

    /// Get every failed job, newest first.
    async fn all(&self) -> Result<Vec<FailedJob>>;

    /// Get a single failed job.
    async fn find(&self, id: &str) -> Result<Option<FailedJob>>;

    /// Delete a single failed job, returning whether it existed.
    async fn forget(&self, id: &str) -> Result<bool>;

    /// The ids of the failed jobs, optionally only those of the given queue.
    async fn ids(&self, queue: Option<&str>) -> Result<Vec<String>> {
        Ok(self
            .all()
            .await?
            .into_iter()
            .filter(|job| queue.is_none_or(|queue| job.queue == queue))
            .map(|job| job.id)
            .collect())
    }

    /// Delete every failed job, or only those that failed at least `hours`
    /// hours ago.
    async fn flush(&self, hours: Option<u64>) -> Result<()> {
        let before = Carbon::now().sub_hours(hours.unwrap_or(0) as i64);
        for job in self.all().await? {
            if hours.is_none() || job.failed_at.lte(&before) {
                self.forget(&job.id).await?;
            }
        }
        Ok(())
    }

    /// Delete the failed jobs that failed before the given date, returning
    /// how many were deleted.
    async fn prune(&self, before: Carbon) -> Result<u64> {
        let mut pruned = 0;
        for job in self.all().await? {
            if job.failed_at.lt(&before) && self.forget(&job.id).await? {
                pruned += 1;
            }
        }
        Ok(pruned)
    }

    /// Count the failed jobs, optionally of a connection and/or queue.
    async fn count(&self, connection: Option<&str>, queue: Option<&str>) -> Result<u64> {
        Ok(self
            .all()
            .await?
            .iter()
            .filter(|job| connection.is_none_or(|connection| job.connection == connection))
            .filter(|job| queue.is_none_or(|queue| job.queue == queue))
            .count() as u64)
    }
}

fn failed_job_id(payload: &str) -> String {
    serde_json::from_str::<Value>(payload)
        .ok()
        .and_then(|payload| {
            payload
                .get("uuid")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .unwrap_or_else(|| illuminate_support::Str::uuid().to_string())
}

/// Keeps failed jobs in memory.
#[derive(Default)]
pub struct InMemoryFailedJobProvider {
    jobs: Mutex<Vec<FailedJob>>,
}

impl InMemoryFailedJobProvider {
    /// Create an empty provider.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl FailedJobProvider for InMemoryFailedJobProvider {
    async fn log(
        &self,
        connection: &str,
        queue: &str,
        payload: &str,
        exception: &str,
    ) -> Result<Option<String>> {
        let id = failed_job_id(payload);
        self.jobs.lock().unwrap().insert(
            0,
            FailedJob {
                id: id.clone(),
                connection: connection.to_string(),
                queue: queue.to_string(),
                payload: payload.to_string(),
                exception: exception.to_string(),
                failed_at: Carbon::now(),
            },
        );
        Ok(Some(id))
    }

    async fn all(&self) -> Result<Vec<FailedJob>> {
        Ok(self.jobs.lock().unwrap().clone())
    }

    async fn find(&self, id: &str) -> Result<Option<FailedJob>> {
        Ok(self
            .jobs
            .lock()
            .unwrap()
            .iter()
            .find(|job| job.id == id)
            .cloned())
    }

    async fn forget(&self, id: &str) -> Result<bool> {
        let mut jobs = self.jobs.lock().unwrap();
        let before = jobs.len();
        jobs.retain(|job| job.id != id);
        Ok(jobs.len() != before)
    }
}

/// Discards failed jobs (`QUEUE_FAILED_DRIVER=null`).
#[derive(Debug, Default, Clone, Copy)]
pub struct NullFailedJobProvider;

#[async_trait]
impl FailedJobProvider for NullFailedJobProvider {
    async fn log(
        &self,
        _connection: &str,
        _queue: &str,
        _payload: &str,
        _exception: &str,
    ) -> Result<Option<String>> {
        Ok(None)
    }

    async fn all(&self) -> Result<Vec<FailedJob>> {
        Ok(Vec::new())
    }

    async fn find(&self, _id: &str) -> Result<Option<FailedJob>> {
        Ok(None)
    }

    async fn forget(&self, _id: &str) -> Result<bool> {
        Ok(false)
    }
}

#[derive(Serialize, Deserialize)]
struct FileRecord {
    id: String,
    connection: String,
    queue: String,
    payload: String,
    exception: String,
    failed_at: String,
    failed_at_timestamp: i64,
}

impl From<FileRecord> for FailedJob {
    fn from(record: FileRecord) -> Self {
        FailedJob {
            id: record.id,
            connection: record.connection,
            queue: record.queue,
            payload: record.payload,
            exception: record.exception,
            failed_at: Carbon::from_timestamp(record.failed_at_timestamp),
        }
    }
}

/// Keeps the most recent failed jobs in a JSON file (`queue.failed.driver`
/// = `file`).
pub struct FileFailedJobProvider {
    path: PathBuf,
    limit: usize,
    lock: tokio::sync::Mutex<()>,
}

impl FileFailedJobProvider {
    /// Store up to `limit` failed jobs in the file at `path`.
    pub fn new(path: impl Into<PathBuf>, limit: usize) -> Self {
        Self {
            path: path.into(),
            limit,
            lock: tokio::sync::Mutex::new(()),
        }
    }

    async fn read(&self) -> Result<Vec<FileRecord>> {
        match tokio::fs::read_to_string(&self.path).await {
            Ok(contents) if contents.trim().is_empty() => Ok(Vec::new()),
            Ok(contents) => Ok(serde_json::from_str(&contents).unwrap_or_default()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error.into()),
        }
    }

    async fn write(&self, records: &[FileRecord]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&self.path, serde_json::to_string_pretty(records)?).await?;
        Ok(())
    }
}

#[async_trait]
impl FailedJobProvider for FileFailedJobProvider {
    async fn log(
        &self,
        connection: &str,
        queue: &str,
        payload: &str,
        exception: &str,
    ) -> Result<Option<String>> {
        let _lock = self.lock.lock().await;
        let id = failed_job_id(payload);
        let now = Carbon::now();
        let mut records = self.read().await?;
        records.insert(
            0,
            FileRecord {
                id: id.clone(),
                connection: connection.to_string(),
                queue: queue.to_string(),
                payload: payload.to_string(),
                exception: exception.to_string(),
                failed_at: now.to_date_time_string(),
                failed_at_timestamp: now.timestamp(),
            },
        );
        records.truncate(self.limit);
        self.write(&records).await?;
        Ok(Some(id))
    }

    async fn all(&self) -> Result<Vec<FailedJob>> {
        Ok(self
            .read()
            .await?
            .into_iter()
            .map(FailedJob::from)
            .collect())
    }

    async fn find(&self, id: &str) -> Result<Option<FailedJob>> {
        Ok(self
            .read()
            .await?
            .into_iter()
            .find(|record| record.id == id)
            .map(FailedJob::from))
    }

    async fn forget(&self, id: &str) -> Result<bool> {
        let _lock = self.lock.lock().await;
        let mut records = self.read().await?;
        let before = records.len();
        records.retain(|record| record.id != id);
        let forgotten = records.len() != before;
        if forgotten {
            self.write(&records).await?;
        }
        Ok(forgotten)
    }

    async fn flush(&self, hours: Option<u64>) -> Result<()> {
        let _lock = self.lock.lock().await;
        let before = Carbon::now()
            .sub_hours(hours.unwrap_or(0) as i64)
            .timestamp();
        let mut records = self.read().await?;
        records.retain(|record| record.failed_at_timestamp > before);
        self.write(&records).await
    }

    async fn prune(&self, before: Carbon) -> Result<u64> {
        let _lock = self.lock.lock().await;
        let mut records = self.read().await?;
        let count = records.len();
        records.retain(|record| record.failed_at_timestamp >= before.timestamp());
        let pruned = (count - records.len()) as u64;
        self.write(&records).await?;
        Ok(pruned)
    }
}

/// Build the failed job provider described by `queue.failed`.
///
/// `database-uuids` (and the legacy `database` driver) keep failed jobs in
/// the `queue.failed.table` table (`failed_jobs`) of the
/// `queue.failed.database` connection (the default one when absent), `file`
/// keeps them in a JSON file and `null` discards them. Without a bound
/// [`DatabaseManager`] — and for drivers provided by other components,
/// which bind `dyn FailedJobProvider` themselves — failed jobs are kept in
/// memory.
pub fn make_failer(config: &Config) -> Arc<dyn FailedJobProvider> {
    make_failer_with(config, try_app::<DatabaseManager>())
}

/// Build the failed job provider described by `queue.failed`, storing
/// database failures through the given database manager.
pub(crate) fn make_failer_with(
    config: &Config,
    database: Option<Arc<DatabaseManager>>,
) -> Arc<dyn FailedJobProvider> {
    let failed = config.get("queue.failed");
    let driver = failed.get("driver");

    match driver {
        Some(Value::Null) => Arc::new(NullFailedJobProvider),
        Some(Value::String(driver)) if driver == "null" => Arc::new(NullFailedJobProvider),
        Some(Value::String(driver)) if driver == "file" => {
            let path = failed
                .get("path")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("storage/framework/cache/failed-jobs.json"));
            let limit = failed
                .get("limit")
                .and_then(ValueExt::to_i64_lossy)
                .unwrap_or(100)
                .max(1) as usize;
            Arc::new(FileFailedJobProvider::new(path, limit))
        }
        Some(Value::String(driver)) if driver == "database-uuids" || driver == "database" => {
            match database {
                Some(database) => {
                    let connection = match failed.get("database") {
                        Some(Value::String(name)) if !name.is_empty() => database.connection(name),
                        _ => database.default_connection(),
                    };
                    let table = match failed.get("table") {
                        Some(Value::String(table)) if !table.is_empty() => table.clone(),
                        _ => "failed_jobs".to_string(),
                    };
                    Arc::new(DatabaseUuidFailedJobProvider::new(connection, table))
                }
                None => Arc::new(InMemoryFailedJobProvider::new()),
            }
        }
        _ => Arc::new(InMemoryFailedJobProvider::new()),
    }
}

/// The failed job provider bound in the container (registered from the
/// configuration on first use).
pub fn failer() -> Arc<dyn FailedJobProvider> {
    if let Some(failer) = try_app::<dyn FailedJobProvider>() {
        return failer;
    }
    let container = Container::get_instance();
    container.singleton_if::<dyn FailedJobProvider>(make_container_failer);
    container.make::<dyn FailedJobProvider>()
}

/// Build the failed job provider from the container's configuration and
/// database manager.
pub(crate) fn make_container_failer(container: &Container) -> Arc<dyn FailedJobProvider> {
    let config = container
        .try_make::<Config>()
        .unwrap_or_else(|_| Arc::new(Config::empty()));
    make_failer_with(&config, container.try_make::<DatabaseManager>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn payload(uuid: &str) -> String {
        json!({"uuid": uuid, "displayName": "ProcessPodcast", "data": {"commandName": "app::ProcessPodcast"}})
            .to_string()
    }

    #[tokio::test]
    async fn in_memory_provider_keeps_failed_jobs() {
        let provider = InMemoryFailedJobProvider::new();
        provider
            .log(
                "database",
                "default",
                &payload("a"),
                "RuntimeException: boom\n trace",
            )
            .await
            .unwrap();
        provider
            .log("database", "emails", &payload("b"), "Exception")
            .await
            .unwrap();

        assert_eq!(provider.ids(None).await.unwrap(), vec!["b", "a"]);
        assert_eq!(provider.ids(Some("default")).await.unwrap(), vec!["a"]);
        assert_eq!(
            provider
                .count(Some("database"), Some("emails"))
                .await
                .unwrap(),
            1
        );

        let job = provider.find("a").await.unwrap().unwrap();
        assert_eq!(job.display_name(), "ProcessPodcast");
        assert_eq!(job.exception_message(), "RuntimeException: boom");

        assert!(provider.forget("a").await.unwrap());
        assert!(!provider.forget("a").await.unwrap());

        provider.flush(Some(1)).await.unwrap();
        assert_eq!(provider.all().await.unwrap().len(), 1);
        provider.flush(None).await.unwrap();
        assert!(provider.all().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn file_provider_keeps_the_latest_jobs() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("failed-jobs.json");
        let provider = FileFailedJobProvider::new(&path, 2);

        for uuid in ["a", "b", "c"] {
            provider
                .log("redis", "default", &payload(uuid), "Exception")
                .await
                .unwrap();
        }

        assert_eq!(provider.ids(None).await.unwrap(), vec!["c", "b"]);
        let stored: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored[0]["id"], json!("c"));
        assert!(stored[0]["failed_at_timestamp"].is_i64());

        assert!(provider.forget("c").await.unwrap());
        assert_eq!(
            provider.find("b").await.unwrap().unwrap().connection,
            "redis"
        );
        assert_eq!(
            provider.prune(Carbon::now().add_seconds(5)).await.unwrap(),
            1
        );
        assert!(provider.all().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn failers_are_built_from_configuration() {
        let null = Config::new(json!({"queue": {"failed": {"driver": null}}}));
        let failer = make_failer(&null);
        assert_eq!(
            failer
                .log("sync", "default", &payload("x"), "E")
                .await
                .unwrap(),
            None
        );

        let memory = Config::new(json!({"queue": {"failed": {"driver": "database-uuids"}}}));
        let failer = make_failer(&memory);
        assert_eq!(
            failer
                .log("sync", "default", &payload("x"), "E")
                .await
                .unwrap(),
            Some("x".to_string())
        );

        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container.clone());
        container.instance(Config::new(
            json!({"queue": {"failed": {"driver": "database-uuids"}}}),
        ));
        let resolved = super::failer();
        assert!(Arc::ptr_eq(&resolved, &super::failer()));
    }
}
