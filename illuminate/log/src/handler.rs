//! Handlers write log records somewhere: files, standard error, syslog, or
//! memory (for tests).

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use regex::Regex;

use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Result};

use crate::formatter::{Formatter, LineFormatter};
use crate::level::Level;
use crate::record::LogRecord;

/// A log handler (Monolog's `HandlerInterface`).
pub trait Handler: Send + Sync {
    /// Whether the handler handles records of the given level.
    fn is_handling(&self, level: Level) -> bool;

    /// Handle a record. Returns `Ok(true)` when the record should not
    /// "bubble" on to the channel's remaining handlers.
    fn handle(&self, record: &LogRecord) -> Result<bool>;

    /// Replace the handler's formatter (handlers that don't format ignore it).
    fn set_formatter(&self, _formatter: Arc<dyn Formatter>) {}

    /// Close any resources (open files, sockets) held by the handler.
    fn close(&self) {}
}

/// The level, bubbling, and formatter shared by formatting handlers.
struct Formatting {
    level: Level,
    bubble: bool,
    formatter: RwLock<Arc<dyn Formatter>>,
}

impl Formatting {
    fn new(formatter: Arc<dyn Formatter>) -> Self {
        Self {
            level: Level::Debug,
            bubble: true,
            formatter: RwLock::new(formatter),
        }
    }

    fn is_handling(&self, level: Level) -> bool {
        self.level.includes(level)
    }

    fn format(&self, record: &LogRecord) -> String {
        let formatter = self.formatter.read().unwrap().clone();
        formatter.format(record)
    }
}

macro_rules! formatting_builders {
    () => {
        /// Only handle records at or above the given level.
        pub fn with_level(mut self, level: Level) -> Self {
            self.formatting.level = level;
            self
        }

        /// Whether records should bubble on to later handlers (default `true`).
        pub fn with_bubble(mut self, bubble: bool) -> Self {
            self.formatting.bubble = bubble;
            self
        }

        /// Use the given formatter.
        pub fn with_formatter(self, formatter: impl Formatter + 'static) -> Self {
            *self.formatting.formatter.write().unwrap() = Arc::new(formatter);
            self
        }

        /// The minimum level this handler handles.
        pub fn level(&self) -> Level {
            self.formatting.level
        }
    };
}

// ----------------------------------------------------------------------
// Streams and files
// ----------------------------------------------------------------------

/// Where a [`StreamHandler`] writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stream {
    /// Append to a file.
    Path(PathBuf),
    /// Write to standard output.
    Stdout,
    /// Write to standard error.
    Stderr,
}

impl Stream {
    /// Parse a stream URL: `php://stderr`, `php://stdout` (or
    /// `php://output`), `stderr`, `stdout`, or a file path.
    pub fn parse(url: &str) -> Stream {
        match url.trim() {
            "php://stderr" | "stderr" => Stream::Stderr,
            "php://stdout" | "php://output" | "stdout" => Stream::Stdout,
            path => Stream::Path(PathBuf::from(path.strip_prefix("file://").unwrap_or(path))),
        }
    }
}

impl From<&str> for Stream {
    fn from(url: &str) -> Self {
        Stream::parse(url)
    }
}

impl From<String> for Stream {
    fn from(url: String) -> Self {
        Stream::parse(&url)
    }
}

impl From<PathBuf> for Stream {
    fn from(path: PathBuf) -> Self {
        Stream::Path(path)
    }
}

impl From<&Path> for Stream {
    fn from(path: &Path) -> Self {
        Stream::Path(path.to_path_buf())
    }
}

/// One lock per log file, shared by every handler writing to it.
static FILE_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn file_lock(path: &Path) -> Arc<Mutex<()>> {
    FILE_LOCKS
        .lock()
        .unwrap()
        .entry(path.to_path_buf())
        .or_default()
        .clone()
}

