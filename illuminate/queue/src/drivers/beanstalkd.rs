//! The `beanstalkd` driver: jobs are [Beanstalkd](https://beanstalkd.github.io/)
//! jobs, in a tube per queue.
//!
//! Exactly like Laravel's `BeanstalkdQueue`, jobs are put with the default
//! priority (1024) and a time-to-run of the connection's `retry_after`, a
//! worker reserves the next job of its tube (waiting up to `block_for`
//! seconds), attempts are the number of times Beanstalkd has reserved the
//! job, and releasing a job puts it back with a delay.
//!
//! ```json
//! "beanstalkd": {
//!     "driver": "beanstalkd",
//!     "host": "localhost",
//!     "queue": "default",
//!     "retry_after": 90,
//!     "block_for": 0,
//!     "after_commit": false
//! }
//! ```
//!
//! The client speaks the Beanstalkd [protocol](https://github.com/beanstalkd/beanstalkd/blob/master/doc/protocol.txt)
//! over Tokio TCP connections: one for putting jobs and one for reserving
//! them (Beanstalkd only lets the connection that reserved a job delete,
//! release or bury it, so jobs keep using the connection that reserved
//! them).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufStream};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Map, Result, Value, ValueExt};

use crate::contracts::Queue;
use crate::delay::ceil_seconds;
use crate::queued_job::{JobBackend, QueuedJob};

/// The port Beanstalkd listens on by default.
pub const DEFAULT_PORT: u16 = 11300;

/// The priority jobs are put and released with (Pheanstalk's default).
pub const DEFAULT_PRIORITY: u32 = 1024;

/// The time-to-run of jobs when the connection has no `retry_after`.
pub const DEFAULT_TTR: u64 = 60;

/// The longest tube name Beanstalkd accepts.
const MAX_TUBE_NAME_LENGTH: usize = 200;

/// The error returned when Beanstalkd reports an error or replies unexpectedly.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Beanstalkd error: {message}")]
pub struct BeanstalkdException {
    /// What went wrong.
    pub message: String,
}

impl BeanstalkdException {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn unexpected(reply: &str) -> illuminate_support::Error {
        Self::new(format!("Unexpected reply [{reply}].")).into()
    }
}

/// A reserved job: its id and its data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedJob {
    /// The job's id.
    pub id: u64,
    /// The job's data (the payload).
    pub data: String,
}

/// An open connection, and the tubes it uses and watches.
struct Connection {
    stream: BufStream<TcpStream>,
    using: String,
    watching: Vec<String>,
}

impl Connection {
    async fn command(&mut self, command: &[u8]) -> Result<String> {
        self.stream.write_all(command).await?;
        self.stream.flush().await?;
        self.read_line().await
    }

