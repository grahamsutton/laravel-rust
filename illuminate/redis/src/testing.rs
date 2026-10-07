//! A throwaway Redis server for your test suite.
//!
//! [`RedisServer::shared`] starts a `redis-server` on a free port the first
//! time a test asks for it, shares it with every test running at the same
//! time, and kills it once the last of them is done. Give each test its own
//! database (or prefix) and they never step on each other:
//!
//! ```no_run
//! use illuminate_redis::testing::RedisServer;
//!
//! let server = RedisServer::shared();
//! let config = server.config(); // A `database.redis` configuration...
//! let database = RedisServer::next_database(); // ...or a fresh database of your own.
//! ```
//!
//! The server binary is `redis-server` from the `PATH`, or the one named by
//! the `REDIS_SERVER_BINARY` environment variable.

use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use illuminate_support::error::RuntimeException;
use illuminate_support::{Result, Value, json};

/// How many databases test servers have, so every test can have its own.
pub const DATABASES: u32 = 1024;

static SHARED: Mutex<Weak<RedisServer>> = Mutex::new(Weak::new());
static NEXT_DATABASE: AtomicU32 = AtomicU32::new(1);

/// A `redis-server` process, killed when dropped.
#[derive(Debug)]
pub struct RedisServer {
    child: Mutex<Child>,
    port: u16,
}

impl RedisServer {
    /// Start a server on a free port, without persistence.
    pub fn start() -> Result<Self> {
        let binary =
            std::env::var("REDIS_SERVER_BINARY").unwrap_or_else(|_| "redis-server".to_string());
        let mut last_error = None;

        // A free port can be taken by someone else before the server binds
        // it, so try a few times.
        for _ in 0..5 {
            let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
            let child = Command::new(&binary)
                .args(["--port", &port.to_string()])
                .args(["--bind", "127.0.0.1"])
                .args(["--save", ""])
                .args(["--appendonly", "no"])
                .args(["--daemonize", "no"])
                .args(["--databases", &DATABASES.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| {
                    RuntimeException::new(format!("Unable to start [{binary}]: {error}"))
                })?;
            let server = Self {
                child: Mutex::new(child),
                port,
            };
            match server.wait_until_ready() {
                Ok(()) => return Ok(server),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error
            .unwrap_or_else(|| RuntimeException::new("Unable to start a Redis server.").into()))
    }

    /// The server shared by the tests currently running, started on demand.
    ///
    /// # Panics
    ///
    /// When the server can't be started.
    pub fn shared() -> Arc<RedisServer> {
        let mut shared = SHARED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(server) = shared.upgrade() {
            return server;
        }
        let server = Arc::new(Self::start().unwrap_or_else(|error| panic!("{error}")));
        *shared = Arc::downgrade(&server);
        server
    }

    /// A database number no other test in this process has been given.
    pub fn next_database() -> u32 {
        // Database 0 is left alone, for anyone poking at the server by hand.
        1 + NEXT_DATABASE.fetch_add(1, Ordering::SeqCst) % (DATABASES - 1)
    }

    /// The port the server listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The server's host.
    pub fn host(&self) -> &'static str {
        "127.0.0.1"
    }

    /// A URL for the given database of the server.
    pub fn url(&self, database: u32) -> String {
        format!("redis://{}:{}/{database}", self.host(), self.port)
    }

    /// A connection's configuration for a fresh database of the server.
    pub fn connection_config(&self) -> Value {
        json!({
            "host": self.host(),
            "port": self.port,
            "database": Self::next_database(),
        })
    }

    /// A `database.redis` configuration pointing at the server: `default`
    /// and `cache` connections, each on a fresh database, without a prefix.
    pub fn config(&self) -> Value {
        json!({
            "client": "phpredis",
            "options": {"cluster": "redis", "prefix": ""},
            "default": self.connection_config(),
            "cache": self.connection_config(),
        })
    }

    fn wait_until_ready(&self) -> Result<()> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            if let Some(status) = self.child.lock().unwrap().try_wait()? {
                return Err(RuntimeException::new(format!(
                    "The Redis server exited early ({status})."
                ))
                .into());
            }
            if TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(RuntimeException::new("The Redis server didn't start in time.").into())
    }
}

impl Drop for RedisServer {
    fn drop(&mut self) {
        let child = self
            .child
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn databases_are_unique_and_in_range() {
        let first = RedisServer::next_database();
        let second = RedisServer::next_database();
        assert_ne!(first, second);
        assert!((1..DATABASES).contains(&first));
    }

    #[test]
    fn servers_start_and_stop() {
        let server = RedisServer::start().unwrap();
        let port = server.port();
        assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());
        assert_eq!(server.url(3), format!("redis://127.0.0.1:{port}/3"));
        assert_eq!(server.config()["default"]["port"], json!(port));
        drop(server);
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    }
}
