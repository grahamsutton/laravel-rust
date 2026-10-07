//! The mail service provider.

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};
use illuminate_view::Factory;

use crate::manager::MailManager;
use crate::markdown::Markdown;
use crate::queue::QueuedMessage;

/// Registers the [`MailManager`] (configured from `config/mail`) and the
/// [`Markdown`] renderer. Booting registers the mail components with Blade
/// and, when the queue component is registered, sends queued mail through
/// the queue (see [`crate::queue`]).
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_mail::{MailManager, MailServiceProvider};
/// use illuminate_support::json;
///
/// let app = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(app.clone());
/// app.instance(Repository::new(json!({
///     "mail": {"default": "array", "mailers": {"array": {"transport": "array"}}},
/// })));
///
/// MailServiceProvider.register(&app);
/// MailServiceProvider.boot(&app);
///
/// assert_eq!(app.make::<MailManager>().mailer(None).unwrap().name(), "array");
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct MailServiceProvider;

impl ServiceProvider for MailServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<MailManager>(|app| {
            let config = app
                .try_make::<Repository>()
                .unwrap_or_else(|_| Arc::new(Repository::empty()));
            Arc::new(MailManager::new(config))
        });
        app.bind::<Markdown>(|app| {
            let factory = app
                .try_make::<Factory>()
                .map(|factory| (*factory).clone())
                .unwrap_or_else(|_| Factory::resolve());
            Arc::new(match app.try_make::<Repository>() {
                Ok(config) => Markdown::from_config(factory, &config),
                Err(_) => Markdown::new(factory),
            })
        });
    }

    fn boot(&self, app: &Container) {
        if let Ok(factory) = app.try_make::<Factory>() {
            Markdown::register(&factory);
        }
        // Push queued mail onto the queue when the queue component is registered.
        if app.bound::<illuminate_queue::QueueManager>() {
            let manager = app.make::<MailManager>();
            if !manager.has_queue_hook() {
                manager.queue_using(QueuedMessage::dispatch);
            }
        }
    }
}
