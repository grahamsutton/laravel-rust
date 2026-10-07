//! The events fired while sending mail.
//!
//! ```no_run
//! use std::sync::Arc;
//! use illuminate_events::Event;
//! use illuminate_mail::events::{MessageSending, MessageSent};
//!
//! // Returning `false` from a `MessageSending` listener cancels the message.
//! Event::listen(|event: Arc<MessageSending>| async move {
//!     Ok(!event.message.has_to("blocked@example.com"))
//! });
//!
//! Event::listen(|event: Arc<MessageSent>| async move {
//!     println!("Sent {}", event.sent.message_id());
//!     Ok(())
//! });
//! ```

use illuminate_support::Value;

use crate::message::{Message, SentMessage};

/// Fired just before a message is handed to the transport. Listeners may
/// halt the event (return `false`) to cancel the message.
#[derive(Clone, Debug)]
pub struct MessageSending {
    /// The message about to be sent.
    pub message: Message,
    /// The message's view data.
    pub data: Value,
    /// The name of the mailer sending the message.
    pub mailer: String,
}

/// Fired after a message has been sent.
#[derive(Clone, Debug)]
pub struct MessageSent {
    /// The sent message.
    pub sent: SentMessage,
    /// The message's view data.
    pub data: Value,
    /// The name of the mailer that sent the message.
    pub mailer: String,
}

impl MessageSent {
    /// The message that was sent.
    pub fn message(&self) -> &Message {
        &self.sent.message
    }
}
