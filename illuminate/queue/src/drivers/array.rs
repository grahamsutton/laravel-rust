//! The `array` driver: an in-memory queue.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;

use illuminate_support::{Carbon, Result, Value};

use crate::contracts::Queue;
use crate::queued_job::{JobBackend, QueuedJob};

#[derive(Debug, Clone)]
struct Record {
    id: u64,
    queue: String,
    payload: String,
    attempts: u32,
    available_at: Instant,
    created_at: i64,
}

#[derive(Default)]
struct State {
    queues: HashMap<String, Vec<Record>>,
    reserved: HashMap<u64, Record>,
}

#[derive(Default)]
struct Store {
    state: Mutex<State>,
    next_id: AtomicU64,
}

impl Store {
    fn insert(&self, queue: &str, payload: String, delay: Duration, attempts: u32) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let record = Record {
            id,
            queue: queue.to_string(),
            payload,
            attempts,
            available_at: Instant::now() + delay,
            created_at: Carbon::now().timestamp(),
        };
        self.state
            .lock()
            .unwrap()
            .queues
            .entry(queue.to_string())
            .or_default()
            .push(record);
        id
    }
}

#[async_trait]
impl JobBackend for Store {
    async fn delete(&self, job: &QueuedJob) -> Result<()> {
        if let Ok(id) = job.job_id().parse::<u64>() {
            self.state.lock().unwrap().reserved.remove(&id);
        }
        Ok(())
    }

    async fn release(&self, job: &QueuedJob, delay: Duration) -> Result<()> {
        let Ok(id) = job.job_id().parse::<u64>() else {
            return Ok(());
        };
        let record = {
            let mut state = self.state.lock().unwrap();
            state.reserved.remove(&id)
        };
        if let Some(record) = record {
            self.insert(&record.queue, record.payload, delay, record.attempts);
        }
        Ok(())
    }
}

/// An in-memory queue: jobs wait in memory until a worker (in the same
/// process) pops them. Each queue is first-in, first-out, and delayed jobs
/// stay invisible until their delay has passed.
///
/// ```
/// use illuminate_queue::ArrayQueue;
/// use illuminate_queue::contracts::Queue;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let queue = ArrayQueue::new("array");
/// queue.push_raw(r#"{"uuid":"1","job":"x","data":{}}"#.to_string(), Some("emails"), None).await.unwrap();
///
/// assert_eq!(queue.size(Some("emails")).await.unwrap(), 1);
///
/// let job = queue.pop(Some("emails")).await.unwrap().unwrap();
/// assert_eq!(job.attempts(), 1);
/// job.delete().await.unwrap();
///
/// assert_eq!(queue.size(Some("emails")).await.unwrap(), 0);
/// # });
/// ```
#[derive(Clone)]
pub struct ArrayQueue {
    name: String,
    default_queue: String,
    after_commit: bool,
    store: Arc<Store>,
}

