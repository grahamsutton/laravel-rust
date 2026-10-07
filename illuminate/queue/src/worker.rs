//! The queue worker: pops jobs off the queue and runs them.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

use illuminate_cache::Repository as CacheRepository;
use illuminate_container::{Container, try_app};
use illuminate_support::{Carbon, Error, Result};

use crate::contracts::Queue;
use crate::events::{
    JobAttempted, JobExceptionOccurred, JobPopped, JobPopping, JobProcessed, JobProcessing,
    JobReleased, JobReleasedAfterException, JobTimedOut, Looping, WorkerIdle, WorkerInterrupted,
    WorkerQueuePaused, WorkerQueueResumed, WorkerStarting, WorkerStopping,
};
use crate::exceptions::{MaxAttemptsExceededException, TimeoutExceededException, unshare};
use crate::failed::{FailedJobProvider, failer};
use crate::manager::{QueueManager, queue_manager};
use crate::queued_job::QueuedJob;

/// The options a worker runs with (the `queue:work` options).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerOptions {
    /// The name of the worker.
    pub name: String,
    /// The seconds to wait before retrying a job that threw (`--backoff`),
    /// per attempt; the last value repeats.
    pub backoff: Vec<u64>,
    /// The memory limit in megabytes (`--memory`, informational).
    pub memory: u64,
    /// How long a job may run (`--timeout`); zero disables timeouts.
    pub timeout: Duration,
    /// How long to sleep when no job is available (`--sleep`).
    pub sleep: Duration,
    /// The number of times to attempt a job (`--tries`); zero retries
    /// forever.
    pub max_tries: u32,
    /// Process jobs even in maintenance mode (`--force`).
    pub force: bool,
    /// Stop when the queue is empty (`--stop-when-empty`).
    pub stop_when_empty: bool,
    /// Stop when no job has been processed for this long
    /// (`--stop-when-empty-for`).
    pub stop_when_empty_for: Duration,
    /// Stop after processing this many jobs (`--max-jobs`); zero is
    /// unlimited.
    pub max_jobs: u64,
    /// Stop after running this long (`--max-time`); zero is unlimited.
    pub max_time: Duration,
    /// How long to rest between jobs (`--rest`).
    pub rest: Duration,
}

impl Default for WorkerOptions {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            backoff: vec![0],
            memory: 128,
            timeout: Duration::from_secs(60),
            sleep: Duration::from_secs(3),
            max_tries: 1,
            force: false,
            stop_when_empty: false,
            stop_when_empty_for: Duration::ZERO,
            max_jobs: 0,
            max_time: Duration::ZERO,
            rest: Duration::ZERO,
        }
    }
}

impl WorkerOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the worker's name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the backoff (seconds) for jobs that throw.
    pub fn backoff(mut self, backoff: impl Into<Vec<u64>>) -> Self {
        self.backoff = backoff.into();
        self
    }

    /// Set the timeout, in seconds.
    pub fn timeout(mut self, seconds: u64) -> Self {
        self.timeout = Duration::from_secs(seconds);
        self
    }

    /// Set the timeout.
    pub fn timeout_duration(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set how long to sleep when no job is available, in seconds.
    pub fn sleep(mut self, seconds: f64) -> Self {
        self.sleep = Duration::try_from_secs_f64(seconds.max(0.0)).unwrap_or_default();
        self
    }

    /// Set the number of times to attempt a job.
    pub fn tries(mut self, tries: u32) -> Self {
        self.max_tries = tries;
        self
    }

    /// Process jobs even in maintenance mode.
    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }

    /// Stop when the queue is empty.
    pub fn stop_when_empty(mut self) -> Self {
        self.stop_when_empty = true;
        self
    }

    /// Stop after processing the given number of jobs.
    pub fn max_jobs(mut self, jobs: u64) -> Self {
        self.max_jobs = jobs;
        self
    }

    /// Stop after running for the given number of seconds.
    pub fn max_time(mut self, seconds: u64) -> Self {
        self.max_time = Duration::from_secs(seconds);
        self
    }

    /// Rest for the given number of seconds between jobs.
    pub fn rest(mut self, seconds: f64) -> Self {
        self.rest = Duration::try_from_secs_f64(seconds.max(0.0)).unwrap_or_default();
        self
    }
}

