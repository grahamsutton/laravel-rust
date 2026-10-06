//! The `Log` facade and the `logger()` / `info()` helpers.

use std::fmt::Display;
use std::sync::Arc;

use serde::Serialize;

use illuminate_config::{Config, Repository};
use illuminate_container::{Container, try_app};
use illuminate_support::{Result, Value};

use crate::level::Level;
use crate::logger::Logger;
use crate::manager::{LogManager, StackChannel};
use crate::monolog::Monolog;
use crate::record::{Context, MessageLogged};

/// Generates the facade's level methods.
macro_rules! facade_level_methods {
    ($($name:ident, $with:ident, $level:ident, $doc:literal;)*) => {
        $(
            #[doc = $doc]
            pub fn $name(message: impl Display) {
                Self::manager().log(Level::$level, message)
            }

            #[doc = $doc]
            ///
            /// The context may be anything serializable, usually a `json!` object.
            pub fn $with(message: impl Display, context: impl Serialize) {
                Self::manager().log_with(Level::$level, message, context)
            }
        )*
    };
}

/// The `Log` facade.
///
/// Every level has a plain method and a `_with` variant taking contextual
/// data (anything serializable; `json!` objects are the usual choice):
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::Container;
/// use illuminate_log::{Log, LogServiceProvider, MessageLogged};
/// use illuminate_container::ServiceProvider;
/// use illuminate_support::json;
///
/// let app = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(app.clone());
/// app.instance(Repository::new(json!({"logging": {"default": "null"}})));
/// LogServiceProvider.register(&app);
///
/// Log::info("User logged in.");
/// Log::error_with("User {id} failed to login.", json!({"id": 1}));
/// Log::channel("null").warning("Something happened!");
/// ```
pub struct Log;

impl Log {
    /// Get the log manager from the container, registering one if the
    /// application hasn't yet.
    pub fn manager() -> Arc<LogManager> {
        if let Some(manager) = try_app::<LogManager>() {
            return manager;
        }
        let container = Container::get_instance();
        container
            .singleton_if::<LogManager>(|app| Arc::new(LogManager::new(config_repository(app))));
        container.make::<LogManager>()
    }

    facade_level_methods!(
        emergency, emergency_with, Emergency, "System is unusable.";
        alert, alert_with, Alert, "Action must be taken immediately (the entire website is down, the database is unavailable, ...).";
        critical, critical_with, Critical, "Critical conditions (an application component is unavailable, an unexpected exception, ...).";
        error, error_with, Error, "Runtime errors that do not require immediate action but should typically be logged and monitored.";
        warning, warning_with, Warning, "Exceptional occurrences that are not errors (use of deprecated APIs, poor use of an API, ...).";
        notice, notice_with, Notice, "Normal but significant events.";
        info, info_with, Info, "Interesting events (a user logs in, SQL logs, ...).";
        debug, debug_with, Debug, "Detailed debug information.";
    );

    /// Log a message at the given level.
    pub fn log(level: Level, message: impl Display) {
        Self::manager().log(level, message);
    }

    /// Log a message with context at the given level.
    pub fn log_with(level: Level, message: impl Display, context: impl Serialize) {
        Self::manager().log_with(level, message, context);
    }

    /// Get a log channel instance.
    pub fn channel(name: &str) -> Arc<Logger> {
        Self::manager().channel(name)
    }

    /// Get the default log channel.
    pub fn driver() -> Arc<Logger> {
        Self::manager().driver(None)
    }

    /// Create an on-demand stack of channels.
    pub fn stack<I, S>(channels: I) -> Arc<Logger>
    where
        I: IntoIterator<Item = S>,
        S: Into<StackChannel>,
    {
        Self::manager().stack(channels)
    }

    /// Build an on-demand channel from the given configuration.
    pub fn build(config: Value) -> Arc<Logger> {
        Self::manager().build(config)
    }

    /// Add context to all future logs on the default channel.
    pub fn with_context(context: impl Serialize) -> Arc<Logger> {
        Self::manager().with_context(context)
    }

    /// Flush the context on every resolved channel.
    pub fn without_context() {
        Self::manager().without_context();
    }

    /// Share context across every channel, including those created later.
    pub fn share_context(context: impl Serialize) {
        Self::manager().share_context(context);
    }

    /// The context shared across channels and stacks.
    pub fn shared_context() -> Context {
        Self::manager().shared_context()
    }

    /// Flush the shared context.
    pub fn flush_shared_context() {
        Self::manager().flush_shared_context();
    }

    /// Register a listener that runs whenever a message is logged.
    pub fn listen(callback: impl Fn(&MessageLogged) + Send + Sync + 'static) {
        Self::manager().listen(callback);
    }

    /// Register a custom driver creator.
    pub fn extend<F>(driver: &str, creator: F)
    where
        F: Fn(&Container, &Value) -> Result<Monolog> + Send + Sync + 'static,
    {
        Self::manager().extend(driver, creator);
    }

    /// Forget a resolved channel so it is rebuilt from configuration.
    pub fn forget_channel(name: &str) {
        Self::manager().forget_channel(Some(name));
    }

    /// The default channel name.
    pub fn get_default_driver() -> Option<String> {
        Self::manager().get_default_driver()
    }

    /// Set the default channel name.
    pub fn set_default_driver(name: &str) {
        Self::manager().set_default_driver(name);
    }
}

pub(crate) fn config_repository(app: &Container) -> Arc<Repository> {
    app.try_make::<Repository>()
        .unwrap_or_else(|_| Config::repository())
}

/// Get the log manager.
///
/// ```
/// # use std::sync::Arc;
/// # use illuminate_container::Container;
/// use illuminate_log::logger;
///
/// # let app = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(app);
/// logger().debug("Debug message");
/// ```
pub fn logger() -> Arc<LogManager> {
    Log::manager()
}

/// Write an informational message to the logs.
pub fn info(message: impl Display) {
    Log::info(message);
}

/// Write an informational message with context to the logs.
pub fn info_with(message: impl Display, context: impl Serialize) {
    Log::info_with(message, context);
}
