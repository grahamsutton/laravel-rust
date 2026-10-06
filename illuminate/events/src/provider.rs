//! The events service provider.

use std::sync::Arc;

use illuminate_container::{Container, ServiceProvider};

use crate::dispatcher::Dispatcher;

/// Registers the event [`Dispatcher`] as a singleton.
///
/// Rust can't discover listeners by scanning your `app/Listeners`
/// directory, so register them in a service provider's `boot` method:
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_events::{Event, EventServiceProvider};
///
/// struct PodcastProcessed { id: u64 }
///
/// struct AppServiceProvider;
///
/// impl ServiceProvider for AppServiceProvider {
///     fn boot(&self, _app: &Container) {
///         Event::listen(|event: &PodcastProcessed| {
///             let id = event.id;
///             async move { println!("Podcast {id} processed") }
///         });
///     }
/// }
///
/// let app = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(app.clone());
///
/// EventServiceProvider.register(&app);
/// AppServiceProvider.boot(&app);
///
/// assert!(Event::has_listeners::<PodcastProcessed>());
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct EventServiceProvider;

impl ServiceProvider for EventServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<Dispatcher>(|_| Arc::new(Dispatcher::new()));
    }
}
