//! The console service provider.

use std::sync::Arc;

use illuminate_container::{Container, ServiceProvider};

use crate::application::Application;
use crate::facades::{default_application, default_schedule};
use crate::scheduling::{
    EventMutex, InMemoryEventMutex, InMemorySchedulingMutex, Schedule, SchedulingMutex,
};

/// Registers the console application (Artisan), the schedule and the
/// scheduling mutexes.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_console::{Application, ConsoleServiceProvider};
/// use illuminate_container::{Container, ServiceProvider};
///
/// let container = Container::new();
/// ConsoleServiceProvider.register(&container);
///
/// let artisan = container.make::<Application>();
/// assert!(artisan.has("schedule:run"));
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct ConsoleServiceProvider;

impl ServiceProvider for ConsoleServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton_if::<dyn EventMutex>(|_| Arc::new(InMemoryEventMutex::new()));
        app.singleton_if::<dyn SchedulingMutex>(|_| Arc::new(InMemorySchedulingMutex::new()));

        app.singleton_if::<Application>(|_| default_application());

        app.singleton_if::<Schedule>(|container| {
            let schedule = default_schedule();
            if let Ok(mutex) = container.try_make::<dyn EventMutex>() {
                schedule.use_event_mutex(mutex);
            }
            if let Ok(mutex) = container.try_make::<dyn SchedulingMutex>() {
                schedule.use_scheduling_mutex(mutex);
            }
            Arc::new(schedule)
        });
    }
}
