//! The broadcasting contracts: broadcastable events and broadcasters.

use std::any::type_name;

use serde::Serialize;

use illuminate_http::Request;
use illuminate_support::{Map, Result, Str, Value, to_value};

use crate::channel::Channel;

/// Marks an event that should be broadcast to your JavaScript application.
///
/// Only [`broadcast_on`](ShouldBroadcast::broadcast_on) is required; every
/// other method mirrors one of Laravel's conventions and has its default.
/// The event's *serialized fields* are its broadcast payload, just like an
/// event's public properties in Laravel:
///
/// ```
/// use illuminate_broadcasting::{Channel, PrivateChannel, ShouldBroadcast};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct OrderShipmentStatusUpdated {
///     order_id: u64,
///     status: String,
/// }
///
/// impl ShouldBroadcast for OrderShipmentStatusUpdated {
///     fn broadcast_on(&self) -> Vec<Channel> {
///         vec![PrivateChannel::new(format!("orders.{}", self.order_id))]
///     }
/// }
///
/// let event = OrderShipmentStatusUpdated { order_id: 1, status: "shipped".into() };
/// assert_eq!(event.broadcast_on(), vec![Channel::new("private-orders.1")]);
/// assert!(event.broadcast_as().ends_with("OrderShipmentStatusUpdated"));
/// assert_eq!(event.broadcast_with()["status"], "shipped");
/// ```
///
/// Once your event implements `ShouldBroadcast`, dispatch it like any other
/// event (after [registering it](crate::register_broadcast)) or hand it to
/// the [`broadcast`](crate::broadcast) helper. A queued job broadcasts it
/// using your default broadcast connection.
pub trait ShouldBroadcast: Serialize + Send + Sync + 'static {
    /// Get the channels the event should broadcast on.
    fn broadcast_on(&self) -> Vec<Channel>;

    /// The event's broadcast name.
    ///
    /// By default, the event is broadcast using its "class name": the type's
    /// path written the way Laravel writes namespaces, without your crate's
    /// name and file module. An `OrderShipped` event living in
    /// `app/events/order_shipped.rs` is broadcast as `App\Events\OrderShipped`
    /// — exactly what Laravel Echo listens for by default.
    fn broadcast_as(&self) -> String {
        class_name::<Self>()
    }

    /// Get the data to broadcast: an object. By default, every serialized
    /// field of the event.
    fn broadcast_with(&self) -> Value {
        to_value(self)
    }

    /// Determine if this event should broadcast.
    fn broadcast_when(&self) -> bool {
        true
    }

    /// The broadcast connections the event should be broadcast on (Laravel's
    /// `broadcastVia`). Empty uses the default connection.
    fn broadcast_connections(&self) -> Vec<String> {
        Vec::new()
    }

    /// The name of the queue on which to place the broadcasting job.
    fn broadcast_queue(&self) -> Option<String> {
        None
    }

    /// The queue connection the broadcasting job should be sent to (Laravel's
    /// `#[Connection]` attribute). `None` uses the default queue connection.
    fn queue_connection(&self) -> Option<String> {
        None
    }

    /// Broadcast the event immediately instead of queueing it (Laravel's
    /// `ShouldBroadcastNow`).
    fn should_broadcast_now(&self) -> bool {
        false
    }

    /// Always exclude the user who triggered the event from receiving it,
    /// using the current request's `X-Socket-ID` header (Laravel's
    /// `dontBroadcastToCurrentUser`).
    fn dont_broadcast_to_current_user(&self) -> bool {
        false
    }

    /// Report broadcasting failures instead of returning them (Laravel's
    /// `ShouldRescue`).
    fn should_rescue(&self) -> bool {
        false
    }

    /// Queue the broadcast only after open database transactions commit
    /// (Laravel's `ShouldDispatchAfterCommit`). `None` defers to the queue
    /// connection's `after_commit` option.
    fn after_commit(&self) -> Option<bool> {
        None
    }

    /// The number of times the broadcasting job may be attempted.
    fn tries(&self) -> Option<u32> {
        None
    }

    /// The number of seconds the broadcasting job can run before timing out.
    fn timeout(&self) -> Option<u64> {
        None
    }

    /// The seconds to wait before retrying the broadcasting job.
    fn backoff(&self) -> Vec<u64> {
        Vec::new()
    }

    /// The maximum number of unhandled exceptions to allow before failing.
    fn max_exceptions(&self) -> Option<u32> {
        None
    }

    /// Make the broadcast unique (Laravel's `ShouldBeUnique`): while a
    /// broadcast of this event with the same id is queued, new ones are
    /// ignored. Requires a cache store for the lock.
    fn unique_id(&self) -> Option<String> {
        None
    }

    /// The number of seconds the unique lock should be maintained.
    fn unique_for(&self) -> u64 {
        0
    }
}

