//! The queue fake.

use std::any::TypeId;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use async_trait::async_trait;

use illuminate_support::Result;

use super::{FakeFilter, JobTypes, job_type_name};
use crate::closure::CallQueuedClosure;
use crate::contracts::Queue;
use crate::envelope::Envelope;
use crate::job::ShouldQueue;
use crate::manager::QueueManager;
use crate::queued_job::QueuedJob;

/// A job pushed onto the fake queue.
#[derive(Debug, Clone)]
pub struct PushedJob {
    /// The job.
    pub job: Envelope,
    /// The queue it was pushed onto.
    pub queue: Option<String>,
    /// The delay it was pushed with.
    pub delay: Option<Duration>,
}

/// A raw payload pushed onto the fake queue.
#[derive(Debug, Clone)]
pub struct RawPush {
    /// The payload.
    pub payload: String,
    /// The queue it was pushed onto.
    pub queue: Option<String>,
    /// The delay it was pushed with.
    pub delay: Option<Duration>,
}

/// Records the jobs pushed onto the queue instead of storing them.
///
/// Swap it in with [`Queue::fake`](crate::Queue::fake):
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_queue::{Dispatchable, Queue, ShouldQueue, async_trait};
/// use illuminate_support::Result;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct ShipOrder {
///     order_id: u64,
/// }
///
/// #[async_trait]
/// impl ShouldQueue for ShipOrder {
///     async fn handle(&self) -> Result<()> {
///         Ok(())
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// Queue::fake();
///
/// ShipOrder { order_id: 7 }.dispatch().await?;
///
/// Queue::assert_pushed::<ShipOrder>();
/// Queue::assert_pushed_with::<ShipOrder>(|job| job.order_id == 7);
/// Queue::assert_pushed_once::<ShipOrder>();
/// Queue::assert_count(1);
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub struct QueueFake {
    manager: Weak<QueueManager>,
    filter: RwLock<FakeFilter>,
    pushed: Mutex<Vec<PushedJob>>,
    raw: Mutex<Vec<RawPush>>,
}

impl QueueFake {
    /// Create a fake for the given manager. Jobs that aren't faked are
    /// pushed onto the manager's real connections.
    pub fn new(manager: &Arc<QueueManager>) -> Self {
        Self {
            manager: Arc::downgrade(manager),
            filter: RwLock::new(FakeFilter::default()),
            pushed: Mutex::new(Vec::new()),
            raw: Mutex::new(Vec::new()),
        }
    }

    /// Only fake jobs of type `T` (call once per type); other jobs are
    /// pushed for real.
    pub fn only<T: ShouldQueue>(&self) -> &Self {
        self.filter.write().unwrap().only(TypeId::of::<T>());
        self
    }

    /// Push jobs of type `T` for real instead of faking them.
    pub fn except<T: ShouldQueue>(&self) -> &Self {
        self.filter.write().unwrap().except(TypeId::of::<T>());
        self
    }

    fn record(&self, job: &Envelope, queue: Option<&str>, delay: Option<Duration>) {
        self.pushed.lock().unwrap().push(PushedJob {
            job: job.clone(),
            queue: queue.map(String::from),
            delay,
        });
    }

    fn real_connection(&self, job: &Envelope) -> Result<Arc<dyn Queue>> {
        let manager = self.manager.upgrade().ok_or_else(|| {
            illuminate_support::error::RuntimeException::new("The queue manager is gone.")
        })?;
        manager.real_connection(job.connection_name())
    }

    // ------------------------------------------------------------------
    // Inspecting
    // ------------------------------------------------------------------

    /// Every pushed job.
    pub fn pushed_jobs(&self) -> Vec<PushedJob> {
        self.pushed.lock().unwrap().clone()
    }

    /// The pushed jobs of type `T`.
    pub fn pushed<T: ShouldQueue>(&self) -> Vec<Arc<T>> {
        self.pushed_matching::<T>(|_, _| true)
            .into_iter()
            .filter_map(|pushed| pushed.job.downcast_arc::<T>())
            .collect()
    }

