//! A Memcached client speaking the [text protocol](https://github.com/memcached/memcached/blob/master/doc/protocol.txt)
//! over Tokio TCP connections (what PHP's `Memcached` class is to Laravel).
//!
//! Keys are spread across the configured servers with weighted
//! [rendezvous hashing](https://en.wikipedia.org/wiki/Rendezvous_hashing):
//! every key always lives on the same server, a server's share of the keys
//! follows its `weight`, and adding or removing a server only moves the
//! keys that belonged to it.
//!
//! ```no_run
//! use illuminate_cache::memcached::{Memcached, MemcachedServer};
//!
//! # async fn example() -> illuminate_support::Result<()> {
//! let memcached = Memcached::new(vec![MemcachedServer::new("127.0.0.1", 11211, 100)]);
//!
//! memcached.set("name", b"Taylor", 0, 0).await?;
//! let item = memcached.get("name").await?.unwrap();
//!
//! assert_eq!(item.value, b"Taylor");
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufStream};
use tokio::net::TcpStream;

use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Result, Value, ValueExt};

/// `Memcached::OPT_POLL_TIMEOUT`: how long to wait for a reply, in milliseconds.
pub const OPT_POLL_TIMEOUT: i64 = 8;

/// `Memcached::OPT_CONNECT_TIMEOUT`: how long to wait for a connection, in milliseconds.
pub const OPT_CONNECT_TIMEOUT: i64 = 14;

/// The longest key Memcached accepts.
const MAX_KEY_LENGTH: usize = 250;

/// The error returned when Memcached reports an error (`ERROR`,
/// `CLIENT_ERROR ...` or `SERVER_ERROR ...`) or replies unexpectedly.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Memcached error: {message}")]
pub struct MemcachedException {
    /// What went wrong.
    pub message: String,
}

impl MemcachedException {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// A Memcached server: its host, port and weight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemcachedServer {
    /// The server's host name or IP address.
    pub host: String,
    /// The server's port (11211 by default).
    pub port: u16,
    /// The server's share of the keys, relative to the other servers.
    pub weight: u32,
}

impl MemcachedServer {
    /// The port Memcached listens on by default.
    pub const DEFAULT_PORT: u16 = 11211;

    /// Describe a server.
    pub fn new(host: impl Into<String>, port: u16, weight: u32) -> Self {
        Self {
            host: host.into(),
            port,
            weight,
        }
    }

    /// Read a server from a `servers` entry: `{"host": ..., "port": ..., "weight": ...}`.
    pub fn from_config(config: &Value) -> Self {
        let host = config
            .get("host")
            .filter(|host| !host.is_blank())
            .map(ValueExt::to_string_lossy)
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let port = config
            .get("port")
            .and_then(ValueExt::to_i64_lossy)
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port > 0)
            .unwrap_or(Self::DEFAULT_PORT);
        let weight = config
            .get("weight")
            .and_then(ValueExt::to_i64_lossy)
            .map(|weight| weight.clamp(0, i64::from(u32::MAX)) as u32)
            .unwrap_or(0);
        Self::new(host, port, weight)
    }

    fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// An item read from Memcached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemcachedItem {
    /// The stored bytes.
    pub value: Vec<u8>,
    /// The flags stored with the item.
    pub flags: u32,
    /// The item's CAS token (only read by [`Memcached::gets`]).
    pub cas: Option<u64>,
}

/// A connection to one server.
struct Connection {
    stream: BufStream<TcpStream>,
}

impl Connection {
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.stream.write_all(bytes).await?;
        self.stream.flush().await?;
        Ok(())
    }

    async fn read_line(&mut self) -> Result<String> {
        let mut line = String::new();
        if self.stream.read_line(&mut line).await? == 0 {
            return Err(MemcachedException::new("The connection was closed by the server.").into());
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line == "ERROR" || line.starts_with("CLIENT_ERROR") || line.starts_with("SERVER_ERROR") {
            return Err(MemcachedException::new(line).into());
        }
        Ok(line)
    }

    async fn read_block(&mut self, length: usize) -> Result<Vec<u8>> {
        let mut block = vec![0; length + 2];
        self.stream.read_exact(&mut block).await?;
        block.truncate(length);
        Ok(block)
    }

    /// Read `VALUE` lines until `END`.
    async fn read_values(&mut self) -> Result<Vec<(String, MemcachedItem)>> {
        let mut items = Vec::new();
        loop {
            let line = self.read_line().await?;
            if line == "END" {
                return Ok(items);
            }
            let parts: Vec<&str> = line.split(' ').collect();
            let (["VALUE", key, flags, length] | ["VALUE", key, flags, length, _]) =
                parts.as_slice()
            else {
                return Err(MemcachedException::new(format!("Unexpected reply [{line}].")).into());
            };
            let parse = |part: &str| {
                part.parse::<u64>()
                    .map_err(|_| MemcachedException::new(format!("Unexpected reply [{line}].")))
            };
            let flags = parse(flags)? as u32;
            let length = parse(length)? as usize;
            let cas = match parts.get(4) {
                Some(cas) => Some(parse(cas)?),
                None => None,
            };
            let value = self.read_block(length).await?;
            items.push((key.to_string(), MemcachedItem { value, flags, cas }));
        }
    }
}