/// A broadcaster: the driver that delivers events (Pusher, Reverb, Ably,
/// the log, ...) and answers channel authorization requests.
///
/// Implement it to add your own driver with
/// [`Broadcast::extend`](crate::Broadcast::extend). The
/// [`ChannelRegistry`](crate::ChannelRegistry) does the heavy lifting of
/// channel authorization for you:
///
/// ```
/// use illuminate_broadcasting::{Broadcast, Broadcaster, Channel, async_trait};
/// use illuminate_http::Request;
/// use illuminate_support::{Map, Result, Value, json};
///
/// struct TelegraphBroadcaster;
///
/// #[async_trait]
/// impl Broadcaster for TelegraphBroadcaster {
///     async fn auth(&self, request: &Request) -> Result<Value> {
///         let channel = request.string("channel_name");
///         Broadcast::channels().verify_user_can_access_channel(request, &channel, self).await
///     }
///
///     async fn valid_authentication_response(&self, _request: &Request, result: Value) -> Result<Value> {
///         Ok(json!({"authorized": result}))
///     }
///
///     async fn broadcast(&self, channels: &[Channel], event: &str, payload: Map<String, Value>) -> Result<()> {
///         // Tap it out in Morse code...
///         Ok(())
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait Broadcaster: Send + Sync + 'static {
    /// Authenticate the incoming request for a given channel, returning the
    /// authorization payload (`Value::Null` for an empty response).
    async fn auth(&self, request: &Request) -> Result<Value>;

    /// Return the valid authentication response for an authorized request.
    async fn valid_authentication_response(
        &self,
        request: &Request,
        result: Value,
    ) -> Result<Value>;

    /// Broadcast the given event.
    async fn broadcast(
        &self,
        channels: &[Channel],
        event: &str,
        payload: Map<String, Value>,
    ) -> Result<()>;

    /// Resolve the authenticated user payload for an incoming connection
    /// request (Pusher's user authentication), or `None` when there is no
    /// user to authenticate.
    async fn resolve_authenticated_user(&self, request: &Request) -> Result<Option<Value>> {
        Ok(crate::facade::Broadcast::channels()
            .resolve_authenticated_user(request)
            .await)
    }

    /// Determine if JSONP callbacks are allowed on authorization responses.
    fn allows_jsonp(&self) -> bool {
        false
    }
}

/// The Laravel-style "class name" of a type: its path with the crate name
/// and the type's own file module removed, each segment studly cased and
/// joined with backslashes.
///
/// ```
/// use illuminate_broadcasting::contracts::class_name_from_type;
///
/// assert_eq!(
///     class_name_from_type("example_app::app::events::order_shipped::OrderShipped"),
///     "App\\Events\\OrderShipped"
/// );
/// assert_eq!(class_name_from_type("my_tests::OrderShipped"), "OrderShipped");
/// assert_eq!(class_name_from_type("app::events::Wrapper<u64>"), "Events\\Wrapper");
/// ```
pub fn class_name_from_type(type_name: &str) -> String {
    let path = type_name.split('<').next().unwrap_or(type_name);
    let mut segments: Vec<&str> = path.split("::").collect();
    let ty = segments.pop().unwrap_or(path);
    if !segments.is_empty() {
        segments.remove(0);
    }
    if segments
        .last()
        .is_some_and(|module| *module == Str::snake(ty))
    {
        segments.pop();
    }
    segments
        .iter()
        .filter(|segment| !segment.is_empty() && !segment.starts_with('{'))
        .map(|segment| Str::studly(segment))
        .chain(std::iter::once(ty.to_string()))
        .collect::<Vec<_>>()
        .join("\\")
}

/// The Laravel-style "class name" of the type `T`. See
/// [`class_name_from_type`].
pub fn class_name<T: ?Sized>() -> String {
    class_name_from_type(type_name::<T>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::PrivateChannel;
    use illuminate_support::json;

    #[derive(Serialize)]
    struct ServerCreated {
        id: u64,
    }

    impl ShouldBroadcast for ServerCreated {
        fn broadcast_on(&self) -> Vec<Channel> {
            vec![PrivateChannel::new(format!("user.{}", self.id))]
        }
    }

    #[test]
    fn events_have_laravel_defaults() {
        let event = ServerCreated { id: 1 };
        assert_eq!(event.broadcast_as(), "Contracts\\Tests\\ServerCreated");
        assert_eq!(event.broadcast_with(), json!({"id": 1}));
        assert!(event.broadcast_when());
        assert!(event.broadcast_connections().is_empty());
        assert_eq!(event.broadcast_queue(), None);
        assert_eq!(event.queue_connection(), None);
        assert!(!event.should_broadcast_now());
        assert!(!event.dont_broadcast_to_current_user());
        assert!(!event.should_rescue());
        assert_eq!(event.after_commit(), None);
        assert_eq!(event.tries(), None);
        assert_eq!(event.timeout(), None);
        assert!(event.backoff().is_empty());
        assert_eq!(event.max_exceptions(), None);
        assert_eq!(event.unique_id(), None);
        assert_eq!(event.unique_for(), 0);
    }

    #[test]
    fn class_names_follow_laravel_namespaces() {
        assert_eq!(
            class_name_from_type("example_app::app::events::ServerCreated"),
            "App\\Events\\ServerCreated"
        );
        assert_eq!(
            class_name_from_type("example_app::app::events::server_created::ServerCreated"),
            "App\\Events\\ServerCreated"
        );
        assert_eq!(
            class_name_from_type("example_app::app::admin_events::Deployed"),
            "App\\AdminEvents\\Deployed"
        );
        assert_eq!(class_name_from_type("ServerCreated"), "ServerCreated");
        assert_eq!(
            class_name::<ServerCreated>(),
            "Contracts\\Tests\\ServerCreated"
        );
    }
}
