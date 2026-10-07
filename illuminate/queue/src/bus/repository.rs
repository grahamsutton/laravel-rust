//! Batch repositories: where batch meta information lives.

use std::sync::Mutex;

use async_trait::async_trait;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use illuminate_support::{Carbon, Result, Str, Value};

use super::batch::UpdatedBatchJobCounts;

/// The stored state of a batch (a row of Laravel's `job_batches` table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchRecord {
    /// The batch's UUID.
    pub id: String,
    /// The batch's name.
    pub name: String,
    /// The number of jobs added to the batch.
    pub total_jobs: u64,
    /// The number of jobs that haven't completed successfully yet.
    pub pending_jobs: u64,
    /// The number of jobs that failed.
    pub failed_jobs: u64,
    /// The UUIDs of the jobs that failed.
    pub failed_job_ids: Vec<String>,
    /// The batch's options (queue, connection, callbacks, ...).
    pub options: Value,
    /// When the batch was created.
    pub created_at: Carbon,
    /// When the batch was cancelled.
    pub cancelled_at: Option<Carbon>,
    /// When the batch finished.
    pub finished_at: Option<Carbon>,
}

/// Stores batches and keeps their job counts.
///
/// The queue ships with an [`InMemoryBatchRepository`]; the database
/// component provides one backed by the `job_batches` table. Bind yours
/// into the container as `dyn BatchRepository`.
#[async_trait]
pub trait BatchRepository: Send + Sync + 'static {
    /// Retrieve a list of batches, newest first.
    async fn get(&self, limit: usize, before: Option<&str>) -> Result<Vec<BatchRecord>>;

    /// Retrieve information about an existing batch.
    async fn find(&self, batch_id: &str) -> Result<Option<BatchRecord>>;

    /// Store a new pending batch.
    async fn store(&self, name: &str, options: &Value) -> Result<BatchRecord>;

    /// Increment the total number of jobs within the batch.
    async fn increment_total_jobs(&self, batch_id: &str, amount: u64) -> Result<()>;

    /// Decrement the total number of pending jobs for the batch.
    async fn decrement_pending_jobs(
        &self,
        batch_id: &str,
        job_id: &str,
    ) -> Result<UpdatedBatchJobCounts>;

    /// Increment the total number of failed jobs for the batch.
    async fn increment_failed_jobs(
        &self,
        batch_id: &str,
        job_id: &str,
    ) -> Result<UpdatedBatchJobCounts>;

    /// Mark the batch that has the given id as finished.
    async fn mark_as_finished(&self, batch_id: &str) -> Result<()>;

    /// Cancel the batch that has the given id.
    async fn cancel(&self, batch_id: &str) -> Result<()>;

    /// Delete the batch that has the given id.
    async fn delete(&self, batch_id: &str) -> Result<()>;

    /// Prune the finished batches created before the given date.
    async fn prune(&self, before: Carbon) -> Result<u64>;

    /// Prune the unfinished batches created before the given date.
    async fn prune_unfinished(&self, before: Carbon) -> Result<u64>;

    /// Prune the cancelled batches created before the given date.
    async fn prune_cancelled(&self, before: Carbon) -> Result<u64>;
}

/// A batch repository that keeps batches in memory.
#[derive(Default)]
pub struct InMemoryBatchRepository {
    batches: Mutex<IndexMap<String, BatchRecord>>,
}

impl InMemoryBatchRepository {
    /// Create an empty repository.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every stored batch, oldest first.
    pub fn all(&self) -> Vec<BatchRecord> {
        self.batches.lock().unwrap().values().cloned().collect()
    }

    fn update<R>(&self, batch_id: &str, callback: impl FnOnce(&mut BatchRecord) -> R) -> Option<R> {
        self.batches.lock().unwrap().get_mut(batch_id).map(callback)
    }

    fn prune_where(&self, predicate: impl Fn(&BatchRecord) -> bool) -> u64 {
        let mut batches = self.batches.lock().unwrap();
        let before = batches.len();
        batches.retain(|_, batch| !predicate(batch));
        (before - batches.len()) as u64
    }
}

#[async_trait]
impl BatchRepository for InMemoryBatchRepository {
    async fn get(&self, limit: usize, before: Option<&str>) -> Result<Vec<BatchRecord>> {
        let batches = self.batches.lock().unwrap();
        let mut records: Vec<BatchRecord> = batches.values().rev().cloned().collect();
        if let Some(before) = before
            && let Some(position) = records.iter().position(|batch| batch.id == before)
        {
            records.drain(..=position);
        }
        records.truncate(limit);
        Ok(records)
    }

    async fn find(&self, batch_id: &str) -> Result<Option<BatchRecord>> {
        Ok(self.batches.lock().unwrap().get(batch_id).cloned())
    }

