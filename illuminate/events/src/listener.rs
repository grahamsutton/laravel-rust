//! Listeners: closures, listener structs, queued listeners, and the small
//! amount of plumbing that lets the dispatcher treat them all the same way.

use std::any::{Any, TypeId, type_name};
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use illuminate_support::{Error, Result, Value};

/// Whether an event should keep travelling to the remaining listeners.
///
/// Returning [`Propagation::Stop`] (or simply `false`) from a listener is the
/// Rust spelling of Laravel's "return `false` to stop propagation".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Propagation {
    /// Keep calling the remaining listeners.
    #[default]
    Continue,
    /// Stop propagating the event to any further listeners.
    Stop,
}

/// An "exception" that halts an event without being treated as a failure.
///
/// Listener structs return `Result<()>`, so they stop propagation by
/// returning this error, usually through [`stop_propagation`]. The
/// dispatcher recognizes it and simply skips the remaining listeners.
#[derive(Debug, Clone, Copy, Default, thiserror::Error)]
#[error("Event propagation was stopped by a listener.")]
pub struct StopPropagation;

/// Stop the event from reaching any further listeners.
///
/// ```
/// use illuminate_events::{Listener, async_trait, stop_propagation};
/// use illuminate_support::Result;
///
/// struct OrderShipped { digital: bool }
/// struct ShipPackage;
///
/// #[async_trait]
/// impl Listener<OrderShipped> for ShipPackage {
///     async fn handle(&self, event: &OrderShipped) -> Result<()> {
///         if event.digital {
///             return stop_propagation();
///         }
///         Ok(())
///     }
/// }
/// ```
pub fn stop_propagation() -> Result<()> {
    Err(StopPropagation.into())
}

/// Values a listener may return: `()`, `bool`, [`Propagation`], or a
/// `Result` of any of those (with the framework's [`Error`]).
pub trait ListenerOutput: Send + 'static {
    /// Convert the listener's return value into a propagation decision.
    fn into_propagation(self) -> Result<Propagation>;
}

impl ListenerOutput for () {
    fn into_propagation(self) -> Result<Propagation> {
        Ok(Propagation::Continue)
    }
}

impl ListenerOutput for bool {
    fn into_propagation(self) -> Result<Propagation> {
        Ok(if self {
            Propagation::Continue
        } else {
            Propagation::Stop
        })
    }
}

impl ListenerOutput for Propagation {
    fn into_propagation(self) -> Result<Propagation> {
        Ok(self)
    }
}

impl ListenerOutput for Result<(), Error> {
    fn into_propagation(self) -> Result<Propagation> {
        self.map(|_| Propagation::Continue)
    }
}

impl ListenerOutput for Result<bool, Error> {
    fn into_propagation(self) -> Result<Propagation> {
        self.and_then(ListenerOutput::into_propagation)
    }
}

impl ListenerOutput for Result<Propagation, Error> {
    fn into_propagation(self) -> Result<Propagation> {
        self
    }
}

/// A class-style event listener.
///
/// ```
/// use illuminate_events::{Listener, async_trait};
/// use illuminate_support::Result;
///
/// struct OrderShipped { order_id: u64 }
///
/// struct SendShipmentNotification;
///
/// #[async_trait]
/// impl Listener<OrderShipped> for SendShipmentNotification {
///     async fn handle(&self, event: &OrderShipped) -> Result<()> {
///         println!("Order {} shipped!", event.order_id);
///         Ok(())
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait Listener<E: Send + Sync + 'static>: Send + Sync + 'static {
    /// Handle the event.
    async fn handle(&self, event: &E) -> Result<()>;

    /// Determine whether a [queued](ShouldQueue) listener should be queued
    /// for the given event. Returning `false` skips the listener entirely.
    fn should_queue(&self, _event: &E) -> bool {
        true
    }
}

