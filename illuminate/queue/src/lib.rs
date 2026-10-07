//! # Illuminate Queue
//!
//! Laravel's queues and command bus: move time consuming work — sending
//! email, processing uploads, talking to slow APIs — out of the request and
//! into the background.
//!
//! ## Writing jobs
//!
//! A job is a serializable struct implementing [`ShouldQueue`]:
//!
//! ```
//! use illuminate_queue::{ShouldQueue, async_trait};
//! use illuminate_support::Result;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize)]
//! pub struct ProcessPodcast {
//!     pub podcast_id: u64,
//! }
//!
//! #[async_trait]
//! impl ShouldQueue for ProcessPodcast {
//!     async fn handle(&self) -> Result<()> {
//!         // Process the uploaded podcast...
//!         Ok(())
//!     }
//! }
//! ```
//!
//! ## Dispatching jobs
//!
//! Every job can [`dispatch`](Dispatchable::dispatch) itself. Configure the
//! [`PendingDispatch`] and `.await` it:
//!
//! ```
//! # use std::sync::Arc;
//! # use illuminate_config::Repository;
//! # use illuminate_container::Container;
//! # use illuminate_queue::{ShouldQueue, async_trait};
//! # use illuminate_support::{Result, json};
//! # use serde::{Deserialize, Serialize};
//! # #[derive(Serialize, Deserialize)]
//! # pub struct ProcessPodcast { pub podcast_id: u64 }
//! # #[async_trait]
//! # impl ShouldQueue for ProcessPodcast {
//! #     async fn handle(&self) -> Result<()> { Ok(()) }
//! # }
//! use illuminate_queue::{Bus, Dispatchable, Queue, Worker, WorkerOptions};
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container.clone());
//! container.instance(Repository::new(json!({
//!     "queue": {
//!         "default": "array",
//!         "connections": {"array": {"driver": "array", "queue": "default"}},
//!     },
//! })));
//!
//! ProcessPodcast { podcast_id: 1 }.dispatch().await?;
//! ProcessPodcast { podcast_id: 2 }.dispatch().on_queue("podcasts").delay(0).await?;
//!
//! assert_eq!(Queue::size(Some("default")).await?, 1);
//!
//! // Later, in a `queue:work` process...
//! let worker = Worker::make();
//! worker.daemon("array", "podcasts,default", &WorkerOptions::new().stop_when_empty()).await?;
//! assert_eq!(worker.jobs_processed(), 2);
//! # Ok::<(), illuminate_support::Error>(())
//! # }).unwrap();
//! ```
//!
//! [`Bus::chain`] runs jobs one after another and [`Bus::batch`] runs a
//! group of jobs with completion callbacks; job [middleware] wrap logic
//! around jobs (rate limiting, preventing overlaps, throttling
//! exceptions); [`Worker`] processes jobs with Laravel's semantics
//! (attempts, backoff, timeouts, failed jobs); and [`Queue::fake`] /
//! [`Bus::fake`] make all of it easy to test.
//!
//! ## Connections
//!
//! The `database` ([`DatabaseQueue`]), `redis` ([`RedisQueue`]), `sync`,
//! `array` (in memory), `deferred`, `background`, `failover` and `null`
//! drivers ship with the queue. Other drivers are registered with
//! [`QueueManager::extend`].
//!
//! The `database` driver keeps jobs in the `jobs` table, failed jobs are
//! logged to the `failed_jobs` table ([`DatabaseUuidFailedJobProvider`],
//! `queue.failed.driver = "database-uuids"`) and batches live in the
//! `job_batches` table ([`DatabaseBatchRepository`], `queue.batching`), all
//! through the container's `illuminate_database::DatabaseManager`.
//!
//! The `sqs` driver ([`SqsQueue`]) sends jobs to Amazon SQS and the
//! `beanstalkd` driver ([`BeanstalkdQueue`]) puts them into Beanstalkd
//! tubes. Failed jobs and batches may also live in DynamoDB
//! ([`DynamoDbFailedJobProvider`], `queue.failed.driver = "dynamodb"`;
//! [`DynamoBatchRepository`], `queue.batching.driver = "dynamodb"`). The AWS
//! APIs are called through the `Http` client, so `Http::fake()` works in
//! tests.
//!
//! The `redis` driver keeps jobs on a connection from `database.redis`,
//! exactly like Laravel's `RedisQueue` (`queues:{name}` lists, with
//! `:delayed` and `:reserved` sorted sets):
//!
//! ```json
//! "redis": {
//!     "driver": "redis",
//!     "connection": "default",
//!     "queue": "default",
//!     "retry_after": 90,
//!     "block_for": null,
//!     "after_commit": false
//! }
//! ```

