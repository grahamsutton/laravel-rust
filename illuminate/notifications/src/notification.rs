//! The [`Notification`] trait.

use std::any::Any;
use std::time::Duration;

use illuminate_mail::Mailable;
use illuminate_support::{Carbon, Value, class_basename, to_value};
use serde::Serialize;

use crate::messages::MailMessage;
use crate::notifiable::Notifiable;
use crate::slack::SlackMessage;

/// What every notification gets for free from `#[derive(Serialize)]`:
/// identification, downcasting, and its data (for the fake and queue).
pub trait NotificationData: Send + Sync + 'static {
    /// The notification as [`Any`], for downcasting.
    fn as_any(&self) -> &dyn Any;

    /// The notification's full type name (Laravel's class name).
    fn notification_type(&self) -> &'static str;

    /// The notification's short type name, like `InvoicePaid`.
    fn notification_name(&self) -> String;

    /// The notification's serialized fields.
    fn notification_data(&self) -> Value;
}

impl<T: Serialize + Send + Sync + 'static> NotificationData for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn notification_type(&self) -> &'static str {
        std::any::type_name::<T>()
    }

    fn notification_name(&self) -> String {
        class_basename::<T>()
    }

    fn notification_data(&self) -> Value {
        to_value(self)
    }
}

/// A notification: a short, informational message delivered over one or
/// more channels (`mail`, `database`, or your own).
///
/// ```
/// use illuminate_notifications::{MailMessage, Notifiable, Notification};
/// use illuminate_support::{Value, json};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct InvoicePaid {
///     invoice_id: u64,
///     amount: f64,
/// }
///
/// impl Notification for InvoicePaid {
///     fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
///         vec!["mail".into(), "database".into()]
///     }
///
///     fn to_mail(&self, _notifiable: &dyn Notifiable) -> Option<MailMessage> {
///         Some(
///             MailMessage::new()
///                 .greeting("Hello!")
///                 .line("One of your invoices has been paid!")
///                 .line_if(self.amount > 0.0, format!("Amount paid: {}", self.amount))
///                 .action("View Invoice", format!("https://example.com/invoice/{}", self.invoice_id))
///                 .line("Thank you for using our application!"),
///         )
///     }
///
///     fn to_array(&self, _notifiable: &dyn Notifiable) -> Option<Value> {
///         Some(json!({"invoice_id": self.invoice_id, "amount": self.amount}))
///     }
/// }
/// ```
///
/// Notifications that should be queued return `true` from
/// [`should_queue`](Notification::should_queue).
pub trait Notification: NotificationData {
    /// Get the notification's delivery channels.
    fn via(&self, notifiable: &dyn Notifiable) -> Vec<String>;

    /// Get the mail representation of the notification.
    fn to_mail(&self, _notifiable: &dyn Notifiable) -> Option<MailMessage> {
        None
    }

    /// Get a mailable to send instead of a [`MailMessage`] (Laravel lets
    /// `toMail` return a mailable).
    fn to_mailable(&self, _notifiable: &dyn Notifiable) -> Option<Box<dyn Mailable>> {
        None
    }

    /// Get the Slack representation of the notification.
    ///
    /// ```
    /// use illuminate_notifications::slack::SlackMessage;
    /// use illuminate_notifications::{Notifiable, Notification};
    /// use serde::Serialize;
    ///
    /// #[derive(Serialize)]
    /// struct InvoicePaid;
    ///
    /// impl Notification for InvoicePaid {
    ///     fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
    ///         vec!["slack".into()]
    ///     }
    ///
    ///     fn to_slack(&self, _notifiable: &dyn Notifiable) -> Option<SlackMessage> {
    ///         Some(
    ///             SlackMessage::new()
    ///                 .text("One of your invoices has been paid!")
    ///                 .header_block("Invoice Paid")
    ///                 .section_block(|block| {
    ///                     block.text("An invoice has been paid.");
    ///                 }),
    ///         )
    ///     }
    /// }
    /// ```
    fn to_slack(&self, _notifiable: &dyn Notifiable) -> Option<SlackMessage> {
        None
    }

    /// Get the data stored by the `database` channel (falls back to
    /// [`to_array`](Notification::to_array)).
    fn to_database(&self, _notifiable: &dyn Notifiable) -> Option<Value> {
        None
    }

    /// Get the array representation of the notification.
    fn to_array(&self, _notifiable: &dyn Notifiable) -> Option<Value> {
        None
    }

    /// Get the payload for a custom channel.
    fn to_channel(&self, _channel: &str, _notifiable: &dyn Notifiable) -> Option<Value> {
        None
    }

    /// The notification's type, as stored by the `database` channel.
    fn database_type(&self, _notifiable: &dyn Notifiable) -> String {
        self.notification_type().to_string()
    }

    /// The initial `read_at` value stored by the `database` channel.
    fn initial_database_read_at_value(&self, _notifiable: &dyn Notifiable) -> Option<Carbon> {
        None
    }

    /// Determine if the notification should be sent over the given channel.
    fn should_send(&self, _notifiable: &dyn Notifiable, _channel: &str) -> bool {
        true
    }

    /// The locale the notification should be sent in.
    fn locale(&self) -> Option<String> {
        None
    }

    /// Whether the notification should be queued (Laravel's `ShouldQueue`).
    fn should_queue(&self) -> bool {
        false
    }

    /// The queue connection to use for a channel (Laravel's `viaConnections`).
    fn queue_connection(&self, _channel: &str) -> Option<String> {
        None
    }

    /// The queue to use for a channel (Laravel's `viaQueues`).
    fn queue_name(&self, _channel: &str) -> Option<String> {
        None
    }

    /// The delay before a queued notification is sent over a channel
    /// (Laravel's `withDelay`).
    fn queue_delay(&self, _notifiable: &dyn Notifiable, _channel: &str) -> Option<Duration> {
        None
    }

    /// Called after the notification was sent over a channel.
    fn after_sending(&self, _notifiable: &dyn Notifiable, _channel: &str, _response: &Value) {}
}
