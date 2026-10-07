//! Notification channels: how notifications are delivered.
//!
//! The `mail` ([`MailChannel`]), `database` ([`DatabaseChannel`]) and
//! `slack` ([`SlackChannel`]) channels are built in. Register your own with
//! [`ChannelManager::extend`](crate::ChannelManager::extend):
//!
//! ```
//! use illuminate_notifications::{Channel, Notifiable, Notification, async_trait};
//! use illuminate_support::{Result, Value};
//!
//! struct SmsChannel;
//!
//! #[async_trait]
//! impl Channel for SmsChannel {
//!     async fn send(&self, notifiable: &dyn Notifiable, notification: &dyn Notification, _id: &str) -> Result<Value> {
//!         let Some(number) = notifiable.route_notification_for("sms", notification) else {
//!             return Ok(Value::Null);
//!         };
//!         let text = notification.to_channel("sms", notifiable).unwrap_or_default();
//!         // ...send `text` to `number` with your SMS provider...
//!         # let _ = (number, text);
//!         Ok(Value::Null)
//!     }
//! }
//! ```
//!
//! Channels may also support queued notifications by
//! [`prepare`](Channel::prepare)-ing a serializable payload when the
//! notification is queued and [`deliver`](Channel::deliver)-ing it on the
//! worker. Channels that don't are sent when the notification is queued.

mod database;
mod mail;
mod slack;

pub use database::DatabaseChannel;
pub use mail::MailChannel;
pub use slack::SlackChannel;

use async_trait::async_trait;
use illuminate_support::error::RuntimeException;
use illuminate_support::{Result, Value};

use crate::notifiable::Notifiable;
use crate::notification::Notification;

/// A notification channel.
#[async_trait]
pub trait Channel: Send + Sync + 'static {
    /// Send the given notification, returning the channel's response.
    async fn send(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
        id: &str,
    ) -> Result<Value>;

    /// Render the notification into a serializable payload for the queue.
    /// `Ok(None)` means the channel can't pre-render notifications (or
    /// there is nothing to send).
    async fn prepare(
        &self,
        _notifiable: &dyn Notifiable,
        _notification: &dyn Notification,
        _id: &str,
    ) -> Result<Option<Value>> {
        Ok(None)
    }

    /// Whether this channel pre-renders queued notifications with
    /// [`prepare`](Channel::prepare).
    fn supports_queueing(&self) -> bool {
        false
    }

    /// Deliver a payload created by [`prepare`](Channel::prepare).
    async fn deliver(&self, _payload: Value) -> Result<Value> {
        Err(
            RuntimeException::new("This notification channel does not support queued payloads.")
                .into(),
        )
    }
}
