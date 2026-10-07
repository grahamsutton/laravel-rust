//! The `Event` facade and the `event()` helper.

use std::future::Future;
use std::sync::Arc;

use illuminate_container::{Container, try_app};
use illuminate_support::{Result, Value};

use crate::dispatcher::{Dispatcher, EventSubscriber};
use crate::fake::Matcher;
use crate::listener::{IntoListener, Listener, ListenerOutput, QueuedListener, ShouldQueue};

/// The `Event` facade.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_events::Event;
///
/// struct OrderShipped { order_id: u64 }
///
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// Event::listen(|event: Arc<OrderShipped>| async move {
///     println!("Order {} shipped!", event.order_id);
///     Ok(())
/// });
///
/// assert!(Event::has_listeners::<OrderShipped>());
/// ```
pub struct Event;

impl Event {
    /// Get the event dispatcher from the container, registering one if the
    /// application hasn't yet.
    pub fn dispatcher() -> Arc<Dispatcher> {
        if let Some(dispatcher) = try_app::<Dispatcher>() {
            return dispatcher;
        }
        let container = Container::get_instance();
        container.singleton_if::<Dispatcher>(|_| Arc::new(Dispatcher::new()));
        container.make::<Dispatcher>()
    }

    /// Register an event listener with the dispatcher.
    ///
    /// See [`Dispatcher::listen`] for the shapes a listener may take.
    pub fn listen<E, M>(listener: impl IntoListener<E, M>)
    where
        E: Send + Sync + 'static,
    {
        Self::dispatcher().listen(listener);
    }

    /// Register a [`Listener`] implementation for the event `E`.
    pub fn listen_with<E, L>(listener: L)
    where
        E: Send + Sync + 'static,
        L: Listener<E>,
    {
        Self::dispatcher().listen_with::<E, L>(listener);
    }

    /// Register a queued listener for the event `E`.
    pub fn listen_queued<E, L>(listener: L)
    where
        E: Send + Sync + 'static,
        L: Listener<E> + ShouldQueue,
    {
        Self::dispatcher().listen_queued::<E, L>(listener);
    }

    /// Register a listener for a string-named event (`*` wildcards allowed).
    pub fn listen_named<F, Fut>(event: &str, listener: F)
    where
        F: Fn(&str, &Value) -> Fut + Send + Sync + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: ListenerOutput,
    {
        Self::dispatcher().listen_named(event, listener);
    }

    /// Register an event subscriber with the dispatcher.
    pub fn subscribe(subscriber: impl EventSubscriber) {
        Self::dispatcher().subscribe(subscriber);
    }

