//! The bus fake.

use std::any::TypeId;
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;

use illuminate_support::Result;

use super::{FakeFilter, JobTypes, job_type_name};
use crate::bus::batch::{Batch, PendingBatch};
use crate::bus::dispatcher::QueueingDispatcher;
use crate::bus::repository::{BatchRepository, InMemoryBatchRepository};
use crate::envelope::Envelope;
use crate::job::ShouldQueue;

/// Records dispatched jobs, chains and batches instead of running them.
///
/// Swap it in with [`Bus::fake`](crate::Bus::fake):
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_queue::{Bus, Dispatchable, PendingBatch, ShouldQueue, async_trait};
/// use illuminate_support::Result;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct ImportCsv {
///     row: u64,
/// }
///
/// #[async_trait]
/// impl ShouldQueue for ImportCsv {
///     async fn handle(&self) -> Result<()> {
///         Ok(())
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// Bus::fake();
///
/// Bus::batch(vec![Box::new(ImportCsv { row: 1 }), Box::new(ImportCsv { row: 2 })])
///     .name("Import CSV")
///     .dispatch()
///     .await?;
///
/// Bus::assert_batched(|batch: &PendingBatch| {
///     batch.name == "Import CSV" && batch.jobs.len() == 2
/// });
/// Bus::assert_batch_count(1);
/// Bus::assert_nothing_dispatched();
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub struct BusFake {
    dispatcher: Arc<dyn QueueingDispatcher>,
    filter: RwLock<FakeFilter>,
    commands: Mutex<Vec<Envelope>>,
    commands_sync: Mutex<Vec<Envelope>>,
    commands_after_response: Mutex<Vec<Envelope>>,
    batches: Mutex<Vec<PendingBatch>>,
    batch_repository: Arc<InMemoryBatchRepository>,
}

impl BusFake {
    /// Create a fake wrapping the real dispatcher (used for jobs that
    /// aren't faked).
    pub fn new(dispatcher: Arc<dyn QueueingDispatcher>) -> Self {
        Self {
            dispatcher,
            filter: RwLock::new(FakeFilter::default()),
            commands: Mutex::new(Vec::new()),
            commands_sync: Mutex::new(Vec::new()),
            commands_after_response: Mutex::new(Vec::new()),
            batches: Mutex::new(Vec::new()),
            batch_repository: Arc::new(InMemoryBatchRepository::new()),
        }
    }

    /// Only fake jobs of type `T` (call once per type); other jobs are
    /// dispatched for real.
    pub fn only<T: ShouldQueue>(&self) -> &Self {
        self.filter.write().unwrap().only(TypeId::of::<T>());
        self
    }

    /// Dispatch jobs of type `T` for real instead of faking them.
    pub fn except<T: ShouldQueue>(&self) -> &Self {
        self.filter.write().unwrap().except(TypeId::of::<T>());
        self
    }

    fn should_fake(&self, job: &Envelope) -> bool {
        self.filter.read().unwrap().should_fake(job)
    }

    // ------------------------------------------------------------------
    // Inspecting
    // ------------------------------------------------------------------

    fn matching<T: ShouldQueue>(
        commands: &Mutex<Vec<Envelope>>,
        callback: impl Fn(&T) -> bool,
    ) -> Vec<Envelope> {
        commands
            .lock()
            .unwrap()
            .iter()
            .filter(|job| job.downcast_ref::<T>().is_some_and(&callback))
            .cloned()
            .collect()
    }

    /// The dispatched jobs of type `T`.
    pub fn dispatched<T: ShouldQueue>(&self) -> Vec<Arc<T>> {
        Self::matching::<T>(&self.commands, |_| true)
            .iter()
            .filter_map(Envelope::downcast_arc::<T>)
            .collect()
    }

    /// The synchronously dispatched jobs of type `T`.
    pub fn dispatched_sync<T: ShouldQueue>(&self) -> Vec<Arc<T>> {
        Self::matching::<T>(&self.commands_sync, |_| true)
            .iter()
            .filter_map(Envelope::downcast_arc::<T>)
            .collect()
    }

    /// The jobs of type `T` dispatched after the response.
    pub fn dispatched_after_response<T: ShouldQueue>(&self) -> Vec<Arc<T>> {
        Self::matching::<T>(&self.commands_after_response, |_| true)
            .iter()
            .filter_map(Envelope::downcast_arc::<T>)
            .collect()
    }

