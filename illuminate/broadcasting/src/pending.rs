//! Broadcasting events: the `broadcast()` helper and the pending broadcast.

use std::any::TypeId;
use std::cell::RefCell;
use std::future::IntoFuture;

use futures::future::BoxFuture;

use illuminate_events::Event;
use illuminate_support::Result;

use crate::contracts::ShouldBroadcast;
use crate::facade::Broadcast;
use crate::registration::ensure_registered;

/// How a particular dispatch of an event should be broadcast.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BroadcastOptions {
    /// The socket ID to exclude from receiving the broadcast.
    pub socket: Option<String>,
    /// The broadcast connections to use instead of the event's own.
    pub connections: Vec<String>,
}

impl BroadcastOptions {
    /// Exclude the given socket from receiving the broadcast.
    pub fn except_socket(mut self, socket: impl Into<String>) -> Self {
        self.socket = Some(socket.into());
        self
    }

    /// Broadcast using the given connection.
    pub fn via(mut self, connection: impl Into<String>) -> Self {
        self.connections = vec![connection.into()];
        self
    }
}

/// The options of the broadcast currently being dispatched, handed from the
/// [`PendingBroadcast`] to the event's broadcast listener.
struct InFlight {
    event: TypeId,
    options: BroadcastOptions,
}

tokio::task_local! {
    static IN_FLIGHT: RefCell<Option<InFlight>>;
}

/// Take the options of the broadcast being dispatched for the event `E`.
pub(crate) fn take_options<E: 'static>() -> Option<BroadcastOptions> {
    IN_FLIGHT
        .try_with(|cell| {
            cell.borrow_mut()
                .take_if(|in_flight| in_flight.event == TypeId::of::<E>())
                .map(|in_flight| in_flight.options)
        })
        .ok()
        .flatten()
}

/// An event on its way to being broadcast (Laravel's `PendingBroadcast`).
///
/// Configure it, then `.await` it: the event is dispatched through the
/// event dispatcher — so its listeners run, and `Event::fake()` sees it —
/// and broadcast through the queue.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_broadcasting::{Broadcast, Channel, PrivateChannel, ShouldBroadcast, broadcast};
/// use illuminate_container::Container;
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct NewMessage { room_id: u64, body: String }
///
/// impl ShouldBroadcast for NewMessage {
///     fn broadcast_on(&self) -> Vec<Channel> {
///         vec![PrivateChannel::new(format!("chat.{}", self.room_id))]
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// let fake = Broadcast::fake();
///
/// broadcast(NewMessage { room_id: 1, body: "Hello!".into() })
///     .to_others()
///     .await?;
///
/// fake.assert_broadcast_on::<NewMessage>("private-chat.1");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[must_use = "broadcasts are only sent once the `PendingBroadcast` is `.await`ed"]
pub struct PendingBroadcast<E> {
    event: E,
    to_others: bool,
    connection: Option<String>,
}

impl<E: ShouldBroadcast> PendingBroadcast<E> {
    /// Create a pending broadcast for the given event.
    pub fn new(event: E) -> Self {
        Self {
            event,
            to_others: false,
            connection: None,
        }
    }

    /// Broadcast the event using a specific broadcast connection.
    pub fn via(mut self, connection: impl Into<String>) -> Self {
        self.connection = Some(connection.into());
        self
    }

    /// Broadcast the event to everyone except the current user: the
    /// connection whose socket ID the current request carries in its
    /// `X-Socket-ID` header.
    pub fn to_others(mut self) -> Self {
        self.to_others = true;
        self
    }

    /// The event being broadcast.
    pub fn event(&self) -> &E {
        &self.event
    }

    /// Dispatch the event.
    pub async fn dispatch(self) -> Result<()> {
        let dispatcher = Event::dispatcher();
        ensure_registered::<E>(&dispatcher);

        let options = BroadcastOptions {
            socket: if self.to_others {
                Broadcast::socket(None)
            } else {
                None
            },
            connections: self.connection.into_iter().collect(),
        };
        let in_flight = InFlight {
            event: TypeId::of::<E>(),
            options,
        };

        IN_FLIGHT
            .scope(
                RefCell::new(Some(in_flight)),
                dispatcher.dispatch(self.event),
            )
            .await
    }
}

impl<E: ShouldBroadcast> IntoFuture for PendingBroadcast<E> {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<'static, Result<()>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.dispatch())
    }
}

impl<E> std::fmt::Debug for PendingBroadcast<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingBroadcast")
            .field("event", &std::any::type_name::<E>())
            .field("to_others", &self.to_others)
            .field("connection", &self.connection)
            .finish()
    }
}

/// Begin broadcasting an event — Laravel's `broadcast()` helper.
///
/// ```ignore
/// broadcast(OrderShipmentStatusUpdated { order }).to_others().await?;
/// broadcast(OrderShipmentStatusUpdated { order }).via("pusher").await?;
/// ```
pub fn broadcast<E: ShouldBroadcast>(event: E) -> PendingBroadcast<E> {
    PendingBroadcast::new(event)
}