    async fn read_line(&mut self) -> Result<String> {
        let mut line = String::new();
        if self.stream.read_line(&mut line).await? == 0 {
            return Err(
                BeanstalkdException::new("The connection was closed by the server.").into(),
            );
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if matches!(
            line.as_str(),
            "OUT_OF_MEMORY" | "INTERNAL_ERROR" | "BAD_FORMAT" | "UNKNOWN_COMMAND"
        ) {
            return Err(BeanstalkdException::new(line).into());
        }
        Ok(line)
    }

    async fn read_data(&mut self, length: usize) -> Result<Vec<u8>> {
        let mut data = vec![0; length + 2];
        self.stream.read_exact(&mut data).await?;
        data.truncate(length);
        Ok(data)
    }

    /// Read the YAML body of an `OK <bytes>` reply.
    async fn read_yaml(&mut self, reply: &str) -> Result<Vec<u8>> {
        let length = reply
            .strip_prefix("OK ")
            .and_then(|length| length.parse().ok())
            .ok_or_else(|| BeanstalkdException::unexpected(reply))?;
        self.read_data(length).await
    }

    /// Make the connection put jobs into the given tube.
    async fn use_tube(&mut self, tube: &str) -> Result<()> {
        if self.using == tube {
            return Ok(());
        }
        let reply = self.command(format!("use {tube}\r\n").as_bytes()).await?;
        if reply != format!("USING {tube}") {
            return Err(BeanstalkdException::unexpected(&reply));
        }
        self.using = tube.to_string();
        Ok(())
    }

    /// Make the connection watch only the given tube (Laravel watches the
    /// tube, then ignores every other watched tube).
    async fn watch_only(&mut self, tube: &str) -> Result<()> {
        if self.watching == [tube] {
            return Ok(());
        }
        if !self.watching.iter().any(|watched| watched == tube) {
            let reply = self.command(format!("watch {tube}\r\n").as_bytes()).await?;
            if !reply.starts_with("WATCHING ") {
                return Err(BeanstalkdException::unexpected(&reply));
            }
            self.watching.push(tube.to_string());
        }
        for watched in std::mem::take(&mut self.watching) {
            if watched == tube {
                continue;
            }
            let reply = self
                .command(format!("ignore {watched}\r\n").as_bytes())
                .await?;
            if !reply.starts_with("WATCHING ") && reply != "NOT_IGNORED" {
                return Err(BeanstalkdException::unexpected(&reply));
            }
        }
        self.watching = vec![tube.to_string()];
        Ok(())
    }
}

/// Parse the `key: value` lines of a stats reply.
fn parse_stats(yaml: &[u8]) -> Map<String, Value> {
    String::from_utf8_lossy(yaml)
        .lines()
        .filter_map(|line| line.split_once(": "))
        .map(|(key, value)| {
            let value = value.trim();
            let value = value
                .parse::<i64>()
                .map(Value::from)
                .unwrap_or_else(|_| Value::String(value.to_string()));
            (key.trim().to_string(), value)
        })
        .collect()
}

/// Validate a tube name the way Beanstalkd will.
fn check_tube(tube: &str) -> Result<()> {
    let valid = !tube.is_empty()
        && tube.len() <= MAX_TUBE_NAME_LENGTH
        && !tube.starts_with('-')
        && tube.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '-' | '+' | '/' | ';' | '.' | '$' | '_' | '(' | ')')
        });
    if valid {
        Ok(())
    } else {
        Err(InvalidArgumentException::new(format!(
            "The tube name [{tube}] is not valid: tubes are at most 200 letters, digits and \"-+/;.$_()\", and may not begin with a hyphen."
        ))
        .into())
    }
}

/// A client holding a single connection to a Beanstalkd server (what
/// Pheanstalk is to Laravel).
///
/// The connection is opened on first use, and reopened after a failure.
/// Commands are serialized: a `reserve` waiting for a job holds the
/// connection until it returns.
pub struct Beanstalkd {
    host: String,
    port: u16,
    connect_timeout: Duration,
    connection: Mutex<Option<Connection>>,
}