/// A server's reply.
enum Reply {
    Line(String),
    Values(Vec<(String, MemcachedItem)>),
}

/// The connections to one server, kept open between commands.
struct ServerPool {
    server: MemcachedServer,
    idle: Mutex<Vec<Connection>>,
}

/// A client for a pool of Memcached servers.
///
/// Connections are opened on first use and kept for the next command, so
/// creating a client never touches the network.
#[derive(Clone)]
pub struct Memcached {
    servers: Arc<Vec<ServerPool>>,
    credentials: Option<(String, String)>,
    persistent_id: Option<String>,
    connect_timeout: Duration,
    timeout: Duration,
}

impl fmt::Debug for Memcached {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Memcached")
            .field("servers", &self.get_server_list())
            .field("persistent_id", &self.persistent_id)
            .field("authenticated", &self.credentials.is_some())
            .finish_non_exhaustive()
    }
}

impl Memcached {
    /// Create a client for the given servers.
    pub fn new(servers: Vec<MemcachedServer>) -> Self {
        Self {
            servers: Arc::new(
                servers
                    .into_iter()
                    .map(|server| ServerPool {
                        server,
                        idle: Mutex::new(Vec::new()),
                    })
                    .collect(),
            ),
            credentials: None,
            persistent_id: None,
            connect_timeout: Duration::from_secs(4),
            timeout: Duration::from_secs(5),
        }
    }

    /// Create a client from a cache store's configuration (Laravel's
    /// `MemcachedConnector`): `servers`, `persistent_id`, `sasl` and
    /// `options`.
    ///
    /// - `sasl` (`[username, password]`) authenticates with Memcached's
    ///   [text protocol authentication](https://github.com/memcached/memcached/wiki/SASLHowto)
    ///   (servers started with `-Y`), since this client doesn't speak the
    ///   binary protocol SASL needs.
    /// - Of the `options`, `Memcached::OPT_CONNECT_TIMEOUT` (`14`) and
    ///   `Memcached::OPT_POLL_TIMEOUT` (`8`) are honoured, in milliseconds;
    ///   the others only make sense to libmemcached and are ignored.
    /// - `persistent_id` is accepted for compatibility: connections are
    ///   always kept open between commands.
    ///
    /// ```
    /// use illuminate_cache::memcached::Memcached;
    /// use illuminate_support::json;
    ///
    /// let memcached = Memcached::from_config(&json!({
    ///     "servers": [{"host": "127.0.0.1", "port": 11211, "weight": 100}],
    ///     "sasl": [null, null],
    ///     "options": {"14": 2000},
    /// }))
    /// .unwrap();
    ///
    /// assert_eq!(memcached.get_server_list()[0].port, 11211);
    /// ```
    pub fn from_config(config: &Value) -> Result<Self> {
        let servers: Vec<MemcachedServer> = match config.get("servers") {
            Some(Value::Array(servers)) => {
                servers.iter().map(MemcachedServer::from_config).collect()
            }
            Some(Value::Object(_)) => config["servers"]
                .as_object()
                .into_iter()
                .flat_map(|servers| servers.values())
                .map(MemcachedServer::from_config)
                .collect(),
            _ => Vec::new(),
        };
        if servers.is_empty() {
            return Err(InvalidArgumentException::new(
                "The memcached cache driver requires at least one server.",
            )
            .into());
        }

        let mut memcached = Self::new(servers);
        memcached.persistent_id = config
            .get("persistent_id")
            .filter(|id| !id.is_blank())
            .map(ValueExt::to_string_lossy);

        let sasl: Vec<String> = config
            .get("sasl")
            .and_then(Value::as_array)
            .map(|sasl| {
                sasl.iter()
                    .filter(|value| !value.is_blank())
                    .map(ValueExt::to_string_lossy)
                    .collect()
            })
            .unwrap_or_default();
        if let [username, password] = sasl.as_slice() {
            memcached = memcached.with_credentials(username.clone(), password.clone());
        }

        if let Some(options) = config.get("options").and_then(Value::as_object) {
            for (option, value) in options {
                let Some(milliseconds) = value.to_i64_lossy().filter(|ms| *ms > 0) else {
                    continue;
                };
                let duration = Duration::from_millis(milliseconds as u64);
                match option.as_str() {
                    "14" | "OPT_CONNECT_TIMEOUT" | "Memcached::OPT_CONNECT_TIMEOUT" => {
                        memcached.connect_timeout = duration;
                    }
                    "8" | "OPT_POLL_TIMEOUT" | "Memcached::OPT_POLL_TIMEOUT" => {
                        memcached.timeout = duration;
                    }
                    _ => {}
                }
            }
        }
        Ok(memcached)
    }