/// Why a worker stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerStopReason {
    /// The worker received a signal (or [`WorkerHandle::quit`]).
    Interrupted,
    /// The connection to the queue was lost.
    LostConnection,
    /// `--max-jobs` was reached.
    MaxJobsExceeded,
    /// `--memory` was exceeded.
    MaxMemoryExceeded,
    /// `--max-time` was reached.
    MaxTimeExceeded,
    /// `--stop-when-empty` and the queue is empty.
    QueueEmpty,
    /// `--stop-when-empty-for` and the queue stayed empty.
    QueueEmptyFor,
    /// `queue:restart` was called.
    ReceivedRestartSignal,
    /// A job timed out.
    TimedOut,
}

impl WorkerStopReason {
    /// The reason's value (`"max_jobs"`, ...).
    pub fn value(&self) -> &'static str {
        match self {
            Self::Interrupted => "interrupted",
            Self::LostConnection => "lost_connection",
            Self::MaxJobsExceeded => "max_jobs",
            Self::MaxMemoryExceeded => "memory",
            Self::MaxTimeExceeded => "max_time",
            Self::QueueEmpty => "empty",
            Self::QueueEmptyFor => "empty_for",
            Self::ReceivedRestartSignal => "restart_signal",
            Self::TimedOut => "timed_out",
        }
    }

    /// A human description of the reason.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Interrupted => "Interrupted",
            Self::LostConnection => "Lost connection",
            Self::MaxJobsExceeded => "Maximum jobs exceeded",
            Self::MaxMemoryExceeded => "Memory limit exceeded",
            Self::MaxTimeExceeded => "Maximum run time exceeded",
            Self::QueueEmpty => "Queue empty",
            Self::QueueEmptyFor => "Queue empty for the configured duration",
            Self::ReceivedRestartSignal => "Received restart signal",
            Self::TimedOut => "Job timed out",
        }
    }

    /// The exit status the worker process should exit with.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::MaxMemoryExceeded => Worker::EXIT_MEMORY_LIMIT,
            Self::TimedOut => Worker::EXIT_ERROR,
            _ => Worker::EXIT_SUCCESS,
        }
    }
}

impl fmt::Display for WorkerStopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.description())
    }
}

#[derive(Default)]
struct Signals {
    should_quit: AtomicBool,
    paused: AtomicBool,
    notify: Notify,
}

/// A handle for controlling a running worker from elsewhere (a signal
/// handler, another task, a test).
#[derive(Clone)]
pub struct WorkerHandle {
    signals: Arc<Signals>,
}

impl WorkerHandle {
    /// Ask the worker to stop once its current job is done.
    pub fn quit(&self) {
        self.signals.should_quit.store(true, Ordering::SeqCst);
        self.signals.notify.notify_waiters();
    }

    /// Pause the worker: it keeps running but processes no jobs.
    pub fn pause(&self) {
        self.signals.paused.store(true, Ordering::SeqCst);
    }

    /// Resume a paused worker.
    pub fn resume(&self) {
        self.signals.paused.store(false, Ordering::SeqCst);
        self.signals.notify.notify_waiters();
    }

    /// Determine if the worker was asked to stop.
    pub fn should_quit(&self) -> bool {
        self.signals.should_quit.load(Ordering::SeqCst)
    }

    /// Determine if the worker is paused.
    pub fn is_paused(&self) -> bool {
        self.signals.paused.load(Ordering::SeqCst)
    }
}

type MaintenanceCheck = Arc<dyn Fn() -> bool + Send + Sync>;

