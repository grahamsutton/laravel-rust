//! Job batches: a group of jobs with completion callbacks.

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use illuminate_container::{Container, try_app};
use illuminate_support::{Carbon, Conditionable, Error, Map, Result, Value, json};

use super::chain::{PendingCallback, PendingChain};
use super::dispatcher::dispatcher;
use super::repository::{BatchRecord, BatchRepository};
use crate::callbacks::{self, BatchCallback, CallbackRef, ChainCatchCallback};
use crate::context::current_context;
use crate::deferred::DeferredCallbacks;
use crate::envelope::{Envelope, SerializedJob};
use crate::events::{self, BatchCanceled, BatchDispatched, BatchFinished};
use crate::exceptions::MissingClosureException;
use crate::job::ShouldQueue;
use crate::manager::queue_manager;

const CALLBACK_KINDS: [&str; 6] = ["before", "progress", "then", "catch", "finally", "failure"];

/// The job counts of a batch after an update.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpdatedBatchJobCounts {
    /// The number of pending jobs remaining for the batch.
    pub pending_jobs: u64,
    /// The number of failed jobs that belong to the batch.
    pub failed_jobs: u64,
}

impl UpdatedBatchJobCounts {
    /// Create a new batch job counts object.
    pub fn new(pending_jobs: u64, failed_jobs: u64) -> Self {
        Self {
            pending_jobs,
            failed_jobs,
        }
    }

    /// Determine if all jobs have run exactly once.
    pub fn all_jobs_have_ran_exactly_once(&self) -> bool {
        self.pending_jobs == self.failed_jobs
    }
}

/// The batch repository bound in the container (registered on first use:
/// a [`DatabaseBatchRepository`](super::DatabaseBatchRepository) when
/// `queue.batching` is configured and a database manager is bound, an
/// in-memory one otherwise).
pub fn batch_repository() -> Arc<dyn BatchRepository> {
    if let Some(repository) = try_app::<dyn BatchRepository>() {
        return repository;
    }
    let container = Container::get_instance();
    container.singleton_if::<dyn BatchRepository>(super::database::make_batch_repository);
    container.make::<dyn BatchRepository>()
}

/// Find a batch through the bus (so faked buses answer with their fakes).
pub(crate) async fn find_batch(batch_id: &str) -> Result<Option<Batch>> {
    dispatcher().find_batch(batch_id).await
}

/// Find a batch in the bound repository.
pub(crate) async fn find_stored_batch(batch_id: &str) -> Result<Option<Batch>> {
    let repository = batch_repository();
    Ok(repository
        .find(batch_id)
        .await?
        .map(|record| Batch::from_record(record, repository)))
}

/// A dispatched batch of jobs.
///
/// Batches are JSON serializable, so you may return one straight from a
/// route to report its progress.
#[derive(Clone)]
pub struct Batch {
    /// The batch's UUID.
    pub id: String,
    /// The batch's name.
    pub name: String,
    /// The number of jobs assigned to the batch.
    pub total_jobs: u64,
    /// The number of jobs that have not been processed yet.
    pub pending_jobs: u64,
    /// The number of jobs that have failed.
    pub failed_jobs: u64,
    /// The UUIDs of the failed jobs.
    pub failed_job_ids: Vec<String>,
    /// The batch's options.
    pub options: Value,
    /// When the batch was created.
    pub created_at: Carbon,
    /// When the batch was cancelled.
    pub cancelled_at: Option<Carbon>,
    /// When the batch finished.
    pub finished_at: Option<Carbon>,
    repository: Arc<dyn BatchRepository>,
}

impl Batch {
    /// Create a batch from its stored record.
    pub fn from_record(record: BatchRecord, repository: Arc<dyn BatchRepository>) -> Self {
        Self {
            id: record.id,
            name: record.name,
            total_jobs: record.total_jobs,
            pending_jobs: record.pending_jobs,
            failed_jobs: record.failed_jobs,
            failed_job_ids: record.failed_job_ids,
            options: record.options,
            created_at: record.created_at,
            cancelled_at: record.cancelled_at,
            finished_at: record.finished_at,
            repository,
        }
    }

    /// The repository the batch is stored in.
    pub fn repository(&self) -> Arc<dyn BatchRepository> {
        self.repository.clone()
    }

