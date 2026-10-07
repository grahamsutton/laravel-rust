//! The HTTP client service provider.

use std::sync::Arc;

use illuminate_container::{Container, ServiceProvider};

use crate::factory::Factory;

/// Registers the HTTP client [`Factory`] (the `Http` facade's service) as a
/// singleton.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_http_client::{Factory, Http, HttpClientServiceProvider};
///
/// let app = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(app.clone());
///
/// HttpClientServiceProvider.register(&app);
///
/// assert!(Arc::ptr_eq(&Http::factory(), &app.make::<Factory>()));
/// ```
///
/// Events are dispatched through the `illuminate_events::Dispatcher` bound
/// in the container, when there is one.
#[derive(Clone, Copy, Debug, Default)]
pub struct HttpClientServiceProvider;

impl ServiceProvider for HttpClientServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<Factory>(|_| Arc::new(Factory::new()));
    }
}
