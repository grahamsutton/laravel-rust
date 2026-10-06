//! The log manager: resolves configured channels and forwards to the default.

use std::collections::HashMap;
use std::fmt::{self, Display};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use indexmap::IndexMap;
use serde::Serialize;

use illuminate_config::Repository;
use illuminate_container::Container;
use illuminate_support::error::{InvalidArgumentException, RuntimeException};
use illuminate_support::{Error, Result, Str, Value, ValueExt, json};

use crate::formatter::{Formatter, JsonFormatter, LineFormatter};
use crate::handler::{
    Handler, NullHandler, RotatingFileHandler, Stream, StreamHandler, SyslogHandler,
    SyslogUdpHandler, WhatFailureGroupHandler, syslog_facility,
};
use crate::level::Level;
use crate::logger::{LogListeners, Logger, level_methods};
use crate::monolog::Monolog;
use crate::processor::{Processor, PsrLogMessageProcessor};
use crate::record::{Context, MessageLogged, to_context};

/// A custom driver creator registered with [`LogManager::extend`].
pub type CustomCreator = Arc<dyn Fn(&Container, &Value) -> Result<Monolog> + Send + Sync>;

/// A channel in an on-demand stack: a configured channel's name, or a
/// logger you already have (such as one made with [`LogManager::build`]).
#[derive(Clone)]
pub enum StackChannel {
    /// A channel from the `logging.channels` configuration.
    Name(String),
    /// An existing channel logger.
    Logger(Arc<Logger>),
}

impl From<&str> for StackChannel {
    fn from(name: &str) -> Self {
        StackChannel::Name(name.to_string())
    }
}

impl From<String> for StackChannel {
    fn from(name: String) -> Self {
        StackChannel::Name(name)
    }
}

impl From<&String> for StackChannel {
    fn from(name: &String) -> Self {
        StackChannel::Name(name.clone())
    }
}

impl From<Arc<Logger>> for StackChannel {
    fn from(logger: Arc<Logger>) -> Self {
        StackChannel::Logger(logger)
    }
}

impl From<&Arc<Logger>> for StackChannel {
    fn from(logger: &Arc<Logger>) -> Self {
        StackChannel::Logger(logger.clone())
    }
}

/// The log manager behind the `Log` facade.
///
/// Channels are read from the `logging` configuration:
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_log::LogManager;
/// use illuminate_support::json;
///
/// let dir = tempfile::tempdir().unwrap();
/// let path = dir.path().join("laravel.log");
///
/// let config = Repository::new(json!({
///     "app": {"env": "local"},
///     "logging": {
///         "default": "single",
///         "channels": {
///             "single": {"driver": "single", "path": path, "level": "debug"},
///         },
///     },
/// }));
///
/// let log = LogManager::new(Arc::new(config));
/// log.info("User logged in.");
///
/// let contents = std::fs::read_to_string(&path).unwrap();
/// assert!(contents.ends_with("] local.INFO: User logged in.  \n"));
/// ```
pub struct LogManager {
    config: Arc<Repository>,
    channels: RwLock<IndexMap<String, Arc<Logger>>>,
    shared_context: RwLock<Context>,
    custom_creators: RwLock<HashMap<String, CustomCreator>>,
    listeners: LogListeners,
    date_format: String,
}

impl LogManager {
    /// Create a log manager reading channels from the given configuration.
    pub fn new(config: Arc<Repository>) -> Self {
        Self {
            config,
            channels: RwLock::new(IndexMap::new()),
            shared_context: RwLock::new(Context::new()),
            custom_creators: RwLock::new(HashMap::new()),
            listeners: LogListeners::default(),
            date_format: LineFormatter::LARAVEL_DATE.to_string(),
        }
    }

    // ------------------------------------------------------------------
    // Channels
    // ------------------------------------------------------------------

    /// Get a log channel instance.
    pub fn channel(&self, name: &str) -> Arc<Logger> {
        self.driver(Some(name))
    }

    /// Get a log driver instance (the default channel when `None`).
    pub fn driver(&self, name: Option<&str>) -> Arc<Logger> {
        let name = match name {
            Some(name) => name.trim().to_string(),
            None => self
                .get_default_driver()
                .unwrap_or_else(|| "null".to_string()),
        };
        self.get(&name, None)
    }