impl fmt::Debug for Beanstalkd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Beanstalkd")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl Beanstalkd {
    /// Create a client for the server at the given host and port.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            connect_timeout: Duration::from_secs(10),
            connection: Mutex::new(None),
        }
    }

    /// Give up connecting after the given time (the connection's `timeout`).
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// The server's host.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The server's port.
    pub fn port(&self) -> u16 {
        self.port
    }

    async fn connect(&self) -> Result<Connection> {
        let address = format!("{}:{}", self.host, self.port);
        let stream = tokio::time::timeout(self.connect_timeout, TcpStream::connect(&address))
            .await
            .map_err(|_| {
                BeanstalkdException::new(format!(
                    "Timed out connecting to Beanstalkd at [{address}]."
                ))
            })?
            .map_err(|error| {
                BeanstalkdException::new(format!(
                    "Unable to connect to Beanstalkd at [{address}]: {error}"
                ))
            })?;
        stream.set_nodelay(true)?;
        Ok(Connection {
            stream: BufStream::new(stream),
            using: "default".to_string(),
            watching: vec!["default".to_string()],
        })
    }

    /// Run an exchange on the connection (opening it when needed).
    ///
    /// The connection is only kept when the exchange succeeds: after an
    /// error — or when the exchange is cancelled halfway — the reply may
    /// never have been read, so the next command starts on a fresh
    /// connection.
    async fn with<T>(&self, exchange: impl AsyncFnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut guard = self.connection.lock().await;
        let mut connection = match guard.take() {
            Some(connection) => connection,
            None => self.connect().await?,
        };
        let result = exchange(&mut connection).await;
        if result.is_ok() {
            *guard = Some(connection);
        }
        result
    }

    /// Put a job into a tube, returning its id.
    pub async fn put(
        &self,
        tube: &str,
        data: &[u8],
        priority: u32,
        delay: u64,
        ttr: u64,
    ) -> Result<u64> {
        check_tube(tube)?;
        self.with(async |connection: &mut Connection| {
            connection.use_tube(tube).await?;
            let mut command =
                format!("put {priority} {delay} {ttr} {}\r\n", data.len()).into_bytes();
            command.extend_from_slice(data);
            command.extend_from_slice(b"\r\n");
            let reply = connection.command(&command).await?;
            match reply.split_once(' ') {
                Some(("INSERTED", id)) => id
                    .parse()
                    .map_err(|_| BeanstalkdException::unexpected(&reply)),
                Some(("BURIED", _)) => Err(BeanstalkdException::new(
                    "The server ran out of memory and buried the job.",
                )
                .into()),
                _ => Err(BeanstalkdException::new(reply).into()),
            }
        })
        .await
    }

    /// Reserve the next job of a tube, waiting up to `timeout` seconds
    /// (`0` returns right away). Returns `None` when no job arrived.
    pub async fn reserve(&self, tube: &str, timeout: u64) -> Result<Option<ReservedJob>> {
        check_tube(tube)?;
        self.with(async |connection: &mut Connection| {
            connection.watch_only(tube).await?;
            let reply = connection
                .command(format!("reserve-with-timeout {timeout}\r\n").as_bytes())
                .await?;
            if reply == "TIMED_OUT" || reply == "DEADLINE_SOON" {
                return Ok(None);
            }
            let parts: Vec<&str> = reply.split(' ').collect();
            let ["RESERVED", id, length] = parts.as_slice() else {
                return Err(BeanstalkdException::unexpected(&reply));
            };
            let id = id
                .parse()
                .map_err(|_| BeanstalkdException::unexpected(&reply))?;
            let length = length
                .parse()
                .map_err(|_| BeanstalkdException::unexpected(&reply))?;
            let data = connection.read_data(length).await?;
            Ok(Some(ReservedJob {
                id,
                data: String::from_utf8_lossy(&data).into_owned(),
            }))
        })
        .await
    }

    /// Run a command about a job answered by a status line, returning
    /// whether the job was found.
    async fn job_command(&self, command: String, expected: &[&str]) -> Result<bool> {
        self.with(async |connection: &mut Connection| {
            let reply = connection.command(command.as_bytes()).await?;
            if reply == "NOT_FOUND" {
                return Ok(false);
            }
            if expected.contains(&reply.as_str()) {
                return Ok(true);
            }
            Err(BeanstalkdException::unexpected(&reply))
        })
        .await
    }

    /// Delete a job, returning whether it was found.
    pub async fn delete(&self, id: u64) -> Result<bool> {
        self.job_command(format!("delete {id}\r\n"), &["DELETED"])
            .await
    }

    /// Put a reserved job back into its tube after `delay` seconds.
    pub async fn release(&self, id: u64, priority: u32, delay: u64) -> Result<bool> {
        self.job_command(
            format!("release {id} {priority} {delay}\r\n"),
            &["RELEASED", "BURIED"],
        )
        .await
    }

    /// Bury a reserved job.
    pub async fn bury(&self, id: u64, priority: u32) -> Result<bool> {
        self.job_command(format!("bury {id} {priority}\r\n"), &["BURIED"])
            .await
    }

    /// The statistics of a job (`reserves`, `state`, `tube`, ...), or
    /// `None` when it doesn't exist.
    pub async fn stats_job(&self, id: u64) -> Result<Option<Map<String, Value>>> {
        self.stats(format!("stats-job {id}\r\n")).await
    }

    /// The statistics of a tube (`current-jobs-ready`, ...), or `None`
    /// when it doesn't exist.
    pub async fn stats_tube(&self, tube: &str) -> Result<Option<Map<String, Value>>> {
        check_tube(tube)?;
        self.stats(format!("stats-tube {tube}\r\n")).await
    }

    async fn stats(&self, command: String) -> Result<Option<Map<String, Value>>> {
        self.with(async |connection: &mut Connection| {
            let reply = connection.command(command.as_bytes()).await?;
            if reply == "NOT_FOUND" {
                return Ok(None);
            }
            let yaml = connection.read_yaml(&reply).await?;
            Ok(Some(parse_stats(&yaml)))
        })
        .await
    }
}

