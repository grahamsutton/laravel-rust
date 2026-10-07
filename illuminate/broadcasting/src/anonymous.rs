//! Anonymous events: broadcast without defining an event type.

use serde::{Deserialize, Serialize};

use illuminate_support::{Map, Result, Value, to_value};

use crate::channel::{Channel, IntoChannels};
use crate::contracts::ShouldBroadcast;
use crate::pending::broadcast;

/// A simple event broadcast to your application's frontend without a
/// dedicated event type (Laravel's `AnonymousEvent`). Start one with
/// [`Broadcast::on`](crate::Broadcast::on),
/// [`Broadcast::private`](crate::Broadcast::private) or
/// [`Broadcast::presence`](crate::Broadcast::presence):
///
/// ```
/// use std::sync::Arc;
/// use illuminate_broadcasting::{AnonymousEvent, Broadcast};
/// use illuminate_container::Container;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// let fake = Broadcast::fake();
///
/// Broadcast::on("orders.1").send().await?;
///
/// Broadcast::private("orders.1")
///     .as_("OrderPlaced")
///     .with(json!({"id": 1, "total": 100}))
///     .to_others()
///     .send_now()
///     .await?;
///
/// fake.assert_broadcast_on::<AnonymousEvent>("orders.1");
/// fake.assert_broadcast_as_with("OrderPlaced", |broadcast| broadcast.payload["total"] == 100);
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnonymousEvent {
    channels: Vec<Channel>,
    connection: Option<String>,
    name: Option<String>,
    payload: Value,
    include_current_user: bool,
    should_broadcast_now: bool,
}

impl AnonymousEvent {
    /// Create a new anonymous broadcastable event instance.
    pub fn new(channels: impl IntoChannels) -> Self {
        Self {
            channels: channels.into_channels(),
            connection: None,
            name: None,
            payload: Value::Object(Map::new()),
            include_current_user: true,
            should_broadcast_now: false,
        }
    }

    /// Set the connection the event should be broadcast on.
    pub fn via(mut self, connection: impl Into<String>) -> Self {
        self.connection = Some(connection.into());
        self
    }

    /// Set the name the event should be broadcast as.
    pub fn as_(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the payload the event should be broadcast with: anything that
    /// serializes to an object.
    pub fn with(mut self, payload: impl Serialize) -> Self {
        self.payload = to_value(&payload);
        self
    }

    /// Broadcast the event to everyone except the current user.
    pub fn to_others(mut self) -> Self {
        self.include_current_user = false;
        self
    }

    /// Broadcast the event through the queue.
    pub async fn send(self) -> Result<()> {
        let to_others = !self.include_current_user;
        let connection = self.connection.clone();

        let mut pending = broadcast(self);
        if let Some(connection) = connection {
            pending = pending.via(connection);
        }
        if to_others {
            pending = pending.to_others();
        }
        pending.await
    }

    /// Broadcast the event immediately, without queueing it.
    pub async fn send_now(mut self) -> Result<()> {
        self.should_broadcast_now = true;
        self.send().await
    }
}

impl ShouldBroadcast for AnonymousEvent {
    fn broadcast_on(&self) -> Vec<Channel> {
        self.channels.clone()
    }

    fn broadcast_as(&self) -> String {
        self.name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "AnonymousEvent".to_string())
    }

    fn broadcast_with(&self) -> Value {
        self.payload.clone()
    }

    fn should_broadcast_now(&self) -> bool {
        self.should_broadcast_now
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::PrivateChannel;
    use illuminate_support::json;

    #[test]
    fn anonymous_events_are_configurable() {
        let event = AnonymousEvent::new(vec![PrivateChannel::new("a"), Channel::new("b")]);
        assert_eq!(event.broadcast_as(), "AnonymousEvent");
        assert_eq!(event.broadcast_with(), json!({}));
        assert_eq!(event.broadcast_on().len(), 2);
        assert!(!event.should_broadcast_now());

        let event = AnonymousEvent::new("orders.1")
            .as_("OrderPlaced")
            .with(json!({"id": 1}))
            .via("pusher")
            .to_others();
        assert_eq!(event.broadcast_as(), "OrderPlaced");
        assert_eq!(event.broadcast_with(), json!({"id": 1}));
        assert_eq!(event.connection.as_deref(), Some("pusher"));
        assert!(!event.include_current_user);
    }
}
