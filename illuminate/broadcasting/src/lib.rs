//! # Illuminate Broadcasting
//!
//! In many modern web applications, WebSockets are used to implement
//! realtime, live-updating user interfaces. Laravel makes it easy to
//! "broadcast" your server-side events over a WebSocket connection, so your
//! server and your JavaScript application (with Laravel Echo) share the same
//! event names and data.
//!
//! ## Defining broadcast events
//!
//! Implement [`ShouldBroadcast`] on an event and tell it which
//! [channels](Channel) to broadcast on. Its serialized fields are its
//! payload:
//!
//! ```
//! use illuminate_broadcasting::{Channel, PrivateChannel, ShouldBroadcast, register_broadcast};
//! use serde::Serialize;
//!
//! #[derive(Serialize)]
//! pub struct OrderShipmentStatusUpdated {
//!     pub order_id: u64,
//!     pub status: String,
//! }
//!
//! impl ShouldBroadcast for OrderShipmentStatusUpdated {
//!     fn broadcast_on(&self) -> Vec<Channel> {
//!         vec![PrivateChannel::new(format!("orders.{}", self.order_id))]
//!     }
//! }
//!
//! // Broadcast it whenever it's dispatched...
//! register_broadcast!(OrderShipmentStatusUpdated);
//! ```
//!
//! Then fire the event as you normally would — `Event::dispatch(event)` —
//! or use the [`broadcast`] helper, which can also exclude the current user:
//!
//! ```ignore
//! broadcast(OrderShipmentStatusUpdated { order_id: 1, status: "shipped".into() })
//!     .to_others()
//!     .await?;
//! ```
//!
//! Events are broadcast by a queued [`BroadcastEvent`] job, so your
//! responses stay fast; events that
//! [should broadcast now](ShouldBroadcast::should_broadcast_now) skip the
//! queue.
//!
//! ## Authorizing channels
//!
//! Private and presence channels are authorized by the callbacks you
//! register in `routes/channels.rs`. Laravel Echo asks the
//! `/broadcasting/auth` route (see [`Broadcast::routes`]) for permission to
//! subscribe:
//!
//! ```
//! # use std::sync::Arc;
//! # use illuminate_container::Container;
//! # let container = Arc::new(Container::new());
//! # let _guard = Container::set_local_instance(container);
//! use illuminate_auth::AuthUser;
//! use illuminate_broadcasting::Broadcast;
//! use illuminate_support::json;
//!
//! Broadcast::channel("orders.{order_id}", |user: AuthUser, order_id: u64| async move {
//!     user.id() == json!(order_id)
//! });
//!
//! Broadcast::channel("chat.{room_id}", |user: AuthUser, room_id: u64| async move {
//!     (room_id > 0).then(|| json!({"id": user.id()}))
//! });
//! ```
//!
//! ## Drivers
//!
//! Connections are configured in `config/broadcasting.php`
//! (`broadcasting.default`, `broadcasting.connections.*`): `reverb` and
//! `pusher` ([`PusherBroadcaster`]), `ably` ([`AblyBroadcaster`]), `log`
//! ([`LogBroadcaster`]) and `null` ([`NullBroadcaster`]). Add your own with
//! [`Broadcast::extend`]. HTTP APIs are called through the
//! `illuminate-http-client` `Http` facade, so `Http::fake()` works.
//!
//! ## Testing
//!
//! Like Laravel, `Event::fake()` records broadcast events instead of
//! broadcasting them, and the `log` and `null` drivers are handy in tests.
//! [`Broadcast::fake`] records every broadcast (name, channels, payload,
//! excluded socket) for assertions.

mod anonymous;
pub mod authorization;
pub mod broadcasters;
pub mod channel;
pub mod contracts;
mod event;
mod exceptions;
mod facade;
mod manager;
mod pending;
mod provider;
mod registration;
pub mod secretbox;
mod testing;

pub use anonymous::AnonymousEvent;
pub use authorization::{
    ChannelAuthorizer, ChannelCallback, ChannelOptions, ChannelRegistry, FromChannelParameter,
    IntoChannelResult, PendingChannel, UserAuthenticator,
};
pub use broadcasters::{
    AblyBroadcaster, LogBroadcaster, NullBroadcaster, Pusher, PusherBroadcaster, PusherException,
    PusherSettings,
};
pub use channel::{
    Channel, EncryptedPrivateChannel, HasBroadcastChannel, IntoChannels, PresenceChannel,
    PrivateChannel,
};
pub use contracts::{Broadcaster, ShouldBroadcast};
pub use event::BroadcastEvent;
pub use exceptions::BroadcastException;
pub use facade::Broadcast;
pub use manager::{BroadcastManager, BroadcasterCreator};
pub use pending::{BroadcastOptions, PendingBroadcast, broadcast};
pub use provider::BroadcastServiceProvider;
pub use registration::{
    BroadcastListener, BroadcastRegistration, install, is_registered, registered_events,
};
pub use testing::BroadcastFake;

/// Re-exported so implementors of [`Broadcaster`] and
/// [`FromChannelParameter`] don't need their own dependency.
pub use async_trait::async_trait;

/// The facades provided by this component.
pub mod facades {
    pub use crate::facade::Broadcast;
}

/// Everything you need to broadcast events and authorize channels, in one
/// import.
pub mod prelude {
    pub use crate::{
        Broadcast, Channel, EncryptedPrivateChannel, PresenceChannel, PrivateChannel,
        ShouldBroadcast, broadcast, register_broadcast,
    };
}

/// Re-exports used by the framework's macros. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    pub use illuminate_queue::__private::inventory;
}
