//! The `failover` and `roundrobin` transports.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use illuminate_support::Result;
use illuminate_support::error::RuntimeException;

use super::Transport;
use crate::message::{Message, SentMessage};

/// Shared logic: a list of transports, some of which may be temporarily
/// considered "dead" after failing.
struct Pool {
    transports: Vec<Arc<dyn Transport>>,
    retry_after: Duration,
    dead: Mutex<HashMap<usize, Instant>>,
}

impl Pool {
    fn new(transports: Vec<Arc<dyn Transport>>, retry_after: Duration) -> Result<Self> {
        if transports.is_empty() {
            return Err(RuntimeException::new(
                "A transport must have at least one underlying transport.",
            )
            .into());
        }
        Ok(Self {
            transports,
            retry_after,
            dead: Mutex::new(HashMap::new()),
        })
    }

    fn is_alive(&self, index: usize) -> bool {
        let mut dead = self.dead.lock().unwrap();
        match dead.get(&index) {
            Some(since) if since.elapsed() < self.retry_after => false,
            Some(_) => {
                dead.remove(&index);
                true
            }
            None => true,
        }
    }

    /// Try each transport in turn, starting at `start`.
    async fn send(&self, message: &Message, start: usize, kind: &str) -> Result<SentMessage> {
        let count = self.transports.len();
        let mut errors = Vec::new();
        for offset in 0..count {
            let index = (start + offset) % count;
            if !self.is_alive(index) {
                continue;
            }
            let transport = &self.transports[index];
            match transport.send(message).await {
                Ok(sent) => return Ok(sent),
                Err(error) => {
                    self.dead.lock().unwrap().insert(index, Instant::now());
                    errors.push(format!("{}: {error}", transport.name()));
                }
            }
        }
        Err(RuntimeException::new(if errors.is_empty() {
            format!("All transports of the {kind} transport are temporarily unavailable.")
        } else {
            format!("All transports failed: {}", errors.join("; "))
        })
        .into())
    }

    fn names(&self) -> String {
        self.transports
            .iter()
            .map(|t| t.name())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Tries each mailer's transport in order, falling back to the next one
/// when a transport fails. A failed transport is skipped for
/// `retry_after` before being tried again.
pub struct FailoverTransport {
    pool: Pool,
}

impl FailoverTransport {
    /// Create a failover transport.
    pub fn new(transports: Vec<Arc<dyn Transport>>, retry_after: Duration) -> Result<Self> {
        Ok(Self {
            pool: Pool::new(transports, retry_after)?,
        })
    }
}

#[async_trait]
impl Transport for FailoverTransport {
    async fn send(&self, message: &Message) -> Result<SentMessage> {
        self.pool.send(message, 0, "failover").await
    }

    fn name(&self) -> String {
        format!("failover({})", self.pool.names())
    }
}

/// Spreads messages across several transports, one after another,
/// skipping transports that recently failed.
pub struct RoundRobinTransport {
    pool: Pool,
    cursor: Mutex<usize>,
}

impl RoundRobinTransport {
    /// Create a round robin transport (starting at a random transport, like Symfony).
    pub fn new(transports: Vec<Arc<dyn Transport>>, retry_after: Duration) -> Result<Self> {
        let pool = Pool::new(transports, retry_after)?;
        let start = rand::random_range(0..pool.transports.len());
        Ok(Self {
            pool,
            cursor: Mutex::new(start),
        })
    }
}

#[async_trait]
impl Transport for RoundRobinTransport {
    async fn send(&self, message: &Message) -> Result<SentMessage> {
        let start = {
            let mut cursor = self.cursor.lock().unwrap();
            let start = *cursor;
            *cursor = (start + 1) % self.pool.transports.len();
            start
        };
        self.pool.send(message, start, "round robin").await
    }

    fn name(&self) -> String {
        format!("roundrobin({})", self.pool.names())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArrayTransport;
    use crate::transport::downcast_transport;

    struct Broken;

    #[async_trait]
    impl Transport for Broken {
        async fn send(&self, _message: &Message) -> Result<SentMessage> {
            Err(RuntimeException::new("connection refused").into())
        }

        fn name(&self) -> String {
            "broken".into()
        }
    }

    fn message() -> Message {
        let mut message = Message::new();
        message.from("a@example.com").to("b@example.com");
        message
    }

    #[tokio::test]
    async fn failover_falls_back_to_the_next_transport() {
        let array: Arc<dyn Transport> = Arc::new(ArrayTransport::new());
        let transport = FailoverTransport::new(
            vec![Arc::new(Broken), array.clone()],
            Duration::from_secs(60),
        )
        .unwrap();
        assert_eq!(transport.name(), "failover(broken array)");

        transport.send(&message()).await.unwrap();
        transport.send(&message()).await.unwrap();
        assert_eq!(
            downcast_transport::<ArrayTransport>(&array)
                .unwrap()
                .messages()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn failover_reports_when_every_transport_fails() {
        let transport =
            FailoverTransport::new(vec![Arc::new(Broken)], Duration::from_secs(60)).unwrap();
        let error = transport.send(&message()).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("All transports failed: broken: connection refused")
        );
        // The broken transport is now dead until the retry period passes.
        let error = transport.send(&message()).await.unwrap_err();
        assert!(error.to_string().contains("temporarily unavailable"));

        let transport = FailoverTransport::new(vec![Arc::new(Broken)], Duration::ZERO).unwrap();
        let _ = transport.send(&message()).await;
        let error = transport.send(&message()).await.unwrap_err();
        assert!(error.to_string().contains("All transports failed"));
        assert!(FailoverTransport::new(Vec::new(), Duration::ZERO).is_err());
    }

    #[tokio::test]
    async fn round_robin_rotates_between_transports() {
        let first: Arc<dyn Transport> = Arc::new(ArrayTransport::new());
        let second: Arc<dyn Transport> = Arc::new(ArrayTransport::new());
        let transport =
            RoundRobinTransport::new(vec![first.clone(), second.clone()], Duration::from_secs(60))
                .unwrap();
        for _ in 0..4 {
            transport.send(&message()).await.unwrap();
        }
        assert_eq!(
            downcast_transport::<ArrayTransport>(&first)
                .unwrap()
                .messages()
                .len(),
            2
        );
        assert_eq!(
            downcast_transport::<ArrayTransport>(&second)
                .unwrap()
                .messages()
                .len(),
            2
        );
        assert!(transport.name().starts_with("roundrobin("));
    }
}