    /// Set the hook used to put queued listeners on the queue.
    pub fn queue_listeners_using<F, Fut>(hook: F)
    where
        F: Fn(QueuedListener) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self::dispatcher().queue_listeners_using(hook);
    }

    /// Determine if the event `E` has listeners.
    pub fn has_listeners<E: 'static>() -> bool {
        Self::dispatcher().has_listeners::<E>()
    }

    /// Determine if a string-named event has listeners.
    pub fn has_listeners_named(event: &str) -> bool {
        Self::dispatcher().has_listeners_named(event)
    }

    /// Dispatch an event and call its listeners.
    pub async fn dispatch<E: Send + Sync + 'static>(event: E) -> Result<()> {
        let dispatcher = Self::dispatcher();
        dispatcher.dispatch(event).await
    }

    /// Dispatch an event, returning `Ok(false)` if a listener halted it.
    pub async fn until<E: Send + Sync + 'static>(event: E) -> Result<bool> {
        let dispatcher = Self::dispatcher();
        dispatcher.until(event).await
    }

    /// Dispatch a string-named event with a payload.
    pub async fn dispatch_named(event: &str, payload: Value) -> Result<()> {
        let dispatcher = Self::dispatcher();
        dispatcher.dispatch_named(event, payload).await
    }

    /// Dispatch a string-named event, returning `Ok(false)` if a listener
    /// halted it.
    pub async fn until_named(event: &str, payload: Value) -> Result<bool> {
        let dispatcher = Self::dispatcher();
        dispatcher.until_named(event, payload).await
    }

    /// Run the future, dispatching the events it fires only once it
    /// completes (and never, if it fails).
    ///
    /// ```ignore
    /// Event::defer(async {
    ///     let user = User::create(json!({"name": "Victoria Otwell"})).await?;
    ///     user.posts().create(json!({"title": "My first post!"})).await?;
    ///     Ok(())
    /// })
    /// .await?;
    /// ```
    pub async fn defer<R>(callback: impl std::future::Future<Output = Result<R>>) -> Result<R> {
        let dispatcher = Self::dispatcher();
        dispatcher.defer(callback).await
    }

    /// Run the future, deferring only the given events.
    pub async fn defer_only<S: Into<String>, R>(
        events: impl IntoIterator<Item = S>,
        callback: impl std::future::Future<Output = Result<R>>,
    ) -> Result<R> {
        let dispatcher = Self::dispatcher();
        dispatcher.defer_only(events, callback).await
    }

    /// Register a named event and payload to be dispatched later.
    pub fn push(event: &str, payload: Value) {
        Self::dispatcher().push(event, payload);
    }

    /// Dispatch every payload pushed for the given event.
    pub async fn flush(event: &str) -> Result<()> {
        let dispatcher = Self::dispatcher();
        dispatcher.flush(event).await
    }

    /// Remove every listener for the event `E`.
    pub fn forget<E: 'static>() {
        Self::dispatcher().forget::<E>();
    }

    /// Remove the listeners for a string-named event (or wildcard pattern).
    pub fn forget_named(event: &str) {
        Self::dispatcher().forget_named(event);
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    /// Replace the bound dispatcher with a fake that records every event
    /// instead of calling its listeners.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_container::Container;
    /// use illuminate_events::Event;
    ///
    /// struct OrderShipped { order_id: u64 }
    /// struct OrderFailedToShip;
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// # let container = Arc::new(Container::new());
    /// # let _guard = Container::set_local_instance(container);
    /// Event::fake();
    ///
    /// Event::dispatch(OrderShipped { order_id: 1 }).await?;
    ///
    /// Event::assert_dispatched::<OrderShipped>();
    /// Event::assert_dispatched_with(|event: &OrderShipped| event.order_id == 1);
    /// Event::assert_dispatched_once::<OrderShipped>();
    /// Event::assert_not_dispatched::<OrderFailedToShip>();
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn fake() -> Arc<Dispatcher> {
        Self::swap_fake(Vec::new())
    }

    /// Fake only the event `E`; every other event is dispatched as normal.
    /// Chain [`Dispatcher::also_fake`] to fake more.
    pub fn fake_only<E: 'static>() -> Arc<Dispatcher> {
        Self::swap_fake(vec![Matcher::Type(std::any::TypeId::of::<E>())])
    }

    /// Fake only the matching named events (`*` wildcards allowed).
    pub fn fake_only_named(events: &[&str]) -> Arc<Dispatcher> {
        Self::swap_fake(
            events
                .iter()
                .map(|event| Matcher::Name(event.to_string()))
                .collect(),
        )
    }

    fn swap_fake(to_fake: Vec<Matcher>) -> Arc<Dispatcher> {
        let fake = Arc::new(Dispatcher::fake_of(Self::dispatcher(), to_fake));
        Container::get_instance().instance_arc::<Dispatcher>(fake.clone());
        fake
    }

    /// Fake events for the duration of the given callback, then restore the
    /// real dispatcher.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_container::Container;
    /// use illuminate_events::Event;
    ///
    /// struct OrderCreated;
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// # let container = Arc::new(Container::new());
    /// # let _guard = Container::set_local_instance(container);
    /// let count = Event::fake_for(|| async {
    ///     Event::dispatch(OrderCreated).await?;
    ///     Event::assert_dispatched::<OrderCreated>();
    ///     Ok::<_, illuminate_support::Error>(1)
    /// })
    /// .await?;
    ///
    /// assert!(!Event::dispatcher().is_fake());
    /// # assert_eq!(count, 1);
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub async fn fake_for<F, Fut, R>(callback: F) -> R
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = R>,
    {
        /// Puts the real dispatcher back, even if the callback panics.
        struct Restore {
            container: Arc<Container>,
            original: Arc<Dispatcher>,
        }

        impl Drop for Restore {
            fn drop(&mut self) {
                self.container
                    .instance_arc::<Dispatcher>(self.original.clone());
            }
        }

        let _restore = Restore {
            container: Container::get_instance(),
            original: Self::dispatcher(),
        };
        Self::fake();
        callback().await
    }

    /// Get every recorded event of type `E`.
    pub fn dispatched<E: Send + Sync + 'static>() -> Vec<Arc<E>> {
        Self::dispatcher().dispatched::<E>()
    }

    /// Get the payloads of every recorded named event matching the name.
    pub fn dispatched_named(event: &str) -> Vec<Value> {
        Self::dispatcher().dispatched_named(event)
    }

    /// Determine if the event `E` has been dispatched.
    pub fn has_dispatched<E: Send + Sync + 'static>() -> bool {
        Self::dispatcher().has_dispatched::<E>()
    }

    /// Assert that the event `E` was dispatched.
    #[track_caller]
    pub fn assert_dispatched<E: Send + Sync + 'static>() {
        Self::dispatcher().assert_dispatched::<E>();
    }

    /// Assert that an event `E` passing the given truth test was dispatched.
    #[track_caller]
    pub fn assert_dispatched_with<E: Send + Sync + 'static>(callback: impl Fn(&E) -> bool) {
        Self::dispatcher().assert_dispatched_with::<E>(callback);
    }

    /// Assert that the event `E` was dispatched exactly `times` times.
    #[track_caller]
    pub fn assert_dispatched_times<E: Send + Sync + 'static>(times: usize) {
        Self::dispatcher().assert_dispatched_times::<E>(times);
    }

    /// Assert that the event `E` was dispatched exactly once.
    #[track_caller]
    pub fn assert_dispatched_once<E: Send + Sync + 'static>() {
        Self::dispatcher().assert_dispatched_once::<E>();
    }

    /// Assert that the event `E` was not dispatched.
    #[track_caller]
    pub fn assert_not_dispatched<E: Send + Sync + 'static>() {
        Self::dispatcher().assert_not_dispatched::<E>();
    }

    /// Assert that no event `E` passing the given truth test was dispatched.
    #[track_caller]
    pub fn assert_not_dispatched_with<E: Send + Sync + 'static>(callback: impl Fn(&E) -> bool) {
        Self::dispatcher().assert_not_dispatched_with::<E>(callback);
    }

    /// Assert that a named event was dispatched.
    #[track_caller]
    pub fn assert_dispatched_named(event: &str) {
        Self::dispatcher().assert_dispatched_named(event);
    }

    /// Assert that a named event passing the given truth test was dispatched.
    #[track_caller]
    pub fn assert_dispatched_named_with(event: &str, callback: impl Fn(&Value) -> bool) {
        Self::dispatcher().assert_dispatched_named_with(event, callback);
    }

    /// Assert that a named event was dispatched exactly `times` times.
    #[track_caller]
    pub fn assert_dispatched_named_times(event: &str, times: usize) {
        Self::dispatcher().assert_dispatched_named_times(event, times);
    }

    /// Assert that a named event was not dispatched.
    #[track_caller]
    pub fn assert_not_dispatched_named(event: &str) {
        Self::dispatcher().assert_not_dispatched_named(event);
    }

    /// Assert that no events were dispatched.
    #[track_caller]
    pub fn assert_nothing_dispatched() {
        Self::dispatcher().assert_nothing_dispatched();
    }

    /// Assert that the listener `L` is attached to the event `E`.
    #[track_caller]
    pub fn assert_listening<E: 'static, L: 'static>() {
        Self::dispatcher().assert_listening::<E, L>();
    }

    /// Assert that the event `E` has at least one listener.
    #[track_caller]
    pub fn assert_has_listeners<E: 'static>() {
        Self::dispatcher().assert_has_listeners::<E>();
    }
}