    /// The pushed jobs of type `T` passing the given test.
    pub fn pushed_matching<T: ShouldQueue>(
        &self,
        callback: impl Fn(&T, &PushedJob) -> bool,
    ) -> Vec<PushedJob> {
        self.pushed
            .lock()
            .unwrap()
            .iter()
            .filter(|pushed| {
                pushed
                    .job
                    .downcast_ref::<T>()
                    .is_some_and(|job| callback(job, pushed))
            })
            .cloned()
            .collect()
    }

    /// The raw payloads pushed onto the queue.
    pub fn raw_pushes(&self) -> Vec<RawPush> {
        self.raw.lock().unwrap().clone()
    }

    /// Determine if a job of type `T` was pushed.
    pub fn has_pushed<T: ShouldQueue>(&self) -> bool {
        !self.pushed_matching::<T>(|_, _| true).is_empty()
    }

    /// Forget every pushed job.
    pub fn clear_pushed(&self) {
        self.pushed.lock().unwrap().clear();
        self.raw.lock().unwrap().clear();
    }

    // ------------------------------------------------------------------
    // Assertions
    // ------------------------------------------------------------------

    /// Assert that a job of type `T` was pushed.
    #[track_caller]
    pub fn assert_pushed<T: ShouldQueue>(&self) {
        assert!(
            self.has_pushed::<T>(),
            "The expected [{}] job was not pushed.",
            job_type_name::<T>()
        );
    }

    /// Assert that a job of type `T` passing the given test was pushed.
    #[track_caller]
    pub fn assert_pushed_with<T: ShouldQueue>(&self, callback: impl Fn(&T) -> bool) {
        assert!(
            !self.pushed_matching::<T>(|job, _| callback(job)).is_empty(),
            "The expected [{}] job was not pushed.",
            job_type_name::<T>()
        );
    }

    /// Assert that a job of type `T` was pushed exactly `times` times.
    #[track_caller]
    pub fn assert_pushed_times<T: ShouldQueue>(&self, times: usize) {
        let count = self.pushed_matching::<T>(|_, _| true).len();
        assert_eq!(
            count,
            times,
            "The expected [{}] job was pushed {count} times instead of {times} times.",
            job_type_name::<T>()
        );
    }

    /// Assert that a job of type `T` was pushed exactly once.
    #[track_caller]
    pub fn assert_pushed_once<T: ShouldQueue>(&self) {
        self.assert_pushed_times::<T>(1);
    }

    /// Assert that a job of type `T` was pushed onto the given queue.
    #[track_caller]
    pub fn assert_pushed_on<T: ShouldQueue>(&self, queue: &str) {
        self.assert_pushed_on_with::<T>(queue, |_| true);
    }

    /// Assert that a job of type `T` passing the test was pushed onto the
    /// given queue.
    #[track_caller]
    pub fn assert_pushed_on_with<T: ShouldQueue>(
        &self,
        queue: &str,
        callback: impl Fn(&T) -> bool,
    ) {
        assert!(
            !self
                .pushed_matching::<T>(
                    |job, pushed| pushed.queue.as_deref() == Some(queue) && callback(job)
                )
                .is_empty(),
            "The expected [{}] job was not pushed to the [{queue}] queue.",
            job_type_name::<T>()
        );
    }

    /// Assert that a job of type `T` was pushed with the given chain:
    /// `assert_pushed_with_chain::<ShipOrder, (RecordShipment, UpdateInventory)>()`.
    #[track_caller]
    pub fn assert_pushed_with_chain<T: ShouldQueue, C: JobTypes>(&self) {
        self.assert_pushed::<T>();
        let expected = C::job_names();
        assert!(
            !self
                .pushed_matching::<T>(|_, pushed| pushed.job.chained_job_names() == expected)
                .is_empty(),
            "The expected chain [{}] was not pushed.",
            expected.join(", ")
        );
    }

