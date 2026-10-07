//! The events the tests broadcast.

use illuminate_broadcasting::{
    Channel, EncryptedPrivateChannel, PresenceChannel, PrivateChannel, ShouldBroadcast,
    register_broadcast,
};
use illuminate_events::Dispatchable;
use serde::Serialize;

/// The example from Laravel's documentation.
#[derive(Debug, Clone, Serialize)]
pub struct OrderShipmentStatusUpdated {
    pub order_id: u64,
    pub status: String,
}

impl OrderShipmentStatusUpdated {
    pub fn new(order_id: u64) -> Self {
        Self {
            order_id,
            status: "shipped".into(),
        }
    }
}

impl ShouldBroadcast for OrderShipmentStatusUpdated {
    fn broadcast_on(&self) -> Vec<Channel> {
        vec![PrivateChannel::new(format!("orders.{}", self.order_id))]
    }
}

impl Dispatchable for OrderShipmentStatusUpdated {}

register_broadcast!(OrderShipmentStatusUpdated);

/// Broadcast immediately, with a custom name and payload.
#[derive(Debug, Clone, Serialize)]
pub struct ServerCreated {
    pub id: u64,
    pub secret: String,
}

impl ShouldBroadcast for ServerCreated {
    fn broadcast_on(&self) -> Vec<Channel> {
        vec![PrivateChannel::new(format!("user.{}", self.id))]
    }

    fn broadcast_as(&self) -> String {
        "server.created".into()
    }

    fn broadcast_with(&self) -> illuminate_support::Value {
        illuminate_support::json!({"id": self.id})
    }

    fn should_broadcast_now(&self) -> bool {
        true
    }
}

/// A chat message on a presence channel, always hidden from its sender.
#[derive(Debug, Clone, Serialize)]
pub struct NewMessage {
    pub room_id: u64,
    pub body: String,
}

impl ShouldBroadcast for NewMessage {
    fn broadcast_on(&self) -> Vec<Channel> {
        vec![PresenceChannel::new(format!("chat.{}", self.room_id))]
    }

    fn dont_broadcast_to_current_user(&self) -> bool {
        true
    }
}

/// Only broadcast for big orders, on its own queue.
#[derive(Debug, Clone, Serialize)]
pub struct OrderPlaced {
    pub value: u64,
}

impl ShouldBroadcast for OrderPlaced {
    fn broadcast_on(&self) -> Vec<Channel> {
        vec![Channel::new("orders")]
    }

    fn broadcast_when(&self) -> bool {
        self.value > 100
    }

    fn broadcast_queue(&self) -> Option<String> {
        Some("broadcasts".into())
    }

    fn queue_connection(&self) -> Option<String> {
        Some("array".into())
    }
}

/// Broadcast on several connections at once.
#[derive(Debug, Clone, Serialize)]
pub struct Announcement {
    pub message: String,
}

impl ShouldBroadcast for Announcement {
    fn broadcast_on(&self) -> Vec<Channel> {
        vec![Channel::new("announcements")]
    }

    fn broadcast_connections(&self) -> Vec<String> {
        vec!["pusher".into(), "reverb".into()]
    }

    fn should_broadcast_now(&self) -> bool {
        true
    }
}

/// An end-to-end encrypted event.
#[derive(Debug, Clone, Serialize)]
pub struct InvoicePaid {
    pub invoice_id: u64,
    pub amount: u64,
}

impl ShouldBroadcast for InvoicePaid {
    fn broadcast_on(&self) -> Vec<Channel> {
        vec![EncryptedPrivateChannel::new(format!(
            "invoices.{}",
            self.invoice_id
        ))]
    }

    fn should_broadcast_now(&self) -> bool {
        true
    }
}

/// Broadcasts that may fail without bothering the user.
#[derive(Debug, Clone, Serialize)]
pub struct TypingStarted {
    pub rescue: bool,
}

impl ShouldBroadcast for TypingStarted {
    fn broadcast_on(&self) -> Vec<Channel> {
        vec![Channel::new("typing")]
    }

    fn should_broadcast_now(&self) -> bool {
        true
    }

    fn should_rescue(&self) -> bool {
        self.rescue
    }
}
