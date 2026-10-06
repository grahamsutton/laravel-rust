//! The session service provider.

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};

use crate::manager::SessionManager;
use crate::middleware::StartSession;

/// Registers the [`SessionManager`] (configured by `session.*`) and the
/// [`StartSession`] middleware.
pub struct SessionServiceProvider;

impl ServiceProvider for SessionServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<SessionManager>(|container| {
            let config = container
                .try_make::<Repository>()
                .unwrap_or_else(|_| Arc::new(Repository::empty()));
            Arc::new(SessionManager::new(config))
        });

        app.singleton::<StartSession>(|container| {
            Arc::new(StartSession::with_manager(
                container.make::<SessionManager>(),
            ))
        });
    }
}
