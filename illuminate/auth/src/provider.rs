//! The auth service provider.

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};

use crate::access::AccessGate;
use crate::manager::AuthManager;
use crate::passwords::PasswordBrokerManager;

/// Registers the [`AuthManager`] (configured by `auth.*`), the
/// [`AccessGate`], and the [`PasswordBrokerManager`] (`auth.passwords.*`).
pub struct AuthServiceProvider;

fn config(container: &Container) -> Arc<Repository> {
    container
        .try_make::<Repository>()
        .unwrap_or_else(|_| Arc::new(Repository::empty()))
}

impl ServiceProvider for AuthServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<AuthManager>(|container| Arc::new(AuthManager::new(config(container))));
        app.singleton::<AccessGate>(|_| Arc::new(AccessGate::new()));
        app.singleton::<PasswordBrokerManager>(|container| {
            Arc::new(PasswordBrokerManager::new(config(container)))
        });
    }
}
