//! Eloquent's shared state: booted models, event listeners, global scopes,
//! the password hasher hook, the event dispatcher and exception reporter
//! hooks, mass assignment guarding and the morph map.
//!
//! The state lives in the service container, so every application (and
//! every test with its own container) gets an isolated set of listeners.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use illuminate_container::{Container, try_app};
use illuminate_support::Error;
use indexmap::IndexMap;

use super::builder::Builder;
use super::events::{EventDispatcher, Listener};
use super::model::Model;

/// A global scope: a closure that constrains every query for a model.
pub(crate) type ScopeFn<M> = Arc<dyn Fn(Builder<M>) -> Builder<M> + Send + Sync>;

type Hasher = Arc<dyn Fn(&str) -> String + Send + Sync>;

type Reporter = Arc<dyn Fn(&Error) + Send + Sync>;

tokio::task_local! {
    static EVENTS_MUTED: bool;
    static UNGUARDED: bool;
}

/// The listeners and global scopes registered for one model type.
pub(crate) struct Registry<M: Model> {
    pub(crate) listeners: Vec<Listener<M>>,
    pub(crate) scopes: Vec<(String, ScopeFn<M>)>,
}

impl<M: Model> Default for Registry<M> {
    fn default() -> Self {
        Self {
            listeners: Vec::new(),
            scopes: Vec::new(),
        }
    }
}

/// Eloquent's state for one application container.
#[derive(Default)]
pub(crate) struct EloquentState {
    booted: Mutex<HashSet<TypeId>>,
    registries: RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    hasher: RwLock<Option<Hasher>>,
    dispatcher: RwLock<Option<Arc<dyn EventDispatcher>>>,
    reporter: RwLock<Option<Reporter>>,
    unguarded: AtomicBool,
    morph_map: RwLock<IndexMap<String, String>>,
}

impl EloquentState {
    /// Resolve the state from the current container.
    pub(crate) fn resolve() -> Arc<Self> {
        if let Some(state) = try_app::<Self>() {
            return state;
        }
        let container = Container::get_instance();
        container.singleton_if::<Self>(|_| Arc::new(Self::default()));
        container.make::<Self>()
    }

    /// Boot the model type if it hasn't been booted yet (Laravel's
    /// `bootIfNotBooted`): its `#[observed_by]` observers are registered.
    pub(crate) fn boot<M: Model>(&self) {
        let first = self.booted.lock().unwrap().insert(TypeId::of::<M>());
        if first {
            M::boot();
        }
    }

    fn with_registry<M: Model, R>(&self, callback: impl FnOnce(&mut Registry<M>) -> R) -> R {
        let mut registries = self.registries.write().unwrap();
        let entry = registries
            .entry(TypeId::of::<M>())
            .or_insert_with(|| Box::new(Registry::<M>::default()));
        let registry = entry
            .downcast_mut::<Registry<M>>()
            .expect("registries are keyed by their model type");
        callback(registry)
    }

    fn read_registry<M: Model, R>(&self, callback: impl FnOnce(&Registry<M>) -> R) -> Option<R> {
        let registries = self.registries.read().unwrap();
        registries
            .get(&TypeId::of::<M>())
            .and_then(|entry| entry.downcast_ref::<Registry<M>>())
            .map(callback)
    }

    pub(crate) fn add_listener<M: Model>(&self, listener: Listener<M>) {
        self.with_registry::<M, _>(|registry| registry.listeners.push(listener));
    }

    pub(crate) fn listeners<M: Model>(&self) -> Vec<Listener<M>> {
        self.read_registry::<M, _>(|registry| registry.listeners.clone())
            .unwrap_or_default()
    }

    pub(crate) fn clear_listeners<M: Model>(&self) {
        self.with_registry::<M, _>(|registry| registry.listeners.clear());
    }

    pub(crate) fn add_global_scope<M: Model>(&self, name: String, scope: ScopeFn<M>) {
        self.with_registry::<M, _>(|registry| {
            registry.scopes.retain(|(existing, _)| existing != &name);
            registry.scopes.push((name, scope));
        });
    }

    pub(crate) fn global_scopes<M: Model>(&self) -> Vec<(String, ScopeFn<M>)> {
        self.read_registry::<M, _>(|registry| registry.scopes.clone())
            .unwrap_or_default()
    }

    pub(crate) fn hash(&self, value: &str) -> Option<String> {
        self.hasher
            .read()
            .unwrap()
            .as_ref()
            .map(|hasher| hasher(value))
    }

    pub(crate) fn event_dispatcher(&self) -> Option<Arc<dyn EventDispatcher>> {
        self.dispatcher.read().unwrap().clone()
    }

    pub(crate) fn set_event_dispatcher(&self, dispatcher: Option<Arc<dyn EventDispatcher>>) {
        *self.dispatcher.write().unwrap() = dispatcher;
    }

