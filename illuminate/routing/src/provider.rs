//! The routing service provider.

use std::sync::Arc;

use illuminate_container::{Container, ServiceProvider};

use crate::router::Router;
use crate::url::UrlGenerator;

/// Registers the [`Router`] and the [`UrlGenerator`] in the container.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_routing::{Router, RoutingServiceProvider, UrlGenerator};
///
/// let container = Container::new();
/// RoutingServiceProvider.register(&container);
///
/// let router = container.make::<Router>();
/// router.get("/", || async { "Welcome" }).name("home");
///
/// assert!(container.make::<UrlGenerator>().router().has("home"));
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct RoutingServiceProvider;

impl ServiceProvider for RoutingServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton_if::<Router>(|_| Arc::new(Router::new()));
        app.singleton_if::<UrlGenerator>(|container| {
            Arc::new(UrlGenerator::new((*container.make::<Router>()).clone()))
        });
    }
}