/// Marks a listener that should run on the queue instead of inline.
///
/// Register queued listeners with
/// [`Dispatcher::listen_queued`](crate::Dispatcher::listen_queued). The
/// queue component installs a hook with
/// [`Dispatcher::queue_listeners_using`](crate::Dispatcher::queue_listeners_using);
/// until it does, queued listeners simply run inline (like Laravel's `sync`
/// queue driver).
pub trait ShouldQueue: Send + Sync + 'static {
    /// The name of the queue connection the job should be sent to.
    fn via_connection(&self) -> Option<String> {
        None
    }

    /// The name of the queue the job should be sent to.
    fn via_queue(&self) -> Option<String> {
        None
    }

    /// The time before the job should be processed.
    fn with_delay(&self) -> Option<Duration> {
        None
    }
}

/// A listener on its way to the queue.
///
/// Queue hooks receive one of these for every queued listener that should
/// run. Call [`QueuedListener::handle`] (now, or later on a worker) to run
/// the listener against the event.
pub struct QueuedListener {
    /// The listener's type name.
    pub listener: &'static str,
    /// The event's type name.
    pub event: &'static str,
    /// The connection requested by the listener's `via_connection`.
    pub connection: Option<String>,
    /// The queue requested by the listener's `via_queue`.
    pub queue: Option<String>,
    /// The delay requested by the listener's `with_delay`.
    pub delay: Option<Duration>,
    job: Box<dyn FnOnce() -> BoxFuture<'static, Result<()>> + Send>,
}

impl QueuedListener {
    /// Create a queued listener from a job closure.
    pub fn new(
        listener: &'static str,
        event: &'static str,
        job: impl FnOnce() -> BoxFuture<'static, Result<()>> + Send + 'static,
    ) -> Self {
        Self {
            listener,
            event,
            connection: None,
            queue: None,
            delay: None,
            job: Box::new(job),
        }
    }

    /// Run the listener.
    pub async fn handle(self) -> Result<()> {
        match (self.job)().await {
            Err(error) if error.is::<StopPropagation>() => Ok(()),
            other => other,
        }
    }
}

impl fmt::Debug for QueuedListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueuedListener")
            .field("listener", &self.listener)
            .field("event", &self.event)
            .field("connection", &self.connection)
            .field("queue", &self.queue)
            .field("delay", &self.delay)
            .finish_non_exhaustive()
    }
}

pub(crate) type AnyEvent = Arc<dyn Any + Send + Sync>;

/// The function a queue hook implements.
pub(crate) type QueueHook =
    Arc<dyn Fn(QueuedListener) -> BoxFuture<'static, Result<()>> + Send + Sync>;

pub(crate) type TypedHandler = Arc<
    dyn Fn(AnyEvent, Option<QueueHook>) -> BoxFuture<'static, Result<Propagation>> + Send + Sync,
>;

pub(crate) type NamedHandler =
    Arc<dyn Fn(&str, &Value) -> BoxFuture<'static, Result<Propagation>> + Send + Sync>;

/// A typed listener, ready to be stored by the dispatcher.
pub struct RegisteredListener {
    pub(crate) name: &'static str,
    pub(crate) type_id: TypeId,
    pub(crate) queued: bool,
    pub(crate) handler: TypedHandler,
}

impl RegisteredListener {
    /// The listener's type name (closures are named after the function
    /// that defined them).
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Whether the listener runs on the queue.
    pub fn is_queued(&self) -> bool {
        self.queued
    }
}

impl fmt::Debug for RegisteredListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisteredListener")
            .field("name", &self.name)
            .field("queued", &self.queued)
            .finish_non_exhaustive()
    }
}

/// A listener for string-named events (`"eloquent.created: App\\Models\\User"`).
pub(crate) struct NamedListener {
    pub(crate) name: &'static str,
    pub(crate) handler: NamedHandler,
}

/// Anything that can listen for events of type `E`.
///
/// You never implement this yourself: closures taking `&E` or `Arc<E>` and
/// [`Listener`] implementations all qualify. The `Marker` parameter only
/// exists to keep those implementations apart.
pub trait IntoListener<E, Marker>: Send + Sync + 'static {
    /// Convert into a listener the dispatcher can store.
    fn into_listener(self) -> RegisteredListener;
}

#[doc(hidden)]
pub struct ByRef<Fut>(PhantomData<fn() -> Fut>);

#[doc(hidden)]
pub struct ByArc<Fut>(PhantomData<fn() -> Fut>);

