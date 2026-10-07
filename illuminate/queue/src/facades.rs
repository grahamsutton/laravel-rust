//! The `Queue` and `Bus` facades.

use std::sync::Arc;

use serde::de::DeserializeOwned;

use illuminate_container::{Container, try_app};
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Carbon, Map, Result, Value};

use crate::bus::batch::{Batch, PendingBatch};
use crate::bus::chain::PendingChain;
use crate::bus::dispatcher::{QueueingDispatcher, dispatcher};
use crate::contracts::{Queue as QueueContract, QueueConnector};
use crate::delay::IntoDelay;
use crate::envelope::Envelope;
use crate::events::{
    JobExceptionOccurred, JobFailed, JobProcessed, JobProcessing, Looping, QueueEvent,
    QueueEventType, WorkerStarting, WorkerStopping,
};
use crate::failed::{FailedJob, FailedJobProvider, failer};
use crate::job::ShouldQueue;
use crate::manager::{QueueManager, queue_manager};
use crate::queued_job::QueuedJob;
use crate::registry::JobRegistry;
use crate::testing::{BusFake, JobTypes, QueueFake};
use crate::worker::Worker;

/// The `Queue` facade.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::Container;
/// use illuminate_queue::{Queue, ShouldQueue, async_trait};
/// use illuminate_support::{Result, json};
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct ProcessPodcast;
///
/// #[async_trait]
/// impl ShouldQueue for ProcessPodcast {
///     async fn handle(&self) -> Result<()> {
///         Ok(())
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({
///     "queue": {"default": "array", "connections": {"array": {"driver": "array"}}},
/// })));
///
/// Queue::push_on("podcasts", ProcessPodcast).await?;
///
/// assert_eq!(Queue::size(Some("podcasts")).await?, 1);
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub struct Queue;

impl Queue {
    /// The queue manager.
    pub fn manager() -> Arc<QueueManager> {
        queue_manager()
    }

    /// Get a queue connection by name.
    pub fn connection(name: &str) -> Result<Arc<dyn QueueContract>> {
        queue_manager().connection(Some(name))
    }

    /// Get the default queue connection.
    pub fn default_connection() -> Result<Arc<dyn QueueContract>> {
        queue_manager().connection(None)
    }

    /// Register a custom queue driver.
    pub fn extend(driver: &str, connector: impl QueueConnector) {
        queue_manager().extend(driver, connector);
    }

    /// Register a job type so workers in this process can deserialize it.
    pub fn register<T: ShouldQueue + DeserializeOwned>() {
        JobRegistry::register::<T>();
    }

    /// Push a job onto the default queue of the default connection.
    pub async fn push(job: impl Into<Envelope>) -> Result<Option<String>> {
        let job = job.into();
        Self::default_connection()?.push(&job, None).await
    }

    /// Push a job onto the given queue.
    pub async fn push_on(queue: &str, job: impl Into<Envelope>) -> Result<Option<String>> {
        let job = job.into();
        Self::default_connection()?.push(&job, Some(queue)).await
    }

    /// Push a job onto the queue after a delay.
    pub async fn later(delay: impl IntoDelay, job: impl Into<Envelope>) -> Result<Option<String>> {
        let job = job.into();
        Self::default_connection()?
            .later(delay.into_delay(), &job, None)
            .await
    }

    /// Push a job onto the given queue after a delay.
    pub async fn later_on(
        queue: &str,
        delay: impl IntoDelay,
        job: impl Into<Envelope>,
    ) -> Result<Option<String>> {
        let job = job.into();
        Self::default_connection()?
            .later(delay.into_delay(), &job, Some(queue))
            .await
    }

    /// Push several jobs onto the queue.
    pub async fn bulk(jobs: Vec<Box<dyn ShouldQueue>>, queue: Option<&str>) -> Result<()> {
        let jobs: Vec<Envelope> = jobs.into_iter().map(Envelope::from_box).collect();
        Self::default_connection()?.bulk(&jobs, queue).await
    }

    /// Push a raw payload onto the queue.
    pub async fn push_raw(
        payload: impl Into<String>,
        queue: Option<&str>,
    ) -> Result<Option<String>> {
        Self::default_connection()?
            .push_raw(payload.into(), queue, None)
            .await
    }

    /// Pop the next job off the queue.
    pub async fn pop(queue: Option<&str>) -> Result<Option<QueuedJob>> {
        Self::default_connection()?.pop(queue).await
    }

    /// The number of jobs on the queue.
    pub async fn size(queue: Option<&str>) -> Result<u64> {
        Self::default_connection()?.size(queue).await
    }

    /// Delete every job from the queue.
    pub async fn clear(queue: Option<&str>) -> Result<u64> {
        Self::default_connection()?.clear(queue).await
    }

    // ------------------------------------------------------------------
    // Routing
    // ------------------------------------------------------------------

    /// Send jobs of type `T` to the given queue and/or connection by
    /// default: `Queue::route::<ProcessPodcast>("podcasts", "redis")`.
    pub fn route<'a, T: ShouldQueue>(
        queue: impl Into<Option<&'a str>>,
        connection: impl Into<Option<&'a str>>,
    ) {
        queue_manager().route(T::job_name(), queue.into(), connection.into());
    }

    /// Forward jobs pushed onto `queue` to another queue and/or connection:
    /// `Queue::forward("reports", "reports.fifo", "sqs")`.
    pub fn forward<'a>(
        queue: &str,
        to: impl Into<Option<&'a str>>,
        connection: impl Into<Option<&'a str>>,
    ) {
        queue_manager().forward(queue, to.into(), connection.into());
    }

    // ------------------------------------------------------------------
    // Events
    // ------------------------------------------------------------------

    /// Listen for queue events of type `E`.
    pub fn listen<E: QueueEventType>(callback: impl Fn(&E) + Send + Sync + 'static) {
        queue_manager().listen(callback);
    }

    /// Listen for every queue event.
    pub fn listen_all(callback: impl Fn(&QueueEvent) + Send + Sync + 'static) {
        queue_manager().listen_all(callback);
    }

    /// Run the callback before each job is processed.
    pub fn before(callback: impl Fn(&JobProcessing) + Send + Sync + 'static) {
        queue_manager().before(callback);
    }

    /// Run the callback after each job is processed.
    pub fn after(callback: impl Fn(&JobProcessed) + Send + Sync + 'static) {
        queue_manager().after(callback);
    }

    /// Run the callback when a job throws an exception.
    pub fn exception_occurred(callback: impl Fn(&JobExceptionOccurred) + Send + Sync + 'static) {
        queue_manager().exception_occurred(callback);
    }

    /// Run the callback before a worker looks for a job.
    pub fn looping(callback: impl Fn(&Looping) + Send + Sync + 'static) {
        queue_manager().looping(callback);
    }

    /// Run the callback when a job fails.
    pub fn failing(callback: impl Fn(&JobFailed) + Send + Sync + 'static) {
        queue_manager().failing(callback);
    }

    /// Run the callback when a worker starts.
    pub fn starting(callback: impl Fn(&WorkerStarting) + Send + Sync + 'static) {
        queue_manager().starting(callback);
    }

    /// Run the callback when a worker stops.
    pub fn stopping(callback: impl Fn(&WorkerStopping) + Send + Sync + 'static) {
        queue_manager().stopping(callback);
    }

    /// Add keys to every payload.
    pub fn create_payload_using(
        callback: impl Fn(&str, &str, &Value) -> Map<String, Value> + Send + Sync + 'static,
    ) {
        queue_manager().create_payload_using(callback);
    }

    // ------------------------------------------------------------------
    // Workers
    // ------------------------------------------------------------------

    /// Create a worker (what `queue:work` runs).
    pub fn worker() -> Worker {
        Worker::make()
    }

    /// Pause a queue.
    pub async fn pause(connection: &str, queue: &str) -> Result<()> {
        queue_manager().pause(connection, queue).await
    }

    /// Pause a queue for the given number of seconds.
    pub async fn pause_for(connection: &str, queue: &str, seconds: u64) -> Result<()> {
        queue_manager().pause_for(connection, queue, seconds).await
    }

    /// Pause every queue.
    pub async fn pause_all() -> Result<()> {
        queue_manager().pause_all().await
    }

    /// Resume a paused queue.
    pub async fn resume(connection: &str, queue: &str) -> Result<()> {
        queue_manager().resume(connection, queue).await
    }

    /// Resume every queue.
    pub async fn resume_all() -> Result<()> {
        queue_manager().resume_all().await
    }

    /// Determine if a queue is paused.
    pub async fn is_paused(connection: &str, queue: &str) -> Result<bool> {
        queue_manager().is_paused(connection, queue).await
    }

    /// Signal every worker to restart after its current job.
    pub async fn restart() -> Result<()> {
        queue_manager().restart().await
    }

    /// Stop workers from polling for restart and pause signals.
    pub fn without_interruption_polling() {
        queue_manager().without_interruption_polling();
    }

    // ------------------------------------------------------------------
    // Failed jobs
    // ------------------------------------------------------------------

    /// The failed job provider.
    pub fn failer() -> Arc<dyn FailedJobProvider> {
        failer()
    }

    /// Every failed job, newest first (`queue:failed`).
    pub async fn failed_jobs() -> Result<Vec<FailedJob>> {
        failer().all().await
    }

    /// Find a failed job.
    pub async fn find_failed(id: &str) -> Result<Option<FailedJob>> {
        failer().find(id).await
    }

    /// Push a failed job back onto its queue and forget it
    /// (`queue:retry {id}`). Returns whether the job was found.
    pub async fn retry_failed(id: &str) -> Result<bool> {
        let failer = failer();
        let Some(job) = failer.find(id).await? else {
            return Ok(false);
        };
        queue_manager().retry(&job).await?;
        failer.forget(id).await?;
        Ok(true)
    }

    /// Retry every failed job, or only those of the given queue
    /// (`queue:retry all`, `queue:retry --queue=`), returning their ids.
    pub async fn retry_all_failed(queue: Option<&str>) -> Result<Vec<String>> {
        let ids = failer().ids(queue).await?;
        let mut retried = Vec::new();
        for id in ids {
            if Self::retry_failed(&id).await? {
                retried.push(id);
            }
        }
        Ok(retried)
    }

    /// Retry the failed jobs of a batch (`queue:retry-batch {id}`),
    /// returning their ids.
    pub async fn retry_batch(batch_id: &str) -> Result<Vec<String>> {
        let Some(batch) = dispatcher().find_batch(batch_id).await? else {
            return Err(InvalidArgumentException::new(format!(
                "Unable to find a batch with ID [{batch_id}]."
            ))
            .into());
        };
        let mut retried = Vec::new();
        for id in &batch.failed_job_ids {
            if Self::retry_failed(id).await? {
                retried.push(id.clone());
            }
        }
        Ok(retried)
    }

    /// Delete a failed job (`queue:forget`).
    pub async fn forget_failed(id: &str) -> Result<bool> {
        failer().forget(id).await
    }

    /// Delete every failed job, or those older than `hours` (`queue:flush`).
    pub async fn flush_failed(hours: Option<u64>) -> Result<()> {
        failer().flush(hours).await
    }

    /// Delete failed jobs older than the given date (`queue:prune-failed`).
    pub async fn prune_failed(before: Carbon) -> Result<u64> {
        failer().prune(before).await
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    /// Replace the queue with a fake that records pushed jobs.
    pub fn fake() -> Arc<QueueFake> {
        let manager = queue_manager();
        let fake = Arc::new(QueueFake::new(&manager));
        manager.set_fake(Some(fake.clone()));
        Container::get_instance().instance_arc::<QueueFake>(fake.clone());
        fake
    }

    /// The current fake.
    ///
    /// # Panics
    ///
    /// Panics when the queue isn't being faked.
    #[track_caller]
    pub fn fake_instance() -> Arc<QueueFake> {
        try_app::<QueueFake>()
            .or_else(|| queue_manager().fake_instance())
            .expect("The queue is not being faked. Call `Queue::fake()` first.")
    }

    /// The pushed jobs of type `T`.
    pub fn pushed<T: ShouldQueue>() -> Vec<Arc<T>> {
        Self::fake_instance().pushed::<T>()
    }

    /// Assert that a job of type `T` was pushed.
    #[track_caller]
    pub fn assert_pushed<T: ShouldQueue>() {
        Self::fake_instance().assert_pushed::<T>();
    }

    /// Assert that a job of type `T` passing the test was pushed.
    #[track_caller]
    pub fn assert_pushed_with<T: ShouldQueue>(callback: impl Fn(&T) -> bool) {
        Self::fake_instance().assert_pushed_with::<T>(callback);
    }

    /// Assert that a job of type `T` was pushed `times` times.
    #[track_caller]
    pub fn assert_pushed_times<T: ShouldQueue>(times: usize) {
        Self::fake_instance().assert_pushed_times::<T>(times);
    }

    /// Assert that a job of type `T` was pushed once.
    #[track_caller]
    pub fn assert_pushed_once<T: ShouldQueue>() {
        Self::fake_instance().assert_pushed_once::<T>();
    }

    /// Assert that a job of type `T` was pushed onto the given queue.
    #[track_caller]
    pub fn assert_pushed_on<T: ShouldQueue>(queue: &str) {
        Self::fake_instance().assert_pushed_on::<T>(queue);
    }

    /// Assert that a job of type `T` passing the test was pushed onto the
    /// given queue.
    #[track_caller]
    pub fn assert_pushed_on_with<T: ShouldQueue>(queue: &str, callback: impl Fn(&T) -> bool) {
        Self::fake_instance().assert_pushed_on_with::<T>(queue, callback);
    }

    /// Assert that a job of type `T` was pushed with the chain `C`.
    #[track_caller]
    pub fn assert_pushed_with_chain<T: ShouldQueue, C: JobTypes>() {
        Self::fake_instance().assert_pushed_with_chain::<T, C>();
    }

    /// Assert that a job of type `T` was pushed without a chain.
    #[track_caller]
    pub fn assert_pushed_without_chain<T: ShouldQueue>() {
        Self::fake_instance().assert_pushed_without_chain::<T>();
    }

    /// Assert that no job of type `T` was pushed.
    #[track_caller]
    pub fn assert_not_pushed<T: ShouldQueue>() {
        Self::fake_instance().assert_not_pushed::<T>();
    }

    /// Assert that no job of type `T` passing the test was pushed.
    #[track_caller]
    pub fn assert_not_pushed_with<T: ShouldQueue>(callback: impl Fn(&T) -> bool) {
        Self::fake_instance().assert_not_pushed_with::<T>(callback);
    }

    /// Assert that a queued closure was pushed.
    #[track_caller]
    pub fn assert_closure_pushed() {
        Self::fake_instance().assert_closure_pushed();
    }

    /// Assert that no queued closure was pushed.
    #[track_caller]
    pub fn assert_closure_not_pushed() {
        Self::fake_instance().assert_closure_not_pushed();
    }

    /// Assert that no jobs were pushed.
    #[track_caller]
    pub fn assert_nothing_pushed() {
        Self::fake_instance().assert_nothing_pushed();
    }

    /// Assert the total number of jobs pushed.
    #[track_caller]
    pub fn assert_count(count: usize) {
        Self::fake_instance().assert_count(count);
    }
}

/// The `Bus` facade.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_queue::{Bus, ShouldQueue, async_trait};
/// use illuminate_support::Result;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct ProcessPodcast;
///
/// #[async_trait]
/// impl ShouldQueue for ProcessPodcast {
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
/// Bus::dispatch(ProcessPodcast).await?;
///
/// Bus::assert_dispatched::<ProcessPodcast>();
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub struct Bus;

impl Bus {
    /// The bus dispatcher.
    pub fn dispatcher() -> Arc<dyn QueueingDispatcher> {
        dispatcher()
    }

    /// Dispatch a job to its queue.
    pub async fn dispatch(job: impl Into<Envelope>) -> Result<()> {
        let job = job.into();
        dispatcher().dispatch(job).await
    }

    /// Run a job immediately, in the current process.
    pub async fn dispatch_sync(job: impl Into<Envelope>) -> Result<()> {
        let job = job.into();
        dispatcher().dispatch_sync(job).await
    }

    /// Push a job onto its queue, returning the driver's job id.
    pub async fn dispatch_to_queue(job: impl Into<Envelope>) -> Result<Option<String>> {
        let job = job.into();
        dispatcher().dispatch_to_queue(job).await
    }

    /// Run a job after the response has been sent.
    pub async fn dispatch_after_response(job: impl Into<Envelope>) -> Result<()> {
        let job = job.into();
        dispatcher().dispatch_after_response(job).await
    }

    /// Dispatch several jobs, grouped by connection and queue.
    pub async fn bulk(jobs: Vec<Box<dyn ShouldQueue>>) -> Result<()> {
        let jobs = jobs.into_iter().map(Envelope::from_box).collect();
        dispatcher().bulk(jobs).await
    }

    /// Create a chain of jobs.
    pub fn chain(jobs: Vec<Box<dyn ShouldQueue>>) -> PendingChain {
        PendingChain::new(jobs)
    }

    /// Create a batch of jobs.
    pub fn batch(jobs: Vec<Box<dyn ShouldQueue>>) -> PendingBatch {
        PendingBatch::new(jobs)
    }

    /// Find a batch by its id.
    pub async fn find_batch(batch_id: &str) -> Result<Option<Batch>> {
        dispatcher().find_batch(batch_id).await
    }

    /// Run "after response" jobs immediately instead (only on the default
    /// dispatcher).
    pub fn without_dispatching_after_responses() {
        let container = Container::get_instance();
        let dispatcher = Arc::new(crate::bus::dispatcher::Dispatcher::new());
        dispatcher.without_dispatching_after_responses();
        container.instance_arc::<dyn QueueingDispatcher>(dispatcher);
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    /// Replace the dispatcher with a fake that records dispatched jobs.
    pub fn fake() -> Arc<BusFake> {
        let fake = Arc::new(BusFake::new(dispatcher()));
        let container = Container::get_instance();
        container.instance_arc::<dyn QueueingDispatcher>(fake.clone());
        container.instance_arc::<BusFake>(fake.clone());
        fake
    }

    /// The current fake.
    ///
    /// # Panics
    ///
    /// Panics when the bus isn't being faked.
    #[track_caller]
    pub fn fake_instance() -> Arc<BusFake> {
        try_app::<BusFake>().expect("The bus is not being faked. Call `Bus::fake()` first.")
    }

    /// The dispatched jobs of type `T`.
    pub fn dispatched<T: ShouldQueue>() -> Vec<Arc<T>> {
        Self::fake_instance().dispatched::<T>()
    }

    /// The dispatched batches.
    pub fn dispatched_batches() -> Vec<PendingBatch> {
        Self::fake_instance().dispatched_batches()
    }

    /// Assert that a job of type `T` was dispatched.
    #[track_caller]
    pub fn assert_dispatched<T: ShouldQueue>() {
        Self::fake_instance().assert_dispatched::<T>();
    }

    /// Assert that a job of type `T` passing the test was dispatched.
    #[track_caller]
    pub fn assert_dispatched_with<T: ShouldQueue>(callback: impl Fn(&T) -> bool) {
        Self::fake_instance().assert_dispatched_with::<T>(callback);
    }

    /// Assert that a job of type `T` was dispatched `times` times.
    #[track_caller]
    pub fn assert_dispatched_times<T: ShouldQueue>(times: usize) {
        Self::fake_instance().assert_dispatched_times::<T>(times);
    }

    /// Assert that a job of type `T` was dispatched once.
    #[track_caller]
    pub fn assert_dispatched_once<T: ShouldQueue>() {
        Self::fake_instance().assert_dispatched_once::<T>();
    }

    /// Assert that no job of type `T` was dispatched.
    #[track_caller]
    pub fn assert_not_dispatched<T: ShouldQueue>() {
        Self::fake_instance().assert_not_dispatched::<T>();
    }

    /// Assert that no job of type `T` passing the test was dispatched.
    #[track_caller]
    pub fn assert_not_dispatched_with<T: ShouldQueue>(callback: impl Fn(&T) -> bool) {
        Self::fake_instance().assert_not_dispatched_with::<T>(callback);
    }

    /// Assert that no jobs were dispatched.
    #[track_caller]
    pub fn assert_nothing_dispatched() {
        Self::fake_instance().assert_nothing_dispatched();
    }

    /// Assert that a job of type `T` was dispatched synchronously.
    #[track_caller]
    pub fn assert_dispatched_sync<T: ShouldQueue>() {
        Self::fake_instance().assert_dispatched_sync::<T>();
    }

    /// Assert that a job of type `T` passing the test was dispatched
    /// synchronously.
    #[track_caller]
    pub fn assert_dispatched_sync_with<T: ShouldQueue>(callback: impl Fn(&T) -> bool) {
        Self::fake_instance().assert_dispatched_sync_with::<T>(callback);
    }

    /// Assert that a job of type `T` was dispatched synchronously `times`
    /// times.
    #[track_caller]
    pub fn assert_dispatched_sync_times<T: ShouldQueue>(times: usize) {
        Self::fake_instance().assert_dispatched_sync_times::<T>(times);
    }

    /// Assert that no job of type `T` was dispatched synchronously.
    #[track_caller]
    pub fn assert_not_dispatched_sync<T: ShouldQueue>() {
        Self::fake_instance().assert_not_dispatched_sync::<T>();
    }

    /// Assert that a job of type `T` was dispatched after the response.
    #[track_caller]
    pub fn assert_dispatched_after_response<T: ShouldQueue>() {
        Self::fake_instance().assert_dispatched_after_response::<T>();
    }

    /// Assert that a job of type `T` passing the test was dispatched after
    /// the response.
    #[track_caller]
    pub fn assert_dispatched_after_response_with<T: ShouldQueue>(callback: impl Fn(&T) -> bool) {
        Self::fake_instance().assert_dispatched_after_response_with::<T>(callback);
    }

    /// Assert that a job of type `T` was dispatched after the response
    /// `times` times.
    #[track_caller]
    pub fn assert_dispatched_after_response_times<T: ShouldQueue>(times: usize) {
        Self::fake_instance().assert_dispatched_after_response_times::<T>(times);
    }

    /// Assert that no job of type `T` was dispatched after the response.
    #[track_caller]
    pub fn assert_not_dispatched_after_response<T: ShouldQueue>() {
        Self::fake_instance().assert_not_dispatched_after_response::<T>();
    }

    /// Assert that a chain of the given job types was dispatched.
    #[track_caller]
    pub fn assert_chained<C: JobTypes>() {
        Self::fake_instance().assert_chained::<C>();
    }

    /// Assert that a chain of exactly these jobs was dispatched.
    #[track_caller]
    pub fn assert_chained_jobs(jobs: Vec<Box<dyn ShouldQueue>>) {
        Self::fake_instance().assert_chained_jobs(jobs);
    }

    /// Assert that a job of type `T` was dispatched without a chain.
    #[track_caller]
    pub fn assert_dispatched_without_chain<T: ShouldQueue>() {
        Self::fake_instance().assert_dispatched_without_chain::<T>();
    }

    /// Assert that no chains were dispatched.
    #[track_caller]
    pub fn assert_nothing_chained() {
        Self::fake_instance().assert_nothing_chained();
    }

    /// Assert that a batch passing the test was dispatched.
    #[track_caller]
    pub fn assert_batched(callback: impl Fn(&PendingBatch) -> bool) {
        Self::fake_instance().assert_batched(callback);
    }

    /// Assert the number of batches dispatched.
    #[track_caller]
    pub fn assert_batch_count(count: usize) {
        Self::fake_instance().assert_batch_count(count);
    }

    /// Assert that no batches were dispatched.
    #[track_caller]
    pub fn assert_nothing_batched() {
        Self::fake_instance().assert_nothing_batched();
    }

    /// Assert that no jobs or batches were dispatched.
    #[track_caller]
    pub fn assert_nothing_placed() {
        Self::fake_instance().assert_nothing_placed();
    }
}