    /// Get a fresh instance of the batch.
    pub async fn fresh(&self) -> Result<Option<Batch>> {
        Ok(self
            .repository
            .find(&self.id)
            .await?
            .map(|record| Batch::from_record(record, self.repository.clone())))
    }

    /// Add more jobs to the batch.
    ///
    /// Typically called from within a job of the batch:
    /// `self.batch().await?.unwrap().add(vec![...]).await?`.
    pub async fn add(&self, jobs: Vec<Box<dyn ShouldQueue>>) -> Result<Batch> {
        self.add_items(BatchItem::from_jobs(jobs)).await
    }

    /// Add batch items (jobs and chains) to the batch.
    pub async fn add_items(&self, items: Vec<BatchItem>) -> Result<Batch> {
        let connection = self.option_str("connection");
        let queue = self.option_str("queue");
        let mut count = 0;
        let mut jobs = Vec::with_capacity(items.len());

        for item in items {
            match item {
                BatchItem::Job(job) => {
                    count += 1;
                    jobs.push(job.with_batch_id(&self.id));
                }
                BatchItem::Chain(chain) => {
                    count += chain.len() as u64;
                    let mut chain = chain.into_iter().map(|job| job.with_batch_id(&self.id));
                    let Some(mut first) = chain.next() else {
                        continue;
                    };
                    if let Some(queue) = &queue {
                        first = first.all_on_queue(queue.clone());
                    }
                    if let Some(connection) = &connection {
                        first = first.all_on_connection(connection.clone());
                    }
                    jobs.push(first.chain(chain));
                }
            }
        }

        self.repository
            .increment_total_jobs(&self.id, count)
            .await?;
        queue_manager()
            .connection(connection.as_deref())?
            .bulk(&jobs, queue.as_deref())
            .await?;

        Ok(self.fresh().await?.unwrap_or_else(|| self.clone()))
    }

    /// The number of jobs that have been processed (successfully or not).
    pub fn processed_jobs(&self) -> u64 {
        self.total_jobs.saturating_sub(self.pending_jobs)
    }

    /// The percentage of jobs that have been processed (0-100).
    pub fn progress(&self) -> u64 {
        if self.total_jobs == 0 {
            return 0;
        }
        ((self.processed_jobs() as f64 / self.total_jobs as f64) * 100.0).round() as u64
    }

    /// Record that a job within the batch finished successfully, running
    /// the progress / then / finally callbacks as appropriate.
    pub async fn record_successful_job(&self, job_id: &str) -> Result<()> {
        let counts = self.decrement_pending_jobs(job_id).await?;

        if self.has_callbacks("progress") {
            self.invoke_callbacks("progress", None).await;
        }

        if counts.pending_jobs == 0 {
            self.repository.mark_as_finished(&self.id).await?;
            let batch = self.fresh().await?.unwrap_or_else(|| self.clone());
            events::dispatch(BatchFinished { batch });
        }

        if counts.pending_jobs == 0 && self.has_then_callbacks() {
            self.invoke_callbacks("then", None).await;
        }

        if counts.all_jobs_have_ran_exactly_once() && self.has_finally_callbacks() {
            self.invoke_callbacks("finally", None).await;
        }

        // Once every job succeeded, the callbacks can't fire again (failed
        // jobs may still be retried until then).
        if counts.pending_jobs == 0 {
            self.forget_closures();
        }

        Ok(())
    }

    /// Decrement the pending jobs for the batch.
    pub async fn decrement_pending_jobs(&self, job_id: &str) -> Result<UpdatedBatchJobCounts> {
        self.repository
            .decrement_pending_jobs(&self.id, job_id)
            .await
    }

    /// Record that a job within the batch failed, running the catch /
    /// failure / finally callbacks as appropriate.
    pub async fn record_failed_job(&self, job_id: &str, error: Arc<Error>) -> Result<()> {
        let counts = self.increment_failed_jobs(job_id).await?;

        if counts.failed_jobs == 1 && !self.allows_failures() {
            self.cancel_because(Some(error.clone())).await?;
        }

        if self.allows_failures() {
            if self.has_callbacks("progress") {
                self.invoke_callbacks("progress", Some(error.clone())).await;
            }
            if self.has_callbacks("failure") {
                self.invoke_callbacks("failure", Some(error.clone())).await;
            }
        }

        if counts.failed_jobs == 1 && self.has_catch_callbacks() {
            self.invoke_callbacks("catch", Some(error.clone())).await;
        }

        if counts.all_jobs_have_ran_exactly_once() && self.has_finally_callbacks() {
            self.invoke_callbacks("finally", None).await;
        }

        Ok(())
    }

