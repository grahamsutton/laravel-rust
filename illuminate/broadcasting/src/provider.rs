//! The broadcasting service provider.

use std::sync::Arc;

use illuminate_container::{Container, ServiceProvider};
use illuminate_events::Dispatcher;

use crate::facade::make_manager;
use crate::manager::BroadcastManager;
use crate::registration::install;

/// Registers the [`BroadcastManager`] (the `Broadcast` facade), reading the
/// `broadcasting` configuration (`broadcasting.default` and
/// `broadcasting.connections.*`).
///
/// When it boots, every event registered with
/// [`register_broadcast!`](crate::register_broadcast) is hooked into the
/// event dispatcher, so dispatching it broadcasts it.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_broadcasting::{BroadcastManager, BroadcastServiceProvider};
/// use illuminate_config::Repository;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_support::json;
///
/// let container = Container::new();
/// container.instance(Repository::new(json!({
///     "broadcasting": {"default": "log", "connections": {"log": {"driver": "log"}}},
/// })));
/// BroadcastServiceProvider.register(&container);
/// BroadcastServiceProvider.boot(&container);
///
/// assert_eq!(container.make::<BroadcastManager>().get_default_driver(), "log");
/// ```
pub struct BroadcastServiceProvider;

impl ServiceProvider for BroadcastServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<BroadcastManager>(make_manager);
    }

    fn boot(&self, app: &Container) {
        app.singleton_if::<Dispatcher>(|_| Arc::new(Dispatcher::new()));
        install(&app.make::<Dispatcher>());
    }
}
