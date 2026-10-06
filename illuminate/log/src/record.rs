//! Log records, contexts, and the `MessageLogged` event.

use serde::Serialize;

use illuminate_support::{Carbon, Map, Value, to_value};

use crate::level::Level;

/// The contextual data attached to a log message.
pub type Context = Map<String, Value>;

/// A single log entry on its way through a channel's processors and
/// handlers (Monolog's `LogRecord`).
#[derive(Debug, Clone, PartialEq)]
pub struct LogRecord {
    /// When the message was logged.
    pub datetime: Carbon,
    /// The channel name (the application environment, unless configured).
    pub channel: String,
    /// The message's severity.
    pub level: Level,
    /// The log message.
    pub message: String,
    /// Contextual data passed by the caller.
    pub context: Context,
    /// Extra data added by processors.
    pub extra: Context,
}

impl LogRecord {
    /// Create a record logged "now".
    pub fn new(
        channel: impl Into<String>,
        level: Level,
        message: impl Into<String>,
        context: Context,
    ) -> Self {
        Self {
            datetime: Carbon::now(),
            channel: channel.into(),
            level,
            message: message.into(),
            context,
            extra: Context::new(),
        }
    }
}

/// Fired whenever a message is written to the logs.
///
/// Register listeners with `Log::listen`; they're handy for profilers and
/// tools that collect all of the log messages for a request.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageLogged {
    /// The log level.
    pub level: Level,
    /// The log message.
    pub message: String,
    /// The log context.
    pub context: Context,
}

/// Convert anything serializable into a log context.
///
/// Objects become the context as-is, `null` (and `()`) means "no context",
/// lists are keyed by their indexes, and any other value is wrapped as
/// `{"0": value}`.
///
/// ```
/// use illuminate_log::to_context;
/// use illuminate_support::json;
///
/// assert_eq!(to_context(json!({"id": 1}))["id"], 1);
/// assert!(to_context(()).is_empty());
/// assert_eq!(to_context(json!(["a"]))["0"], "a");
/// ```
pub fn to_context(context: impl Serialize) -> Context {
    match to_value(&context) {
        Value::Object(map) => map,
        Value::Null => Context::new(),
        Value::Array(items) => items
            .into_iter()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value))
            .collect(),
        other => {
            let mut map = Context::new();
            map.insert("0".into(), other);
            map
        }
    }
}
