//! The channel-level logger: a name, a stack of handlers, and processors.

use std::fmt;
use std::sync::{Arc, RwLock};

use illuminate_support::{Carbon, Result};

use crate::handler::Handler;
use crate::level::Level;
use crate::processor::Processor;
use crate::record::{Context, LogRecord};

/// The logger that powers a channel (Monolog's `Logger`).
///
/// Every channel driver builds one of these, and custom drivers registered
/// with `Log::extend` return one:
///
/// ```
/// use std::sync::Arc;
/// use illuminate_log::{Level, Monolog, TestHandler, to_context};
/// use illuminate_support::json;
///
/// let handler = Arc::new(TestHandler::new());
/// let monolog = Monolog::new("local").with_handler_arc(handler.clone());
///
/// monolog.add_record(Level::Info, "Hello", to_context(json!({"id": 1}))).unwrap();
///
/// assert!(handler.has_record(Level::Info, "Hello"));
/// ```
pub struct Monolog {
    name: String,
    handlers: RwLock<Vec<Arc<dyn Handler>>>,
    processors: RwLock<Vec<Arc<dyn Processor>>>,
}

impl Monolog {
    /// Create a logger with the given channel name and no handlers.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            handlers: RwLock::new(Vec::new()),
            processors: RwLock::new(Vec::new()),
        }
    }

    /// Create a logger from existing handlers and processors.
    pub fn from_parts(
        name: impl Into<String>,
        handlers: Vec<Arc<dyn Handler>>,
        processors: Vec<Arc<dyn Processor>>,
    ) -> Self {
        Self {
            name: name.into(),
            handlers: RwLock::new(handlers),
            processors: RwLock::new(processors),
        }
    }

    /// The channel name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// A copy of this logger with a different channel name, sharing the
    /// same handlers and processors.
    pub fn with_name(&self, name: impl Into<String>) -> Self {
        Self::from_parts(name, self.handlers(), self.processors())
    }

    /// Add a handler (builder style).
    pub fn with_handler(self, handler: impl Handler + 'static) -> Self {
        self.push_handler(handler);
        self
    }

    /// Add a shared handler (builder style).
    pub fn with_handler_arc(self, handler: Arc<dyn Handler>) -> Self {
        self.push_handler_arc(handler);
        self
    }

    /// Add a processor (builder style).
    pub fn with_processor(self, processor: impl Processor + 'static) -> Self {
        self.push_processor(processor);
        self
    }

    /// Add a handler to the end of the stack.
    pub fn push_handler(&self, handler: impl Handler + 'static) -> &Self {
        self.push_handler_arc(Arc::new(handler))
    }

    /// Add a shared handler to the end of the stack.
    pub fn push_handler_arc(&self, handler: Arc<dyn Handler>) -> &Self {
        self.handlers.write().unwrap().push(handler);
        self
    }

    /// Replace every handler.
    pub fn set_handlers(&self, handlers: Vec<Arc<dyn Handler>>) -> &Self {
        *self.handlers.write().unwrap() = handlers;
        self
    }

    /// The handlers, in the order records reach them.
    pub fn handlers(&self) -> Vec<Arc<dyn Handler>> {
        self.handlers.read().unwrap().clone()
    }

    /// Add a processor.
    pub fn push_processor(&self, processor: impl Processor + 'static) -> &Self {
        self.processors.write().unwrap().push(Arc::new(processor));
        self
    }

    /// Add a shared processor.
    pub fn push_processor_arc(&self, processor: Arc<dyn Processor>) -> &Self {
        self.processors.write().unwrap().push(processor);
        self
    }

    /// The processors, in the order they run.
    pub fn processors(&self) -> Vec<Arc<dyn Processor>> {
        self.processors.read().unwrap().clone()
    }

    /// Whether any handler handles records of the given level.
    pub fn is_handling(&self, level: Level) -> bool {
        self.handlers
            .read()
            .unwrap()
            .iter()
            .any(|handler| handler.is_handling(level))
    }

    /// Log a record "now". Returns whether any handler handled it.
    pub fn add_record(
        &self,
        level: Level,
        message: impl Into<String>,
        context: Context,
    ) -> Result<bool> {
        self.add_record_at(level, message, context, Carbon::now())
    }

    /// Log a record at the given moment.
    pub fn add_record_at(
        &self,
        level: Level,
        message: impl Into<String>,
        context: Context,
        datetime: Carbon,
    ) -> Result<bool> {
        let handlers = self.handlers();
        let processors = self.processors();

        let mut record = LogRecord {
            datetime,
            channel: self.name.clone(),
            level,
            message: message.into(),
            context,
            extra: crate::context::Context::for_logs(),
        };

        // Processors only run once a handler is actually going to handle
        // the record; afterwards it travels down the stack until a handler
        // stops it from bubbling.
        let mut initialized = processors.is_empty();
        let mut handled = false;
        for handler in handlers {
            if !initialized {
                if !handler.is_handling(level) {
                    continue;
                }
                for processor in &processors {
                    record = processor.process(record);
                }
                initialized = true;
            }
            handled = true;
            if handler.handle(&record)? {
                break;
            }
        }
        Ok(handled)
    }

    /// Close every handler.
    pub fn close(&self) {
        for handler in self.handlers() {
            handler.close();
        }
    }
}

impl fmt::Debug for Monolog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Monolog")
            .field("name", &self.name)
            .field("handlers", &self.handlers.read().unwrap().len())
            .field("processors", &self.processors.read().unwrap().len())
            .finish()
    }
}
