//! Service provider bookkeeping, and the framework's default providers.

use std::sync::{Arc, Mutex};

use illuminate_container::{Container, ServiceProvider};
use illuminate_routing::MiddlewareFactory;

use crate::application::Application;
use crate::configuration::Middleware;

/// Providers waiting to be registered when the application bootstraps.
#[derive(Default)]
struct PendingProviders(Mutex<Vec<Box<dyn ServiceProvider>>>);

/// Queue a provider for registration during bootstrapping.
pub fn add_pending(app: &Application, provider: Box<dyn ServiceProvider>) {
    app.singleton_if::<PendingProviders>(|_| Arc::new(PendingProviders::default()));
    app.make::<PendingProviders>().0.lock().unwrap().push(provider);
}

/// Take every pending provider, in the order they were added.
pub fn take_pending(app: &Application) -> Vec<Box<dyn ServiceProvider>> {
    match app.try_make::<PendingProviders>() {
        Ok(pending) => std::mem::take(&mut *pending.0.lock().unwrap()),
        Err(_) => Vec::new(),
    }
}

/// The framework's service providers, registered for every application.
pub fn default_providers() -> Vec<Box<dyn ServiceProvider>> {
    vec![
        Box::new(illuminate_events::EventServiceProvider),
        Box::new(illuminate_database::DatabaseServiceProvider),
        Box::new(illuminate_log::LogServiceProvider),
        Box::new(illuminate_routing::RoutingServiceProvider),
        Box::new(illuminate_redis::RedisServiceProvider),
        Box::new(illuminate_cache::CacheServiceProvider),
        Box::new(illuminate_queue::QueueServiceProvider),
        Box::new(illuminate_queue::BusServiceProvider),
        Box::new(illuminate_process::ProcessServiceProvider),
        Box::new(illuminate_concurrency::ConcurrencyServiceProvider),
        Box::new(illuminate_http_client::HttpClientServiceProvider),
        Box::new(illuminate_cookie::CookieServiceProvider),
        Box::new(illuminate_encryption::EncryptionServiceProvider),
        Box::new(illuminate_filesystem::FilesystemServiceProvider),
        Box::new(illuminate_hashing::HashServiceProvider),
        Box::new(illuminate_pipeline::PipelineServiceProvider),
        Box::new(illuminate_session::SessionServiceProvider),
        Box::new(illuminate_translation::TranslationServiceProvider),
        Box::new(illuminate_validation::ValidationServiceProvider),
        Box::new(illuminate_view::ViewServiceProvider),
        Box::new(illuminate_mail::MailServiceProvider),
        Box::new(illuminate_notifications::NotificationServiceProvider),
        Box::new(illuminate_auth::AuthServiceProvider),
        Box::new(illuminate_console::ConsoleServiceProvider),
        Box::new(FoundationServiceProvider),
    ]
}

/// Artisan commands contributed by framework components.
pub fn extra_framework_commands() -> Vec<Arc<dyn illuminate_console::Command>> {
    Vec::new()
}

/// Middleware aliases contributed by framework components.
pub fn framework_middleware_aliases(_config: &Middleware) -> Vec<(String, MiddlewareFactory)> {
    illuminate_auth::middleware::middleware_aliases()
        .into_iter()
        .map(|(alias, factory)| (alias.to_string(), factory))
        .collect()
}

/// Glues the framework's components together.
pub struct FoundationServiceProvider;

impl ServiceProvider for FoundationServiceProvider {
    fn boot(&self, _app: &Container) {
        crate::integration::boot();
    }
}
