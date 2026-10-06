//! Application helpers: paths, environment, and friends.

use std::sync::Arc;

use crate::application::Application;

/// Get the current application.
pub fn application() -> Arc<Application> {
    Application::current()
}

fn with_app(callback: impl FnOnce(&Application) -> String, fallback: &str) -> String {
    match Application::try_current() {
        Some(app) => callback(&app),
        None => fallback.to_string(),
    }
}

/// Get the path to the base of the install.
pub fn base_path(path: &str) -> String {
    with_app(|app| app.base_path(path), path)
}

/// Get the path to the application folder.
pub fn app_path(path: &str) -> String {
    with_app(|app| app.app_path(path), path)
}

/// Get the path to the bootstrap folder.
pub fn bootstrap_path(path: &str) -> String {
    with_app(|app| app.bootstrap_path(path), path)
}

/// Get the configuration path.
pub fn config_path(path: &str) -> String {
    with_app(|app| app.config_path(path), path)
}

/// Get the database path.
pub fn database_path(path: &str) -> String {
    with_app(|app| app.database_path(path), path)
}

/// Get the path to the language folder.
pub fn lang_path(path: &str) -> String {
    with_app(|app| app.lang_path(path), path)
}

/// Get the path to the public folder.
pub fn public_path(path: &str) -> String {
    with_app(|app| app.public_path(path), path)
}

/// Get the path to the resources folder.
pub fn resource_path(path: &str) -> String {
    with_app(|app| app.resource_path(path), path)
}

/// Get the path to the storage folder.
pub fn storage_path(path: &str) -> String {
    with_app(|app| app.storage_path(path), path)
}

/// Get the current application environment.
pub fn app_environment() -> String {
    with_app(|app| app.environment(), "production")
}
