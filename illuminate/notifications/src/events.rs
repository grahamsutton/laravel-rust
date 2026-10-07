//! The events fired while sending notifications.
//!
//! ```no_run
//! use std::sync::Arc;
//! use illuminate_events::Event;
//! use illuminate_notifications::events::{NotificationSending, NotificationSent};
//!
//! // Returning `false` from a `NotificationSending` listener skips the channel.
//! Event::listen(|event: Arc<NotificationSending>| async move {
//!     Ok(event.channel != "mail")
//! });
//!
//! Event::listen(|event: Arc<NotificationSent>| async move {
//!     println!("{} sent via {}", event.notification_type, event.channel);
//!     Ok(())
//! });
//! ```

use std::sync::Arc;

use illuminate_support::Value;

use crate::notifiable::NotifiableSnapshot;
use crate::notification::Notification;

/// Fired before a notification is sent over a channel. Listeners may halt
/// the event (return `false`) to skip the channel.
#[derive(Clone)]
pub struct NotificationSending {
    /// The notifiable.
    pub notifiable: NotifiableSnapshot,
    /// The notification.
    pub notification: Arc<dyn Notification>,
    /// The notification's ID.
    pub id: String,
    /// The channel name.
    pub channel: String,
}

impl NotificationSending {
    /// The notification, as its concrete type.
    pub fn notification<N: Notification>(&self) -> Option<&N> {
        (*self.notification).as_any().downcast_ref::<N>()
    }
}

/// Fired when a notification was skipped (by `should_send` or a
/// `NotificationSending` listener).
#[derive(Clone)]
pub struct NotificationSkipped {
    /// The notifiable.
    pub notifiable: NotifiableSnapshot,
    /// The notification.
    pub notification: Arc<dyn Notification>,
    /// The notification's ID.
    pub id: String,
    /// The channel name.
    pub channel: String,
}

/// Fired after a notification was sent over a channel.
#[derive(Clone)]
pub struct NotificationSent {
    /// The notifiable.
    pub notifiable: NotifiableSnapshot,
    /// The notification (`None` when a queued, pre-rendered notification
    /// was delivered by a worker).
    pub notification: Option<Arc<dyn Notification>>,
    /// The notification's type name.
    pub notification_type: String,
    /// The notification's ID.
    pub id: String,
    /// The channel name.
    pub channel: String,
    /// The channel's response.
    pub response: Value,
}

impl NotificationSent {
    /// The notification, as its concrete type.
    pub fn notification<N: Notification>(&self) -> Option<&N> {
        self.notification
            .as_deref()
            .and_then(|notification| notification.as_any().downcast_ref::<N>())
    }
}

/// Fired when sending a notification over a channel failed.
#[derive(Clone)]
pub struct NotificationFailed {
    /// The notifiable.
    pub notifiable: NotifiableSnapshot,
    /// The notification (`None` for queued, pre-rendered notifications).
    pub notification: Option<Arc<dyn Notification>>,
    /// The notification's type name.
    pub notification_type: String,
    /// The notification's ID.
    pub id: String,
    /// The channel name.
    pub channel: String,
    /// The error message.
    pub error: String,
}