/// A queue backed by Beanstalkd (`QUEUE_CONNECTION=beanstalkd`).
///
/// ```
/// use illuminate_queue::BeanstalkdQueue;
/// use illuminate_queue::contracts::Queue;
/// use illuminate_support::json;
///
/// let queue = BeanstalkdQueue::from_config(&json!({
///     "driver": "beanstalkd",
///     "host": "localhost",
///     "queue": "default",
///     "retry_after": 90,
///     "block_for": 0,
/// }), "beanstalkd");
///
/// assert_eq!(queue.default_queue(), "default");
/// assert_eq!(queue.get_time_to_run(), 90);
/// assert_eq!(queue.get_pheanstalk().port(), 11300);
/// ```
#[derive(Debug, Clone)]
pub struct BeanstalkdQueue {
    producer: Arc<Beanstalkd>,
    consumer: Arc<Beanstalkd>,
    name: String,
    default_queue: String,
    time_to_run: u64,
    block_for: u64,
    after_commit: bool,
}

impl BeanstalkdQueue {
    /// Create a queue on the Beanstalkd server at the given host and port.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        let host = host.into();
        Self {
            producer: Arc::new(Beanstalkd::new(host.clone(), port)),
            consumer: Arc::new(Beanstalkd::new(host, port)),
            name: "beanstalkd".to_string(),
            default_queue: "default".to_string(),
            time_to_run: DEFAULT_TTR,
            block_for: 0,
            after_commit: false,
        }
    }

    /// Create a queue from a `queue.connections.*` entry (Laravel's
    /// `BeanstalkdConnector`): `host` (default `localhost`), `port` (default
    /// 11300), `timeout` (seconds to wait for a connection), `queue`
    /// (default `default`), `retry_after` (the jobs' time-to-run, default
    /// 60), `block_for` (default 0) and `after_commit`.
    pub fn from_config(config: &Value, name: &str) -> Self {
        let host = config
            .get("host")
            .filter(|host| !host.is_blank())
            .map(ValueExt::to_string_lossy)
            .unwrap_or_else(|| "localhost".to_string());
        let port = config
            .get("port")
            .and_then(ValueExt::to_i64_lossy)
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port > 0)
            .unwrap_or(DEFAULT_PORT);
        let seconds = |key: &str| {
            config
                .get(key)
                .and_then(ValueExt::to_i64_lossy)
                .map(|seconds| seconds.max(0) as u64)
        };

        let mut queue = Self::new(host.clone(), port);
        if let Some(timeout) = config
            .get("timeout")
            .and_then(Value::as_f64)
            .filter(|t| *t > 0.0)
        {
            let timeout = Duration::from_secs_f64(timeout);
            queue.producer =
                Arc::new(Beanstalkd::new(host.clone(), port).with_connect_timeout(timeout));
            queue.consumer = Arc::new(Beanstalkd::new(host, port).with_connect_timeout(timeout));
        }
        queue
            .with_connection_name(name)
            .with_default_queue(
                config
                    .get("queue")
                    .filter(|queue| !queue.is_blank())
                    .map(ValueExt::to_string_lossy)
                    .unwrap_or_else(|| "default".to_string()),
            )
            .with_time_to_run(seconds("retry_after").unwrap_or(DEFAULT_TTR))
            .with_block_for(seconds("block_for").unwrap_or(0))
            .with_after_commit(config.get("after_commit").is_some_and(ValueExt::truthy))
    }

    /// Set the name of the queue connection.
    pub fn with_connection_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the default queue (tube) name.
    pub fn with_default_queue(mut self, queue: impl Into<String>) -> Self {
        self.default_queue = queue.into();
        self
    }

    /// Set the time-to-run of jobs, in seconds: Beanstalkd hands a job to
    /// another worker when it runs longer.
    pub fn with_time_to_run(mut self, seconds: u64) -> Self {
        self.time_to_run = seconds;
        self
    }

    /// Wait up to the given number of seconds for a job when the tube is empty.
    pub fn with_block_for(mut self, seconds: u64) -> Self {
        self.block_for = seconds;
        self
    }

    /// Dispatch jobs after open database transactions commit.
    pub fn with_after_commit(mut self, after_commit: bool) -> Self {
        self.after_commit = after_commit;
        self
    }

    /// The client jobs are put with (Laravel's `getPheanstalk`).
    pub fn get_pheanstalk(&self) -> &Beanstalkd {
        &self.producer
    }

    /// The time-to-run of jobs, in seconds.
    pub fn get_time_to_run(&self) -> u64 {
        self.time_to_run
    }

    /// How long a pop waits for a job, in seconds.
    pub fn get_block_for(&self) -> u64 {
        self.block_for
    }

    /// The tube of a queue (the default queue when `None`).
    pub fn get_queue<'a>(&'a self, queue: Option<&'a str>) -> &'a str {
        match queue {
            Some(queue) if !queue.is_empty() => queue,
            _ => &self.default_queue,
        }
    }

    /// Delete a job by its id (Laravel's `deleteMessage`).
    pub async fn delete_message(&self, _queue: Option<&str>, id: u64) -> Result<bool> {
        self.consumer.delete(id).await
    }

    /// Bury a reserved job, so it is kept but no longer handed to workers
    /// (Laravel's `BeanstalkdJob::bury`).
    pub async fn bury(&self, job: &QueuedJob) -> Result<bool> {
        let id = job.job_id().parse().map_err(|_| {
            InvalidArgumentException::new(format!("Invalid job id [{}].", job.job_id()))
        })?;
        self.consumer.bury(id, DEFAULT_PRIORITY).await
    }

    /// The number of jobs in the given `current-jobs-*` states of a tube.
    async fn count(&self, queue: Option<&str>, states: &[&str]) -> Result<u64> {
        let Some(stats) = self.producer.stats_tube(self.get_queue(queue)).await? else {
            return Ok(0);
        };
        Ok(states
            .iter()
            .map(|state| {
                stats
                    .get(&format!("current-jobs-{state}"))
                    .and_then(ValueExt::to_i64_lossy)
                    .unwrap_or(0)
                    .max(0) as u64
            })
            .sum())
    }

    /// The number of jobs across every tube (not supported: always zero, like Laravel).
    pub async fn total_size(&self) -> Result<u64> {
        Ok(0)
    }

    /// The creation time of the oldest pending job (not supported by Beanstalkd).
    pub async fn creation_time_of_oldest_pending_job(
        &self,
        _queue: Option<&str>,
    ) -> Result<Option<i64>> {
        Ok(None)
    }
}