    /// Build an on-demand channel from the given configuration.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use illuminate_config::Repository;
    /// # use illuminate_log::LogManager;
    /// use illuminate_support::json;
    ///
    /// # let log = LogManager::new(Arc::new(Repository::empty()));
    /// # let dir = tempfile::tempdir().unwrap();
    /// # let path = dir.path().join("custom.log");
    /// log.build(json!({"driver": "single", "path": path})).info("Something happened!");
    /// # assert!(path.exists());
    /// ```
    pub fn build(&self, config: Value) -> Arc<Logger> {
        self.channels.write().unwrap().shift_remove("ondemand");
        self.get("ondemand", Some(config))
    }

    /// Create an on-demand stack of channels.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use illuminate_config::Repository;
    /// # use illuminate_log::LogManager;
    /// use illuminate_log::StackChannel;
    /// use illuminate_support::json;
    ///
    /// # let dir = tempfile::tempdir().unwrap();
    /// # let path = dir.path().join("custom.log");
    /// # let single = dir.path().join("laravel.log");
    /// # let log = LogManager::new(Arc::new(Repository::new(json!({
    /// #     "logging": {"channels": {"single": {"driver": "single", "path": single}}},
    /// # }))));
    /// let channel = log.build(json!({"driver": "single", "path": path}));
    ///
    /// log.stack(["single"]).info("Something happened!");
    /// log.stack([StackChannel::from("single"), channel.into()]).info("Something else happened!");
    /// # assert!(std::fs::read_to_string(&path).unwrap().contains("Something else happened!"));
    /// # assert_eq!(std::fs::read_to_string(&single).unwrap().lines().count(), 2);
    /// ```
    pub fn stack<I, S>(&self, channels: I) -> Arc<Logger>
    where
        I: IntoIterator<Item = S>,
        S: Into<StackChannel>,
    {
        let channels: Vec<StackChannel> = channels.into_iter().map(Into::into).collect();
        let monolog = self.stack_monolog(&channels, self.fallback_channel_name(), false);
        let logger = Logger::with_listeners(Arc::new(monolog), self.listeners.clone());
        logger.with_context(self.shared_context());
        Arc::new(logger)
    }

    fn get(&self, name: &str, config: Option<Value>) -> Arc<Logger> {
        if config.is_none()
            && let Some(logger) = self.channels.read().unwrap().get(name)
        {
            return logger.clone();
        }

        match self.resolve(name, config) {
            Ok(monolog) => {
                let logger = Logger::with_listeners(Arc::new(monolog), self.listeners.clone());
                logger.with_context(self.shared_context());
                let logger = Arc::new(logger);
                self.channels
                    .write()
                    .unwrap()
                    .entry(name.to_string())
                    .or_insert(logger)
                    .clone()
            }
            Err(error) => {
                let logger = self.create_emergency_logger();
                logger.emergency_with(
                    "Unable to create configured logger. Using emergency logger.",
                    json!({"exception": describe_exception(&error)}),
                );
                logger
            }
        }
    }

    /// The logger used when a channel can't be created, so a broken log
    /// configuration never takes the application down with it.
    fn create_emergency_logger(&self) -> Arc<Logger> {
        let path = self.config.get("logging.channels.emergency.path");
        let stream = match path.as_str() {
            Some(path) if !path.is_empty() => Stream::Path(PathBuf::from(path)),
            _ => Stream::Stderr,
        };
        let handler = StreamHandler::new(stream);
        handler.set_formatter(self.formatter());
        let monolog = Monolog::new("laravel").with_handler(handler);
        Arc::new(Logger::with_listeners(
            Arc::new(monolog),
            self.listeners.clone(),
        ))
    }