/// Open a log file for appending, creating its directory if needed.
fn open_append(path: &Path, permission: Option<u32>) -> Result<File> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty())
        && !dir.is_dir()
    {
        fs::create_dir_all(dir).map_err(|error| {
            RuntimeException::new(format!(
                "There is no existing directory at \"{}\" and it could not be created: {error}",
                dir.display()
            ))
        })?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| {
            RuntimeException::new(format!(
                "The stream or file \"{}\" could not be opened in append mode: {error}",
                path.display()
            ))
        })?;
    if let Some(mode) = permission {
        set_permission(path, mode);
    }
    Ok(file)
}

#[cfg(unix)]
fn set_permission(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_permission(_path: &Path, _mode: u32) {}

fn write_stream(
    stream: &Stream,
    file: &Mutex<Option<File>>,
    permission: Option<u32>,
    line: &str,
) -> Result<()> {
    match stream {
        Stream::Stdout => {
            let mut out = std::io::stdout().lock();
            out.write_all(line.as_bytes())?;
            out.flush()?;
        }
        Stream::Stderr => {
            let mut out = std::io::stderr().lock();
            out.write_all(line.as_bytes())?;
            out.flush()?;
        }
        Stream::Path(path) => {
            let lock = file_lock(path);
            let _guard = lock.lock().unwrap();
            let mut file = file.lock().unwrap();
            if file.is_none() {
                *file = Some(open_append(path, permission)?);
            }
            if let Some(handle) = file.as_mut() {
                handle.write_all(line.as_bytes())?;
            }
        }
    }
    Ok(())
}

/// Writes records to a file or a standard stream (Monolog's `StreamHandler`).
///
/// ```no_run
/// use illuminate_log::{Level, Monolog, StreamHandler};
///
/// let logger = Monolog::new("local")
///     .with_handler(StreamHandler::new("storage/logs/laravel.log").with_level(Level::Info))
///     .with_handler(StreamHandler::new("php://stderr").with_level(Level::Error));
/// ```
pub struct StreamHandler {
    stream: Stream,
    permission: Option<u32>,
    formatting: Formatting,
    file: Mutex<Option<File>>,
}

impl StreamHandler {
    /// Create a handler for the given stream (`"php://stderr"`, a path, ...).
    pub fn new(stream: impl Into<Stream>) -> Self {
        Self {
            stream: stream.into(),
            permission: None,
            formatting: Formatting::new(Arc::new(LineFormatter::new())),
            file: Mutex::new(None),
        }
    }

    /// Write to standard error.
    pub fn stderr() -> Self {
        Self::new(Stream::Stderr)
    }

    /// Write to standard output.
    pub fn stdout() -> Self {
        Self::new(Stream::Stdout)
    }

    formatting_builders!();

    /// Set the file's permissions (e.g. `0o664`) when it's opened.
    pub fn with_permission(mut self, permission: Option<u32>) -> Self {
        self.permission = permission;
        self
    }

    /// Where this handler writes.
    pub fn stream(&self) -> &Stream {
        &self.stream
    }
}

impl Handler for StreamHandler {
    fn is_handling(&self, level: Level) -> bool {
        self.formatting.is_handling(level)
    }

    fn handle(&self, record: &LogRecord) -> Result<bool> {
        if !self.is_handling(record.level) {
            return Ok(false);
        }
        let line = self.formatting.format(record);
        write_stream(&self.stream, &self.file, self.permission, &line)?;
        Ok(!self.formatting.bubble)
    }

    fn set_formatter(&self, formatter: Arc<dyn Formatter>) {
        *self.formatting.formatter.write().unwrap() = formatter;
    }

    fn close(&self) {
        *self.file.lock().unwrap() = None;
    }
}

impl fmt::Debug for StreamHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamHandler")
            .field("stream", &self.stream)
            .field("level", &self.formatting.level)
            .finish_non_exhaustive()
    }
}