    /// Increment the failed jobs for the batch.
    pub async fn increment_failed_jobs(&self, job_id: &str) -> Result<UpdatedBatchJobCounts> {
        self.repository
            .increment_failed_jobs(&self.id, job_id)
            .await
    }

    /// Determine if the batch has finished executing.
    pub fn finished(&self) -> bool {
        self.finished_at.is_some()
    }

    /// Determine if the batch has "progress" callbacks.
    pub fn has_progress_callbacks(&self) -> bool {
        self.has_callbacks("progress")
    }

    /// Determine if the batch has "then" callbacks.
    pub fn has_then_callbacks(&self) -> bool {
        self.has_callbacks("then")
    }

    /// Determine if the batch has "catch" callbacks.
    pub fn has_catch_callbacks(&self) -> bool {
        self.has_callbacks("catch")
    }

    /// Determine if the batch has "finally" callbacks.
    pub fn has_finally_callbacks(&self) -> bool {
        self.has_callbacks("finally")
    }

    /// Determine if the batch allows jobs to fail without cancelling it.
    pub fn allows_failures(&self) -> bool {
        self.options.get("allowFailures") == Some(&Value::Bool(true))
    }

    /// Determine if the batch has job failures.
    pub fn has_failures(&self) -> bool {
        self.failed_jobs > 0
    }

    /// Cancel the batch.
    pub async fn cancel(&self) -> Result<()> {
        self.cancel_because(None).await
    }

    async fn cancel_because(&self, error: Option<Arc<Error>>) -> Result<()> {
        self.repository.cancel(&self.id).await?;
        let batch = self.fresh().await?.unwrap_or_else(|| self.clone());
        events::dispatch(BatchCanceled {
            batch,
            exception: error,
        });
        Ok(())
    }

    /// Determine if the batch has been cancelled.
    pub fn cancelled(&self) -> bool {
        self.cancelled_at.is_some()
    }

    /// Determine if the batch has been cancelled (alias of `cancelled`).
    pub fn canceled(&self) -> bool {
        self.cancelled()
    }

    /// Delete the batch from storage.
    pub async fn delete(&self) -> Result<()> {
        self.forget_closures();
        self.repository.delete(&self.id).await
    }

    /// Get an option of the batch.
    pub fn option(&self, key: &str) -> Option<&Value> {
        self.options.get(key)
    }

    /// The batch as Laravel's array representation.
    pub fn to_array(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "totalJobs": self.total_jobs,
            "pendingJobs": self.pending_jobs,
            "processedJobs": self.processed_jobs(),
            "progress": self.progress(),
            "failedJobs": self.failed_jobs,
            "options": self.options,
            "createdAt": self.created_at,
            "cancelledAt": self.cancelled_at,
            "finishedAt": self.finished_at,
        })
    }

    fn option_str(&self, key: &str) -> Option<String> {
        self.options
            .get(key)
            .and_then(Value::as_str)
            .map(String::from)
    }

    fn callbacks(&self, kind: &str) -> Vec<CallbackRef> {
        self.options
            .get(kind)
            .cloned()
            .and_then(|callbacks| serde_json::from_value(callbacks).ok())
            .unwrap_or_default()
    }

    fn has_callbacks(&self, kind: &str) -> bool {
        self.options
            .get(kind)
            .and_then(Value::as_array)
            .is_some_and(|callbacks| !callbacks.is_empty())
    }

    fn forget_closures(&self) {
        let all: Vec<CallbackRef> = CALLBACK_KINDS
            .iter()
            .flat_map(|kind| self.callbacks(kind))
            .collect();
        callbacks::forget_all(&all);
    }

    async fn invoke_callbacks(&self, kind: &str, error: Option<Arc<Error>>) {
        let batch = match self.fresh().await {
            Ok(Some(batch)) => batch,
            _ => self.clone(),
        };

        for callback in self.callbacks(kind) {
            let result = match callback {
                CallbackRef::Closure { id } => {
                    if let Some(callback) = callbacks::get::<BatchCallback>(&id) {
                        callback(batch.clone(), error.clone()).await
                    } else if let Some(callback) = callbacks::get::<ChainCatchCallback>(&id) {
                        // A chain's catch callback, attached to a chained batch.
                        match &error {
                            Some(error) if kind == "catch" && !batch.allows_failures() => {
                                callback(error.clone()).await
                            }
                            _ => Ok(()),
                        }
                    } else {
                        Err(MissingClosureException { id }.into())
                    }
                }
                CallbackRef::Job {
                    job,
                    unless_cancelled,
                } => {
                    if unless_cancelled && batch.cancelled() {
                        Ok(())
                    } else {
                        match Envelope::from_serialized(job) {
                            Ok(envelope) => dispatcher().dispatch(envelope).await,
                            Err(error) => Err(error),
                        }
                    }
                }
            };

            if let Err(error) = result {
                crate::report(&error);
            }
        }
    }
}