    /// Assert that a job of type `T` was pushed without a chain.
    #[track_caller]
    pub fn assert_pushed_without_chain<T: ShouldQueue>(&self) {
        self.assert_pushed::<T>();
        assert!(
            !self
                .pushed_matching::<T>(|_, pushed| pushed.job.chained_job_names().is_empty())
                .is_empty(),
            "The expected [{}] job was pushed with a chain.",
            job_type_name::<T>()
        );
    }

    /// Assert that no job of type `T` was pushed.
    #[track_caller]
    pub fn assert_not_pushed<T: ShouldQueue>(&self) {
        assert!(
            !self.has_pushed::<T>(),
            "The unexpected [{}] job was pushed.",
            job_type_name::<T>()
        );
    }

    /// Assert that no job of type `T` passing the test was pushed.
    #[track_caller]
    pub fn assert_not_pushed_with<T: ShouldQueue>(&self, callback: impl Fn(&T) -> bool) {
        assert!(
            self.pushed_matching::<T>(|job, _| callback(job)).is_empty(),
            "The unexpected [{}] job was pushed.",
            job_type_name::<T>()
        );
    }

    /// Assert that a queued closure was pushed.
    #[track_caller]
    pub fn assert_closure_pushed(&self) {
        assert!(
            self.has_pushed::<CallQueuedClosure>(),
            "The expected [Closure] job was not pushed."
        );
    }

    /// Assert that no queued closure was pushed.
    #[track_caller]
    pub fn assert_closure_not_pushed(&self) {
        assert!(
            !self.has_pushed::<CallQueuedClosure>(),
            "The unexpected [Closure] job was pushed."
        );
    }

    /// Assert that no jobs were pushed.
    #[track_caller]
    pub fn assert_nothing_pushed(&self) {
        let pushed = self.pushed.lock().unwrap();
        assert!(
            pushed.is_empty(),
            "The following jobs were pushed unexpectedly:\n\n- {}",
            pushed
                .iter()
                .map(|pushed| pushed.job.command_name())
                .collect::<Vec<_>>()
                .join("\n- ")
        );
    }

    /// Assert the total number of jobs that were pushed.
    #[track_caller]
    pub fn assert_count(&self, expected: usize) {
        let count = self.pushed.lock().unwrap().len();
        assert_eq!(
            count, expected,
            "Expected {expected} jobs to be pushed, but found {count} instead."
        );
    }
}

impl std::fmt::Debug for QueueFake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueueFake")
            .field("pushed", &self.pushed.lock().unwrap().len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Queue for QueueFake {
    fn connection_name(&self) -> &str {
        "fake"
    }

    async fn size(&self, queue: Option<&str>) -> Result<u64> {
        let pushed = self.pushed.lock().unwrap();
        Ok(pushed
            .iter()
            .filter(|pushed| queue.is_none() || pushed.queue.as_deref() == queue)
            .count() as u64)
    }

    async fn push(&self, job: &Envelope, queue: Option<&str>) -> Result<Option<String>> {
        if self.filter.read().unwrap().should_fake(job) {
            self.record(job, queue, None);
            return Ok(None);
        }
        self.real_connection(job)?.push(job, queue).await
    }

    async fn later(
        &self,
        delay: Duration,
        job: &Envelope,
        queue: Option<&str>,
    ) -> Result<Option<String>> {
        if self.filter.read().unwrap().should_fake(job) {
            self.record(job, queue, Some(delay));
            return Ok(None);
        }
        self.real_connection(job)?.later(delay, job, queue).await
    }

    async fn push_raw(
        &self,
        payload: String,
        queue: Option<&str>,
        delay: Option<Duration>,
    ) -> Result<Option<String>> {
        self.raw.lock().unwrap().push(RawPush {
            payload,
            queue: queue.map(String::from),
            delay,
        });
        Ok(None)
    }

    async fn pop(&self, _queue: Option<&str>) -> Result<Option<QueuedJob>> {
        Ok(None)
    }

    async fn clear(&self, queue: Option<&str>) -> Result<u64> {
        let mut pushed = self.pushed.lock().unwrap();
        let before = pushed.len();
        pushed.retain(|pushed| queue.is_some() && pushed.queue.as_deref() != queue);
        Ok((before - pushed.len()) as u64)
    }
}