#[async_trait]
impl Queue for BeanstalkdQueue {
    fn connection_name(&self) -> &str {
        &self.name
    }

    fn default_queue(&self) -> &str {
        &self.default_queue
    }

    fn dispatches_after_commit(&self) -> bool {
        self.after_commit
    }

    /// The ready, delayed and reserved jobs of the tube.
    async fn size(&self, queue: Option<&str>) -> Result<u64> {
        self.count(queue, &["ready", "delayed", "reserved"]).await
    }

    async fn pending_size(&self, queue: Option<&str>) -> Result<u64> {
        self.count(queue, &["ready"]).await
    }

    async fn delayed_size(&self, queue: Option<&str>) -> Result<u64> {
        self.count(queue, &["delayed"]).await
    }

    async fn reserved_size(&self, queue: Option<&str>) -> Result<u64> {
        self.count(queue, &["reserved"]).await
    }

    async fn push_raw(
        &self,
        payload: String,
        queue: Option<&str>,
        delay: Option<Duration>,
    ) -> Result<Option<String>> {
        let delay = delay.map(ceil_seconds).unwrap_or(0);
        let id = self
            .producer
            .put(
                self.get_queue(queue),
                payload.as_bytes(),
                DEFAULT_PRIORITY,
                delay,
                self.time_to_run,
            )
            .await?;
        Ok(Some(id.to_string()))
    }