impl fmt::Debug for Batch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Batch")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("total_jobs", &self.total_jobs)
            .field("pending_jobs", &self.pending_jobs)
            .field("failed_jobs", &self.failed_jobs)
            .field("cancelled_at", &self.cancelled_at)
            .field("finished_at", &self.finished_at)
            .finish_non_exhaustive()
    }
}

impl Serialize for Batch {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_array().serialize(serializer)
    }
}

/// An entry of a batch: a job, or a chain of jobs that run in order.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum BatchItem {
    /// A single job.
    Job(Envelope),
    /// A chain of jobs.
    Chain(Vec<Envelope>),
}

impl BatchItem {
    /// Turn boxed jobs into batch items: a boxed [`PendingChain`] becomes a
    /// chain within the batch.
    pub fn from_jobs(jobs: Vec<Box<dyn ShouldQueue>>) -> Vec<BatchItem> {
        jobs.into_iter()
            .map(|job| {
                if job.is::<PendingChain>() {
                    let chain = job
                        .into_any_box()
                        .downcast::<PendingChain>()
                        .expect("the job is a pending chain");
                    BatchItem::Chain(chain.jobs)
                } else {
                    BatchItem::Job(Envelope::from_box(job))
                }
            })
            .collect()
    }

    /// The number of jobs in the item.
    pub fn len(&self) -> usize {
        match self {
            BatchItem::Job(_) => 1,
            BatchItem::Chain(jobs) => jobs.len(),
        }
    }

    /// Determine if the item has no jobs.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The jobs in the item.
    pub fn envelopes(&self) -> Vec<&Envelope> {
        match self {
            BatchItem::Job(job) => vec![job],
            BatchItem::Chain(jobs) => jobs.iter().collect(),
        }
    }
}

impl From<Envelope> for BatchItem {
    fn from(job: Envelope) -> Self {
        BatchItem::Job(job)
    }
}

impl From<Vec<Envelope>> for BatchItem {
    fn from(jobs: Vec<Envelope>) -> Self {
        BatchItem::Chain(jobs)
    }
}

/// A batch of jobs waiting to be dispatched.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_queue::{Batch, Bus, ShouldQueue, async_trait};
/// use illuminate_support::Result;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct ImportCsv {
///     start: u64,
///     end: u64,
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
/// let batch = Bus::batch(vec![
///     Box::new(ImportCsv { start: 1, end: 100 }),
///     Box::new(ImportCsv { start: 101, end: 200 }),
/// ])
/// .then(|batch: Batch| async move {
///     println!("Imported {} chunks!", batch.total_jobs);
///     Ok(())
/// })
/// .name("Import CSV")
/// .dispatch()
/// .await?;
///
/// // With the default `sync` connection, the jobs have already run.
/// let batch = batch.fresh().await?.unwrap();
/// assert_eq!(batch.progress(), 100);
/// assert!(batch.finished());
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Clone)]
pub struct PendingBatch {
    /// The batch's name.
    pub name: String,
    /// The jobs (and chains) of the batch.
    pub jobs: Vec<BatchItem>,
    allow_failures: bool,
    connection: Option<String>,
    queue: Option<String>,
    callbacks: IndexMap<&'static str, Vec<PendingCallback>>,
    extra: Map<String, Value>,
}

impl PendingBatch {
    /// Create a pending batch of the given jobs.
    pub fn new(jobs: Vec<Box<dyn ShouldQueue>>) -> Self {
        Self::from_items(BatchItem::from_jobs(jobs))
    }

