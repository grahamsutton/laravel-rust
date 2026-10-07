//! Queue events.
//!
//! The queue and its workers fire events as jobs move through them. Listen
//! with the `Queue` facade (`Queue::before`, `Queue::after`,
//! `Queue::failing`, `Queue::looping`, ...) or for any event with
//! [`Queue::listen`](crate::Queue::listen):
//!
//! ```
//! use std::sync::{Arc, Mutex};
//! use illuminate_container::Container;
//! use illuminate_queue::{Queue, events::JobFailed};
//!
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container);
//!
//! let failures = Arc::new(Mutex::new(Vec::new()));
//! let log = failures.clone();
//! Queue::failing(move |event: &JobFailed| {
//!     log.lock().unwrap().push(event.job.resolve_name());
//! });
//! ```
//!
//! Listeners are plain closures, called synchronously. The console's
//! `queue:work` command uses them to print each job's progress.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use illuminate_container::try_app;
use illuminate_support::Error;

use crate::bus::batch::Batch;
use crate::envelope::Envelope;
use crate::failed::FailedJob;
use crate::manager::QueueManager;
use crate::queued_job::QueuedJob;
use crate::worker::{WorkerOptions, WorkerStopReason};

/// A job is about to be pushed onto a queue.
#[derive(Debug, Clone)]
pub struct JobQueueing {
    /// The connection name.
    pub connection_name: String,
    /// The queue name.
    pub queue: String,
    /// The job.
    pub job: Envelope,
    /// The job's payload.
    pub payload: String,
    /// The job's delay.
    pub delay: Option<Duration>,
}

/// A job was pushed onto a queue.
#[derive(Debug, Clone)]
pub struct JobQueued {
    /// The connection name.
    pub connection_name: String,
    /// The queue name.
    pub queue: String,
    /// The driver's id for the job.
    pub id: Option<String>,
    /// The job.
    pub job: Envelope,
    /// The job's payload.
    pub payload: String,
    /// The job's delay.
    pub delay: Option<Duration>,
}

/// A worker is about to pop a job.
#[derive(Debug, Clone)]
pub struct JobPopping {
    /// The connection name.
    pub connection_name: String,
    /// The queues being popped (comma separated).
    pub queue: String,
}

/// A worker popped a job.
#[derive(Debug, Clone)]
pub struct JobPopped {
    /// The connection name.
    pub connection_name: String,
    /// The job.
    pub job: QueuedJob,
}

/// A job is about to be processed.
#[derive(Debug, Clone)]
pub struct JobProcessing {
    /// The connection name.
    pub connection_name: String,
    /// The job.
    pub job: QueuedJob,
}

/// A job was processed.
#[derive(Debug, Clone)]
pub struct JobProcessed {
    /// The connection name.
    pub connection_name: String,
    /// The job.
    pub job: QueuedJob,
    /// How long the job ran.
    pub duration: Option<Duration>,
}

/// A job threw an exception.
#[derive(Debug, Clone)]
pub struct JobExceptionOccurred {
    /// The connection name.
    pub connection_name: String,
    /// The job.
    pub job: QueuedJob,
    /// The exception.
    pub exception: Arc<Error>,
}

/// A job threw an exception and was released back onto the queue.
#[derive(Debug, Clone)]
pub struct JobReleasedAfterException {
    /// The connection name.
    pub connection_name: String,
    /// The job.
    pub job: QueuedJob,
    /// The number of seconds before the job is retried.
    pub backoff: u64,
    /// The exception.
    pub exception: Arc<Error>,
}

/// A job released itself back onto the queue.
#[derive(Debug, Clone)]
pub struct JobReleased {
    /// The connection name.
    pub connection_name: String,
    /// The job.
    pub job: QueuedJob,
}

/// A job failed.
#[derive(Debug, Clone)]
pub struct JobFailed {
    /// The connection name.
    pub connection_name: String,
    /// The job.
    pub job: QueuedJob,
    /// The exception that failed the job.
    pub exception: Arc<Error>,
}

/// A job attempt finished (successfully or not).
#[derive(Debug, Clone)]
pub struct JobAttempted {
    /// The connection name.
    pub connection_name: String,
    /// The job.
    pub job: QueuedJob,
    /// The exception the attempt ended with.
    pub exception: Option<Arc<Error>>,
}

