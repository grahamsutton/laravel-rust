//! The queue and bus service providers.

use std::sync::Arc;

use illuminate_container::{Container, ServiceProvider};

use crate::bus::database::make_batch_repository;
use crate::bus::dispatcher::{Dispatcher, QueueingDispatcher};
use crate::bus::repository::BatchRepository;
use crate::deferred::DeferredCallbacks;
use crate::failed::{FailedJobProvider, make_container_failer};
use crate::manager::{QueueManager, make_manager};
use crate::middleware::JobRateLimiters;

/// Registers the [`QueueManager`] (the `Queue` facade), the failed job
/// provider, the job rate limiters and the deferred callbacks.
///
/// Configuration is read from the `queue` key: `queue.default`,
/// `queue.connections.*` and `queue.failed`. The `database` connections
/// and the `database-uuids` failed job provider use the container's
/// `illuminate_database::DatabaseManager`.
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

        app.singleton::<dyn FailedJobProvider>(make_container_failer);

        app.singleton::<JobRateLimiters>(|_| Arc::new(JobRateLimiters::new()));
        app.singleton::<DeferredCallbacks>(|_| Arc::new(DeferredCallbacks::new()));
    }
}

/// Registers the bus [`Dispatcher`] (the `Bus` facade) and the batch
/// repository.
///
/// When `queue.batching` is configured (`database` and `table`) and an
/// `illuminate_database::DatabaseManager` is bound, batches are stored in
/// the database by a [`DatabaseBatchRepository`](crate::DatabaseBatchRepository);
/// otherwise they are kept in memory. A repository already bound as
/// `dyn BatchRepository` is left alone.
pub struct BusServiceProvider;

impl ServiceProvider for BusServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<dyn QueueingDispatcher>(|_| Arc::new(Dispatcher::new()));
        app.singleton_if::<dyn BatchRepository>(make_batch_repository);
    }
}