    /// Create a pending batch of the given items.
    pub fn from_items(jobs: Vec<BatchItem>) -> Self {
        Self {
            name: String::new(),
            jobs,
            allow_failures: false,
            connection: None,
            queue: None,
            callbacks: IndexMap::new(),
            extra: Map::new(),
        }
    }

    /// Add jobs to the batch.
    #[allow(clippy::should_implement_trait)]
    pub fn add(mut self, jobs: Vec<Box<dyn ShouldQueue>>) -> Self {
        self.jobs.extend(BatchItem::from_jobs(jobs));
        self
    }

    /// Add a chain of jobs to the batch.
    pub fn add_chain(mut self, jobs: Vec<Box<dyn ShouldQueue>>) -> Self {
        self.jobs.push(BatchItem::Chain(super::prepare_jobs(jobs)));
        self
    }

    /// The number of entries in the batch (a chain counts once).
    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    /// Determine if the batch contains a job of type `T`.
    pub fn has_job<T: ShouldQueue>(&self) -> bool {
        self.jobs
            .iter()
            .flat_map(BatchItem::envelopes)
            .any(|job| job.is::<T>())
    }

    /// Determine if the batch contains a job of type `T` passing the test.
    pub fn has_job_matching<T: ShouldQueue>(&self, callback: impl Fn(&T) -> bool) -> bool {
        self.jobs
            .iter()
            .flat_map(BatchItem::envelopes)
            .filter_map(|job| job.downcast_ref::<T>())
            .any(callback)
    }

    fn register<F, Fut>(&mut self, kind: &'static str, callback: F)
    where
        F: Fn(Batch, Option<Arc<Error>>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let callback: BatchCallback =
            Arc::new(move |batch, error| Box::pin(callback(batch, error)));
        let id = callbacks::store(callback);
        self.callbacks
            .entry(kind)
            .or_default()
            .push(PendingCallback::Ref(CallbackRef::Closure { id }));
    }

    fn register_job(&mut self, kind: &'static str, job: Envelope, unless_cancelled: bool) {
        self.callbacks
            .entry(kind)
            .or_default()
            .push(PendingCallback::Dispatch {
                job: Box::new(job),
                unless_cancelled,
            });
    }

    /// Run the callback once the batch is stored, before any job is added.
    pub fn before<F, Fut>(mut self, callback: F) -> Self
    where
        F: Fn(Batch) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.register("before", move |batch, _| callback(batch));
        self
    }

