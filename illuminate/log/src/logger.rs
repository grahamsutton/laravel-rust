//! The channel logger handed out by the log manager (`Illuminate\Log\Logger`).

use std::fmt::{self, Display};
use std::sync::{Arc, RwLock};

use serde::Serialize;

use crate::handler::Handler;
use crate::level::Level;
use crate::monolog::Monolog;
use crate::record::{Context, MessageLogged, to_context};

/// A `MessageLogged` listener.
pub(crate) type LogListener = Arc<dyn Fn(&MessageLogged) + Send + Sync>;

/// Listeners shared by every channel created by one log manager.
pub(crate) type LogListeners = Arc<RwLock<Vec<LogListener>>>;

/// Generates the eight RFC 5424 level methods (and their `_with` variants).
macro_rules! level_methods {
    (|$this:ident| $target:expr) => {
        level_methods!(@methods |$this| $target;
            emergency, emergency_with, Emergency, "System is unusable.";
            alert, alert_with, Alert, "Action must be taken immediately (the entire website is down, the database is unavailable, ...).";
            critical, critical_with, Critical, "Critical conditions (an application component is unavailable, an unexpected exception, ...).";
            error, error_with, Error, "Runtime errors that do not require immediate action but should typically be logged and monitored.";
            warning, warning_with, Warning, "Exceptional occurrences that are not errors (use of deprecated APIs, poor use of an API, ...).";
            notice, notice_with, Notice, "Normal but significant events.";
            info, info_with, Info, "Interesting events (a user logs in, SQL logs, ...).";
            debug, debug_with, Debug, "Detailed debug information.";
        );
    };
    (@methods |$this:ident| $target:expr; $($name:ident, $with:ident, $level:ident, $doc:literal;)*) => {
        $(
            #[doc = $doc]
            pub fn $name(&$this, message: impl ::std::fmt::Display) {
                $target.log($crate::Level::$level, message)
            }

            #[doc = $doc]
            ///
            /// The context may be anything serializable, usually a `json!` object.
            pub fn $with(&$this, message: impl ::std::fmt::Display, context: impl ::serde::Serialize) {
                $target.log_with($crate::Level::$level, message, context)
            }
        )*
    };
}

pub(crate) use level_methods;

/// A log channel.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_log::{Level, Logger, Monolog, TestHandler};
/// use illuminate_support::json;
///
/// let handler = Arc::new(TestHandler::new());
/// let logger = Logger::new(Monolog::new("local").with_handler_arc(handler.clone()));
///
/// logger.with_context(json!({"request-id": "abc"}));
/// logger.info("Showing the user profile.");
/// logger.error_with("User failed to login.", json!({"id": 1}));
///
/// let records = handler.records();
/// assert_eq!(records[0].context["request-id"], "abc");
/// assert_eq!(records[1].context["id"], 1);
/// ```
pub struct Logger {
    driver: Arc<Monolog>,
    context: RwLock<Context>,
    listeners: LogListeners,
}

impl Logger {
    /// Wrap a channel's underlying logger.
    pub fn new(driver: Monolog) -> Self {
        Self::from_arc(Arc::new(driver))
    }

    /// Wrap a shared channel logger.
    pub fn from_arc(driver: Arc<Monolog>) -> Self {
        Self::with_listeners(driver, LogListeners::default())
    }

    pub(crate) fn with_listeners(driver: Arc<Monolog>, listeners: LogListeners) -> Self {
        Self {
            driver,
            context: RwLock::new(Context::new()),
            listeners,
        }
    }

    level_methods!(|self| self);

    /// Log a message at the given level.
    pub fn log(&self, level: Level, message: impl Display) {
        self.write_log(level, message.to_string(), Context::new());
    }

    /// Log a message with contextual data at the given level.
    pub fn log_with(&self, level: Level, message: impl Display, context: impl Serialize) {
        self.write_log(level, message.to_string(), to_context(context));
    }

    fn write_log(&self, level: Level, message: String, context: Context) {
        if !self.driver.is_handling(level) {
            return;
        }

        let mut merged = self.context.read().unwrap().clone();
        for (key, value) in context {
            merged.insert(key, value);
        }

        match self
            .driver
            .add_record(level, message.clone(), merged.clone())
        {
            Ok(_) => self.fire_log_event(level, message, merged),
            Err(error) => {
                eprintln!(
                    "Unable to write to the [{}] log channel: {error}",
                    self.driver.name()
                );
            }
        }
    }

    fn fire_log_event(&self, level: Level, message: String, context: Context) {
        let listeners = self.listeners.read().unwrap().clone();
        if listeners.is_empty() {
            return;
        }
        let event = MessageLogged {
            level,
            message,
            context,
        };
        for listener in listeners {
            listener(&event);
        }
    }

    /// Add context to all future logs written to this channel.
    pub fn with_context(&self, context: impl Serialize) -> &Self {
        let mut current = self.context.write().unwrap();
        for (key, value) in to_context(context) {
            current.insert(key, value);
        }
        self
    }

    /// Flush the channel's context.
    pub fn without_context(&self) -> &Self {
        self.context.write().unwrap().clear();
        self
    }

    /// Remove the given keys from the channel's context.
    pub fn without_context_keys(&self, keys: &[&str]) -> &Self {
        let mut context = self.context.write().unwrap();
        for key in keys {
            context.shift_remove(*key);
        }
        self
    }

    /// The channel's current context.
    pub fn context(&self) -> Context {
        self.context.read().unwrap().clone()
    }

    /// Register a listener that runs whenever a message is logged.
    pub fn listen(&self, callback: impl Fn(&MessageLogged) + Send + Sync + 'static) {
        self.listeners.write().unwrap().push(Arc::new(callback));
    }

    /// The underlying channel logger.
    pub fn get_logger(&self) -> &Arc<Monolog> {
        &self.driver
    }

    /// The channel's name (the application environment, unless configured).
    pub fn name(&self) -> &str {
        self.driver.name()
    }

    /// The channel's handlers.
    pub fn handlers(&self) -> Vec<Arc<dyn Handler>> {
        self.driver.handlers()
    }

    /// Whether the channel handles messages of the given level.
    pub fn is_handling(&self, level: Level) -> bool {
        self.driver.is_handling(level)
    }
}

impl fmt::Debug for Logger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logger")
            .field("driver", &self.driver)
            .field("context", &self.context.read().unwrap())
            .finish_non_exhaustive()
    }
}
