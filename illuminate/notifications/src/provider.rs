//! The notification service provider.

use std::sync::Arc;

use illuminate_container::{Container, ServiceProvider};

use crate::manager::ChannelManager;
use crate::queue::QueuedNotification;

/// Registers the [`ChannelManager`]. Booting installs the queue hook when
/// the queue component is registered (see [`crate::queue`]).
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_notifications::{ChannelManager, NotificationServiceProvider};
///
/// let app = Arc::new(Container::new());
/// NotificationServiceProvider.register(&app);
/// NotificationServiceProvider.boot(&app);
///
/// assert_eq!(app.make::<ChannelManager>().delivers_via(), "mail");
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct NotificationServiceProvider;

impl ServiceProvider for NotificationServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<ChannelManager>(|_| Arc::new(ChannelManager::new()));
    }

    fn boot(&self, app: &Container) {
        if app.bound::<illuminate_queue::QueueManager>() {
            let manager = app.make::<ChannelManager>();
            if !manager.has_queue_hook() {
                manager.queue_using(QueuedNotification::dispatch);
            }
        }
    }
}