    fn resolve(&self, name: &str, config: Option<Value>) -> Result<Monolog> {
        let config = match config {
            Some(config) => config,
            None => self.configuration_for(name),
        };

        if config.is_null() {
            if name == "null" {
                return Ok(
                    Monolog::new(self.fallback_channel_name()).with_handler(NullHandler::new())
                );
            }
            return Err(
                InvalidArgumentException::new(format!("Log [{name}] is not defined.")).into(),
            );
        }

        let driver = config
            .get("driver")
            .map(|d| d.to_string_lossy())
            .unwrap_or_default();

        let creator = self.custom_creators.read().unwrap().get(&driver).cloned();
        if let Some(creator) = creator {
            return creator(&Container::get_instance(), &config);
        }

        match driver.as_str() {
            "stack" => self.create_stack_driver(&config),
            "single" => self.create_single_driver(&config),
            "daily" => self.create_daily_driver(&config),
            "monthly" => self.create_monthly_driver(&config),
            "syslog" => self.create_syslog_driver(&config),
            "errorlog" => self.create_errorlog_driver(&config),
            "monolog" => self.create_monolog_driver(&config),
            "custom" => self.create_custom_driver(&config),
            "stderr" | "stdout" | "stream" => self.create_stream_driver(&driver, &config),
            "null" => {
                Ok(Monolog::new(self.parse_channel(&config)).with_handler(NullHandler::new()))
            }
            _ => Err(
                InvalidArgumentException::new(format!("Driver [{driver}] is not supported."))
                    .into(),
            ),
        }
    }

