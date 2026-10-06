//! The event dispatcher.

use std::any::{TypeId, type_name};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, RwLock};

use indexmap::IndexMap;

use illuminate_support::{Result, Str, Value};

use crate::fake::EventFake;
use crate::listener::{
    AnyEvent, IntoListener, Listener, ListenerOutput, NamedListener, Propagation, QueueHook,
    QueuedListener, RegisteredListener, ShouldQueue, StopPropagation, named_listener,
    queued_listener,
};

/// The listeners registered for one event type.
struct TypedEntry {
    event: &'static str,
    listeners: Vec<Arc<RegisteredListener>>,
}

/// Event subscribers may subscribe to multiple events from within the
/// subscriber itself, keeping related handlers together.
///
/// ```
/// use illuminate_events::{Dispatcher, EventSubscriber};
///
/// struct Login { user_id: u64 }
/// struct Logout { user_id: u64 }
///
/// struct UserEventSubscriber;
///
/// impl EventSubscriber for UserEventSubscriber {
///     fn subscribe(&self, events: &Dispatcher) {
///         events.listen(|event: &Login| {
///             let id = event.user_id;
///             async move { println!("{id} logged in") }
///         });
///         events.listen(|event: &Logout| {
///             let id = event.user_id;
///             async move { println!("{id} logged out") }
///         });
///     }
/// }
///
/// let events = Dispatcher::new();
/// events.subscribe(UserEventSubscriber);
/// assert!(events.has_listeners::<Login>());
/// ```
pub trait EventSubscriber: Send + Sync + 'static {
    /// Register the listeners for the subscriber.
    fn subscribe(&self, events: &Dispatcher);
}

/// The event dispatcher.
///
/// Events are plain Rust values: any `Send + Sync + 'static` type can be
/// dispatched, and listeners are registered by the event's *type*. The
/// framework also uses string-named events (with `*` wildcards) for things
/// like `eloquent.created: App\Models\User`.
///
/// ```
/// use illuminate_events::Dispatcher;
///
/// struct OrderShipped { order_id: u64 }
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let events = Dispatcher::new();
///
/// events.listen(|event: &OrderShipped| {
///     let id = event.order_id;
///     async move {
///         println!("Order {id} shipped!");
///         Ok(())
///     }
/// });
///
/// events.dispatch(OrderShipped { order_id: 42 }).await?;
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Default)]
pub struct Dispatcher {
    typed: RwLock<IndexMap<TypeId, TypedEntry>>,
    named: RwLock<IndexMap<String, Vec<Arc<NamedListener>>>>,
    wildcards: RwLock<IndexMap<String, Vec<Arc<NamedListener>>>>,
    wildcards_cache: RwLock<HashMap<String, Vec<Arc<NamedListener>>>>,
    pushed: RwLock<IndexMap<String, Vec<Value>>>,
    queue_hook: RwLock<Option<QueueHook>>,
    pub(crate) fake: Option<EventFake>,
}

impl Dispatcher {
    /// Create a new event dispatcher instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// The dispatcher listeners are actually registered on: a fake forwards
    /// registrations to the dispatcher it replaced.
    pub(crate) fn registry(&self) -> &Dispatcher {
        match &self.fake {
            Some(fake) => &fake.original,
            None => self,
        }
    }

    // ------------------------------------------------------------------
    // Registering listeners
    // ------------------------------------------------------------------

    /// Register an event listener with the dispatcher.
    ///
    /// The listener may be a closure receiving `&E` (returning a `'static`
    /// future) or `Arc<E>` (so the future may hold on to the event), or any
    /// [`Listener`] implementation. Listeners may return `()`, `bool`,
    /// [`Propagation`], or a `Result` of those; returning `false` (or
    /// [`Propagation::Stop`]) stops the event from reaching later listeners.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_events::Dispatcher;
    ///
    /// struct PodcastProcessed { title: String }
    ///
    /// let events = Dispatcher::new();
    ///
    /// // Borrow the event, copy what you need, then do the async work...
    /// events.listen(|event: &PodcastProcessed| {
    ///     let title = event.title.clone();
    ///     async move {
    ///         println!("Processed {title}");
    ///         Ok(())
    ///     }
    /// });
    ///
    /// // Or take a shared handle to the event and keep it for the whole future...
    /// events.listen(|event: Arc<PodcastProcessed>| async move {
    ///     println!("Processed {}", event.title);
    ///     Ok(())
    /// });
    ///
    /// assert!(events.has_listeners::<PodcastProcessed>());
    /// ```
    pub fn listen<E, M>(&self, listener: impl IntoListener<E, M>)
    where
        E: Send + Sync + 'static,
    {
        self.register::<E>(listener.into_listener());
    }

