//! Queued closures.

use std::future::Future;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use illuminate_support::error::RuntimeException;
use illuminate_support::{Error, Result};

use crate::bus::pending_dispatch::PendingDispatch;
use crate::callbacks::{self, ChainCatchCallback, QueuedClosure};
use crate::exceptions::MissingClosureException;
use crate::job::ShouldQueue;

/// A closure on the queue (Laravel's `CallQueuedClosure`).
///
/// Rust closures can't be serialized, so a queued closure stays in the
/// memory of the process that dispatched it: it runs on the `sync`,
/// `deferred`, `background` and `array` connections, or on a worker in the
/// same process. Reach for a job when the work must cross processes.
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicBool, Ordering};
/// use illuminate_container::Container;
/// use illuminate_queue::{CallQueuedClosure, Dispatchable};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// let published = Arc::new(AtomicBool::new(false));
///
/// let flag = published.clone();
/// CallQueuedClosure::new(move || {
///     let flag = flag.clone();
///     async move {
///         flag.store(true, Ordering::SeqCst);
///         Ok(())
///     }
/// })
/// .name("Publish Podcast")
/// .dispatch()
/// .await?;
///
/// // The default connection is `sync`, so the closure already ran.
/// assert!(published.load(Ordering::SeqCst));
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallQueuedClosure {
    /// The id of the closure in this process.
    pub id: String,
    /// The name shown for the closure.
    #[serde(default)]
    pub name: Option<String>,
    /// The ids of the closure's `catch` callbacks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub catch_callbacks: Vec<String>,
}

impl CallQueuedClosure {
    /// Queue the given closure.
    pub fn new<F, Fut>(closure: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let closure: QueuedClosure = Arc::new(move || Box::pin(closure()));
        Self {
            id: callbacks::store(closure),
            name: None,
            catch_callbacks: Vec::new(),
        }
    }

    /// Queue a closure that runs at most once — handy for work that
    /// consumes what it captured (a queued event listener and its event).
    /// If the job is retried, it fails: the closure has already been used.
    pub fn once<F, Fut>(closure: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let slot = Arc::new(Mutex::new(Some(closure)));
        Self::new(move || {
            let closure = slot.lock().unwrap().take();
            async move {
                match closure {
                    Some(closure) => closure().await,
                    None => Err(RuntimeException::new(
                        "This queued closure may only run once and has already run.",
                    )
                    .into()),
                }
            }
        })
    }

    /// Name the closure (shown by `queue:work` and in failed jobs).
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Run the callback if the closure fails for good.
    pub fn catch<F, Fut>(mut self, callback: F) -> Self
    where
        F: Fn(Arc<Error>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let callback: ChainCatchCallback = Arc::new(move |error| Box::pin(callback(error)));
        self.catch_callbacks.push(callbacks::store(callback));
        self
    }

    fn forget(&self) {
        callbacks::forget(&self.id);
        for id in &self.catch_callbacks {
            callbacks::forget(id);
        }
    }
}

#[async_trait]
impl ShouldQueue for CallQueuedClosure {
    async fn handle(&self) -> Result<()> {
        let Some(closure) = callbacks::get::<QueuedClosure>(&self.id) else {
            return Err(MissingClosureException {
                id: self.id.clone(),
            }
            .into());
        };
        closure().await?;
        self.forget();
        Ok(())
    }

    async fn failed(&self, error: &Error) -> Result<()> {
        let error = Arc::new(Error::msg(format!("{error:#}")));
        for id in &self.catch_callbacks {
            if let Some(callback) = callbacks::get::<ChainCatchCallback>(id) {
                callback(error.clone()).await?;
            }
        }
        self.forget();
        Ok(())
    }

    fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| "Closure".to_string())
    }

    fn job_name() -> &'static str {
        "Illuminate\\Queue\\CallQueuedClosure"
    }
}

crate::register_job!(CallQueuedClosure);

/// Queue a closure: `dispatch_closure(|| async { ... Ok(()) }).await?`.
pub fn dispatch_closure<F, Fut>(closure: F) -> PendingDispatch
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    PendingDispatch::new(CallQueuedClosure::new(closure))
}