struct RotatingState {
    url: Option<PathBuf>,
    file: Mutex<Option<File>>,
}

/// Writes to one file per day (or month), deleting the oldest files beyond
/// the configured maximum (Monolog's `RotatingFileHandler`).
///
/// A `path` of `storage/logs/laravel.log` writes to
/// `storage/logs/laravel-2024-01-01.log`.
pub struct RotatingFileHandler {
    filename: PathBuf,
    max_files: usize,
    date_format: String,
    permission: Option<u32>,
    formatting: Formatting,
    state: Mutex<RotatingState>,
}

impl RotatingFileHandler {
    /// One file per day.
    pub const FILE_PER_DAY: &'static str = "Y-m-d";
    /// One file per month.
    pub const FILE_PER_MONTH: &'static str = "Y-m";
    /// One file per year.
    pub const FILE_PER_YEAR: &'static str = "Y";

    /// Create a handler keeping at most `max_files` files (`0` keeps them all).
    pub fn new(filename: impl Into<PathBuf>, max_files: usize) -> Self {
        Self {
            filename: filename.into(),
            max_files,
            date_format: Self::FILE_PER_DAY.to_string(),
            permission: None,
            formatting: Formatting::new(Arc::new(LineFormatter::new())),
            state: Mutex::new(RotatingState {
                url: None,
                file: Mutex::new(None),
            }),
        }
    }

    formatting_builders!();

    /// Rotate with a different date format (see the `FILE_PER_*` constants).
    pub fn with_date_format(mut self, format: impl Into<String>) -> Self {
        self.date_format = format.into();
        self
    }

    /// Set the file's permissions (e.g. `0o664`) when it's opened.
    pub fn with_permission(mut self, permission: Option<u32>) -> Self {
        self.permission = permission;
        self
    }

    /// The file a record logged at the given moment is written to.
    pub fn timed_filename(&self, date: &Carbon) -> PathBuf {
        let stem = self
            .filename
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut name = format!("{stem}-{}", date.format(&self.date_format));
        if let Some(extension) = self.filename.extension() {
            name.push('.');
            name.push_str(&extension.to_string_lossy());
        }
        match self.filename.parent() {
            Some(dir) => dir.join(name),
            None => PathBuf::from(name),
        }
    }

    /// Delete the oldest log files beyond `max_files`.
    fn rotate(&self) {
        if self.max_files == 0 {
            return;
        }
        let Some(pattern) = self.file_pattern() else {
            return;
        };
        let dir = match self.filename.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| pattern.is_match(&name.to_string_lossy()))
            })
            .collect();
        if files.len() <= self.max_files {
            return;
        }
        // Sort by name, newest first, and remove everything past the limit.
        files.sort_by(|a, b| b.cmp(a));
        for file in files.into_iter().skip(self.max_files) {
            let _ = fs::remove_file(file);
        }
    }

    /// A pattern matching every file this handler may have written.
    fn file_pattern(&self) -> Option<Regex> {
        let stem = self.filename.file_stem()?.to_string_lossy().into_owned();
        let mut date = String::new();
        for c in self.date_format.chars() {
            match c {
                'Y' => date.push_str(r"\d{4}"),
                'y' | 'm' | 'd' => date.push_str(r"\d{2}"),
                other => date.push_str(&regex::escape(&other.to_string())),
            }
        }
        let extension = self
            .filename
            .extension()
            .map(|ext| format!(r"\.{}", regex::escape(&ext.to_string_lossy())))
            .unwrap_or_default();
        Regex::new(&format!("^{}-{date}{extension}$", regex::escape(&stem))).ok()
    }
}

impl Handler for RotatingFileHandler {
    fn is_handling(&self, level: Level) -> bool {
        self.formatting.is_handling(level)
    }