    async fn pop(&self, queue: Option<&str>) -> Result<Option<QueuedJob>> {
        let tube = self.get_queue(queue).to_string();
        let Some(reserved) = self.consumer.reserve(&tube, self.block_for).await? else {
            return Ok(None);
        };

        let attempts = self
            .consumer
            .stats_job(reserved.id)
            .await?
            .and_then(|stats| stats.get("reserves").and_then(ValueExt::to_i64_lossy))
            .unwrap_or(1)
            .clamp(0, i64::from(u32::MAX)) as u32;

        let backend: Arc<dyn JobBackend> = Arc::new(BeanstalkdJob {
            beanstalkd: self.consumer.clone(),
            id: reserved.id,
        });
        match QueuedJob::new(
            reserved.id.to_string(),
            reserved.data.clone(),
            attempts,
            &self.name,
            &tube,
            backend,
        ) {
            Ok(job) => Ok(Some(job)),
            Err(error) => {
                // A payload that can't be read can never be processed.
                self.consumer.delete(reserved.id).await?;
                let logged = crate::failed::failer()
                    .log(&self.name, &tube, &reserved.data, &format!("{error:?}"))
                    .await;
                if let Err(error) = logged {
                    crate::report(&error);
                }
                Err(error)
            }
        }
    }
}

/// A job reserved from Beanstalkd (Laravel's `BeanstalkdJob`).
struct BeanstalkdJob {
    beanstalkd: Arc<Beanstalkd>,
    id: u64,
}

#[async_trait]
impl JobBackend for BeanstalkdJob {
    async fn delete(&self, _job: &QueuedJob) -> Result<()> {
        self.beanstalkd.delete(self.id).await?;
        Ok(())
    }

    async fn release(&self, _job: &QueuedJob, delay: Duration) -> Result<()> {
        self.beanstalkd
            .release(self.id, DEFAULT_PRIORITY, ceil_seconds(delay))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn stats_are_parsed() {
        let stats = parse_stats(b"---\nname: default\ncurrent-jobs-ready: 3\nreserves: 2\n");
        assert_eq!(stats["name"], json!("default"));
        assert_eq!(stats["current-jobs-ready"], json!(3));
        assert_eq!(stats["reserves"], json!(2));
    }

    #[test]
    fn tube_names_are_validated() {
        assert!(check_tube("default").is_ok());
        assert!(check_tube("emails.high-priority_(1)").is_ok());
        assert!(check_tube("-nope").is_err());
        assert!(check_tube("has space").is_err());
        assert!(check_tube("").is_err());
        assert!(check_tube(&"a".repeat(201)).is_err());
    }

    #[test]
    fn queues_are_configured_like_laravel() {
        let queue = BeanstalkdQueue::from_config(
            &json!({"host": "queue.test", "port": "11301", "queue": "emails", "retry_after": 90, "block_for": 5, "after_commit": true, "timeout": 2}),
            "jobs",
        );
        assert_eq!(queue.connection_name(), "jobs");
        assert_eq!(queue.get_queue(None), "emails");
        assert_eq!(queue.get_queue(Some("other")), "other");
        assert_eq!(queue.get_time_to_run(), 90);
        assert_eq!(queue.get_block_for(), 5);
        assert!(queue.dispatches_after_commit());
        assert_eq!(queue.get_pheanstalk().host(), "queue.test");
        assert_eq!(queue.get_pheanstalk().port(), 11301);
        assert_eq!(queue.consumer.connect_timeout, Duration::from_secs(2));

        let queue = BeanstalkdQueue::from_config(&json!({}), "beanstalkd");
        assert_eq!(queue.get_pheanstalk().host(), "localhost");
        assert_eq!(queue.get_time_to_run(), DEFAULT_TTR);
        assert_eq!(queue.get_block_for(), 0);
        assert!(!queue.dispatches_after_commit());
    }
}