impl ArrayQueue {
    /// Create an in-memory queue connection with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            default_queue: "default".to_string(),
            after_commit: false,
            store: Arc::new(Store::default()),
        }
    }

    /// Set the default queue name.
    pub fn with_default_queue(mut self, queue: impl Into<String>) -> Self {
        self.default_queue = queue.into();
        self
    }

    /// Dispatch jobs after open database transactions commit.
    pub fn with_after_commit(mut self, after_commit: bool) -> Self {
        self.after_commit = after_commit;
        self
    }

    fn queue_name<'a>(&'a self, queue: Option<&'a str>) -> &'a str {
        queue.unwrap_or(&self.default_queue)
    }

    /// The payloads waiting on the queue (including delayed jobs).
    pub fn payloads(&self, queue: Option<&str>) -> Vec<Value> {
        let queue = self.queue_name(queue);
        self.store
            .state
            .lock()
            .unwrap()
            .queues
            .get(queue)
            .map(|records| {
                records
                    .iter()
                    .filter_map(|record| serde_json::from_str(&record.payload).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The UNIX timestamp the oldest waiting job was pushed at.
    pub fn creation_time_of_oldest_pending_job(&self, queue: Option<&str>) -> Option<i64> {
        let queue = self.queue_name(queue);
        let now = Instant::now();
        self.store
            .state
            .lock()
            .unwrap()
            .queues
            .get(queue)?
            .iter()
            .filter(|record| record.available_at <= now)
            .map(|record| record.created_at)
            .min()
    }
}

#[async_trait]
impl Queue for ArrayQueue {
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
        let queue = self.queue_name(queue);
        let state = self.store.state.lock().unwrap();
        let waiting = state.queues.get(queue).map_or(0, Vec::len);
        let reserved = state
            .reserved
            .values()
            .filter(|record| record.queue == queue)
            .count();
        Ok((waiting + reserved) as u64)
    }

    async fn pending_size(&self, queue: Option<&str>) -> Result<u64> {
        let queue = self.queue_name(queue);
        let now = Instant::now();
        let state = self.store.state.lock().unwrap();
        Ok(state.queues.get(queue).map_or(0, |records| {
            records
                .iter()
                .filter(|record| record.available_at <= now)
                .count()
        }) as u64)
    }

    async fn delayed_size(&self, queue: Option<&str>) -> Result<u64> {
        let queue = self.queue_name(queue);
        let now = Instant::now();
        let state = self.store.state.lock().unwrap();
        Ok(state.queues.get(queue).map_or(0, |records| {
            records
                .iter()
                .filter(|record| record.available_at > now)
                .count()
        }) as u64)
    }

    async fn reserved_size(&self, queue: Option<&str>) -> Result<u64> {
        let queue = self.queue_name(queue);
        let state = self.store.state.lock().unwrap();
        Ok(state
            .reserved
            .values()
            .filter(|record| record.queue == queue)
            .count() as u64)
    }

    async fn push_raw(
        &self,
        payload: String,
        queue: Option<&str>,
        delay: Option<Duration>,
    ) -> Result<Option<String>> {
        let queue = self.queue_name(queue).to_string();
        let id = self
            .store
            .insert(&queue, payload, delay.unwrap_or_default(), 0);
        Ok(Some(id.to_string()))
    }

    async fn pop(&self, queue: Option<&str>) -> Result<Option<QueuedJob>> {
        let queue = self.queue_name(queue).to_string();
        let record = {
            let mut state = self.store.state.lock().unwrap();
            let now = Instant::now();
            let Some(records) = state.queues.get_mut(&queue) else {
                return Ok(None);
            };
            let Some(position) = records.iter().position(|record| record.available_at <= now)
            else {
                return Ok(None);
            };
            let mut record = records.remove(position);
            record.attempts += 1;
            state.reserved.insert(record.id, record.clone());
            record
        };

        let backend: Arc<dyn JobBackend> = self.store.clone();
        let job = QueuedJob::new(
            record.id.to_string(),
            record.payload,
            record.attempts,
            &self.name,
            &queue,
            backend,
        );

        match job {
            Ok(job) => Ok(Some(job)),
            Err(error) => {
                // An unreadable payload can never be processed: drop it.
                self.store.state.lock().unwrap().reserved.remove(&record.id);
                Err(error)
            }
        }
    }

    async fn clear(&self, queue: Option<&str>) -> Result<u64> {
        let queue = self.queue_name(queue);
        let mut state = self.store.state.lock().unwrap();
        let waiting = state
            .queues
            .remove(queue)
            .map_or(0, |records| records.len());
        let before = state.reserved.len();
        state.reserved.retain(|_, record| record.queue != queue);
        Ok((waiting + before - state.reserved.len()) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(uuid: &str) -> String {
        format!(r#"{{"uuid":"{uuid}","displayName":"Test","job":"x","data":{{}}}}"#)
    }

    #[tokio::test]
    async fn queues_are_first_in_first_out() {
        let queue = ArrayQueue::new("array");
        queue.push_raw(payload("a"), None, None).await.unwrap();
        queue.push_raw(payload("b"), None, None).await.unwrap();

        assert_eq!(queue.pop(None).await.unwrap().unwrap().uuid(), Some("a"));
        assert_eq!(queue.pop(None).await.unwrap().unwrap().uuid(), Some("b"));
        assert!(queue.pop(None).await.unwrap().is_none());
        assert_eq!(queue.reserved_size(None).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn delayed_jobs_wait_for_their_delay() {
        let queue = ArrayQueue::new("array");
        queue
            .push_raw(payload("later"), None, Some(Duration::from_millis(60)))
            .await
            .unwrap();
        queue.push_raw(payload("now"), None, None).await.unwrap();

        assert_eq!(queue.delayed_size(None).await.unwrap(), 1);
        assert_eq!(queue.pending_size(None).await.unwrap(), 1);
        assert_eq!(queue.pop(None).await.unwrap().unwrap().uuid(), Some("now"));
        assert!(queue.pop(None).await.unwrap().is_none());

        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            queue.pop(None).await.unwrap().unwrap().uuid(),
            Some("later")
        );
    }

    #[tokio::test]
    async fn released_jobs_keep_their_attempts() {
        let queue = ArrayQueue::new("array");
        queue
            .push_raw(payload("a"), Some("emails"), None)
            .await
            .unwrap();

        let job = queue.pop(Some("emails")).await.unwrap().unwrap();
        assert_eq!(job.attempts(), 1);
        job.release(0).await.unwrap();
        assert!(job.is_released());
        assert_eq!(queue.reserved_size(Some("emails")).await.unwrap(), 0);

        let job = queue.pop(Some("emails")).await.unwrap().unwrap();
        assert_eq!(job.attempts(), 2);
        assert_eq!(job.queue(), "emails");
        job.delete().await.unwrap();
        assert_eq!(queue.size(Some("emails")).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn queues_can_be_cleared() {
        let queue = ArrayQueue::new("array").with_default_queue("low");
        queue.push_raw(payload("a"), None, None).await.unwrap();
        queue.push_raw(payload("b"), None, None).await.unwrap();
        queue
            .push_raw(payload("c"), Some("high"), None)
            .await
            .unwrap();
        let _reserved = queue.pop(None).await.unwrap();

        assert_eq!(queue.payloads(None).len(), 1);
        assert!(queue.creation_time_of_oldest_pending_job(None).is_some());
        assert_eq!(queue.clear(None).await.unwrap(), 2);
        assert_eq!(queue.size(None).await.unwrap(), 0);
        assert_eq!(queue.size(Some("high")).await.unwrap(), 1);
    }
}
