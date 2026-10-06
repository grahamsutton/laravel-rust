//! The log service provider.

use std::sync::Arc;

use illuminate_container::{Container, ServiceProvider};

use crate::facade::config_repository;
use crate::manager::LogManager;

/// Registers the [`LogManager`] as a singleton, configured by the
/// `logging` configuration in the container's config repository.
#[derive(Debug, Default, Clone, Copy)]
pub struct LogServiceProvider;

impl ServiceProvider for LogServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<LogManager>(|app| Arc::new(LogManager::new(config_repository(app))));
    }
}