    fn configuration_for(&self, name: &str) -> Value {
        match self.config.get("logging.channels") {
            Value::Object(channels) => channels.get(name).cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }

    // ------------------------------------------------------------------
    // Drivers
    // ------------------------------------------------------------------

    /// Create a custom driver: `via` names a creator registered with
    /// [`LogManager::extend`].
    fn create_custom_driver(&self, config: &Value) -> Result<Monolog> {
        let via = config
            .get("via")
            .map(|v| v.to_string_lossy())
            .unwrap_or_default();
        let creator = self.custom_creators.read().unwrap().get(&via).cloned();
        match creator {
            Some(creator) => creator(&Container::get_instance(), config),
            None => Err(InvalidArgumentException::new(format!(
                "Custom log driver [{via}] has not been registered. Register it with Log::extend()."
            ))
            .into()),
        }
    }

    fn create_stack_driver(&self, config: &Value) -> Result<Monolog> {
        let channels: Vec<StackChannel> = match config.get("channels") {
            Some(Value::String(list)) => list
                .split(',')
                .map(|c| StackChannel::Name(c.trim().into()))
                .collect(),
            Some(Value::Array(list)) => list
                .iter()
                .map(|c| StackChannel::Name(c.to_string_lossy()))
                .collect(),
            _ => Vec::new(),
        };
        let ignore_exceptions = config
            .get("ignore_exceptions")
            .is_some_and(ValueExt::truthy);
        Ok(self.stack_monolog(&channels, self.parse_channel(config), ignore_exceptions))
    }

    fn stack_monolog(
        &self,
        channels: &[StackChannel],
        name: String,
        ignore_exceptions: bool,
    ) -> Monolog {
        let loggers: Vec<Arc<Logger>> = channels
            .iter()
            .filter(|channel| !matches!(channel, StackChannel::Name(name) if name.is_empty()))
            .map(|channel| match channel {
                StackChannel::Name(name) => self.channel(name),
                StackChannel::Logger(logger) => logger.clone(),
            })
            .collect();

        let mut handlers: Vec<Arc<dyn Handler>> = loggers
            .iter()
            .flat_map(|logger| logger.handlers())
            .collect();
        let processors: Vec<Arc<dyn Processor>> = loggers
            .iter()
            .flat_map(|logger| logger.get_logger().processors())
            .collect();

        if ignore_exceptions {
            handlers = vec![Arc::new(WhatFailureGroupHandler::new(handlers))];
        }

        Monolog::from_parts(name, handlers, processors)
    }

    fn create_single_driver(&self, config: &Value) -> Result<Monolog> {
        let handler = StreamHandler::new(Stream::Path(self.path(config)?))
            .with_level(self.level(config)?)
            .with_bubble(self.bubble(config))
            .with_permission(permission(config));
        self.monolog_with(config, self.prepare_handler(Arc::new(handler), config)?)
    }

    fn create_daily_driver(&self, config: &Value) -> Result<Monolog> {
        let max_files = config
            .get("max_files")
            .or_else(|| config.get("days"))
            .and_then(ValueExt::to_i64_lossy)
            .unwrap_or(7);
        self.create_rotating_driver(config, RotatingFileHandler::FILE_PER_DAY, max_files)
    }

    fn create_monthly_driver(&self, config: &Value) -> Result<Monolog> {
        let max_files = config
            .get("max_files")
            .and_then(ValueExt::to_i64_lossy)
            .unwrap_or(3);
        self.create_rotating_driver(config, RotatingFileHandler::FILE_PER_MONTH, max_files)
    }

    fn create_rotating_driver(
        &self,
        config: &Value,
        date_format: &str,
        max_files: i64,
    ) -> Result<Monolog> {
        let handler = RotatingFileHandler::new(self.path(config)?, max_files.max(0) as usize)
            .with_date_format(date_format)
            .with_level(self.level(config)?)
            .with_bubble(self.bubble(config))
            .with_permission(permission(config));
        self.monolog_with(config, self.prepare_handler(Arc::new(handler), config)?)
    }

    fn create_syslog_driver(&self, config: &Value) -> Result<Monolog> {
        let ident = Str::snake_with(&self.config.string_or("app.name", "Laravel"), "-");
        let facility = config.get("facility").map(syslog_facility).unwrap_or(8);
        let handler = SyslogHandler::new(ident, facility)
            .with_level(self.level(config)?)
            .with_bubble(self.bubble(config));
        self.monolog_with(config, self.prepare_handler(Arc::new(handler), config)?)
    }

    fn create_errorlog_driver(&self, config: &Value) -> Result<Monolog> {
        let handler = StreamHandler::stderr().with_level(self.level(config)?);
        self.monolog_with(config, self.prepare_handler(Arc::new(handler), config)?)
    }

    /// `stderr`, `stdout`, and `stream` channels write to a standard stream
    /// (or `with.stream` / `stream` / `path`).
    fn create_stream_driver(&self, driver: &str, config: &Value) -> Result<Monolog> {
        let stream = match driver {
            "stderr" => Stream::Stderr,
            "stdout" => Stream::Stdout,
            _ => {
                let target = config
                    .dot("with.stream")
                    .or_else(|| config.dot("handler_with.stream"))
                    .or_else(|| config.get("stream"))
                    .or_else(|| config.get("path"))
                    .map(|v| v.to_string_lossy())
                    .unwrap_or_else(|| "php://stderr".to_string());
                Stream::parse(&target)
            }
        };
        let handler = StreamHandler::new(stream)
            .with_level(self.level(config)?)
            .with_bubble(self.bubble(config))
            .with_permission(permission(config));
        self.monolog_with(config, self.prepare_handler(Arc::new(handler), config)?)
    }

    /// The `monolog` driver: pick one of the built-in handlers by name
    /// (`StreamHandler`, `RotatingFileHandler`, `SyslogUdpHandler`,
    /// `SyslogHandler`, `ErrorLogHandler`, `NullHandler`), configured with
    /// `handler_with` (or `with`).
    fn create_monolog_driver(&self, config: &Value) -> Result<Monolog> {
        let handler_name = config
            .get("handler")
            .map(|h| h.to_string_lossy())
            .unwrap_or_default();
        let with = config
            .get("handler_with")
            .or_else(|| config.get("with"))
            .cloned()
            .unwrap_or(Value::Null);
        let argument = |key: &str| with.get(key).filter(|v| !v.is_null());
        let level = self.level(config)?;
        let bubble = argument("bubble").map(ValueExt::truthy).unwrap_or(true);

        let handler: Arc<dyn Handler> = match class_basename(&handler_name).as_str() {
            "StreamHandler" => {
                let stream = argument("stream")
                    .or_else(|| argument("url"))
                    .map(|v| v.to_string_lossy())
                    .unwrap_or_else(|| "php://stderr".into());
                Arc::new(
                    StreamHandler::new(Stream::parse(&stream))
                        .with_level(level)
                        .with_bubble(bubble)
                        .with_permission(argument("filePermission").and_then(parse_permission)),
                )
            }
            "RotatingFileHandler" => {
                let filename = argument("filename")
                    .map(|v| PathBuf::from(v.to_string_lossy()))
                    .ok_or_else(|| {
                        InvalidArgumentException::new("RotatingFileHandler requires a filename.")
                    })?;
                let max_files = argument("maxFiles")
                    .and_then(ValueExt::to_i64_lossy)
                    .unwrap_or(0);
                Arc::new(
                    RotatingFileHandler::new(filename, max_files.max(0) as usize)
                        .with_level(level)
                        .with_bubble(bubble),
                )
            }
            "SyslogUdpHandler" => {
                let host = argument("host")
                    .map(|v| v.to_string_lossy())
                    .unwrap_or_default();
                let port = argument("port")
                    .and_then(ValueExt::to_i64_lossy)
                    .unwrap_or(514);
                let mut handler =
                    SyslogUdpHandler::new(&host, port.clamp(0, u16::MAX as i64) as u16)
                        .with_level(level)
                        .with_bubble(bubble);
                if let Some(facility) = argument("facility") {
                    handler = handler.with_facility(syslog_facility(facility));
                }
                if let Some(ident) = argument("ident") {
                    handler = handler.with_ident(ident.to_string_lossy());
                }
                Arc::new(handler)
            }
            "SyslogHandler" => {
                let ident = argument("ident")
                    .map(|v| v.to_string_lossy())
                    .unwrap_or_else(|| {
                        Str::snake_with(&self.config.string_or("app.name", "Laravel"), "-")
                    });
                let facility = argument("facility").map(syslog_facility).unwrap_or(8);
                Arc::new(
                    SyslogHandler::new(ident, facility)
                        .with_level(level)
                        .with_bubble(bubble),
                )
            }
            "ErrorLogHandler" => Arc::new(
                StreamHandler::stderr()
                    .with_level(level)
                    .with_bubble(bubble),
            ),
            "NullHandler" => Arc::new(NullHandler::with_level(level)),
            _ => {
                return Err(InvalidArgumentException::new(format!(
                    "{handler_name} must be an instance of Monolog\\Handler\\HandlerInterface"
                ))
                .into());
            }
        };

        let handler = self.prepare_handler(handler, config)?;
        let monolog = Monolog::new(self.parse_channel(config)).with_handler_arc(handler);

        if let Some(Value::Array(processors)) = config.get("processors") {
            for processor in processors {
                let (name, with) = match processor {
                    Value::Object(map) => (
                        map.get("processor")
                            .map(|p| p.to_string_lossy())
                            .unwrap_or_default(),
                        map.get("with").cloned().unwrap_or(Value::Null),
                    ),
                    other => (other.to_string_lossy(), Value::Null),
                };
                match class_basename(&name).as_str() {
                    "PsrLogMessageProcessor" => {
                        let remove = with
                            .get("removeUsedContextFields")
                            .is_some_and(ValueExt::truthy);
                        monolog.push_processor(
                            PsrLogMessageProcessor::new().remove_used_context_fields(remove),
                        );
                    }
                    _ => {
                        return Err(InvalidArgumentException::new(format!(
                            "{name} must be an instance of Monolog\\Processor\\ProcessorInterface"
                        ))
                        .into());
                    }
                }
            }
        }

        Ok(monolog)
    }

    /// Wrap a prepared handler in a channel logger, adding the placeholder
    /// processor when `replace_placeholders` is enabled.
    fn monolog_with(&self, config: &Value, handler: Arc<dyn Handler>) -> Result<Monolog> {
        let monolog = Monolog::new(self.parse_channel(config)).with_handler_arc(handler);
        if config
            .get("replace_placeholders")
            .is_some_and(ValueExt::truthy)
        {
            monolog.push_processor(PsrLogMessageProcessor::new());
        }
        Ok(monolog)
    }

    /// Apply the configured formatter (Laravel's line format by default).
    fn prepare_handler(
        &self,
        handler: Arc<dyn Handler>,
        config: &Value,
    ) -> Result<Arc<dyn Handler>> {
        let formatter = config.get("formatter").filter(|f| !f.is_null());
        match formatter.map(|f| f.to_string_lossy()) {
            None => handler.set_formatter(self.formatter()),
            Some(name) if name == "default" => {}
            Some(name) => {
                let with = config.get("formatter_with").cloned().unwrap_or(Value::Null);
                handler.set_formatter(self.named_formatter(&name, &with)?);
            }
        }
        Ok(handler)
    }

    fn named_formatter(&self, name: &str, with: &Value) -> Result<Arc<dyn Formatter>> {
        match class_basename(name).to_ascii_lowercase().as_str() {
            "json" | "jsonformatter" => Ok(Arc::new(JsonFormatter::new())),
            "line" | "lineformatter" => {
                let mut formatter = LineFormatter::new();
                if let Some(format) = with.get("format").filter(|v| !v.is_null()) {
                    formatter = formatter.with_format(format.to_string_lossy());
                }
                if let Some(date) = with.get("dateFormat").or_else(|| with.get("date_format")) {
                    formatter = formatter.with_date_format(date.to_string_lossy());
                }
                if let Some(allow) = with
                    .get("allowInlineLineBreaks")
                    .or_else(|| with.get("allow_inline_line_breaks"))
                {
                    formatter = formatter.allow_inline_line_breaks(allow.truthy());
                }
                if let Some(ignore) = with
                    .get("ignoreEmptyContextAndExtra")
                    .or_else(|| with.get("ignore_empty_context_and_extra"))
                {
                    formatter = formatter.ignore_empty_context_and_extra(ignore.truthy());
                }
                Ok(Arc::new(formatter))
            }
            _ => Err(InvalidArgumentException::new(format!(
                "Log formatter [{name}] is not supported."
            ))
            .into()),
        }
    }

    /// The formatter Laravel gives every channel.
    fn formatter(&self) -> Arc<dyn Formatter> {
        Arc::new(LineFormatter::laravel().with_date_format(self.date_format.clone()))
    }

    fn path(&self, config: &Value) -> Result<PathBuf> {
        match config.get("path").map(|p| p.to_string_lossy()) {
            Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
            _ => Err(InvalidArgumentException::new("Log channel is missing a [path].").into()),
        }
    }

    /// Parse the channel's minimum level (`debug` when not configured).
    fn level(&self, config: &Value) -> Result<Level> {
        match config.get("level").filter(|l| !l.is_null()) {
            None => Ok(Level::Debug),
            Some(level) => Level::parse(&level.to_string_lossy())
                .ok_or_else(|| InvalidArgumentException::new("Invalid log level.").into()),
        }
    }

    fn bubble(&self, config: &Value) -> bool {
        config.get("bubble").is_none_or(ValueExt::truthy)
    }

    /// The channel name: the configured `name`, or the application environment.
    fn parse_channel(&self, config: &Value) -> String {
        match config.get("name").filter(|n| !n.is_null()) {
            Some(name) => name.to_string_lossy(),
            None => self.fallback_channel_name(),
        }
    }

    fn fallback_channel_name(&self) -> String {
        self.config.string_or("app.env", "production")
    }

    // ------------------------------------------------------------------
    // Context
    // ------------------------------------------------------------------

    /// Share context across every channel, including those created later.
    pub fn share_context(&self, context: impl Serialize) -> &Self {
        let context = to_context(context);
        for channel in self.channels.read().unwrap().values() {
            channel.with_context(&context);
        }
        let mut shared = self.shared_context.write().unwrap();
        for (key, value) in context {
            shared.insert(key, value);
        }
        self
    }

    /// The context shared across channels and stacks.
    pub fn shared_context(&self) -> Context {
        self.shared_context.read().unwrap().clone()
    }

    /// Flush the context on every resolved channel.
    pub fn without_context(&self) -> &Self {
        for channel in self.channels.read().unwrap().values() {
            channel.without_context();
        }
        self
    }

    /// Remove the given context keys from every resolved channel.
    pub fn without_context_keys(&self, keys: &[&str]) -> &Self {
        for channel in self.channels.read().unwrap().values() {
            channel.without_context_keys(keys);
        }
        self
    }

    /// Flush the shared context.
    pub fn flush_shared_context(&self) -> &Self {
        self.shared_context.write().unwrap().clear();
        self
    }

    /// Add context to the default channel.
    pub fn with_context(&self, context: impl Serialize) -> Arc<Logger> {
        let logger = self.driver(None);
        logger.with_context(context);
        logger
    }

    // ------------------------------------------------------------------
    // Configuration
    // ------------------------------------------------------------------

    /// The default channel name (`logging.default`).
    pub fn get_default_driver(&self) -> Option<String> {
        match self.config.get("logging.default") {
            Value::Null => None,
            name => Some(name.to_string_lossy()).filter(|n| !n.trim().is_empty()),
        }
    }

    /// Set the default channel name.
    pub fn set_default_driver(&self, name: &str) {
        self.config.set("logging.default", name);
    }

    /// Register a custom driver creator.
    ///
    /// The creator receives the container and the channel's configuration,
    /// and returns the channel's [`Monolog`] logger. Channels use it by
    /// setting `driver` to its name (or `driver: "custom"` with `via`).
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use illuminate_config::Repository;
    /// use illuminate_log::{LogManager, Monolog, NullHandler};
    /// use illuminate_support::json;
    ///
    /// let log = LogManager::new(Arc::new(Repository::new(json!({
    ///     "logging": {"channels": {"mongo": {"driver": "mongodb"}}},
    /// }))));
    ///
    /// log.extend("mongodb", |_app, config| {
    ///     Ok(Monolog::new("mongo").with_handler(NullHandler::new()))
    /// });
    ///
    /// assert_eq!(log.channel("mongo").name(), "mongo");
    /// ```
    pub fn extend<F>(&self, driver: &str, creator: F) -> &Self
    where
        F: Fn(&Container, &Value) -> Result<Monolog> + Send + Sync + 'static,
    {
        self.custom_creators
            .write()
            .unwrap()
            .insert(driver.to_string(), Arc::new(creator));
        self
    }

    /// Forget a resolved channel (the default channel when `None`).
    pub fn forget_channel(&self, name: Option<&str>) {
        let name = match name {
            Some(name) => name.trim().to_string(),
            None => self
                .get_default_driver()
                .unwrap_or_else(|| "null".to_string()),
        };
        self.channels.write().unwrap().shift_remove(&name);
    }

    /// Every resolved channel, by name.
    pub fn get_channels(&self) -> IndexMap<String, Arc<Logger>> {
        self.channels.read().unwrap().clone()
    }

    /// Register a listener that runs whenever a message is logged on any
    /// channel created by this manager.
    pub fn listen(&self, callback: impl Fn(&MessageLogged) + Send + Sync + 'static) {
        self.listeners.write().unwrap().push(Arc::new(callback));
    }

    // ------------------------------------------------------------------
    // Logging to the default channel
    // ------------------------------------------------------------------

    level_methods!(|self| self.driver(None));

    /// Log a message at the given level on the default channel.
    pub fn log(&self, level: Level, message: impl Display) {
        self.driver(None).log(level, message);
    }

    /// Log a message with context at the given level on the default channel.
    pub fn log_with(&self, level: Level, message: impl Display, context: impl Serialize) {
        self.driver(None).log_with(level, message, context);
    }
}

impl fmt::Debug for LogManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LogManager")
            .field(
                "channels",
                &self.channels.read().unwrap().keys().collect::<Vec<_>>(),
            )
            .field("shared_context", &self.shared_context.read().unwrap())
            .finish_non_exhaustive()
    }
}

/// `Monolog\Handler\StreamHandler` → `StreamHandler`.
fn class_basename(name: &str) -> String {
    name.rsplit(['\\', ':'])
        .next()
        .unwrap_or(name)
        .trim()
        .to_string()
}

/// Parse a file permission: an integer (`436`, i.e. PHP's `0664`) or an
/// octal string (`"0664"`).
fn parse_permission(value: &Value) -> Option<u32> {
    match value {
        Value::Number(number) => number.as_u64().map(|n| n as u32),
        Value::String(text) => u32::from_str_radix(text.trim().trim_start_matches("0o"), 8).ok(),
        _ => None,
    }
}

fn permission(config: &Value) -> Option<u32> {
    config.get("permission").and_then(parse_permission)
}

/// Describe an error the way Monolog prints exceptions in the context.
fn describe_exception(error: &Error) -> String {
    let class = if error.is::<InvalidArgumentException>() {
        "InvalidArgumentException"
    } else if error.is::<RuntimeException>() {
        "RuntimeException"
    } else {
        "Exception"
    };
    format!("[object] ({class}(code: 0): {error})")
}