impl JobAttempted {
    /// Determine if the attempt ended with an exception.
    pub fn exception_occurred(&self) -> bool {
        self.exception.is_some()
    }

    /// Determine if the attempt was successful.
    pub fn successful(&self) -> bool {
        self.exception.is_none()
    }
}

/// A job ran longer than its timeout.
#[derive(Debug, Clone)]
pub struct JobTimedOut {
    /// The connection name.
    pub connection_name: String,
    /// The job.
    pub job: QueuedJob,
    /// The timeout that was exceeded.
    pub timeout: Duration,
}

/// A failed job is being retried.
#[derive(Debug, Clone)]
pub struct JobRetryRequested {
    /// The failed job.
    pub job: FailedJob,
}

/// A worker is about to look for a job.
#[derive(Debug, Clone)]
pub struct Looping {
    /// The connection name.
    pub connection_name: String,
    /// The queues being worked.
    pub queue: String,
}

/// A worker started.
#[derive(Debug, Clone)]
pub struct WorkerStarting {
    /// The connection name.
    pub connection_name: String,
    /// The queues being worked.
    pub queue: String,
    /// The worker's options.
    pub options: WorkerOptions,
}

/// A worker is stopping.
#[derive(Debug, Clone)]
pub struct WorkerStopping {
    /// The exit status.
    pub status: i32,
    /// Why the worker stopped.
    pub reason: WorkerStopReason,
    /// The number of jobs the worker processed.
    pub jobs_processed: u64,
    /// The connection name.
    pub connection_name: String,
    /// The queues being worked.
    pub queue: String,
}

/// A worker found no job to process.
#[derive(Debug, Clone)]
pub struct WorkerIdle {
    /// The connection name.
    pub connection_name: String,
    /// The queues being worked.
    pub queue: String,
}

/// A worker received a signal to stop.
#[derive(Debug, Clone)]
pub struct WorkerInterrupted {
    /// The connection name.
    pub connection_name: String,
    /// The queues being worked.
    pub queue: String,
}

/// A worker noticed one of its queues was paused.
#[derive(Debug, Clone)]
pub struct WorkerQueuePaused {
    /// The connection name.
    pub connection_name: String,
    /// The queue.
    pub queue: String,
}

/// A worker noticed one of its queues was resumed.
#[derive(Debug, Clone)]
pub struct WorkerQueueResumed {
    /// The connection name.
    pub connection_name: String,
    /// The queue.
    pub queue: String,
}

/// A queue was paused.
#[derive(Debug, Clone)]
pub struct QueuePaused {
    /// The connection name.
    pub connection_name: String,
    /// The queue.
    pub queue: String,
    /// How long the queue is paused for, in seconds.
    pub ttl: Option<u64>,
}

/// A queue was resumed.
#[derive(Debug, Clone)]
pub struct QueueResumed {
    /// The connection name.
    pub connection_name: String,
    /// The queue.
    pub queue: String,
}

/// Every queue was paused.
#[derive(Debug, Clone)]
pub struct QueuesPaused;

/// Every queue was resumed.
#[derive(Debug, Clone)]
pub struct QueuesResumed;

/// A queue has more jobs than its monitored threshold.
#[derive(Debug, Clone)]
pub struct QueueBusy {
    /// The connection name.
    pub connection_name: String,
    /// The queue.
    pub queue: String,
    /// The number of jobs on the queue.
    pub size: u64,
}

/// A failover connection could not push to one of its connections.
#[derive(Debug, Clone)]
pub struct QueueFailedOver {
    /// The failover connection's name.
    pub connection_name: String,
    /// The connection that failed.
    pub failed_connection: String,
    /// The error.
    pub exception: Arc<Error>,
}

/// A unique job was not dispatched because its lock is held.
#[derive(Debug, Clone)]
pub struct UniqueJobSkipped {
    /// The job.
    pub job: Envelope,
}

/// A batch was dispatched.
#[derive(Debug, Clone)]
pub struct BatchDispatched {
    /// The batch.
    pub batch: Batch,
}

/// A batch was cancelled.
#[derive(Debug, Clone)]
pub struct BatchCanceled {
    /// The batch.
    pub batch: Batch,
    /// The exception that cancelled the batch.
    pub exception: Option<Arc<Error>>,
}

/// Every job of a batch completed successfully.
#[derive(Debug, Clone)]
pub struct BatchFinished {
    /// The batch.
    pub batch: Batch,
}

