//! The queue and bus service providers.

use std::sync::Arc;

use illuminate_config::Repository as Config;
use illuminate_container::{Container, ServiceProvider};

use crate::bus::dispatcher::{Dispatcher, QueueingDispatcher};
use crate::bus::repository::{BatchRepository, InMemoryBatchRepository};
use crate::deferred::DeferredCallbacks;
use crate::failed::{FailedJobProvider, make_failer};
use crate::manager::{QueueManager, make_manager};
use crate::middleware::JobRateLimiters;

/// Registers the [`QueueManager`] (the `Queue` facade), the failed job
/// provider, the job rate limiters and the deferred callbacks.
///
/// Configuration is read from the `queue` key: `queue.default`,
/// `queue.connections.*` and `queue.failed`.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_queue::{QueueManager, QueueServiceProvider};
/// use illuminate_support::json;
///
/// let container = Container::new();
/// container.instance(Repository::new(json!({
///     "queue": {"default": "sync", "connections": {"sync": {"driver": "sync"}}},
/// })));
/// QueueServiceProvider.register(&container);
///
/// assert_eq!(container.make::<QueueManager>().get_default_driver(), "sync");
/// ```
pub struct QueueServiceProvider;

impl ServiceProvider for QueueServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<QueueManager>(make_manager);

        app.singleton::<dyn FailedJobProvider>(|container| {
            let config = container
                .try_make::<Config>()
                .unwrap_or_else(|_| Arc::new(Config::empty()));
            make_failer(&config)
        });

        app.singleton::<JobRateLimiters>(|_| Arc::new(JobRateLimiters::new()));
        app.singleton::<DeferredCallbacks>(|_| Arc::new(DeferredCallbacks::new()));
    }
}

/// Registers the bus [`Dispatcher`] (the `Bus` facade) and the batch
/// repository.
///
/// The repository is kept in memory; the database component replaces it
/// with one backed by the `queue.batching.table` table.
pub struct BusServiceProvider;

impl ServiceProvider for BusServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<dyn QueueingDispatcher>(|_| Arc::new(Dispatcher::new()));
        app.singleton_if::<dyn BatchRepository>(|_| Arc::new(InMemoryBatchRepository::new()));
    }
}