#[doc(hidden)]
pub struct ByListener;

impl<E, F, Fut> IntoListener<E, ByRef<Fut>> for F
where
    E: Send + Sync + 'static,
    F: Fn(&E) -> Fut + Send + Sync + 'static,
    Fut: Future + Send + 'static,
    Fut::Output: ListenerOutput,
{
    fn into_listener(self) -> RegisteredListener {
        let handler: TypedHandler = Arc::new(move |event: AnyEvent, _| {
            let event = event
                .downcast_ref::<E>()
                .expect("event listener received a mismatched event type");
            let future = self(event);
            Box::pin(async move { future.await.into_propagation() })
        });
        RegisteredListener {
            name: type_name::<F>(),
            type_id: TypeId::of::<F>(),
            queued: false,
            handler,
        }
    }
}

impl<E, F, Fut> IntoListener<E, ByArc<Fut>> for F
where
    E: Send + Sync + 'static,
    F: Fn(Arc<E>) -> Fut + Send + Sync + 'static,
    Fut: Future + Send + 'static,
    Fut::Output: ListenerOutput,
{
    fn into_listener(self) -> RegisteredListener {
        let handler: TypedHandler = Arc::new(move |event: AnyEvent, _| {
            let event = event
                .downcast::<E>()
                .unwrap_or_else(|_| panic!("event listener received a mismatched event type"));
            let future = self(event);
            Box::pin(async move { future.await.into_propagation() })
        });
        RegisteredListener {
            name: type_name::<F>(),
            type_id: TypeId::of::<F>(),
            queued: false,
            handler,
        }
    }
}

impl<E, L> IntoListener<E, ByListener> for L
where
    E: Send + Sync + 'static,
    L: Listener<E>,
{
    fn into_listener(self) -> RegisteredListener {
        let listener = Arc::new(self);
        let handler: TypedHandler = Arc::new(move |event: AnyEvent, _| {
            let listener = listener.clone();
            Box::pin(async move {
                let event = event
                    .downcast_ref::<E>()
                    .expect("event listener received a mismatched event type");
                listener.handle(event).await?;
                Ok(Propagation::Continue)
            })
        });
        RegisteredListener {
            name: type_name::<L>(),
            type_id: TypeId::of::<L>(),
            queued: false,
            handler,
        }
    }
}

/// Build the registration for a queued listener.
pub(crate) fn queued_listener<E, L>(listener: L) -> RegisteredListener
where
    E: Send + Sync + 'static,
    L: Listener<E> + ShouldQueue,
{
    let listener = Arc::new(listener);
    let handler: TypedHandler = Arc::new(move |event: AnyEvent, hook: Option<QueueHook>| {
        let listener = listener.clone();
        Box::pin(async move {
            let event = event
                .downcast::<E>()
                .unwrap_or_else(|_| panic!("event listener received a mismatched event type"));

            if !listener.should_queue(&event) {
                return Ok(Propagation::Continue);
            }

            let mut job = {
                let listener = listener.clone();
                QueuedListener::new(type_name::<L>(), type_name::<E>(), move || {
                    Box::pin(async move { listener.handle(&event).await })
                })
            };
            job.connection = listener.via_connection();
            job.queue = listener.via_queue();
            job.delay = listener.with_delay();

            match hook {
                Some(hook) => hook(job).await?,
                None => job.handle().await?,
            }

            Ok(Propagation::Continue)
        })
    });
    RegisteredListener {
        name: type_name::<L>(),
        type_id: TypeId::of::<L>(),
        queued: true,
        handler,
    }
}

/// Build the registration for a string-named event listener.
pub(crate) fn named_listener<F, Fut>(listener: F) -> NamedListener
where
    F: Fn(&str, &Value) -> Fut + Send + Sync + 'static,
    Fut: Future + Send + 'static,
    Fut::Output: ListenerOutput,
{
    NamedListener {
        name: type_name::<F>(),
        handler: Arc::new(move |name: &str, payload: &Value| {
            let future = listener(name, payload);
            Box::pin(async move { future.await.into_propagation() })
        }),
    }
}
