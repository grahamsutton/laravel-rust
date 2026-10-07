//! The concurrency service provider.

use std::sync::Arc;

use illuminate_container::{Container, ServiceProvider};

use crate::deferred::DeferredCallbacks;
use crate::manager::ConcurrencyManager;

/// Registers the [`ConcurrencyManager`] (the `Concurrency` facade) and the
/// application's [`DeferredCallbacks`].
///
/// The default driver is read lazily from `concurrency.default`.
///
/// ```
/// use illuminate_concurrency::{ConcurrencyManager, ConcurrencyServiceProvider, DeferredCallbacks};
/// use illuminate_config::Repository;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_support::json;
///
/// let container = Container::new();
/// container.instance(Repository::new(json!({"concurrency": {"default": "sync"}})));
/// ConcurrencyServiceProvider.register(&container);
///
/// assert!(container.make::<DeferredCallbacks>().is_empty());
/// # let _ = container.make::<ConcurrencyManager>();
/// ```
pub struct ConcurrencyServiceProvider;

impl ServiceProvider for ConcurrencyServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<ConcurrencyManager>(|_| Arc::new(ConcurrencyManager::new()));
        app.singleton_if::<DeferredCallbacks>(|_| Arc::new(DeferredCallbacks::new()));
    }
}
