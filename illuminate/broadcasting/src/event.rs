//! The queued job that broadcasts an event.

use std::any::type_name;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use illuminate_queue::ShouldQueue;
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Map, Result, Value};

use crate::channel::Channel;
use crate::contracts::{ShouldBroadcast, class_name};
use crate::facade::Broadcast;

/// The job that broadcasts an event (Laravel's `BroadcastEvent`).
///
/// The event is captured when it's dispatched: its name, channels and
/// payload are serialized onto the queue, so the worker broadcasts exactly
/// what your application saw — no need to register the event type with the
/// worker.
///
/// ```
/// use illuminate_broadcasting::{BroadcastEvent, Channel, PrivateChannel, ShouldBroadcast};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct OrderShipped { order_id: u64 }
///
/// impl ShouldBroadcast for OrderShipped {
///     fn broadcast_on(&self) -> Vec<Channel> {
///         vec![PrivateChannel::new(format!("orders.{}", self.order_id))]
///     }
///
///     fn broadcast_as(&self) -> String {
///         "order.shipped".into()
///     }
/// }
///
/// let job = BroadcastEvent::new(&OrderShipped { order_id: 1 }).unwrap();
///
/// assert_eq!(job.name, "order.shipped");
/// assert_eq!(job.channels, vec![Channel::new("private-orders.1")]);
/// assert_eq!(job.payload["order_id"], 1);
/// assert!(job.payload["socket"].is_null());
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BroadcastEvent {
    /// The event's "class name" (shown by `queue:work` and failed jobs).
    pub event: String,
    /// The name the event is broadcast as.
    pub name: String,
    /// The channels the event is broadcast on.
    pub channels: Vec<Channel>,
    /// The payload, including the `socket` to exclude (or `null`).
    pub payload: Map<String, Value>,
    /// The broadcast connections to use (empty for the default connection).
    #[serde(default)]
    pub connections: Vec<String>,
    /// The number of times the job may be attempted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tries: Option<u32>,
    /// The number of seconds the job can run before timing out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// The seconds to wait before retrying the job.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backoff: Vec<u64>,
    /// The maximum number of unhandled exceptions to allow before failing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_exceptions: Option<u32>,
    /// The unique id of a unique broadcast.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique_id: Option<String>,
    /// The number of seconds the unique lock should be maintained.
    #[serde(default)]
    pub unique_for: u64,
}

impl BroadcastEvent {
    /// Capture an event for broadcasting.
    pub fn new<E: ShouldBroadcast>(event: &E) -> Result<Self> {
        Self::capture(event, None, Vec::new())
    }

    /// Capture an event for broadcasting, excluding the given socket and
    /// using the given connections (when not empty) instead of the event's.
    pub(crate) fn capture<E: ShouldBroadcast>(
        event: &E,
        socket: Option<String>,
        connections: Vec<String>,
    ) -> Result<Self> {
        let mut payload = match event.broadcast_with() {
            Value::Object(payload) => payload,
            Value::Null => Map::new(),
            other => {
                return Err(InvalidArgumentException::new(format!(
                    "The broadcast payload of [{}] must be an object, got: {other}",
                    type_name::<E>()
                ))
                .into());
            }
        };
        payload.insert(
            "socket".into(),
            socket.map(Value::from).unwrap_or(Value::Null),
        );

        let connections = if connections.is_empty() {
            event.broadcast_connections()
        } else {
            connections
        };

        Ok(Self {
            event: class_name::<E>(),
            name: event.broadcast_as(),
            channels: event.broadcast_on(),
            payload,
            connections,
            tries: event.tries(),
            timeout: event.timeout(),
            backoff: event.backoff(),
            max_exceptions: event.max_exceptions(),
            unique_id: event
                .unique_id()
                .map(|id| format!("{}:{id}", type_name::<E>())),
            unique_for: event.unique_for(),
        })
    }

    /// The socket the broadcast excludes, if any.
    pub fn socket(&self) -> Option<&str> {
        self.payload.get("socket").and_then(Value::as_str)
    }

    /// Determine if the event is broadcast on the given channel.
    pub fn broadcasts_on(&self, channel: impl AsRef<str>) -> bool {
        self.channels
            .iter()
            .any(|candidate| candidate.name() == channel.as_ref())
    }

    /// The connections to broadcast on: `None` is the default connection.
    pub fn connection_names(&self) -> Vec<Option<String>> {
        if self.connections.is_empty() {
            vec![None]
        } else {
            self.connections.iter().cloned().map(Some).collect()
        }
    }
}