    pub(crate) fn reporter(&self) -> Option<Reporter> {
        self.reporter.read().unwrap().clone()
    }

    pub(crate) fn is_unguarded(&self) -> bool {
        self.unguarded.load(Ordering::SeqCst) || UNGUARDED.try_with(|u| *u).unwrap_or(false)
    }

    pub(crate) fn morph_alias(&self, class: &str) -> String {
        self.morph_map
            .read()
            .unwrap()
            .iter()
            .find(|(_, mapped)| mapped.as_str() == class)
            .map(|(alias, _)| alias.clone())
            .unwrap_or_else(|| class.to_string())
    }
}

/// Hash `#[hashed]` attributes with the given closure when models are saved.
///
/// The framework wires this to the `Hash` facade; values that already look
/// like bcrypt or Argon2 hashes are never re-hashed.
///
/// ```
/// use illuminate_database::eloquent::hash_using;
///
/// // In the framework: hash_using(|value| Hash::make(value));
/// hash_using(|value| format!("$2y$12${:0>53}", value.len()));
/// ```
pub fn hash_using(hasher: impl Fn(&str) -> String + Send + Sync + 'static) {
    *EloquentState::resolve().hasher.write().unwrap() = Some(Arc::new(hasher));
}

/// Report the exceptions Eloquent recovers from with the given closure
/// (Laravel reports them through the exception handler).
///
/// Pruning uses it: when pruning one model fails, the error is reported and
/// pruning carries on with the next model. Without a reporter, the error
/// is returned instead. The framework wires this to `report()`.
///
/// ```
/// use illuminate_database::eloquent::report_exceptions_using;
///
/// report_exceptions_using(|error| eprintln!("{error}"));
/// ```
pub fn report_exceptions_using(reporter: impl Fn(&Error) + Send + Sync + 'static) {
    *EloquentState::resolve().reporter.write().unwrap() = Some(Arc::new(reporter));
}

/// Determine whether a value is already a bcrypt or Argon2 hash.
///
/// ```
/// use illuminate_database::eloquent::is_hashed;
///
/// assert!(is_hashed("$2y$12$R9h/cIPz0gi.URNNX3kh2OPST9/PgBkqquzi.Ss7KIUgO2t0jWMUW"));
/// assert!(is_hashed("$argon2id$v=19$m=65536,t=4,p=1$c29tZXNhbHQ$hash"));
/// assert!(!is_hashed("password"));
/// ```
pub fn is_hashed(value: &str) -> bool {
    const PREFIXES: [&str; 6] = ["$2y$", "$2a$", "$2b$", "$2x$", "$argon2i$", "$argon2id$"];
    PREFIXES.iter().any(|prefix| value.starts_with(prefix)) && value.len() > 20
}

/// Hash a value with the registered hasher (when there is one and the value
/// isn't already hashed).
pub(crate) fn hash_value(value: &str) -> Option<String> {
    if value.is_empty() || is_hashed(value) {
        return None;
    }
    EloquentState::resolve().hash(value)
}

/// Disable mass assignment protection for every model (Laravel's
/// `Model::unguard()`).
pub fn unguard() {
    EloquentState::resolve()
        .unguarded
        .store(true, Ordering::SeqCst);
}

/// Re-enable mass assignment protection (Laravel's `Model::reguard()`).
pub fn reguard() {
    EloquentState::resolve()
        .unguarded
        .store(false, Ordering::SeqCst);
}

/// Determine whether mass assignment protection is currently disabled.
pub fn is_unguarded() -> bool {
    EloquentState::resolve().is_unguarded()
}

/// Run the future with mass assignment protection disabled (Laravel's
/// `Model::unguarded(fn () => ...)`).
pub async fn unguarded<F: Future>(future: F) -> F::Output {
    UNGUARDED.scope(true, future).await
}

/// Run the future without firing model events (Laravel's
/// `Model::withoutEvents(fn () => ...)`).
pub async fn without_events<F: Future>(future: F) -> F::Output {
    EVENTS_MUTED.scope(true, future).await
}

pub(crate) fn events_muted() -> bool {
    EVENTS_MUTED.try_with(|muted| *muted).unwrap_or(false)
}

/// Map short aliases to model classes for polymorphic relations, so the
/// `*_type` columns store `"post"` instead of `"Post"` (Laravel's
/// `Relation::enforceMorphMap`).
///
/// ```
/// use illuminate_database::eloquent::morph_map;
///
/// morph_map([("post", "Post"), ("video", "Video")]);
/// ```
pub fn morph_map<A: Into<String>, C: Into<String>>(map: impl IntoIterator<Item = (A, C)>) {
    let state = EloquentState::resolve();
    let mut morph_map = state.morph_map.write().unwrap();
    for (alias, class) in map {
        morph_map.insert(alias.into(), class.into());
    }
}