    /// Run the callback each time a job of the batch completes.
    pub fn progress<F, Fut>(mut self, callback: F) -> Self
    where
        F: Fn(Batch) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.register("progress", move |batch, _| callback(batch));
        self
    }

    /// Run the callback once every job of the batch completed successfully.
    pub fn then<F, Fut>(mut self, callback: F) -> Self
    where
        F: Fn(Batch) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.register("then", move |batch, _| callback(batch));
        self
    }

    /// Run the callback when the first job of the batch fails.
    pub fn catch<F, Fut>(mut self, callback: F) -> Self
    where
        F: Fn(Batch, Arc<Error>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.register("catch", move |batch, error: Option<Arc<Error>>| {
            let error = error.unwrap_or_else(|| Arc::new(crate::ManuallyFailedException.into()));
            callback(batch, error)
        });
        self
    }

    /// Run the callback once every job of the batch has run (successfully
    /// or not).
    pub fn finally<F, Fut>(mut self, callback: F) -> Self
    where
        F: Fn(Batch) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.register("finally", move |batch, _| callback(batch));
        self
    }

    /// Dispatch the given job once every job completed successfully.
    pub fn then_dispatch(mut self, job: impl Into<Envelope>) -> Self {
        self.register_job("then", job.into(), false);
        self
    }

    /// Dispatch the given job when the first job of the batch fails.
    pub fn catch_dispatch(mut self, job: impl Into<Envelope>) -> Self {
        self.register_job("catch", job.into(), false);
        self
    }

    /// Dispatch the given job once every job of the batch has run.
    pub fn finally_dispatch(mut self, job: impl Into<Envelope>) -> Self {
        self.register_job("finally", job.into(), false);
        self
    }

    /// Don't cancel the batch when one of its jobs fails.
    pub fn allow_failures(mut self) -> Self {
        self.allow_failures = true;
        self
    }

    /// Don't cancel the batch when one of its jobs fails, and run the
    /// callback for each failure.
    pub fn allow_failures_with<F, Fut>(mut self, callback: F) -> Self
    where
        F: Fn(Batch, Arc<Error>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.allow_failures = true;
        self.register("failure", move |batch, error: Option<Arc<Error>>| {
            let error = error.unwrap_or_else(|| Arc::new(crate::ManuallyFailedException.into()));
            callback(batch, error)
        });
        self
    }

    /// Determine if the batch allows jobs to fail.
    pub fn allows_failures(&self) -> bool {
        self.allow_failures
    }

    /// Set the batch's name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the connection the batch's jobs run on.
    pub fn on_connection(mut self, connection: impl Into<String>) -> Self {
        self.connection = Some(connection.into());
        self
    }

    /// The connection the batch's jobs run on.
    pub fn connection(&self) -> Option<&str> {
        self.connection.as_deref()
    }

    /// Set the queue the batch's jobs run on.
    pub fn on_queue(mut self, queue: impl Into<String>) -> Self {
        self.queue = Some(queue.into());
        self
    }

    /// The queue the batch's jobs run on.
    pub fn queue(&self) -> Option<&str> {
        self.queue.as_deref()
    }

    /// Add an option to the batch.
    pub fn with_option(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }

    /// The number of callbacks of the given kind (`then`, `catch`, ...).
    pub fn callback_count(&self, kind: &str) -> usize {
        self.callbacks.get(kind).map_or(0, Vec::len)
    }

    /// The batch's options, as stored by the repository.
    pub fn options(&self) -> Result<Value> {
        let mut options = self.extra.clone();
        options.insert("allowFailures".into(), Value::Bool(self.allow_failures));
        if let Some(connection) = &self.connection {
            options.insert("connection".into(), Value::String(connection.clone()));
        }
        if let Some(queue) = &self.queue {
            options.insert("queue".into(), Value::String(queue.clone()));
        }
        for (kind, callbacks) in &self.callbacks {
            if callbacks.is_empty() {
                continue;
            }
            let callbacks = callbacks
                .iter()
                .map(PendingCallback::resolve)
                .collect::<Result<Vec<_>>>()?;
            options.insert((*kind).to_string(), serde_json::to_value(callbacks)?);
        }
        Ok(Value::Object(options))
    }

    /// Dispatch the batch.
    pub async fn dispatch(self) -> Result<Batch> {
        dispatcher().dispatch_batch(self).await
    }

    /// Dispatch the batch if the given condition is true.
    pub async fn dispatch_if(self, condition: bool) -> Result<Option<Batch>> {
        if condition {
            self.dispatch().await.map(Some)
        } else {
            Ok(None)
        }
    }

    /// Dispatch the batch unless the given condition is true.
    pub async fn dispatch_unless(self, condition: bool) -> Result<Option<Batch>> {
        self.dispatch_if(!condition).await
    }

    /// Store the batch now, and add its jobs after the response has been
    /// sent to the browser.
    pub async fn dispatch_after_response(self) -> Result<Batch> {
        let batch = self.store().await?;
        let jobs = self.jobs;
        let pending = batch.clone();
        DeferredCallbacks::current().defer(move || async move {
            match pending.add_items(jobs).await {
                Ok(batch) => events::dispatch(BatchDispatched { batch }),
                Err(error) => {
                    let _ = pending.delete().await;
                    crate::report(&error);
                }
            }
        });
        Ok(batch)
    }

    async fn store(&self) -> Result<Batch> {
        let repository = batch_repository();
        let record = repository.store(&self.name, &self.options()?).await?;
        let batch = Batch::from_record(record, repository);
        if batch.has_callbacks("before") {
            batch.invoke_callbacks("before", None).await;
        }
        Ok(batch)
    }

    /// Store the batch and push its jobs (what the real dispatcher does).
    pub(crate) async fn store_and_dispatch(self) -> Result<Batch> {
        let batch = self.store().await?;
        match batch.add_items(self.jobs).await {
            Ok(batch) => {
                events::dispatch(BatchDispatched {
                    batch: batch.clone(),
                });
                Ok(batch)
            }
            Err(error) => {
                batch.delete().await?;
                Err(error)
            }
        }
    }
}

impl Conditionable for PendingBatch {}

impl fmt::Debug for PendingBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingBatch")
            .field("name", &self.name)
            .field("jobs", &self.jobs)
            .field("allow_failures", &self.allow_failures)
            .field("connection", &self.connection)
            .field("queue", &self.queue)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::large_enum_variant)]
