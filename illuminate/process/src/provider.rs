//! The process service provider.

use std::sync::Arc;

use illuminate_container::{Container, ServiceProvider};

use crate::factory::Factory;

/// Registers the process [`Factory`] (the `Process` facade) as a singleton.
///
/// ```
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_process::{Factory, ProcessServiceProvider};
///
/// let container = Container::new();
/// ProcessServiceProvider.register(&container);
///
/// assert!(!container.make::<Factory>().is_recording());
/// ```
pub struct ProcessServiceProvider;

impl ServiceProvider for ProcessServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<Factory>(|_| Arc::new(Factory::new()));
    }
}