/// Dispatch an event and call its listeners.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_events::{event, Event};
///
/// struct UserRegistered { id: u64 }
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// Event::fake();
///
/// event(UserRegistered { id: 1 }).await?;
///
/// Event::assert_dispatched::<UserRegistered>();
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub async fn event<E: Send + Sync + 'static>(event: E) -> Result<()> {
    Event::dispatch(event).await
}

/// Events that know how to dispatch themselves.
///
/// Implement it (there is nothing to write) to get Laravel's
/// `OrderShipped::dispatch($order)` style:
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_events::{Dispatchable, Event};
///
/// struct OrderShipped { order_id: u64 }
///
/// impl Dispatchable for OrderShipped {}
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// Event::fake();
///
/// OrderShipped { order_id: 1 }.dispatch().await?;
/// OrderShipped { order_id: 2 }.dispatch_if(false).await?;
///
/// Event::assert_dispatched_once::<OrderShipped>();
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub trait Dispatchable: Sized + Send + Sync + 'static {
    /// Dispatch the event with the given arguments.
    fn dispatch(self) -> impl Future<Output = Result<()>> + Send {
        Event::dispatch(self)
    }

    /// Dispatch the event if the given condition is true.
    fn dispatch_if(self, condition: bool) -> impl Future<Output = Result<()>> + Send {
        async move {
            if condition {
                Event::dispatch(self).await
            } else {
                Ok(())
            }
        }
    }

    /// Dispatch the event unless the given condition is true.
    fn dispatch_unless(self, condition: bool) -> impl Future<Output = Result<()>> + Send {
        self.dispatch_if(!condition)
    }
}
