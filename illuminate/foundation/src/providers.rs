//! Service provider bookkeeping.

use std::sync::Mutex;

use illuminate_container::ServiceProvider;

use crate::application::Application;

/// Providers waiting to be registered when the application bootstraps.
#[derive(Default)]
struct PendingProviders(Mutex<Vec<Box<dyn ServiceProvider>>>);

/// Queue a provider for registration during bootstrapping.
pub fn add_pending(app: &Application, provider: Box<dyn ServiceProvider>) {
    app.singleton_if::<PendingProviders>(|_| std::sync::Arc::new(PendingProviders::default()));
    app.make::<PendingProviders>().0.lock().unwrap().push(provider);
}

/// Take every pending provider, in the order they were added.
pub fn take_pending(app: &Application) -> Vec<Box<dyn ServiceProvider>> {
    match app.try_make::<PendingProviders>() {
        Ok(pending) => std::mem::take(&mut *pending.0.lock().unwrap()),
        Err(_) => Vec::new(),
    }
}