pub mod bus;
pub mod callbacks;
pub mod closure;
pub mod context;
pub mod contracts;
pub mod deferred;
pub mod delay;
pub mod drivers;
pub mod envelope;
pub mod events;
pub mod exceptions;
pub mod facades;
pub mod failed;
mod handler;
pub mod job;
pub mod manager;
pub mod middleware;
pub mod payload;
pub mod provider;
pub mod queued_job;
pub mod registry;
pub mod routes;
pub mod testing;
pub mod worker;

pub use async_trait::async_trait;

pub use bus::{
    Batch, BatchItem, BatchRecord, BatchRepository, DatabaseBatchRepository, DebounceFor,
    DebounceLock, Dispatchable, Dispatcher, DynamoBatchRepository, InMemoryBatchRepository,
    PendingBatch, PendingChain, PendingDispatch, QueueingDispatcher, UniqueLock,
    UpdatedBatchJobCounts, dispatch, dispatch_sync,
};
pub use callbacks::CallbackRef;
pub use closure::{CallQueuedClosure, dispatch_closure};
pub use context::{InteractsWithQueue, current_job, with_job};
pub use contracts::{
    QueueConnector, TransactionCallback, TransactionManager, enqueue, enqueue_using, enqueue_with,
};
pub use deferred::DeferredCallbacks;
pub use delay::IntoDelay;
pub use drivers::{
    ArrayQueue, BackgroundQueue, Beanstalkd, BeanstalkdQueue, DatabaseQueue, DeferredQueue,
    FailoverQueue, NullQueue, OverflowStorage, RedisQueue, SqsClient, SqsQueue, SyncQueue,
};
pub use envelope::{Envelope, JobEncrypter, SerializedJob};
pub use exceptions::{
    InvalidPayloadException, ManuallyFailedException, MaxAttemptsExceededException,
    MissingClosureException, SharedError, TimeoutExceededException, UnknownJobException,
    UnsupportedOperationException,
};
pub use facades::{Bus, Queue};
pub use failed::{
    DatabaseUuidFailedJobProvider, DynamoDbFailedJobProvider, FailedJob, FailedJobProvider,
    FileFailedJobProvider, InMemoryFailedJobProvider, NullFailedJobProvider,
};
pub use job::{IntoFailure, ShouldQueue};
pub use manager::QueueManager;
pub use middleware::JobMiddleware;
pub use payload::{CALL_QUEUED_HANDLER, create_payload};
pub use provider::{BusServiceProvider, QueueServiceProvider};
pub use queued_job::{JobBackend, NullBackend, QueuedJob};
pub use registry::{JobRegistration, JobRegistry};
pub use routes::QueueRoutes;
pub use testing::{BusFake, JobTypes, QueueFake};
pub use worker::{Worker, WorkerHandle, WorkerOptions, WorkerStopReason};

/// Everything you need to write and dispatch jobs, in one import.
pub mod prelude {
    pub use crate::middleware::{JobMiddleware, Next, RateLimitsJobs};
    pub use crate::{
        Batch, Bus, DebounceFor, Dispatchable, InteractsWithQueue, Queue, ShouldQueue, async_trait,
        dispatch, dispatch_sync,
    };
}

/// Re-exports used by the framework's macros. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    pub use inventory;
}

/// Report an error to the application's exception handler (or stderr when
/// none is bound).
pub(crate) fn report(error: &illuminate_support::Error) {
    match illuminate_container::try_app::<dyn illuminate_http::ExceptionHandler>() {
        Some(handler) => {
            if handler.should_report(error) {
                handler.report(error);
            }
        }
        None => eprintln!("[queue] {error:?}"),
    }
}