    fn handle(&self, record: &LogRecord) -> Result<bool> {
        if !self.is_handling(record.level) {
            return Ok(false);
        }
        let line = self.formatting.format(record);
        let target = self.timed_filename(&record.datetime);

        let mut state = self.state.lock().unwrap();
        let mut must_rotate = false;
        if state.url.as_ref() != Some(&target) {
            // A new day (or the first write): rotate once the file exists.
            must_rotate = state.url.is_some() || !target.exists();
            state.url = Some(target.clone());
            state.file = Mutex::new(None);
        }
        write_stream(&Stream::Path(target), &state.file, self.permission, &line)?;
        if must_rotate {
            self.rotate();
        }
        Ok(!self.formatting.bubble)
    }

    fn set_formatter(&self, formatter: Arc<dyn Formatter>) {
        *self.formatting.formatter.write().unwrap() = formatter;
    }

    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.url = None;
        state.file = Mutex::new(None);
    }
}

impl fmt::Debug for RotatingFileHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RotatingFileHandler")
            .field("filename", &self.filename)
            .field("max_files", &self.max_files)
            .field("date_format", &self.date_format)
            .finish_non_exhaustive()
    }
}

// ----------------------------------------------------------------------
// Null, test, and group handlers
// ----------------------------------------------------------------------

/// Swallows every record at or above its level (Monolog's `NullHandler`).
#[derive(Debug, Clone, Copy)]
pub struct NullHandler {
    level: Level,
}

impl NullHandler {
    /// Discard every record.
    pub fn new() -> Self {
        Self {
            level: Level::Debug,
        }
    }

    /// Discard records at or above the given level.
    pub fn with_level(level: Level) -> Self {
        Self { level }
    }
}

impl Default for NullHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl Handler for NullHandler {
    fn is_handling(&self, level: Level) -> bool {
        self.level.includes(level)
    }

    fn handle(&self, record: &LogRecord) -> Result<bool> {
        Ok(self.is_handling(record.level))
    }
}

/// Keeps records in memory, which is wonderful for testing.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_log::{Level, Logger, Monolog, TestHandler};
///
/// let handler = Arc::new(TestHandler::new());
/// let logger = Logger::new(Monolog::new("testing").with_handler_arc(handler.clone()));
///
/// logger.warning("Disk space is low.");
///
/// assert!(handler.has_record(Level::Warning, "Disk space is low."));
/// ```
pub struct TestHandler {
    formatting: Formatting,
    records: Mutex<Vec<(LogRecord, String)>>,
}

impl TestHandler {
    /// Create an in-memory handler.
    pub fn new() -> Self {
        Self {
            formatting: Formatting::new(Arc::new(LineFormatter::new())),
            records: Mutex::new(Vec::new()),
        }
    }

    formatting_builders!();

    /// Every handled record.
    pub fn records(&self) -> Vec<LogRecord> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|(r, _)| r.clone())
            .collect()
    }

    /// Every handled record, formatted.
    pub fn formatted(&self) -> Vec<String> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|(_, f)| f.clone())
            .collect()
    }

    /// The handled messages.
    pub fn messages(&self) -> Vec<String> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|(r, _)| r.message.clone())
            .collect()
    }

    /// Whether any record of the given level was handled.
    pub fn has_records(&self, level: Level) -> bool {
        self.records
            .lock()
            .unwrap()
            .iter()
            .any(|(r, _)| r.level == level)
    }

    /// Whether a record with the given level and message was handled.
    pub fn has_record(&self, level: Level, message: &str) -> bool {
        self.records
            .lock()
            .unwrap()
            .iter()
            .any(|(r, _)| r.level == level && r.message == message)
    }

    /// Whether a record of the given level containing `needle` was handled.
    pub fn has_record_that_contains(&self, level: Level, needle: &str) -> bool {
        self.records
            .lock()
            .unwrap()
            .iter()
            .any(|(r, _)| r.level == level && r.message.contains(needle))
    }

    /// Forget every handled record.
    pub fn clear(&self) {
        self.records.lock().unwrap().clear();
    }
}

