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

/// A strict mode violation handler: the model class and the attributes (or
/// relation) involved.
pub(crate) type ViolationHandler = Arc<dyn Fn(&str, &[String]) + Send + Sync>;

tokio::task_local! {
    static EVENTS_MUTED: bool;
    static UNGUARDED: bool;
    static IGNORING_TIMESTAMPS: Vec<TypeId>;
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
    require_morph_map: AtomicBool,
    prevent_lazy_loading: AtomicBool,
    prevent_discarding_attributes: AtomicBool,
    prevent_missing_attributes: AtomicBool,
    discarded_attribute_handler: RwLock<Option<ViolationHandler>>,
    missing_attribute_handler: RwLock<Option<ViolationHandler>>,
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

    pub(crate) fn has_morph_alias(&self, class: &str) -> bool {
        self.morph_map
            .read()
            .unwrap()
            .values()
            .any(|mapped| mapped == class)
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

// ----------------------------------------------------------------------
// Morph map enforcement
// ----------------------------------------------------------------------

/// Require every polymorphic model to be in the morph map (Laravel's
/// `Relation::requireMorphMap()`): writing or querying a polymorphic type
/// for an unmapped model fails with a
/// [`ClassMorphViolationException`](super::ClassMorphViolationException).
pub fn require_morph_map(require: bool) {
    EloquentState::resolve()
        .require_morph_map
        .store(require, Ordering::SeqCst);
}

/// Determine whether the morph map is required.
pub fn requires_morph_map() -> bool {
    EloquentState::resolve()
        .require_morph_map
        .load(Ordering::SeqCst)
}

/// Define the morph map and require every polymorphic model to be in it
/// (Laravel's `Relation::enforceMorphMap`).
///
/// ```
/// use illuminate_database::eloquent::enforce_morph_map;
///
/// enforce_morph_map([("post", "Post"), ("video", "Video")]);
/// ```
pub fn enforce_morph_map<A: Into<String>, C: Into<String>>(map: impl IntoIterator<Item = (A, C)>) {
    morph_map(map);
    require_morph_map(true);
}

/// The morph class of a model, failing when the morph map is required and
/// the model isn't in it.
pub(crate) fn checked_morph_class<M: Model>() -> illuminate_support::Result<String> {
    let state = EloquentState::resolve();
    if state.require_morph_map.load(Ordering::SeqCst) && !state.has_morph_alias(M::class_name()) {
        return Err(super::ClassMorphViolationException::new(M::class_name()).into());
    }
    Ok(state.morph_alias(M::class_name()))
}

// ----------------------------------------------------------------------
// Timestamps
// ----------------------------------------------------------------------

/// Run the future without the model type `M` maintaining its timestamps
/// (Laravel's `Model::withoutTimestamps`).
pub(crate) async fn without_timestamps_for<M: Model, F: Future>(future: F) -> F::Output {
    let mut ignoring = IGNORING_TIMESTAMPS
        .try_with(|ignoring| ignoring.clone())
        .unwrap_or_default();
    ignoring.push(TypeId::of::<M>());
    IGNORING_TIMESTAMPS.scope(ignoring, future).await
}

/// Determine whether the model type is currently ignoring its timestamps.
pub(crate) fn is_ignoring_timestamps<M: Model>() -> bool {
    IGNORING_TIMESTAMPS
        .try_with(|ignoring| ignoring.contains(&TypeId::of::<M>()))
        .unwrap_or(false)
}

// ----------------------------------------------------------------------
// Strict mode
// ----------------------------------------------------------------------

/// Turn Eloquent's strict mode on or off: prevent lazy loading, silently
/// discarding attributes and accessing missing attributes (Laravel's
/// `Model::shouldBeStrict()`).
///
/// ```
/// use illuminate_database::eloquent::{should_be_strict, prevents_silently_discarding_attributes};
///
/// should_be_strict(true);
/// assert!(prevents_silently_discarding_attributes());
/// should_be_strict(false);
/// ```
pub fn should_be_strict(strict: bool) {
    prevent_lazy_loading(strict);
    prevent_silently_discarding_attributes(strict);
    prevent_accessing_missing_attributes(strict);
}

/// Prevent lazy loading relationships.
///
/// Rust models never lazy load: a relationship is either eager loaded into
/// its `#[relation]` field (`with`, `load`) or queried explicitly through
/// its method, so there is never a hidden query to prevent. The setting is
/// kept so strict mode reads the same as in Laravel.
pub fn prevent_lazy_loading(prevent: bool) {
    EloquentState::resolve()
        .prevent_lazy_loading
        .store(prevent, Ordering::SeqCst);
}

/// Determine whether lazy loading is prevented.
pub fn prevents_lazy_loading() -> bool {
    EloquentState::resolve()
        .prevent_lazy_loading
        .load(Ordering::SeqCst)
}

/// Fail (or report) when `fill`, `create`, `update`, ... are given
/// attributes that aren't mass assignable, instead of silently discarding
/// them.
pub fn prevent_silently_discarding_attributes(prevent: bool) {
    EloquentState::resolve()
        .prevent_discarding_attributes
        .store(prevent, Ordering::SeqCst);
}

/// Determine whether discarding attributes is prevented.
pub fn prevents_silently_discarding_attributes() -> bool {
    EloquentState::resolve()
        .prevent_discarding_attributes
        .load(Ordering::SeqCst)
}

/// Register a handler for discarded attributes (the model class and the
/// discarded keys) instead of failing.
pub fn handle_discarded_attribute_violation_using(
    handler: impl Fn(&str, &[String]) + Send + Sync + 'static,
) {
    *EloquentState::resolve()
        .discarded_attribute_handler
        .write()
        .unwrap() = Some(Arc::new(handler));
}

/// Fail (or report) when `try_get_attribute` reads an attribute that
/// doesn't exist or wasn't retrieved (a partial select).
pub fn prevent_accessing_missing_attributes(prevent: bool) {
    EloquentState::resolve()
        .prevent_missing_attributes
        .store(prevent, Ordering::SeqCst);
}

/// Determine whether accessing missing attributes is prevented.
pub fn prevents_accessing_missing_attributes() -> bool {
    EloquentState::resolve()
        .prevent_missing_attributes
        .load(Ordering::SeqCst)
}

/// Register a handler for missing attributes (the model class and the key)
/// instead of failing.
pub fn handle_missing_attribute_violation_using(
    handler: impl Fn(&str, &[String]) + Send + Sync + 'static,
) {
    *EloquentState::resolve()
        .missing_attribute_handler
        .write()
        .unwrap() = Some(Arc::new(handler));
}

pub(crate) fn discarded_attribute_handler() -> Option<ViolationHandler> {
    EloquentState::resolve()
        .discarded_attribute_handler
        .read()
        .unwrap()
        .clone()
}

pub(crate) fn missing_attribute_handler() -> Option<ViolationHandler> {
    EloquentState::resolve()
        .missing_attribute_handler
        .read()
        .unwrap()
        .clone()
}