#[async_trait]
impl ShouldQueue for BroadcastEvent {
    async fn handle(&self) -> Result<()> {
        Broadcast::manager().broadcast_event(self).await
    }

    fn tries(&self) -> Option<u32> {
        self.tries
    }

    fn max_exceptions(&self) -> Option<u32> {
        self.max_exceptions
    }

    fn backoff(&self) -> Vec<u64> {
        self.backoff.clone()
    }

    fn timeout(&self) -> Option<u64> {
        self.timeout
    }

    fn display_name(&self) -> String {
        self.event.clone()
    }

    fn unique_id(&self) -> Option<String> {
        self.unique_id.clone()
    }

    fn unique_for(&self) -> u64 {
        self.unique_for
    }

    fn job_name() -> &'static str {
        "Illuminate\\Broadcasting\\BroadcastEvent"
    }
}

illuminate_queue::register_job!(BroadcastEvent);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::PresenceChannel;
    use illuminate_queue::JobRegistry;
    use illuminate_support::json;

    #[derive(Serialize)]
    struct NewMessage {
        room: u64,
        body: String,
    }

    impl ShouldBroadcast for NewMessage {
        fn broadcast_on(&self) -> Vec<Channel> {
            vec![PresenceChannel::new(format!("chat.{}", self.room))]
        }

        fn broadcast_connections(&self) -> Vec<String> {
            vec!["pusher".into(), "ably".into()]
        }

        fn tries(&self) -> Option<u32> {
            Some(3)
        }

        fn unique_id(&self) -> Option<String> {
            Some(self.room.to_string())
        }
    }

    #[test]
    fn events_are_captured_with_their_public_data() {
        let job = BroadcastEvent::capture(
            &NewMessage {
                room: 1,
                body: "Hi".into(),
            },
            Some("1234.5678".into()),
            Vec::new(),
        )
        .unwrap();

        assert_eq!(job.event, "Event\\Tests\\NewMessage");
        assert_eq!(job.name, "Event\\Tests\\NewMessage");
        assert_eq!(
            Value::Object(job.payload.clone()),
            json!({"room": 1, "body": "Hi", "socket": "1234.5678"})
        );
        assert_eq!(job.socket(), Some("1234.5678"));
        assert!(job.broadcasts_on("presence-chat.1"));
        assert!(!job.broadcasts_on("chat.1"));
        assert_eq!(
            job.connection_names(),
            vec![Some("pusher".to_string()), Some("ably".to_string())]
        );
        assert_eq!(ShouldQueue::tries(&job), Some(3));
        assert_eq!(job.display_name(), "Event\\Tests\\NewMessage");
        assert!(
            ShouldQueue::unique_id(&job)
                .unwrap()
                .ends_with("NewMessage:1")
        );
    }

    #[test]
    fn explicit_connections_win() {
        let job = BroadcastEvent::capture(
            &NewMessage {
                room: 1,
                body: "Hi".into(),
            },
            None,
            vec!["log".into()],
        )
        .unwrap();
        assert_eq!(job.connection_names(), vec![Some("log".to_string())]);
        assert_eq!(job.socket(), None);
    }

    #[test]
    fn payloads_must_be_objects() {
        #[derive(Serialize)]
        struct Ping;
        impl ShouldBroadcast for Ping {
            fn broadcast_on(&self) -> Vec<Channel> {
                Vec::new()
            }
        }
        let job = BroadcastEvent::new(&Ping).unwrap();
        assert_eq!(Value::Object(job.payload.clone()), json!({"socket": null}));
        assert_eq!(job.connection_names(), vec![None]);

        #[derive(Serialize)]
        struct Scalar(u64);
        impl ShouldBroadcast for Scalar {
            fn broadcast_on(&self) -> Vec<Channel> {
                Vec::new()
            }
        }
        let error = BroadcastEvent::new(&Scalar(1)).unwrap_err();
        assert!(error.to_string().contains("must be an object"));
    }

    #[test]
    fn jobs_round_trip_through_the_queue() {
        let job = BroadcastEvent::new(&NewMessage {
            room: 2,
            body: "Yo".into(),
        })
        .unwrap();
        let serialized = serde_json::to_value(&job).unwrap();
        let back: BroadcastEvent = serde_json::from_value(serialized).unwrap();
        assert_eq!(back, job);

        assert_eq!(
            BroadcastEvent::job_name(),
            "Illuminate\\Broadcasting\\BroadcastEvent"
        );
        assert!(JobRegistry::has("Illuminate\\Broadcasting\\BroadcastEvent"));
    }
}
