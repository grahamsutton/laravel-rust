//! Teaching the event dispatcher which events to broadcast.
//!
//! Laravel's dispatcher notices `ShouldBroadcast` events by reflection.
//! Rust can't, so broadcastable events are registered: with
//! [`register_broadcast!`](crate::register_broadcast) (picked up when the
//! [`BroadcastServiceProvider`](crate::BroadcastServiceProvider) boots), or
//! at runtime with [`Broadcast::register`](crate::Broadcast::register).
//! The [`broadcast`](crate::broadcast) helper registers events itself.

use std::any::type_name;
use std::marker::PhantomData;
use std::sync::Mutex;

use async_trait::async_trait;

use illuminate_events::{Dispatcher, Listener};
use illuminate_support::Result;

use crate::contracts::ShouldBroadcast;
use crate::facade::Broadcast;
use crate::pending::take_options;

/// The listener that broadcasts the event `E` whenever it's dispatched.
pub struct BroadcastListener<E>(PhantomData<fn() -> E>);

impl<E> BroadcastListener<E> {
    /// Create the listener.
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<E> Default for BroadcastListener<E> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<E: ShouldBroadcast> Listener<E> for BroadcastListener<E> {
    async fn handle(&self, event: &E) -> Result<()> {
        let options = take_options::<E>().unwrap_or_default();
        if !event.broadcast_when() {
            return Ok(());
        }
        Broadcast::manager().queue_with(event, options).await
    }
}

/// Serializes registrations so concurrent first broadcasts don't register
/// the listener twice.
static REGISTERING: Mutex<()> = Mutex::new(());

/// Make sure dispatching `E` through the dispatcher broadcasts it.
pub(crate) fn ensure_registered<E: ShouldBroadcast>(dispatcher: &Dispatcher) -> bool {
    let _lock = REGISTERING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if is_registered::<E>(dispatcher) {
        return false;
    }
    dispatcher.listen_with::<E, _>(BroadcastListener::<E>::new());
    true
}

/// Determine if dispatching `E` through the dispatcher broadcasts it.
pub fn is_registered<E: ShouldBroadcast>(dispatcher: &Dispatcher) -> bool {
    dispatcher
        .get_listeners::<E>()
        .contains(&type_name::<BroadcastListener<E>>())
}

/// A broadcastable event type, collected by
/// [`register_broadcast!`](crate::register_broadcast).
#[derive(Clone, Copy)]
pub struct BroadcastRegistration {
    name: fn() -> &'static str,
    register: fn(&Dispatcher) -> bool,
}

impl BroadcastRegistration {
    /// The registration for the event `E`.
    pub const fn of<E: ShouldBroadcast>() -> Self {
        Self {
            name: type_name::<E>,
            register: ensure_registered::<E>,
        }
    }

    /// The event's type name.
    pub fn name(&self) -> &'static str {
        (self.name)()
    }

    /// Register the event's broadcast listener on the dispatcher (once).
    pub fn register(&self, dispatcher: &Dispatcher) -> bool {
        (self.register)(dispatcher)
    }
}

impl std::fmt::Debug for BroadcastRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BroadcastRegistration")
            .field("event", &self.name())
            .finish()
    }
}

illuminate_queue::__private::inventory::collect!(BroadcastRegistration);

/// Every event type registered with [`register_broadcast!`](crate::register_broadcast).
pub fn registered_events() -> Vec<&'static str> {
    let mut names: Vec<_> = illuminate_queue::__private::inventory::iter::<BroadcastRegistration>
        .into_iter()
        .map(BroadcastRegistration::name)
        .collect();
    names.sort_unstable();
    names
}

/// Hook broadcasting into the event dispatcher: every event registered with
/// [`register_broadcast!`](crate::register_broadcast) is broadcast whenever
/// it's dispatched. Returns the number of events newly registered.
///
/// The [`BroadcastServiceProvider`](crate::BroadcastServiceProvider) calls
/// this when it boots; calling it again is harmless.
pub fn install(dispatcher: &Dispatcher) -> usize {
    illuminate_queue::__private::inventory::iter::<BroadcastRegistration>
        .into_iter()
        .filter(|registration| registration.register(dispatcher))
        .count()
}

/// Register broadcastable events, so dispatching them — `Event::dispatch`,
/// `event()`, `OrderShipped { .. }.dispatch()` — broadcasts them, just like
/// Laravel's `ShouldBroadcast` events.
///
/// ```
/// use illuminate_broadcasting::{Channel, ShouldBroadcast, register_broadcast};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// pub struct OrderShipped {
///     pub order_id: u64,
/// }
///
/// impl ShouldBroadcast for OrderShipped {
///     fn broadcast_on(&self) -> Vec<Channel> {
///         vec![Channel::private(format!("orders.{}", self.order_id))]
///     }
/// }
///
/// register_broadcast!(OrderShipped);
///
/// assert!(illuminate_broadcasting::registered_events()
///     .iter()
///     .any(|name| name.ends_with("OrderShipped")));
/// ```
#[macro_export]
macro_rules! register_broadcast {
    ($($event:ty),+ $(,)?) => {
        $(
            $crate::__private::inventory::submit! {
                $crate::BroadcastRegistration::of::<$event>()
            }
        )+
    };
}
