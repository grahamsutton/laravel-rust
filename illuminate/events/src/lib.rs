//! # Illuminate Events
//!
//! Laravel's events provide a simple observer pattern implementation,
//! allowing you to subscribe and listen for various events that occur
//! within your application. Events serve as a great way to decouple various
//! aspects of your application, since a single event can have multiple
//! listeners that do not depend on each other.
//!
//! ## Defining events and listeners
//!
//! An event is any `Send + Sync + 'static` value: a plain struct carrying
//! the information related to the event. Listeners are async closures or
//! types implementing [`Listener`]:
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_container::Container;
//! use illuminate_events::{Event, Listener, async_trait};
//! use illuminate_support::Result;
//!
//! struct OrderShipped {
//!     order_id: u64,
//! }
//!
//! struct SendShipmentNotification;
//!
//! #[async_trait]
//! impl Listener<OrderShipped> for SendShipmentNotification {
//!     async fn handle(&self, event: &OrderShipped) -> Result<()> {
//!         println!("Order {} shipped!", event.order_id);
//!         Ok(())
//!     }
//! }
//!
//! # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
//! # let container = Arc::new(Container::new());
//! # let _guard = Container::set_local_instance(container);
//! // Typically in your AppServiceProvider's boot method...
//! Event::listen_with::<OrderShipped, _>(SendShipmentNotification);
//!
//! Event::listen(|event: Arc<OrderShipped>| async move {
//!     println!("Logging order {}", event.order_id);
//!     Ok(())
//! });
//!
//! // Then, anywhere in your application...
//! Event::dispatch(OrderShipped { order_id: 1 }).await?;
//! # Ok::<(), illuminate_support::Error>(())
//! # }).unwrap();
//! ```
//!
//! ## Registering listeners
//!
//! Rust has no runtime reflection, so Laravel's event discovery isn't
//! available: register your listeners and subscribers in the `boot` method
//! of a service provider (see [`EventServiceProvider`]).
//!
//! ## Stopping propagation
//!
//! Closure listeners may return `false` (or [`Propagation::Stop`]) to stop
//! the event from reaching later listeners; listener structs return
//! [`stop_propagation()`]. [`Dispatcher::until`] reports whether an event was
//! halted, which lets listeners cancel operations.
//!
//! ## Named events
//!
//! The framework uses string-named events (with `*` wildcards) for things
//! like Eloquent model events: see [`Dispatcher::listen_named`] and
//! [`Dispatcher::dispatch_named`].
//!
//! ## Queued listeners
//!
//! Listeners implementing [`ShouldQueue`] and registered with
//! [`Dispatcher::listen_queued`] are handed to the queue through the hook
//! installed with [`Dispatcher::queue_listeners_using`].
//!
//! ## Testing
//!
//! [`Event::fake`] swaps in a fake dispatcher that records events instead of
//! running listeners, so you may assert on them with
//! [`Event::assert_dispatched`] and friends.

mod defer;
mod dispatcher;
mod facade;
mod fake;
mod listener;
mod provider;

pub use async_trait::async_trait;
pub use dispatcher::{Dispatcher, EventSubscriber};
pub use facade::{Dispatchable, Event, event};
pub use listener::{
    IntoListener, Listener, ListenerOutput, Propagation, QueuedListener, RegisteredListener,
    ShouldQueue, StopPropagation, stop_propagation,
};
pub use provider::EventServiceProvider;

/// Implementation details referenced in public signatures.
#[doc(hidden)]
pub mod __private {
    pub use crate::listener::{ByArc, ByListener, ByRef};
}