macro_rules! queue_events {
    ($($event:ident),* $(,)?) => {
        /// Any queue event.
        #[derive(Debug, Clone)]
        pub enum QueueEvent {
            $(
                #[doc = concat!("A [`", stringify!($event), "`] event.")]
                $event($event),
            )*
        }

        impl QueueEvent {
            /// The event's name (`"JobProcessed"`, ...).
            pub fn name(&self) -> &'static str {
                match self {
                    $(QueueEvent::$event(_) => stringify!($event),)*
                }
            }
        }

        $(
            impl From<$event> for QueueEvent {
                fn from(event: $event) -> Self {
                    QueueEvent::$event(event)
                }
            }

            impl QueueEventType for $event {
                fn from_event(event: &QueueEvent) -> Option<&Self> {
                    match event {
                        QueueEvent::$event(event) => Some(event),
                        #[allow(unreachable_patterns)]
                        _ => None,
                    }
                }
            }
        )*
    };
}

/// A specific kind of queue event.
pub trait QueueEventType: Into<QueueEvent> + Clone + Send + Sync + 'static {
    /// Extract this kind of event from any queue event.
    fn from_event(event: &QueueEvent) -> Option<&Self>;
}

queue_events!(
    JobQueueing,
    JobQueued,
    JobPopping,
    JobPopped,
    JobProcessing,
    JobProcessed,
    JobExceptionOccurred,
    JobReleasedAfterException,
    JobReleased,
    JobFailed,
    JobAttempted,
    JobTimedOut,
    JobRetryRequested,
    Looping,
    WorkerStarting,
    WorkerStopping,
    WorkerIdle,
    WorkerInterrupted,
    WorkerQueuePaused,
    WorkerQueueResumed,
    QueuePaused,
    QueueResumed,
    QueuesPaused,
    QueuesResumed,
    QueueBusy,
    QueueFailedOver,
    UniqueJobSkipped,
    BatchDispatched,
    BatchCanceled,
    BatchFinished,
);

type Listener = Arc<dyn Fn(&QueueEvent) + Send + Sync>;

/// The queue's event listeners.
#[derive(Default)]
pub struct QueueEvents {
    listeners: RwLock<Vec<Listener>>,
}

impl QueueEvents {
    /// Create an empty set of listeners.
    pub fn new() -> Self {
        Self::default()
    }

    /// Listen for events of type `E`.
    pub fn listen<E: QueueEventType>(&self, callback: impl Fn(&E) + Send + Sync + 'static) {
        self.listen_all(move |event| {
            if let Some(event) = E::from_event(event) {
                callback(event);
            }
        });
    }

    /// Listen for every queue event.
    pub fn listen_all(&self, callback: impl Fn(&QueueEvent) + Send + Sync + 'static) {
        self.listeners.write().unwrap().push(Arc::new(callback));
    }

    /// Fire an event.
    pub fn dispatch(&self, event: impl Into<QueueEvent>) {
        let listeners = self.listeners.read().unwrap().clone();
        if listeners.is_empty() {
            return;
        }
        let event = event.into();
        for listener in listeners {
            listener(&event);
        }
    }

    /// Determine if any listeners are registered.
    pub fn has_listeners(&self) -> bool {
        !self.listeners.read().unwrap().is_empty()
    }

    /// Remove every listener.
    pub fn flush(&self) {
        self.listeners.write().unwrap().clear();
    }
}

/// Fire an event through the queue manager bound in the container.
pub(crate) fn dispatch(event: impl Into<QueueEvent>) {
    if let Some(manager) = try_app::<QueueManager>() {
        manager.events().dispatch(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn typed_listeners_only_see_their_events() {
        let events = QueueEvents::new();
        let seen = Arc::new(Mutex::new(Vec::new()));

        let log = seen.clone();
        events.listen(move |event: &QueuePaused| log.lock().unwrap().push(event.queue.clone()));
        let all = seen.clone();
        events.listen_all(move |event| all.lock().unwrap().push(event.name().to_string()));

        events.dispatch(QueuePaused {
            connection_name: "redis".into(),
            queue: "emails".into(),
            ttl: None,
        });
        events.dispatch(QueuesResumed);

        assert_eq!(
            *seen.lock().unwrap(),
            vec!["emails", "QueuePaused", "QueuesResumed"]
        );
        assert!(events.has_listeners());
        events.flush();
        assert!(!events.has_listeners());
    }
}
