//! Queued mail.
//!
//! The mail component doesn't depend on the queue: queueing a mailable
//! renders it right away into a [`QueuedMessage`] — a plain, serializable
//! value — and hands it to the hook installed with
//! [`Mail::queue_using`](crate::Mail::queue_using). The queue integration
//! pushes that value onto a queue, and the worker delivers it with
//! [`Mail::send_queued`](crate::Mail::send_queued):
//!
//! ```no_run
//! use illuminate_mail::{Mail, QueuedMessage};
//!
//! // When the application boots...
//! Mail::queue_using(|queued: QueuedMessage| async move {
//!     let payload = serde_json::to_string(&queued)?;
//!     // push `payload` onto `queued.connection` / `queued.queue`, delayed by `queued.delay`...
//!     # let _ = payload;
//!     Ok(())
//! });
//!
//! // ...and in the job that runs on the worker:
//! # async fn job(payload: &str) -> illuminate_support::Result<()> {
//! let queued: QueuedMessage = serde_json::from_str(payload)?;
//! Mail::send_queued(queued).await?;
//! # Ok(()) }
//! ```
//!
//! Without a hook, queued mail is sent immediately, just like Laravel's
//! `sync` queue driver.
//!
//! ## The queue component
//!
//! When the queue component is registered (a `QueueManager` is bound), the
//! [`MailServiceProvider`](crate::MailServiceProvider) installs a hook that
//! dispatches a [`SendQueuedMailable`] job, honoring the mailable's queue
//! connection, queue name and delay. Workers deliver the job through
//! [`Mail::send_queued`](crate::Mail::send_queued).

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use illuminate_http::BoxFuture;
use illuminate_queue::{Dispatchable, ShouldQueue, async_trait, register_job};
use illuminate_support::{Result, Value};
use serde::{Deserialize, Serialize};

use crate::message::Message;

/// A fully rendered message waiting on the queue.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueuedMessage {
    /// The name of the mailer that should deliver the message.
    pub mailer: String,
    /// The name of the mailable that produced the message (for display).
    pub mailable: String,
    /// The rendered message, attachments included.
    pub message: Message,
    /// The view data the message was rendered with (for events).
    pub data: Value,
    /// The queue connection to use (`None` for the default).
    pub connection: Option<String>,
    /// The queue to push onto (`None` for the default).
    pub queue: Option<String>,
    /// How long to wait before delivering the message.
    #[serde(with = "optional_seconds")]
    pub delay: Option<Duration>,
}

impl QueuedMessage {
    /// A display name for the queued job, like Laravel's `SendQueuedMailable`.
    pub fn display_name(&self) -> String {
        self.mailable.clone()
    }
}

impl QueuedMessage {
    /// Dispatch the message onto the queue as a [`SendQueuedMailable`] job.
    pub async fn dispatch(self) -> Result<()> {
        let (connection, queue, delay) = (self.connection.clone(), self.queue.clone(), self.delay);
        let mut pending = SendQueuedMailable { message: self }.dispatch();
        if let Some(connection) = connection {
            pending = pending.on_connection(connection);
        }
        if let Some(queue) = queue {
            pending = pending.on_queue(queue);
        }
        if let Some(delay) = delay {
            pending = pending.delay(delay);
        }
        pending.await
    }
}

/// The queued job that delivers a [`QueuedMessage`] (Laravel's
/// `SendQueuedMailable`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendQueuedMailable {
    /// The rendered message.
    pub message: QueuedMessage,
}

#[async_trait]
impl ShouldQueue for SendQueuedMailable {
    async fn handle(&self) -> Result<()> {
        crate::Mail::send_queued(self.message.clone())
            .await
            .map(|_| ())
    }

    fn display_name(&self) -> String {
        self.message.display_name()
    }

    fn job_name() -> &'static str {
        "Illuminate\\Mail\\SendQueuedMailable"
    }
}

register_job!(SendQueuedMailable);

/// The hook that pushes queued messages onto the queue.
pub type QueueHook = Arc<dyn Fn(QueuedMessage) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// Wrap a closure returning a future into a [`QueueHook`].
pub(crate) fn queue_hook<F, Fut>(hook: F) -> QueueHook
where
    F: Fn(QueuedMessage) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    Arc::new(move |queued| Box::pin(hook(queued)))
}

mod optional_seconds {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        delay: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match delay {
            Some(delay) => serializer.serialize_some(&delay.as_secs_f64()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        Ok(Option::<f64>::deserialize(deserializer)?
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            .map(Duration::from_secs_f64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn queued_messages_round_trip_through_json() {
        let mut message = Message::new();
        message
            .from("a@example.com")
            .to("b@example.com")
            .subject("Hi")
            .html("<p>Hi</p>");
        message.attach_data("x", "x.txt");
        let queued = QueuedMessage {
            mailer: "smtp".into(),
            mailable: "OrderShipped".into(),
            message,
            data: json!({"order": {"id": 1}}),
            connection: Some("redis".into()),
            queue: None,
            delay: Some(Duration::from_secs(90)),
        };
        let json = serde_json::to_string(&queued).unwrap();
        let back: QueuedMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.delay, Some(Duration::from_secs(90)));
        assert_eq!(back.message.subject.as_deref(), Some("Hi"));
        assert_eq!(back.message.attachments[0].data, b"x");
        assert_eq!(back.display_name(), "OrderShipped");
        assert_eq!(back.data["order"]["id"], 1);
    }
}