    /// Authenticate every connection with the given username and password.
    pub fn with_credentials(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.credentials = Some((username.into(), password.into()));
        self
    }

    /// Give up connecting to a server after the given time.
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Give up waiting for a reply after the given time.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The configured servers.
    pub fn get_server_list(&self) -> Vec<MemcachedServer> {
        self.servers
            .iter()
            .map(|pool| pool.server.clone())
            .collect()
    }

    /// The persistent connection identifier, when configured.
    pub fn persistent_id(&self) -> Option<&str> {
        self.persistent_id.as_deref()
    }

    /// The index of the server a key lives on.
    pub fn server_index(&self, key: &str) -> usize {
        let mut best = (0, f64::MIN);
        for (index, pool) in self.servers.iter().enumerate() {
            let weight = f64::from(pool.server.weight.max(1));
            let mut hash = fnv1a(pool.server.address().as_bytes(), FNV_OFFSET);
            hash = fnv1a(b"\0", hash);
            hash = mix(fnv1a(key.as_bytes(), hash));
            // A uniform number in (0, 1), weighted: -weight / ln(u).
            let uniform = ((hash >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
            let score = -weight / uniform.ln();
            if score > best.1 {
                best = (index, score);
            }
        }
        best.0
    }

    /// Validate a key the way Memcached will.
    fn check_key(key: &str) -> Result<()> {
        if key.is_empty()
            || key.len() > MAX_KEY_LENGTH
            || key.bytes().any(|byte| byte <= b' ' || byte == 0x7f)
        {
            return Err(InvalidArgumentException::new(format!(
                "The key [{key}] is not a valid Memcached key: keys are at most 250 bytes, without spaces or control characters."
            ))
            .into());
        }
        Ok(())
    }

    /// Take an idle connection to the server, or open a new one.
    async fn checkout(&self, index: usize) -> Result<Connection> {
        let pool = &self.servers[index];
        if let Some(connection) = pool.idle.lock().unwrap().pop() {
            return Ok(connection);
        }
        let address = pool.server.address();
        let stream = tokio::time::timeout(self.connect_timeout, TcpStream::connect(&address))
            .await
            .map_err(|_| {
                MemcachedException::new(format!(
                    "Timed out connecting to Memcached at [{address}]."
                ))
            })?
            .map_err(|error| {
                MemcachedException::new(format!(
                    "Unable to connect to Memcached at [{address}]: {error}"
                ))
            })?;
        stream.set_nodelay(true)?;
        let mut connection = Connection {
            stream: BufStream::new(stream),
        };
        if let Some((username, password)) = &self.credentials {
            let credentials = format!("{username} {password}");
            connection
                .write(
                    format!("set _auth 0 0 {}\r\n{credentials}\r\n", credentials.len()).as_bytes(),
                )
                .await?;
            let reply = connection.read_line().await?;
            if reply != "STORED" {
                return Err(MemcachedException::new(format!(
                    "Authentication with Memcached at [{address}] failed: {reply}"
                ))
                .into());
            }
        }
        Ok(connection)
    }

    /// Return a healthy connection to the pool.
    fn checkin(&self, index: usize, connection: Connection) {
        self.servers[index].idle.lock().unwrap().push(connection);
    }

    /// Send a command to a server and read its reply. The connection is
    /// only reused when the whole exchange succeeded (or the server
    /// reported an error, which leaves the connection in a known state).
    async fn exchange(&self, index: usize, command: &[u8], values: bool) -> Result<Reply> {
        let mut connection = self.checkout(index).await?;
        let result = tokio::time::timeout(self.timeout, async {
            connection.write(command).await?;
            if values {
                connection.read_values().await.map(Reply::Values)
            } else {
                connection.read_line().await.map(Reply::Line)
            }
        })
        .await
        .map_err(|_| MemcachedException::new("Timed out waiting for Memcached to reply."))?;
        match result {
            Ok(reply) => {
                self.checkin(index, connection);
                Ok(reply)
            }
            Err(error) => {
                if error.downcast_ref::<MemcachedException>().is_some_and(|e| {
                    e.message == "ERROR"
                        || e.message.starts_with("CLIENT_ERROR")
                        || e.message.starts_with("SERVER_ERROR")
                }) {
                    self.checkin(index, connection);
                }
                Err(error)
            }
        }
    }

    /// Send a command answered by a single line.
    async fn line(&self, index: usize, command: &[u8]) -> Result<String> {
        match self.exchange(index, command, false).await? {
            Reply::Line(line) => Ok(line),
            Reply::Values(_) => Err(MemcachedException::new("Unexpected reply.").into()),
        }
    }

    /// Read a single item.
    pub async fn get(&self, key: &str) -> Result<Option<MemcachedItem>> {
        Ok(self.retrieve("get", &[key.to_string()]).await?.remove(key))
    }

    /// Read a single item with its CAS token.
    pub async fn gets(&self, key: &str) -> Result<Option<MemcachedItem>> {
        Ok(self.retrieve("gets", &[key.to_string()]).await?.remove(key))
    }

    /// Read several items at once (one `get` per server).
    pub async fn get_multi(&self, keys: &[String]) -> Result<HashMap<String, MemcachedItem>> {
        self.retrieve("get", keys).await
    }

    async fn retrieve(
        &self,
        command: &str,
        keys: &[String],
    ) -> Result<HashMap<String, MemcachedItem>> {
        let mut by_server: Vec<Vec<&str>> = vec![Vec::new(); self.servers.len()];
        for key in keys {
            Self::check_key(key)?;
            let keys = &mut by_server[self.server_index(key)];
            if !keys.contains(&key.as_str()) {
                keys.push(key);
            }
        }
        let mut items = HashMap::new();
        for (index, keys) in by_server.into_iter().enumerate() {
            if keys.is_empty() {
                continue;
            }
            let line = format!("{command} {}\r\n", keys.join(" "));
            if let Reply::Values(values) = self.exchange(index, line.as_bytes(), true).await? {
                items.extend(values);
            }
        }
        Ok(items)
    }

    /// Run a storage command (`set`, `add`, `replace`, `cas`), returning
    /// whether the item was stored.
    async fn store(
        &self,
        command: &str,
        key: &str,
        value: &[u8],
        flags: u32,
        exptime: i64,
        cas: Option<u64>,
    ) -> Result<bool> {
        Self::check_key(key)?;
        let mut line = format!("{command} {key} {flags} {exptime} {}", value.len());
        if let Some(cas) = cas {
            line.push_str(&format!(" {cas}"));
        }
        line.push_str("\r\n");
        let mut bytes = line.into_bytes();
        bytes.extend_from_slice(value);
        bytes.extend_from_slice(b"\r\n");
        let reply = self.line(self.server_index(key), &bytes).await?;
        match reply.as_str() {
            "STORED" => Ok(true),
            "NOT_STORED" | "EXISTS" | "NOT_FOUND" => Ok(false),
            other => Err(MemcachedException::new(format!("Unexpected reply [{other}].")).into()),
        }
    }

    /// Store an item (`exptime` is seconds, or a UNIX timestamp past 30 days; `0` never expires).
    pub async fn set(&self, key: &str, value: &[u8], flags: u32, exptime: i64) -> Result<bool> {
        self.store("set", key, value, flags, exptime, None).await
    }

    /// Store an item unless it already exists.
    pub async fn add(&self, key: &str, value: &[u8], flags: u32, exptime: i64) -> Result<bool> {
        self.store("add", key, value, flags, exptime, None).await
    }

    /// Store an item only if it already exists.
    pub async fn replace(&self, key: &str, value: &[u8], flags: u32, exptime: i64) -> Result<bool> {
        self.store("replace", key, value, flags, exptime, None)
            .await
    }

    /// Store an item only if nobody changed it since it was read with [`Memcached::gets`].
    pub async fn cas(
        &self,
        cas: u64,
        key: &str,
        value: &[u8],
        flags: u32,
        exptime: i64,
    ) -> Result<bool> {
        self.store("cas", key, value, flags, exptime, Some(cas))
            .await
    }

    /// Run `incr` or `decr`, returning the new value (`None` when the item is missing).
    async fn arithmetic(&self, command: &str, key: &str, by: u64) -> Result<Option<u64>> {
        Self::check_key(key)?;
        let reply = self
            .line(
                self.server_index(key),
                format!("{command} {key} {by}\r\n").as_bytes(),
            )
            .await?;
        if reply == "NOT_FOUND" {
            return Ok(None);
        }
        reply
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| MemcachedException::new(format!("Unexpected reply [{reply}].")).into())
    }

    /// Increment a numeric item, returning the new value.
    pub async fn increment(&self, key: &str, by: u64) -> Result<Option<u64>> {
        self.arithmetic("incr", key, by).await
    }

    /// Decrement a numeric item (never below zero), returning the new value.
    pub async fn decrement(&self, key: &str, by: u64) -> Result<Option<u64>> {
        self.arithmetic("decr", key, by).await
    }

    /// Run a command answered by a single status line.
    async fn status(&self, key: &str, command: String) -> Result<String> {
        Self::check_key(key)?;
        self.line(self.server_index(key), command.as_bytes()).await
    }

    /// Delete an item, returning whether it existed.
    pub async fn delete(&self, key: &str) -> Result<bool> {
        Ok(self.status(key, format!("delete {key}\r\n")).await? == "DELETED")
    }

    /// Change an item's expiration, returning whether it existed.
    pub async fn touch(&self, key: &str, exptime: i64) -> Result<bool> {
        Ok(self
            .status(key, format!("touch {key} {exptime}\r\n"))
            .await?
            == "TOUCHED")
    }

    /// Invalidate every item on every server.
    pub async fn flush(&self) -> Result<bool> {
        let mut flushed = true;
        for index in 0..self.servers.len() {
            let reply = self.line(index, b"flush_all\r\n").await?;
            flushed = flushed && reply == "OK";
        }
        Ok(flushed)
    }
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a, continuing from the given state.
fn fnv1a(bytes: &[u8], mut hash: u64) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Spread the bits of a hash (the `splitmix64` finalizer).
fn mix(mut hash: u64) -> u64 {
    hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^ (hash >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn servers_are_read_from_configuration() {
        let memcached = Memcached::from_config(&json!({
            "persistent_id": "app",
            "sasl": ["user", "secret"],
            "options": {"14": 1500, "8": 250, "-1002": "ignored"},
            "servers": [
                {"host": "10.0.0.1", "port": "11212", "weight": 100},
                {"host": null, "port": null, "weight": null},
            ],
        }))
        .unwrap();
        assert_eq!(
            memcached.get_server_list(),
            vec![
                MemcachedServer::new("10.0.0.1", 11212, 100),
                MemcachedServer::new("127.0.0.1", 11211, 0),
            ]
        );
        assert_eq!(memcached.persistent_id(), Some("app"));
        assert_eq!(memcached.connect_timeout, Duration::from_millis(1500));
        assert_eq!(memcached.timeout, Duration::from_millis(250));
        assert!(format!("{memcached:?}").contains("authenticated: true"));
        assert!(!format!("{memcached:?}").contains("secret"));

        let memcached = Memcached::from_config(&json!({
            "sasl": [null, null],
            "servers": [{"host": "localhost", "port": 11211, "weight": 1}],
        }))
        .unwrap();
        assert!(memcached.credentials.is_none());

        assert!(Memcached::from_config(&json!({"servers": []})).is_err());
    }

    #[test]
    fn keys_are_spread_across_servers_by_weight() {
        let memcached = Memcached::new(vec![
            MemcachedServer::new("a", 11211, 100),
            MemcachedServer::new("b", 11211, 100),
            MemcachedServer::new("c", 11211, 200),
        ]);
        let mut counts = [0usize; 3];
        for i in 0..4000 {
            let key = format!("key-{i}");
            let index = memcached.server_index(&key);
            assert_eq!(index, memcached.server_index(&key));
            counts[index] += 1;
        }
        assert!(counts.iter().all(|count| *count > 500), "{counts:?}");
        assert!(counts[2] > counts[0] && counts[2] > counts[1], "{counts:?}");

        // Removing a server only moves the keys that lived on it.
        let fewer = Memcached::new(vec![
            MemcachedServer::new("a", 11211, 100),
            MemcachedServer::new("c", 11211, 200),
        ]);
        for i in 0..1000 {
            let key = format!("key-{i}");
            match memcached.server_index(&key) {
                0 => assert_eq!(fewer.server_index(&key), 0),
                2 => assert_eq!(fewer.server_index(&key), 1),
                _ => {}
            }
        }
    }

    #[test]
    fn keys_are_validated() {
        assert!(Memcached::check_key("users:1").is_ok());
        assert!(Memcached::check_key("has space").is_err());
        assert!(Memcached::check_key("").is_err());
        assert!(Memcached::check_key(&"a".repeat(251)).is_err());
        assert!(Memcached::check_key("line\nbreak").is_err());
    }
}
