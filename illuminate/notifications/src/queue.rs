//! Queued notifications.
//!
//! Like queued mail, queued notifications are rendered when they are
//! queued: each channel [prepares](crate::Channel::prepare) a serializable
//! payload (the `mail` channel renders the whole email, the `database`
//! channel builds the row to insert), which travels as a
//! [`QueuedNotification`] to the hook installed with
//! [`Notification::queue_using`](crate::facades::Notification::queue_using).
//! The worker hands it to
//! [`Notification::send_queued`](crate::facades::Notification::send_queued).
//!
//! Without a hook, queued notifications are delivered immediately (like
//! Laravel's `sync` queue driver). When the queue component is registered,
//! the [`NotificationServiceProvider`](crate::NotificationServiceProvider)
//! installs a hook that dispatches a [`SendQueuedNotification`] job.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use illuminate_http::BoxFuture;
use illuminate_queue::{Dispatchable, ShouldQueue, async_trait, register_job};
use illuminate_support::{Result, Value};
use serde::{Deserialize, Serialize};

use crate::notifiable::NotifiableSnapshot;

/// A notification, rendered for one channel, waiting on the queue.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueuedNotification {
    /// The notification's ID.
    pub id: String,
    /// The notification's type name.
    pub notification: String,
    /// The channel to deliver over.
    pub channel: String,
    /// The channel's prepared payload.
    pub payload: Value,
    /// The notifiable.
    pub notifiable: NotifiableSnapshot,
    /// The locale the notification was rendered in.
    pub locale: Option<String>,
    /// The queue connection to use.
    pub connection: Option<String>,
    /// The queue to push onto.
    pub queue: Option<String>,
    /// How long to wait before delivering the notification.
    #[serde(with = "optional_seconds")]
    pub delay: Option<Duration>,
}

impl QueuedNotification {
    /// A display name for the queued job.
    pub fn display_name(&self) -> String {
        self.notification
            .rsplit("::")
            .next()
            .unwrap_or(&self.notification)
            .to_string()
    }

    /// Dispatch the notification onto the queue as a [`SendQueuedNotification`] job.
    pub async fn dispatch(self) -> Result<()> {
        let (connection, queue, delay) = (self.connection.clone(), self.queue.clone(), self.delay);
        let mut pending = SendQueuedNotification { notification: self }.dispatch();
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

/// The queued job that delivers a [`QueuedNotification`] (Laravel's
/// `SendQueuedNotifications`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendQueuedNotification {
    /// The prepared notification.
    pub notification: QueuedNotification,
}

#[async_trait]
impl ShouldQueue for SendQueuedNotification {
    async fn handle(&self) -> Result<()> {
        crate::facades::Notification::send_queued(self.notification.clone()).await
    }

    fn display_name(&self) -> String {
        self.notification.display_name()
    }

    fn job_name() -> &'static str {
        "Illuminate\\Notifications\\SendQueuedNotifications"
    }
}

register_job!(SendQueuedNotification);

/// The hook that pushes queued notifications onto the queue.
pub type NotificationQueueHook =
    Arc<dyn Fn(QueuedNotification) -> BoxFuture<'static, Result<()>> + Send + Sync>;

pub(crate) fn queue_hook<F, Fut>(hook: F) -> NotificationQueueHook
where
    F: Fn(QueuedNotification) -> Fut + Send + Sync + 'static,
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
    fn queued_notifications_round_trip_through_json() {
        let queued = QueuedNotification {
            id: "abc".into(),
            notification: "app::notifications::InvoicePaid".into(),
            channel: "database".into(),
            payload: json!({"id": "abc"}),
            notifiable: NotifiableSnapshot {
                notifiable_type: "App\\Models\\User".into(),
                key: json!(1),
                attributes: json!({"id": 1}),
            },
            locale: Some("es".into()),
            connection: None,
            queue: Some("notifications".into()),
            delay: Some(Duration::from_secs(5)),
        };
        let back: QueuedNotification =
            serde_json::from_str(&serde_json::to_string(&queued).unwrap()).unwrap();
        assert_eq!(back.delay, Some(Duration::from_secs(5)));
        assert_eq!(back.display_name(), "InvoicePaid");
        assert_eq!(back.notifiable, queued.notifiable);
    }
}
