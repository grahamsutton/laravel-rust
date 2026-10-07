//! The messages notifications are turned into, per channel.

mod mail;

pub use mail::{Action, DEFAULT_TEMPLATE, EMAIL_TEMPLATE, MailMessage};

use illuminate_support::Value;
use serde::{Deserialize, Serialize};

/// The data a notification stores with the `database` channel (Laravel's
/// `DatabaseMessage`).
///
/// ```
/// use illuminate_notifications::DatabaseMessage;
/// use illuminate_support::{Value, json};
///
/// let message = DatabaseMessage::new(json!({"invoice_id": 1}));
/// assert_eq!(Value::from(message), json!({"invoice_id": 1}));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DatabaseMessage {
    /// The data that should be stored with the notification.
    pub data: Value,
}

impl DatabaseMessage {
    /// Create a new database message.
    pub fn new(data: Value) -> Self {
        Self { data }
    }
}

impl From<DatabaseMessage> for Value {
    fn from(message: DatabaseMessage) -> Self {
        message.data
    }
}
