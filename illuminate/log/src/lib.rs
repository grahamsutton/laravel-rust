//! # Illuminate Log
//!
//! To help you learn more about what's happening within your application,
//! Laravel provides robust logging services that allow you to log messages
//! to files, the system error log, and more.
//!
//! Logging is based on *channels*, configured in the `logging`
//! configuration. Each channel uses a driver: `stack`, `single`, `daily`,
//! `monthly`, `errorlog`, `syslog`, `monolog`, `custom`, `null`, plus
//! `stderr`, `stdout` and `stream` for writing to standard streams.
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_config::Repository;
//! use illuminate_container::{Container, ServiceProvider};
//! use illuminate_log::{Log, LogServiceProvider};
//! use illuminate_support::json;
//!
//! let dir = tempfile::tempdir().unwrap();
//! let app = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(app.clone());
//!
//! app.instance(Repository::new(json!({
//!     "app": {"env": "local"},
//!     "logging": {
//!         "default": "stack",
//!         "channels": {
//!             "stack": {"driver": "stack", "channels": ["single"]},
//!             "single": {
//!                 "driver": "single",
//!                 "path": dir.path().join("laravel.log"),
//!                 "replace_placeholders": true,
//!             },
//!         },
//!     },
//! })));
//! LogServiceProvider.register(&app);
//!
//! Log::info_with("User {id} logged in.", json!({"id": 1}));
//!
//! let line = std::fs::read_to_string(dir.path().join("laravel.log")).unwrap();
//! assert!(line.ends_with("] local.INFO: User 1 logged in. {\"id\":1} \n"));
//! ```
//!
//! ## Writing log messages
//!
//! The eight RFC 5424 levels each have a method taking just a message
//! (`Log::info("...")`) and a `_with` variant taking contextual data
//! (`Log::info_with("...", json!({...}))`). The context may be anything
//! serializable; `()` means "no context". Lines are formatted exactly like
//! Laravel's Monolog setup:
//!
//! ```text
//! [2024-01-01 12:00:00] local.INFO: User logged in. {"id":1}
//! ```
//!
//! ## Custom channels
//!
//! [`Log::extend`] registers a driver whose creator returns a [`Monolog`]
//! logger built from the [`Handler`]s, [`Formatter`]s and [`Processor`]s in
//! this crate (or your own implementations).

mod context;
mod facade;
mod formatter;
mod handler;
mod level;
mod logger;
mod manager;
mod monolog;
mod processor;
mod provider;
mod record;

pub use context::{Context, ContextRepository};
pub use facade::{Log, info, info_with, logger};
pub use formatter::{Formatter, JsonFormatter, LineFormatter};
pub use handler::{
    Handler, NullHandler, RotatingFileHandler, Stream, StreamHandler, SyslogHandler,
    SyslogUdpHandler, TestHandler, WhatFailureGroupHandler, syslog_facility,
};
pub use level::Level;
pub use logger::Logger;
pub use manager::{CustomCreator, LogManager, StackChannel};
pub use monolog::Monolog;
pub use processor::{Processor, PsrLogMessageProcessor};
pub use provider::LogServiceProvider;
pub use record::{Context as LogContext, LogRecord, MessageLogged, to_context};