/// Processes jobs from the queue (Laravel's `Illuminate\Queue\Worker`).
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::Container;
/// use illuminate_queue::{Dispatchable, ShouldQueue, Worker, WorkerOptions, async_trait};
/// use illuminate_support::{Result, json};
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct SendNewsletter;
///
/// #[async_trait]
/// impl ShouldQueue for SendNewsletter {
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
/// SendNewsletter.dispatch().await?;
///
/// let worker = Worker::make();
/// let reason = worker
///     .daemon("array", "default", &WorkerOptions::new().stop_when_empty())
///     .await?;
///
/// assert_eq!(worker.jobs_processed(), 1);
/// assert_eq!(reason.description(), "Queue empty");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub struct Worker {
    manager: Arc<QueueManager>,
    name: String,
    cache: Option<CacheRepository>,
    failer: Option<Arc<dyn FailedJobProvider>>,
    signals: Arc<Signals>,
    jobs_processed: AtomicU64,
    last_job_processed_at: Mutex<Option<Instant>>,
    paused_queues: Mutex<Vec<String>>,
    down_for_maintenance: Option<MaintenanceCheck>,
    report_job_exceptions: bool,
}

impl Worker {
    /// The exit status of a worker that stopped normally.
    pub const EXIT_SUCCESS: i32 = 0;
    /// The exit status of a worker that stopped because of an error.
    pub const EXIT_ERROR: i32 = 1;
    /// The exit status of a worker that ran out of memory.
    pub const EXIT_MEMORY_LIMIT: i32 = 12;

    /// Create a worker for the given queue manager.
    pub fn new(manager: Arc<QueueManager>) -> Self {
        Self {
            manager,
            name: "default".to_string(),
            cache: None,
            failer: None,
            signals: Arc::new(Signals::default()),
            jobs_processed: AtomicU64::new(0),
            last_job_processed_at: Mutex::new(None),
            paused_queues: Mutex::new(Vec::new()),
            down_for_maintenance: None,
            report_job_exceptions: true,
        }
    }

    /// Create a worker for the container's queue manager, logging failed
    /// jobs to the container's failed job provider and using the default
    /// cache store (when one is configured) for restart and pause signals
    /// and `max_exceptions`.
    pub fn make() -> Self {
        let mut worker = Self::new(queue_manager()).with_failer(failer());
        if let Some(cache) = try_app::<CacheRepository>() {
            worker.cache = Some((*cache).clone());
        } else if let Ok(cache) = illuminate_cache::Cache::default_store() {
            worker.cache = Some(cache);
        }
        worker
    }

    /// Set the worker's name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Use the given cache for restart/pause signals and `max_exceptions`.
    pub fn with_cache(mut self, cache: CacheRepository) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Log failed jobs to the given provider.
    pub fn with_failer(mut self, failer: Arc<dyn FailedJobProvider>) -> Self {
        self.failer = Some(failer);
        self
    }

    /// Don't log failed jobs.
    pub fn without_failer(mut self) -> Self {
        self.failer = None;
        self
    }

    /// Determine whether the application is in maintenance mode with the
    /// given callback (jobs aren't processed then, unless `force`).
    pub fn down_for_maintenance_using(
        mut self,
        callback: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        self.down_for_maintenance = Some(Arc::new(callback));
        self
    }

    /// Don't report job exceptions to the exception handler.
    pub fn without_reporting_job_exceptions(mut self) -> Self {
        self.report_job_exceptions = false;
        self
    }

    /// The worker's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The queue manager.
    pub fn manager(&self) -> &Arc<QueueManager> {
        &self.manager
    }

    /// A handle to stop or pause the worker.
    pub fn handle(&self) -> WorkerHandle {
        WorkerHandle {
            signals: self.signals.clone(),
        }
    }

    /// The number of jobs processed by the current `daemon` run.
    pub fn jobs_processed(&self) -> u64 {
        self.jobs_processed.load(Ordering::SeqCst)
    }