    /// The dispatched envelopes (jobs with their options).
    pub fn dispatched_envelopes(&self) -> Vec<Envelope> {
        self.commands.lock().unwrap().clone()
    }

    /// The dispatched batches.
    pub fn dispatched_batches(&self) -> Vec<PendingBatch> {
        self.batches.lock().unwrap().clone()
    }

    /// Determine if a job of type `T` was dispatched.
    pub fn has_dispatched<T: ShouldQueue>(&self) -> bool {
        !Self::matching::<T>(&self.commands, |_| true).is_empty()
    }

    /// Determine if a job of type `T` was dispatched synchronously.
    pub fn has_dispatched_sync<T: ShouldQueue>(&self) -> bool {
        !Self::matching::<T>(&self.commands_sync, |_| true).is_empty()
    }

    /// Determine if a job of type `T` was dispatched after the response.
    pub fn has_dispatched_after_response<T: ShouldQueue>(&self) -> bool {
        !Self::matching::<T>(&self.commands_after_response, |_| true).is_empty()
    }

    fn count_everywhere<T: ShouldQueue>(&self, callback: &dyn Fn(&T) -> bool) -> usize {
        Self::matching::<T>(&self.commands, callback).len()
            + Self::matching::<T>(&self.commands_sync, callback).len()
            + Self::matching::<T>(&self.commands_after_response, callback).len()
    }

    // ------------------------------------------------------------------
    // Assertions
    // ------------------------------------------------------------------

    /// Assert that a job of type `T` was dispatched (in any way).
    #[track_caller]
    pub fn assert_dispatched<T: ShouldQueue>(&self) {
        self.assert_dispatched_with::<T>(|_| true);
    }

    /// Assert that a job of type `T` passing the test was dispatched.
    #[track_caller]
    pub fn assert_dispatched_with<T: ShouldQueue>(&self, callback: impl Fn(&T) -> bool) {
        assert!(
            self.count_everywhere::<T>(&callback) > 0,
            "The expected [{}] job was not dispatched.",
            job_type_name::<T>()
        );
    }

    /// Assert that a job of type `T` was dispatched exactly `times` times.
    #[track_caller]
    pub fn assert_dispatched_times<T: ShouldQueue>(&self, times: usize) {
        let count = self.count_everywhere::<T>(&|_| true);
        assert_eq!(
            count,
            times,
            "The expected [{}] job was pushed {count} times instead of {times} times.",
            job_type_name::<T>()
        );
    }

    /// Assert that a job of type `T` was dispatched exactly once.
    #[track_caller]
    pub fn assert_dispatched_once<T: ShouldQueue>(&self) {
        self.assert_dispatched_times::<T>(1);
    }

    /// Assert that no job of type `T` was dispatched.
    #[track_caller]
    pub fn assert_not_dispatched<T: ShouldQueue>(&self) {
        self.assert_not_dispatched_with::<T>(|_| true);
    }

    /// Assert that no job of type `T` passing the test was dispatched.
    #[track_caller]
    pub fn assert_not_dispatched_with<T: ShouldQueue>(&self, callback: impl Fn(&T) -> bool) {
        assert!(
            self.count_everywhere::<T>(&callback) == 0,
            "The unexpected [{}] job was dispatched.",
            job_type_name::<T>()
        );
    }

    /// Assert that no jobs were dispatched.
    #[track_caller]
    pub fn assert_nothing_dispatched(&self) {
        let names: Vec<&str> = [
            &self.commands,
            &self.commands_sync,
            &self.commands_after_response,
        ]
        .iter()
        .flat_map(|commands| {
            commands
                .lock()
                .unwrap()
                .iter()
                .map(Envelope::command_name)
                .collect::<Vec<_>>()
        })
        .collect();
        assert!(
            names.is_empty(),
            "The following jobs were dispatched unexpectedly:\n\n- {}",
            names.join("\n- ")
        );
    }

    /// Assert that a job of type `T` was dispatched synchronously.
    #[track_caller]
    pub fn assert_dispatched_sync<T: ShouldQueue>(&self) {
        self.assert_dispatched_sync_with::<T>(|_| true);
    }