impl Default for TestHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl Handler for TestHandler {
    fn is_handling(&self, level: Level) -> bool {
        self.formatting.is_handling(level)
    }

    fn handle(&self, record: &LogRecord) -> Result<bool> {
        if !self.is_handling(record.level) {
            return Ok(false);
        }
        let formatted = self.formatting.format(record);
        self.records
            .lock()
            .unwrap()
            .push((record.clone(), formatted));
        Ok(!self.formatting.bubble)
    }

    fn set_formatter(&self, formatter: Arc<dyn Formatter>) {
        *self.formatting.formatter.write().unwrap() = formatter;
    }
}

/// Sends records to several handlers, ignoring any failures (Monolog's
/// `WhatFailureGroupHandler`). Stacks use it for `ignore_exceptions`.
pub struct WhatFailureGroupHandler {
    handlers: Vec<Arc<dyn Handler>>,
}

impl WhatFailureGroupHandler {
    /// Group the given handlers.
    pub fn new(handlers: Vec<Arc<dyn Handler>>) -> Self {
        Self { handlers }
    }
}

impl Handler for WhatFailureGroupHandler {
    fn is_handling(&self, level: Level) -> bool {
        self.handlers
            .iter()
            .any(|handler| handler.is_handling(level))
    }

    fn handle(&self, record: &LogRecord) -> Result<bool> {
        for handler in &self.handlers {
            let _ = handler.handle(record);
        }
        Ok(false)
    }

    fn set_formatter(&self, formatter: Arc<dyn Formatter>) {
        for handler in &self.handlers {
            handler.set_formatter(formatter.clone());
        }
    }

    fn close(&self) {
        for handler in &self.handlers {
            handler.close();
        }
    }
}

// ----------------------------------------------------------------------
// Syslog
// ----------------------------------------------------------------------

/// Parse a syslog facility from configuration: PHP's numeric constants
/// (`LOG_USER` is `8`) or names like `"user"` and `"local0"`.
pub fn syslog_facility(value: &illuminate_support::Value) -> u8 {
    use illuminate_support::ValueExt;
    if let Some(number) = value.as_u64() {
        return number.min(184) as u8;
    }
    let name = value.to_string_lossy().to_ascii_lowercase();
    let name = name.trim_start_matches("log_");
    match name {
        "kern" => 0,
        "mail" => 16,
        "daemon" => 24,
        "auth" => 32,
        "syslog" => 40,
        "lpr" => 48,
        "news" => 56,
        "uucp" => 64,
        "cron" => 72,
        "authpriv" => 80,
        local if local.starts_with("local") => local[5..]
            .parse::<u8>()
            .ok()
            .filter(|n| *n <= 7)
            .map(|n| 128 + n * 8)
            .unwrap_or(8),
        _ => name.parse::<u8>().unwrap_or(8),
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            fs::read_to_string("/etc/hostname")
                .ok()
                .map(|h| h.trim().to_string())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_else(|| "-".to_string())
}

/// Sends records to a remote syslog server over UDP using RFC 5424
/// (Monolog's `SyslogUdpHandler`, as used by the Papertrail channel).
pub struct SyslogUdpHandler {
    address: String,
    ident: String,
    facility: u8,
    formatting: Formatting,
    socket: Mutex<Option<std::net::UdpSocket>>,
}

impl SyslogUdpHandler {
    /// Send records to `host:port`.
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            address: format!("{host}:{port}"),
            ident: "php".to_string(),
            facility: 8,
            formatting: Formatting::new(Arc::new(
                LineFormatter::new()
                    .with_format("%channel%.%level_name%: %message% %context% %extra%"),
            )),
            socket: Mutex::new(None),
        }
    }

    formatting_builders!();

    /// The program name sent with each record.
    pub fn with_ident(mut self, ident: impl Into<String>) -> Self {
        self.ident = ident.into();
        self
    }

    /// The syslog facility (see [`syslog_facility`]).
    pub fn with_facility(mut self, facility: u8) -> Self {
        self.facility = facility;
        self
    }
}