enum SerializedBatchItem {
    Job(SerializedJob),
    Chain(Vec<SerializedJob>),
}

#[derive(Serialize, Deserialize)]
struct SerializedBatch {
    name: String,
    jobs: Vec<SerializedBatchItem>,
    options: Value,
}

impl Serialize for PendingBatch {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let jobs = self
            .jobs
            .iter()
            .map(|item| {
                Ok(match item {
                    BatchItem::Job(job) => SerializedBatchItem::Job(job.to_serialized()?),
                    BatchItem::Chain(jobs) => SerializedBatchItem::Chain(
                        jobs.iter()
                            .map(Envelope::to_serialized)
                            .collect::<Result<_>>()?,
                    ),
                })
            })
            .collect::<Result<Vec<_>>>()
            .map_err(serde::ser::Error::custom)?;
        SerializedBatch {
            name: self.name.clone(),
            jobs,
            options: self.options().map_err(serde::ser::Error::custom)?,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PendingBatch {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let serialized = SerializedBatch::deserialize(deserializer)?;
        let jobs = serialized
            .jobs
            .into_iter()
            .map(|item| {
                Ok(match item {
                    SerializedBatchItem::Job(job) => {
                        BatchItem::Job(Envelope::from_serialized(job)?)
                    }
                    SerializedBatchItem::Chain(jobs) => BatchItem::Chain(
                        jobs.into_iter()
                            .map(Envelope::from_serialized)
                            .collect::<Result<_>>()?,
                    ),
                })
            })
            .collect::<Result<Vec<_>>>()
            .map_err(serde::de::Error::custom)?;

        let mut batch = PendingBatch::from_items(jobs).name(serialized.name);
        let Value::Object(mut options) = serialized.options else {
            return Ok(batch);
        };

        batch.allow_failures = options.remove("allowFailures") == Some(Value::Bool(true));
        batch.connection = options
            .remove("connection")
            .and_then(|value| value.as_str().map(String::from));
        batch.queue = options
            .remove("queue")
            .and_then(|value| value.as_str().map(String::from));
        for kind in CALLBACK_KINDS {
            if let Some(callbacks) = options.remove(kind) {
                let callbacks: Vec<CallbackRef> =
                    serde_json::from_value(callbacks).map_err(serde::de::Error::custom)?;
                batch.callbacks.insert(
                    kind,
                    callbacks.into_iter().map(PendingCallback::Ref).collect(),
                );
            }
        }
        batch.extra = options;
        Ok(batch)
    }
}

/// A batch may be placed in a chain (Laravel's `ChainedBatch`): running it
/// dispatches the batch, and the rest of the chain continues once the
/// whole batch has finished.
#[async_trait]
impl ShouldQueue for PendingBatch {
    async fn handle(&self) -> Result<()> {
        let mut batch = self.clone();

        if let Some(context) = current_context() {
            let next = {
                let mut chain = context.chain.lock().unwrap();
                for callback in &chain.chain_catch_callbacks {
                    batch
                        .callbacks
                        .entry("catch")
                        .or_default()
                        .push(PendingCallback::Ref(callback.clone()));
                }
                if chain.chained.is_empty() {
                    None
                } else {
                    chain.consumed = true;
                    let next = chain.chained.remove(0);
                    let rest = std::mem::take(&mut chain.chained);
                    Some((
                        next,
                        rest,
                        chain.chain_connection.clone(),
                        chain.chain_queue.clone(),
                        chain.chain_catch_callbacks.clone(),
                    ))
                }
            };

            if let Some((next, rest, chain_connection, chain_queue, catch_callbacks)) = next {
                let mut next = next.into_envelope()?;
                next.chained = rest;
                if next.connection.is_none() {
                    next.connection = chain_connection.clone();
                }
                if next.queue.is_none() {
                    next.queue = chain_queue.clone();
                }
                next.chain_connection = chain_connection;
                next.chain_queue = chain_queue;
                next.chain_catch_callbacks = catch_callbacks;
                batch.register_job("finally", next, true);
            }
        }

        dispatcher().dispatch_batch(batch).await.map(drop)
    }

    fn display_name(&self) -> String {
        "Illuminate\\Bus\\ChainedBatch".to_string()
    }

    fn job_name() -> &'static str {
        "Illuminate\\Bus\\ChainedBatch"
    }
}

crate::register_job!(PendingBatch);