    /// Assert that a job of type `T` passing the test was dispatched
    /// synchronously.
    #[track_caller]
    pub fn assert_dispatched_sync_with<T: ShouldQueue>(&self, callback: impl Fn(&T) -> bool) {
        assert!(
            !Self::matching::<T>(&self.commands_sync, callback).is_empty(),
            "The expected [{}] job was not dispatched synchronously.",
            job_type_name::<T>()
        );
    }

    /// Assert that a job of type `T` was dispatched synchronously exactly
    /// `times` times.
    #[track_caller]
    pub fn assert_dispatched_sync_times<T: ShouldQueue>(&self, times: usize) {
        let count = Self::matching::<T>(&self.commands_sync, |_| true).len();
        assert_eq!(
            count,
            times,
            "The expected [{}] job was synchronously pushed {count} times instead of {times} times.",
            job_type_name::<T>()
        );
    }

    /// Assert that no job of type `T` was dispatched synchronously.
    #[track_caller]
    pub fn assert_not_dispatched_sync<T: ShouldQueue>(&self) {
        assert!(
            Self::matching::<T>(&self.commands_sync, |_| true).is_empty(),
            "The unexpected [{}] job was dispatched synchronously.",
            job_type_name::<T>()
        );
    }

    /// Assert that a job of type `T` was dispatched after the response.
    #[track_caller]
    pub fn assert_dispatched_after_response<T: ShouldQueue>(&self) {
        self.assert_dispatched_after_response_with::<T>(|_| true);
    }

    /// Assert that a job of type `T` passing the test was dispatched after
    /// the response.
    #[track_caller]
    pub fn assert_dispatched_after_response_with<T: ShouldQueue>(
        &self,
        callback: impl Fn(&T) -> bool,
    ) {
        assert!(
            !Self::matching::<T>(&self.commands_after_response, callback).is_empty(),
            "The expected [{}] job was not dispatched after sending the response.",
            job_type_name::<T>()
        );
    }

    /// Assert that a job of type `T` was dispatched after the response
    /// exactly `times` times.
    #[track_caller]
    pub fn assert_dispatched_after_response_times<T: ShouldQueue>(&self, times: usize) {
        let count = Self::matching::<T>(&self.commands_after_response, |_| true).len();
        assert_eq!(
            count,
            times,
            "The expected [{}] job was pushed {count} times instead of {times} times.",
            job_type_name::<T>()
        );
    }

    /// Assert that no job of type `T` was dispatched after the response.
    #[track_caller]
    pub fn assert_not_dispatched_after_response<T: ShouldQueue>(&self) {
        assert!(
            Self::matching::<T>(&self.commands_after_response, |_| true).is_empty(),
            "The unexpected [{}] job was dispatched after sending the response.",
            job_type_name::<T>()
        );
    }

    /// Assert that a chain of the given job types was dispatched:
    /// `assert_chained::<(ShipOrder, RecordShipment, UpdateInventory)>()`.
    #[track_caller]
    pub fn assert_chained<C: JobTypes>(&self) {
        let expected = C::job_names();
        let Some((first, rest)) = expected.split_first() else {
            panic!("The expected chain can not be empty.");
        };
        let commands = self.commands.lock().unwrap();
        assert!(
            commands.iter().any(|job| job.command_name() == *first),
            "The expected [{first}] job was not dispatched."
        );
        assert!(
            commands
                .iter()
                .any(|job| job.command_name() == *first && job.chained_job_names() == rest),
            "The expected chain was not dispatched: [{}].",
            expected.join(", ")
        );
    }

    /// Assert that a chain of exactly these jobs (same types and data) was
    /// dispatched.
    #[track_caller]
    pub fn assert_chained_jobs(&self, expected: Vec<Box<dyn ShouldQueue>>) {
        let expected: Vec<(String, illuminate_support::Value)> = expected
            .iter()
            .map(|job| {
                (
                    job.command_name().to_string(),
                    job.serialize_command().unwrap_or_default(),
                )
            })
            .collect();
        let found = self.commands.lock().unwrap().iter().any(|job| {
            let Ok(chained) = job.chained_jobs() else {
                return false;
            };
            let actual: Vec<(String, illuminate_support::Value)> = std::iter::once(job)
                .chain(chained.iter())
                .map(|job| {
                    (
                        job.command_name().to_string(),
                        job.job().serialize_command().unwrap_or_default(),
                    )
                })
                .collect();
            actual == expected
        });
        assert!(found, "The expected chain was not dispatched.");
    }