    /// Stop gracefully when the process receives `SIGINT`, `SIGTERM` or
    /// `SIGQUIT` (Ctrl+C on other platforms). Requires a Tokio runtime.
    pub fn listen_for_signals(&self, connection_name: &str, queue: &str) {
        let handle = self.handle();
        let manager = self.manager.clone();
        let (connection_name, queue) = (connection_name.to_string(), queue.to_string());
        tokio::spawn(async move {
            wait_for_shutdown_signal().await;
            handle.quit();
            manager.events().dispatch(WorkerInterrupted {
                connection_name,
                queue,
            });
        });
    }

    // ------------------------------------------------------------------
    // Running
    // ------------------------------------------------------------------

    /// Listen to the given queues (comma separated, in priority order) in a
    /// loop, until something tells the worker to stop.
    pub async fn daemon(
        &self,
        connection_name: &str,
        queue: &str,
        options: &WorkerOptions,
    ) -> Result<WorkerStopReason> {
        let last_restart = self.timestamp_of_last_queue_restart().await;
        let started = Instant::now();
        self.jobs_processed.store(0, Ordering::SeqCst);
        *self.last_job_processed_at.lock().unwrap() = None;

        self.manager.events().dispatch(WorkerStarting {
            connection_name: connection_name.to_string(),
            queue: queue.to_string(),
            options: options.clone(),
        });

        loop {
            if !self.daemon_should_run(options, connection_name, queue) {
                let pause = if options.sleep.is_zero() {
                    Duration::from_secs(1)
                } else {
                    options.sleep
                };
                self.sleep(pause).await;
                if let Some(reason) = self
                    .stop_if_necessary(options, last_restart, started, false)
                    .await
                {
                    return Ok(self.stop(reason, connection_name, queue));
                }
                continue;
            }

            Container::get_instance().forget_scoped_instances();

            let connection = self.manager.connection(Some(connection_name))?;
            let job = self
                .get_next_job(&*connection, connection_name, queue)
                .await;
            let found = job.is_some();

            match job {
                Some(job) => {
                    self.jobs_processed.fetch_add(1, Ordering::SeqCst);
                    self.run_job(job, connection_name, options).await;
                    *self.last_job_processed_at.lock().unwrap() = Some(Instant::now());
                    if !options.rest.is_zero() {
                        self.sleep(options.rest).await;
                    }
                }
                None => {
                    self.manager.events().dispatch(WorkerIdle {
                        connection_name: connection_name.to_string(),
                        queue: queue.to_string(),
                    });
                    if !options.stop_when_empty {
                        self.sleep(options.sleep).await;
                    }
                }
            }

            if let Some(reason) = self
                .stop_if_necessary(options, last_restart, started, found)
                .await
            {
                return Ok(self.stop(reason, connection_name, queue));
            }
        }
    }

    /// Process the next job on the queue (`queue:work --once`), sleeping
    /// when there is none.
    pub async fn run_next_job(
        &self,
        connection_name: &str,
        queue: &str,
        options: &WorkerOptions,
    ) -> Result<()> {
        if !self.work_once(connection_name, queue, options).await? {
            self.sleep(options.sleep).await;
        }
        Ok(())
    }