    async fn store(&self, name: &str, options: &Value) -> Result<BatchRecord> {
        let record = BatchRecord {
            id: Str::ordered_uuid().to_string(),
            name: name.to_string(),
            total_jobs: 0,
            pending_jobs: 0,
            failed_jobs: 0,
            failed_job_ids: Vec::new(),
            options: options.clone(),
            created_at: Carbon::now(),
            cancelled_at: None,
            finished_at: None,
        };
        self.batches
            .lock()
            .unwrap()
            .insert(record.id.clone(), record.clone());
        Ok(record)
    }

    async fn increment_total_jobs(&self, batch_id: &str, amount: u64) -> Result<()> {
        self.update(batch_id, |batch| {
            batch.total_jobs += amount;
            batch.pending_jobs += amount;
            batch.finished_at = None;
        });
        Ok(())
    }

    async fn decrement_pending_jobs(
        &self,
        batch_id: &str,
        job_id: &str,
    ) -> Result<UpdatedBatchJobCounts> {
        Ok(self
            .update(batch_id, |batch| {
                batch.pending_jobs = batch.pending_jobs.saturating_sub(1);
                batch.failed_job_ids.retain(|id| id != job_id);
                UpdatedBatchJobCounts::new(batch.pending_jobs, batch.failed_jobs)
            })
            .unwrap_or_default())
    }

    async fn increment_failed_jobs(
        &self,
        batch_id: &str,
        job_id: &str,
    ) -> Result<UpdatedBatchJobCounts> {
        Ok(self
            .update(batch_id, |batch| {
                batch.failed_jobs += 1;
                if !batch.failed_job_ids.iter().any(|id| id == job_id) {
                    batch.failed_job_ids.push(job_id.to_string());
                }
                UpdatedBatchJobCounts::new(batch.pending_jobs, batch.failed_jobs)
            })
            .unwrap_or_default())
    }

    async fn mark_as_finished(&self, batch_id: &str) -> Result<()> {
        self.update(batch_id, |batch| batch.finished_at = Some(Carbon::now()));
        Ok(())
    }

    async fn cancel(&self, batch_id: &str) -> Result<()> {
        self.update(batch_id, |batch| {
            let now = Carbon::now();
            batch.cancelled_at = Some(now);
            batch.finished_at = Some(now);
        });
        Ok(())
    }

    async fn delete(&self, batch_id: &str) -> Result<()> {
        self.batches.lock().unwrap().shift_remove(batch_id);
        Ok(())
    }

    async fn prune(&self, before: Carbon) -> Result<u64> {
        Ok(self.prune_where(|batch| batch.finished_at.is_some_and(|at| at.lt(&before))))
    }

    async fn prune_unfinished(&self, before: Carbon) -> Result<u64> {
        Ok(self.prune_where(|batch| batch.finished_at.is_none() && batch.created_at.lt(&before)))
    }

    async fn prune_cancelled(&self, before: Carbon) -> Result<u64> {
        Ok(self.prune_where(|batch| batch.cancelled_at.is_some() && batch.created_at.lt(&before)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[tokio::test]
    async fn it_tracks_job_counts() {
        let repository = InMemoryBatchRepository::new();
        let batch = repository.store("Import", &json!({})).await.unwrap();
        repository.increment_total_jobs(&batch.id, 3).await.unwrap();

        let counts = repository
            .decrement_pending_jobs(&batch.id, "a")
            .await
            .unwrap();
        assert_eq!(counts.pending_jobs, 2);

        let counts = repository
            .increment_failed_jobs(&batch.id, "b")
            .await
            .unwrap();
        assert_eq!((counts.pending_jobs, counts.failed_jobs), (2, 1));

        let record = repository.find(&batch.id).await.unwrap().unwrap();
        assert_eq!(record.total_jobs, 3);
        assert_eq!(record.failed_job_ids, vec!["b".to_string()]);

        repository.cancel(&batch.id).await.unwrap();
        let record = repository.find(&batch.id).await.unwrap().unwrap();
        assert!(record.cancelled_at.is_some() && record.finished_at.is_some());
    }

    #[tokio::test]
    async fn it_lists_and_prunes_batches() {
        let repository = InMemoryBatchRepository::new();
        let first = repository.store("first", &json!({})).await.unwrap();
        let second = repository.store("second", &json!({})).await.unwrap();

        let listed = repository.get(10, None).await.unwrap();
        assert_eq!(listed[0].id, second.id);
        assert_eq!(
            repository.get(10, Some(&second.id)).await.unwrap()[0].id,
            first.id
        );

        repository.mark_as_finished(&first.id).await.unwrap();
        let pruned = repository
            .prune(Carbon::now().add_seconds(5))
            .await
            .unwrap();
        assert_eq!(pruned, 1);
        assert_eq!(
            repository
                .prune_unfinished(Carbon::now().add_seconds(5))
                .await
                .unwrap(),
            1
        );
        assert!(repository.all().is_empty());
    }
}