    /// Assert that a job of type `T` was dispatched without a chain.
    #[track_caller]
    pub fn assert_dispatched_without_chain<T: ShouldQueue>(&self) {
        self.assert_dispatched::<T>();
        assert!(
            Self::matching::<T>(&self.commands, |_| true)
                .iter()
                .any(|job| job.chained_job_names().is_empty()),
            "The expected [{}] job was dispatched with a chain.",
            job_type_name::<T>()
        );
    }

    /// Assert that no chains were dispatched.
    #[track_caller]
    pub fn assert_nothing_chained(&self) {
        self.assert_nothing_dispatched();
    }

    /// Assert that a batch passing the given test was dispatched.
    #[track_caller]
    pub fn assert_batched(&self, callback: impl Fn(&PendingBatch) -> bool) {
        assert!(
            self.batches.lock().unwrap().iter().any(callback),
            "The expected batch was not dispatched."
        );
    }

    /// Assert the number of batches that were dispatched.
    #[track_caller]
    pub fn assert_batch_count(&self, count: usize) {
        let actual = self.batches.lock().unwrap().len();
        assert_eq!(
            actual, count,
            "Expected {count} batches to be dispatched, but found {actual}."
        );
    }

    /// Assert that no batches were dispatched.
    #[track_caller]
    pub fn assert_nothing_batched(&self) {
        let batches = self.batches.lock().unwrap();
        assert!(
            batches.is_empty(),
            "The following batched jobs were dispatched unexpectedly:\n\n- {}",
            batches
                .iter()
                .flat_map(|batch| batch.jobs.iter())
                .flat_map(|item| item.envelopes())
                .map(Envelope::command_name)
                .collect::<Vec<_>>()
                .join("\n- ")
        );
    }

    /// Assert that no jobs or batches were dispatched.
    #[track_caller]
    pub fn assert_nothing_placed(&self) {
        self.assert_nothing_dispatched();
        self.assert_nothing_batched();
    }
}

impl std::fmt::Debug for BusFake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BusFake")
            .field("dispatched", &self.commands.lock().unwrap().len())
            .field("batches", &self.batches.lock().unwrap().len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl QueueingDispatcher for BusFake {
    async fn dispatch(&self, job: Envelope) -> Result<()> {
        if self.should_fake(&job) {
            self.commands.lock().unwrap().push(job);
            return Ok(());
        }
        self.dispatcher.dispatch(job).await
    }

    async fn dispatch_sync(&self, job: Envelope) -> Result<()> {
        if self.should_fake(&job) {
            self.commands_sync.lock().unwrap().push(job);
            return Ok(());
        }
        self.dispatcher.dispatch_sync(job).await
    }

    async fn dispatch_to_queue(&self, job: Envelope) -> Result<Option<String>> {
        if self.should_fake(&job) {
            self.commands.lock().unwrap().push(job);
            return Ok(None);
        }
        self.dispatcher.dispatch_to_queue(job).await
    }

    async fn dispatch_after_response(&self, job: Envelope) -> Result<()> {
        if self.should_fake(&job) {
            self.commands_after_response.lock().unwrap().push(job);
            return Ok(());
        }
        self.dispatcher.dispatch_after_response(job).await
    }

    async fn bulk(&self, jobs: Vec<Envelope>) -> Result<()> {
        for job in jobs {
            self.dispatch(job).await?;
        }
        Ok(())
    }

    async fn dispatch_batch(&self, batch: PendingBatch) -> Result<Batch> {
        let repository: Arc<dyn BatchRepository> = self.batch_repository.clone();
        let record = repository.store(&batch.name, &batch.options()?).await?;
        let count: usize = batch.jobs.iter().map(|item| item.len()).sum();
        repository
            .increment_total_jobs(&record.id, count as u64)
            .await?;
        self.batches.lock().unwrap().push(batch);
        let record = repository.find(&record.id).await?.unwrap_or(record);
        Ok(Batch::from_record(record, repository))
    }

    async fn find_batch(&self, batch_id: &str) -> Result<Option<Batch>> {
        let repository: Arc<dyn BatchRepository> = self.batch_repository.clone();
        Ok(repository
            .find(batch_id)
            .await?
            .map(|record| Batch::from_record(record, repository)))
    }
}