    /// Process the next job on the queue, if there is one, returning
    /// whether a job was processed. Never sleeps.
    pub async fn work_once(
        &self,
        connection_name: &str,
        queue: &str,
        options: &WorkerOptions,
    ) -> Result<bool> {
        let connection = self.manager.connection(Some(connection_name))?;
        match self
            .get_next_job(&*connection, connection_name, queue)
            .await
        {
            Some(job) => {
                self.run_job(job, connection_name, options).await;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn daemon_should_run(
        &self,
        options: &WorkerOptions,
        connection_name: &str,
        queue: &str,
    ) -> bool {
        let down = !options.force
            && self
                .down_for_maintenance
                .as_ref()
                .is_some_and(|is_down| is_down());
        if down || self.signals.paused.load(Ordering::SeqCst) {
            return false;
        }
        self.manager.events().dispatch(Looping {
            connection_name: connection_name.to_string(),
            queue: queue.to_string(),
        });
        true
    }

    async fn stop_if_necessary(
        &self,
        options: &WorkerOptions,
        last_restart: Option<i64>,
        started: Instant,
        found_job: bool,
    ) -> Option<WorkerStopReason> {
        if self.signals.should_quit.load(Ordering::SeqCst) {
            return Some(WorkerStopReason::Interrupted);
        }
        if self.queue_should_restart(last_restart).await {
            return Some(WorkerStopReason::ReceivedRestartSignal);
        }
        if options.stop_when_empty && !found_job {
            return Some(WorkerStopReason::QueueEmpty);
        }
        if !options.stop_when_empty_for.is_zero() && !found_job {
            let since = self
                .last_job_processed_at
                .lock()
                .unwrap()
                .unwrap_or(started);
            if since.elapsed() >= options.stop_when_empty_for {
                return Some(WorkerStopReason::QueueEmptyFor);
            }
        }
        if !options.max_time.is_zero() && started.elapsed() >= options.max_time {
            return Some(WorkerStopReason::MaxTimeExceeded);
        }
        if options.max_jobs > 0 && self.jobs_processed.load(Ordering::SeqCst) >= options.max_jobs {
            return Some(WorkerStopReason::MaxJobsExceeded);
        }
        None
    }

    fn stop(
        &self,
        reason: WorkerStopReason,
        connection_name: &str,
        queue: &str,
    ) -> WorkerStopReason {
        self.manager.events().dispatch(WorkerStopping {
            status: reason.exit_code(),
            reason,
            jobs_processed: self.jobs_processed(),
            connection_name: connection_name.to_string(),
            queue: queue.to_string(),
        });
        reason
    }

    /// Sleep, waking up early when the worker is asked to quit.
    pub async fn sleep(&self, duration: Duration) {
        if duration.is_zero() {
            return;
        }
        let notified = self.signals.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.signals.should_quit.load(Ordering::SeqCst) {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(duration) => {}
            _ = notified => {}
        }
    }

    async fn get_next_job(
        &self,
        connection: &dyn Queue,
        connection_name: &str,
        queue: &str,
    ) -> Option<QueuedJob> {
        self.manager.events().dispatch(JobPopping {
            connection_name: connection_name.to_string(),
            queue: queue.to_string(),
        });

        let queues: Vec<String> = queue
            .split(',')
            .map(str::trim)
            .filter(|queue| !queue.is_empty())
            .map(String::from)
            .collect();

        let paused = self.get_paused_queues(connection_name, &queues).await;
        self.raise_paused_queue_events(connection_name, &paused);

        for (index, queue) in queues.iter().enumerate() {
            if paused.contains(queue) {
                continue;
            }
            match connection.pop_at(Some(queue), index).await {
                Ok(Some(job)) => {
                    self.manager.events().dispatch(JobPopped {
                        connection_name: connection_name.to_string(),
                        job: job.clone(),
                    });
                    return Some(job);
                }
                Ok(None) => continue,
                Err(error) => {
                    crate::report(&error);
                    self.sleep(Duration::from_secs(1)).await;
                    return None;
                }
            }
        }
        None
    }

    async fn get_paused_queues(&self, connection_name: &str, queues: &[String]) -> Vec<String> {
        if !self.manager.is_pausable() || self.cache.is_none() {
            return Vec::new();
        }
        self.manager
            .get_paused_queues(connection_name, queues)
            .await
            .unwrap_or_default()
    }

    fn raise_paused_queue_events(&self, connection_name: &str, paused: &[String]) {
        let mut previously = self.paused_queues.lock().unwrap();
        for queue in paused.iter().filter(|queue| !previously.contains(queue)) {
            self.manager.events().dispatch(WorkerQueuePaused {
                connection_name: connection_name.to_string(),
                queue: queue.clone(),
            });
        }
        for queue in previously.iter().filter(|queue| !paused.contains(queue)) {
            self.manager.events().dispatch(WorkerQueueResumed {
                connection_name: connection_name.to_string(),
                queue: queue.clone(),
            });
        }
        *previously = paused.to_vec();
    }

    async fn run_job(&self, job: QueuedJob, connection_name: &str, options: &WorkerOptions) {
        if let Err(error) = self.process(connection_name, job, options).await
            && self.report_job_exceptions
        {
            crate::report(&error);
        }
    }

    /// Process the given job: fire it, then handle the outcome — release it
    /// for another attempt, or fail it for good.
    pub async fn process(
        &self,
        connection_name: &str,
        job: QueuedJob,
        options: &WorkerOptions,
    ) -> Result<()> {
        if let Some(failer) = &self.failer {
            job.log_failures_to(failer.clone());
        }

        let outcome = self.attempt(connection_name, &job, options).await;

        let (result, exception) = match outcome {
            Ok(()) => (Ok(()), None),
            Err(error) => {
                let error = Arc::new(error);
                self.handle_job_exception(connection_name, &job, options, &error)
                    .await;
                (Err(error.clone()), Some(error))
            }
        };

        self.manager.events().dispatch(JobAttempted {
            connection_name: connection_name.to_string(),
            job,
            exception,
        });

        result.map_err(unshare)
    }

    async fn attempt(
        &self,
        connection_name: &str,
        job: &QueuedJob,
        options: &WorkerOptions,
    ) -> Result<()> {
        self.manager.events().dispatch(JobProcessing {
            connection_name: connection_name.to_string(),
            job: job.clone(),
        });

        self.mark_job_as_failed_if_already_exceeds_max_attempts(job, options.max_tries)
            .await?;

        if job.is_deleted() {
            self.manager.events().dispatch(JobProcessed {
                connection_name: connection_name.to_string(),
                job: job.clone(),
                duration: None,
            });
            return Ok(());
        }

        let started = Instant::now();
        let timeout = job
            .timeout()
            .map(Duration::from_secs)
            .unwrap_or(options.timeout);

        let fired = if timeout.is_zero() {
            job.fire().await
        } else {
            match tokio::time::timeout(timeout, job.fire()).await {
                Ok(result) => result,
                Err(_) => return self.handle_timeout(connection_name, job, timeout).await,
            }
        };
        fired?;

        self.manager.events().dispatch(JobProcessed {
            connection_name: connection_name.to_string(),
            job: job.clone(),
            duration: Some(started.elapsed()),
        });

        if job.is_released() && !job.is_deleted() {
            self.manager.events().dispatch(JobReleased {
                connection_name: connection_name.to_string(),
                job: job.clone(),
            });
        }

        Ok(())
    }

    async fn handle_timeout(
        &self,
        connection_name: &str,
        job: &QueuedJob,
        timeout: Duration,
    ) -> Result<()> {
        let error = Arc::new(Error::new(TimeoutExceededException::for_job(
            job.resolve_name(),
        )));

        self.manager.events().dispatch(JobTimedOut {
            connection_name: connection_name.to_string(),
            job: job.clone(),
            timeout,
        });

        if job.should_fail_on_timeout() {
            job.fail_with(error.clone()).await?;
        }

        Err(unshare(error))
    }

    async fn handle_job_exception(
        &self,
        connection_name: &str,
        job: &QueuedJob,
        options: &WorkerOptions,
        error: &Arc<Error>,
    ) {
        if !job.has_failed() {
            if let Err(failure) = self
                .mark_job_as_failed_if_will_exceed_max_attempts(job, options.max_tries, error)
                .await
            {
                crate::report(&failure);
            }
            if let Err(failure) = self
                .mark_job_as_failed_if_will_exceed_max_exceptions(job, error)
                .await
            {
                crate::report(&failure);
            }
        }

        self.manager.events().dispatch(JobExceptionOccurred {
            connection_name: connection_name.to_string(),
            job: job.clone(),
            exception: error.clone(),
        });

        if !job.is_deleted() && !job.is_released() && !job.has_failed() {
            let backoff = self.calculate_backoff(job, options);
            if let Err(failure) = job.release(backoff).await {
                crate::report(&failure);
            }
            self.manager.events().dispatch(JobReleasedAfterException {
                connection_name: connection_name.to_string(),
                job: job.clone(),
                backoff,
                exception: error.clone(),
            });
        }
    }

    async fn mark_job_as_failed_if_already_exceeds_max_attempts(
        &self,
        job: &QueuedJob,
        max_tries: u32,
    ) -> Result<()> {
        let max_tries = job.max_tries().unwrap_or(max_tries);

        match job.retry_until() {
            Some(until) if Carbon::now().timestamp() <= until => return Ok(()),
            None if max_tries == 0 || job.attempts() <= max_tries => return Ok(()),
            _ => {}
        }

        let error = Arc::new(Error::new(MaxAttemptsExceededException::for_job(
            job.resolve_name(),
        )));
        job.fail_with(error.clone()).await?;
        Err(unshare(error))
    }

    async fn mark_job_as_failed_if_will_exceed_max_attempts(
        &self,
        job: &QueuedJob,
        max_tries: u32,
        error: &Arc<Error>,
    ) -> Result<()> {
        let max_tries = job.max_tries().unwrap_or(max_tries);

        let should_fail = match job.retry_until() {
            Some(until) => until <= Carbon::now().timestamp(),
            None => max_tries > 0 && job.attempts() >= max_tries,
        };

        if should_fail {
            job.fail_with(error.clone()).await?;
        }
        Ok(())
    }

    async fn mark_job_as_failed_if_will_exceed_max_exceptions(
        &self,
        job: &QueuedJob,
        error: &Arc<Error>,
    ) -> Result<()> {
        let (Some(cache), Some(uuid), Some(max_exceptions)) =
            (&self.cache, job.uuid(), job.max_exceptions())
        else {
            return Ok(());
        };

        let key = format!("job-exceptions:{uuid}");
        if cache.get(&key).await?.is_none() {
            cache.put(&key, 0, 24 * 60 * 60).await?;
        }

        if i64::from(max_exceptions) <= cache.increment(&key).await? {
            cache.forget(&key).await?;
            job.fail_with(error.clone()).await?;
        }
        Ok(())
    }

    /// The number of seconds to wait before retrying the job.
    pub fn calculate_backoff(&self, job: &QueuedJob, options: &WorkerOptions) -> u64 {
        let backoff: Vec<u64> = match job.backoff() {
            Some(backoff) => backoff
                .split(',')
                .filter_map(|value| value.trim().parse().ok())
                .collect(),
            None => options.backoff.clone(),
        };
        let index = job.attempts().saturating_sub(1) as usize;
        backoff
            .get(index)
            .or_else(|| backoff.last())
            .copied()
            .unwrap_or(0)
    }

    async fn queue_should_restart(&self, last_restart: Option<i64>) -> bool {
        if !self.manager.is_restartable() {
            return false;
        }
        self.timestamp_of_last_queue_restart().await != last_restart
    }

    async fn timestamp_of_last_queue_restart(&self) -> Option<i64> {
        if !self.manager.is_restartable() {
            return None;
        }
        let cache = self.cache.as_ref()?;
        cache
            .get("illuminate:queue:restart")
            .await
            .ok()
            .flatten()
            .and_then(|value| value.as_i64())
    }
}

impl fmt::Debug for Worker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Worker")
            .field("name", &self.name)
            .field("jobs_processed", &self.jobs_processed())
            .finish_non_exhaustive()
    }
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let streams = [
        signal(SignalKind::interrupt()),
        signal(SignalKind::terminate()),
        signal(SignalKind::quit()),
    ];
    let mut streams: Vec<_> = streams
        .into_iter()
        .filter_map(|stream| stream.ok())
        .collect();
    if streams.is_empty() {
        let _ = tokio::signal::ctrl_c().await;
        return;
    }
    let waits = streams
        .iter_mut()
        .map(|stream| Box::pin(async move { stream.recv().await }));
    futures::future::select_all(waits).await;
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