    /// Register a [`Listener`] implementation for the event `E`.
    pub fn listen_with<E, L>(&self, listener: L)
    where
        E: Send + Sync + 'static,
        L: Listener<E>,
    {
        self.register::<E>(IntoListener::<E, _>::into_listener(listener));
    }

    /// Register a [queued](ShouldQueue) listener for the event `E`.
    ///
    /// The listener is handed to the queue hook installed with
    /// [`Dispatcher::queue_listeners_using`]; without one, it runs inline.
    pub fn listen_queued<E, L>(&self, listener: L)
    where
        E: Send + Sync + 'static,
        L: Listener<E> + ShouldQueue,
    {
        self.register::<E>(queued_listener::<E, L>(listener));
    }

    fn register<E: 'static>(&self, listener: RegisteredListener) {
        let registry = self.registry();
        let mut typed = registry.typed.write().unwrap();
        typed
            .entry(TypeId::of::<E>())
            .or_insert_with(|| TypedEntry {
                event: type_name::<E>(),
                listeners: Vec::new(),
            })
            .listeners
            .push(Arc::new(listener));
    }

    /// Register a listener for a string-named event.
    ///
    /// The event name may contain `*` wildcards. Listeners receive the
    /// event's name and its payload.
    ///
    /// ```
    /// use illuminate_events::Dispatcher;
    /// use illuminate_support::Value;
    ///
    /// let events = Dispatcher::new();
    ///
    /// events.listen_named("eloquent.created: *", |name: &str, payload: &Value| {
    ///     println!("{name}: {payload}");
    ///     async { Ok(()) }
    /// });
    ///
    /// assert!(events.has_listeners_named("eloquent.created: App\\Models\\User"));
    /// ```
    pub fn listen_named<F, Fut>(&self, event: &str, listener: F)
    where
        F: Fn(&str, &Value) -> Fut + Send + Sync + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: ListenerOutput,
    {
        let registry = self.registry();
        let listener = Arc::new(named_listener(listener));
        if event.contains('*') {
            registry
                .wildcards
                .write()
                .unwrap()
                .entry(event.to_string())
                .or_default()
                .push(listener);
            registry.wildcards_cache.write().unwrap().clear();
        } else {
            registry
                .named
                .write()
                .unwrap()
                .entry(event.to_string())
                .or_default()
                .push(listener);
        }
    }

    /// Register an event subscriber with the dispatcher.
    pub fn subscribe(&self, subscriber: impl EventSubscriber) {
        subscriber.subscribe(self.registry());
    }

    /// Set the hook used to put queued listeners on the queue.
    ///
    /// The queue component installs this when it boots. The hook receives a
    /// [`QueuedListener`]; it may push it onto a queue, spawn it, or simply
    /// run it with [`QueuedListener::handle`].
    pub fn queue_listeners_using<F, Fut>(&self, hook: F)
    where
        F: Fn(QueuedListener) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let hook: QueueHook = Arc::new(move |job| Box::pin(hook(job)));
        *self.registry().queue_hook.write().unwrap() = Some(hook);
    }

    /// Determine if a queue hook has been installed.
    pub fn has_queue_hook(&self) -> bool {
        self.registry().queue_hook.read().unwrap().is_some()
    }

    // ------------------------------------------------------------------
    // Inspecting listeners
    // ------------------------------------------------------------------

    /// Determine if the event `E` has listeners.
    pub fn has_listeners<E: 'static>(&self) -> bool {
        self.registry()
            .typed
            .read()
            .unwrap()
            .get(&TypeId::of::<E>())
            .is_some_and(|entry| !entry.listeners.is_empty())
    }

    /// Determine if a string-named event has listeners (wildcards included).
    pub fn has_listeners_named(&self, event: &str) -> bool {
        let registry = self.registry();
        registry.named.read().unwrap().contains_key(event)
            || registry.wildcards.read().unwrap().contains_key(event)
            || registry.has_wildcard_listeners(event)
    }

    /// Determine if the given event name has any wildcard listeners.
    pub fn has_wildcard_listeners(&self, event: &str) -> bool {
        self.registry()
            .wildcards
            .read()
            .unwrap()
            .keys()
            .any(|pattern| Str::is(pattern, event))
    }

    /// The names of the listeners registered for the event `E`, in the
    /// order they will run.
    pub fn get_listeners<E: 'static>(&self) -> Vec<&'static str> {
        self.typed_listeners(TypeId::of::<E>())
            .iter()
            .map(|listener| listener.name)
            .collect()
    }

    /// The names of the listeners that will receive a string-named event.
    pub fn get_listeners_named(&self, event: &str) -> Vec<&'static str> {
        self.registry()
            .named_listeners(event)
            .iter()
            .map(|listener| listener.name)
            .collect()
    }

    /// Every registered event (typed, named, and wildcard) with the names of
    /// its listeners, in registration order. Powers `event:list`.
    pub fn get_raw_listeners(&self) -> Vec<(String, Vec<&'static str>)> {
        let registry = self.registry();
        let mut raw: Vec<(String, Vec<&'static str>)> = registry
            .typed
            .read()
            .unwrap()
            .values()
            .map(|entry| {
                (
                    entry.event.to_string(),
                    entry.listeners.iter().map(|l| l.name).collect(),
                )
            })
            .collect();
        for map in [&registry.named, &registry.wildcards] {
            raw.extend(map.read().unwrap().iter().map(|(event, listeners)| {
                (event.clone(), listeners.iter().map(|l| l.name).collect())
            }));
        }
        raw
    }

    pub(crate) fn has_typed_listener(&self, event: TypeId, listener: TypeId) -> bool {
        self.typed_listeners(event)
            .iter()
            .any(|registered| registered.type_id == listener)
    }

    fn typed_listeners(&self, event: TypeId) -> Vec<Arc<RegisteredListener>> {
        self.registry()
            .typed
            .read()
            .unwrap()
            .get(&event)
            .map(|entry| entry.listeners.clone())
            .unwrap_or_default()
    }

    /// The listeners for a named event: exact listeners first, then the
    /// wildcard listeners whose pattern matches.
    fn named_listeners(&self, event: &str) -> Vec<Arc<NamedListener>> {
        let mut listeners = self
            .named
            .read()
            .unwrap()
            .get(event)
            .cloned()
            .unwrap_or_default();
        listeners.extend(self.wildcard_listeners(event));
        listeners
    }

    fn wildcard_listeners(&self, event: &str) -> Vec<Arc<NamedListener>> {
        if let Some(cached) = self.wildcards_cache.read().unwrap().get(event) {
            return cached.clone();
        }
        let found: Vec<Arc<NamedListener>> = self
            .wildcards
            .read()
            .unwrap()
            .iter()
            .filter(|(pattern, _)| Str::is(pattern, event))
            .flat_map(|(_, listeners)| listeners.iter().cloned())
            .collect();
        self.wildcards_cache
            .write()
            .unwrap()
            .insert(event.to_string(), found.clone());
        found
    }

    // ------------------------------------------------------------------
    // Dispatching
    // ------------------------------------------------------------------

    /// Dispatch an event and call its listeners, in registration order.
    ///
    /// The first listener error stops the dispatch and is returned.
    pub async fn dispatch<E: Send + Sync + 'static>(&self, event: E) -> Result<()> {
        self.until(event).await.map(|_| ())
    }

    /// Dispatch an event, reporting whether it ran to completion.
    ///
    /// Returns `Ok(false)` when a listener halted the event, which is how
    /// the framework lets listeners cancel an operation (Laravel's
    /// `Event::until(...) === false`).
    pub async fn until<E: Send + Sync + 'static>(&self, event: E) -> Result<bool> {
        if let Some(fake) = &self.fake {
            if fake.should_fake_typed(TypeId::of::<E>()) {
                fake.record_typed(TypeId::of::<E>(), type_name::<E>(), Arc::new(event));
                return Ok(true);
            }
            return fake.original.invoke_typed::<E>(Arc::new(event)).await;
        }
        self.invoke_typed::<E>(Arc::new(event)).await
    }

    async fn invoke_typed<E: 'static>(&self, event: AnyEvent) -> Result<bool> {
        let listeners = self.typed_listeners(TypeId::of::<E>());
        let hook = self.queue_hook.read().unwrap().clone();
        for listener in listeners {
            let outcome = (listener.handler)(event.clone(), hook.clone()).await;
            if !continues(outcome)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Dispatch a string-named event with a payload.
    ///
    /// ```
    /// use illuminate_events::Dispatcher;
    /// use illuminate_support::{json, Value};
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// let events = Dispatcher::new();
    /// events.listen_named("user.*", |name: &str, payload: &Value| {
    ///     assert_eq!(name, "user.registered");
    ///     assert_eq!(payload["id"], 1);
    ///     async { Ok(()) }
    /// });
    ///
    /// events.dispatch_named("user.registered", json!({"id": 1})).await?;
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub async fn dispatch_named(&self, event: &str, payload: Value) -> Result<()> {
        self.until_named(event, payload).await.map(|_| ())
    }

    /// Dispatch a string-named event, returning `Ok(false)` if a listener
    /// halted it.
    pub async fn until_named(&self, event: &str, payload: Value) -> Result<bool> {
        if let Some(fake) = &self.fake {
            if fake.should_fake_named(event) {
                fake.record_named(event, payload);
                return Ok(true);
            }
            return fake.original.invoke_named(event, payload).await;
        }
        self.invoke_named(event, payload).await
    }

    async fn invoke_named(&self, event: &str, payload: Value) -> Result<bool> {
        for listener in self.named_listeners(event) {
            let outcome = (listener.handler)(event, &payload).await;
            if !continues(outcome)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Register a named event and payload to be dispatched later, when the
    /// event is [flushed](Dispatcher::flush).
    pub fn push(&self, event: &str, payload: Value) {
        if self.fake.is_some() {
            return;
        }
        self.pushed
            .write()
            .unwrap()
            .entry(event.to_string())
            .or_default()
            .push(payload);
    }

    /// Dispatch every payload pushed for the given event.
    pub async fn flush(&self, event: &str) -> Result<()> {
        if self.fake.is_some() {
            return Ok(());
        }
        let payloads = self
            .pushed
            .read()
            .unwrap()
            .get(event)
            .cloned()
            .unwrap_or_default();
        for payload in payloads {
            self.dispatch_named(event, payload).await?;
        }
        Ok(())
    }

    /// Forget all of the pushed events.
    pub fn forget_pushed(&self) {
        if self.fake.is_none() {
            self.pushed.write().unwrap().clear();
        }
    }

    // ------------------------------------------------------------------
    // Removing listeners
    // ------------------------------------------------------------------

    /// Remove every listener for the event `E`.
    pub fn forget<E: 'static>(&self) {
        if self.fake.is_none() {
            self.typed.write().unwrap().shift_remove(&TypeId::of::<E>());
        }
    }

    /// Remove the listeners for a string-named event (or wildcard pattern).
    pub fn forget_named(&self, event: &str) {
        if self.fake.is_some() {
            return;
        }
        if event.contains('*') {
            self.wildcards.write().unwrap().shift_remove(event);
        } else {
            self.named.write().unwrap().shift_remove(event);
        }
        self.wildcards_cache
            .write()
            .unwrap()
            .retain(|key, _| !Str::is(event, key));
    }
}

/// Interpret a listener's outcome: `Ok(true)` keeps going, `Ok(false)` halts.
fn continues(outcome: Result<Propagation>) -> Result<bool> {
    match outcome {
        Ok(Propagation::Continue) => Ok(true),
        Ok(Propagation::Stop) => Ok(false),
        Err(error) if error.is::<StopPropagation>() => Ok(false),
        Err(error) => Err(error),
    }
}

impl fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dispatcher")
            .field("events", &self.get_raw_listeners())
            .field("fake", &self.fake.is_some())
            .finish()
    }
}