impl Handler for SyslogUdpHandler {
    fn is_handling(&self, level: Level) -> bool {
        self.formatting.is_handling(level)
    }

    fn handle(&self, record: &LogRecord) -> Result<bool> {
        if !self.is_handling(record.level) {
            return Ok(false);
        }
        let formatted = self.formatting.format(record);
        let priority = self.facility as u16 + record.level.to_rfc5424() as u16;
        let header = format!(
            "<{priority}>1 {} {} {} {} - - ",
            record.datetime.format("Y-m-d\\TH:i:sP"),
            hostname(),
            self.ident,
            std::process::id()
        );
        let mut socket = self.socket.lock().unwrap();
        if socket.is_none() {
            *socket = Some(std::net::UdpSocket::bind("0.0.0.0:0")?);
        }
        if let Some(socket) = socket.as_ref() {
            for line in formatted.lines().filter(|line| !line.is_empty()) {
                let mut chunk = format!("{header}{line}").into_bytes();
                chunk.truncate(65023);
                socket.send_to(&chunk, &self.address)?;
            }
        }
        Ok(!self.formatting.bubble)
    }

    fn set_formatter(&self, formatter: Arc<dyn Formatter>) {
        *self.formatting.formatter.write().unwrap() = formatter;
    }

    fn close(&self) {
        *self.socket.lock().unwrap() = None;
    }
}

/// Sends records to the local syslog daemon (Monolog's `SyslogHandler`).
pub struct SyslogHandler {
    ident: String,
    facility: u8,
    socket_path: PathBuf,
    formatting: Formatting,
}

impl SyslogHandler {
    /// Log as `ident` with the given facility to `/dev/log`.
    pub fn new(ident: impl Into<String>, facility: u8) -> Self {
        Self {
            ident: ident.into(),
            facility,
            socket_path: PathBuf::from("/dev/log"),
            formatting: Formatting::new(Arc::new(
                LineFormatter::new()
                    .with_format("%channel%.%level_name%: %message% %context% %extra%"),
            )),
        }
    }

    formatting_builders!();

    /// Send to a different syslog socket.
    pub fn with_socket_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.socket_path = path.into();
        self
    }

    #[cfg(unix)]
    fn send(&self, message: &str) -> Result<()> {
        let socket = std::os::unix::net::UnixDatagram::unbound()?;
        socket.send_to(message.as_bytes(), &self.socket_path)?;
        Ok(())
    }

    #[cfg(not(unix))]
    fn send(&self, _message: &str) -> Result<()> {
        Err(RuntimeException::new("The syslog driver requires a Unix system.").into())
    }
}

impl Handler for SyslogHandler {
    fn is_handling(&self, level: Level) -> bool {
        self.formatting.is_handling(level)
    }

    fn handle(&self, record: &LogRecord) -> Result<bool> {
        if !self.is_handling(record.level) {
            return Ok(false);
        }
        let formatted = self.formatting.format(record);
        let priority = self.facility as u16 + record.level.to_rfc5424() as u16;
        let timestamp = format!(
            "{} {:>2} {}",
            record.datetime.format("M"),
            record.datetime.day(),
            record.datetime.format("H:i:s")
        );
        let message = format!(
            "<{priority}>{timestamp} {}[{}]: {}",
            self.ident,
            std::process::id(),
            formatted.trim_end_matches('\n')
        );
        self.send(&message).map_err(|error| {
            RuntimeException::new(format!(
                "Failed to send to syslog at \"{}\": {error}",
                self.socket_path.display()
            ))
        })?;
        Ok(!self.formatting.bubble)
    }

    fn set_formatter(&self, formatter: Arc<dyn Formatter>) {
        *self.formatting.formatter.write().unwrap() = formatter;
    }
}
