//! The `App` facade.

use std::sync::Arc;

use illuminate_container::ServiceProvider;

use crate::application::Application;

/// The `App` facade: quick access to the current application.
///
/// ```ignore
/// if App::is_local() {
///     //
/// }
/// ```
pub struct App;

impl App {
    /// The current application.
    pub fn instance() -> Arc<Application> {
        Application::current()
    }

    /// The current environment.
    pub fn environment() -> String {
        Application::current().environment()
    }

    /// Determine if the environment matches any of the patterns.
    pub fn environment_is(patterns: &[&str]) -> bool {
        Application::current().environment_is(patterns)
    }

    pub fn is_local() -> bool {
        Application::current().is_local()
    }

    pub fn is_production() -> bool {
        Application::current().is_production()
    }

    pub fn running_unit_tests() -> bool {
        Application::current().running_unit_tests()
    }

    pub fn running_in_console() -> bool {
        Application::current().running_in_console()
    }

    pub fn is_down_for_maintenance() -> bool {
        Application::current().is_down_for_maintenance()
    }

    pub fn has_debug_mode_enabled() -> bool {
        Application::current().has_debug_mode_enabled()
    }

    pub fn version() -> &'static str {
        crate::VERSION
    }

    pub fn get_locale() -> String {
        Application::current().get_locale()
    }

    pub fn set_locale(locale: &str) {
        Application::current().set_locale(locale);
        let _ = illuminate_translation::Lang::set_locale(locale);
    }

    pub fn is_locale(locale: &str) -> bool {
        Application::current().is_locale(locale)
    }

    /// Resolve a service from the container.
    pub fn make<T: ?Sized + Send + Sync + 'static>() -> Arc<T> {
        Application::current().make::<T>()
    }

    /// Register a service provider.
    pub fn register(provider: impl ServiceProvider) -> bool {
        Application::current().register(provider)
    }

    /// Register a terminating callback.
    pub fn terminating(callback: impl FnOnce(&Application) + Send + 'static) {
        Application::current().terminating(callback);
    }
}
