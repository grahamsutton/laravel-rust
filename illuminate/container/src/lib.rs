//! # Illuminate Container
//!
//! The service container is a powerful tool for managing class dependencies
//! and performing dependency injection. Services are bound by *type*, and
//! resolved as shared `Arc` handles:
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_container::Container;
//!
//! trait Greeter: Send + Sync {
//!     fn greet(&self) -> String;
//! }
//!
//! struct EnglishGreeter;
//!
//! impl Greeter for EnglishGreeter {
//!     fn greet(&self) -> String {
//!         "Hello!".into()
//!     }
//! }
//!
//! let container = Container::new();
//!
//! container.singleton::<dyn Greeter>(|_| Arc::new(EnglishGreeter));
//!
//! assert_eq!(container.make::<dyn Greeter>().greet(), "Hello!");
//! ```

use std::any::{Any, TypeId, type_name};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

type AnyArc = Box<dyn Any + Send + Sync>;
type Factory = Arc<dyn Fn(&Container) -> AnyArc + Send + Sync>;
type Callback = Arc<dyn Fn(&dyn Any, &Container) + Send + Sync>;
type Extender = Arc<dyn Fn(AnyArc, &Container) -> AnyArc + Send + Sync>;

struct Binding {
    factory: Factory,
    shared: bool,
    scoped: bool,
}

static GLOBAL: LazyLock<RwLock<Option<Arc<Container>>>> = LazyLock::new(|| RwLock::new(None));

thread_local! {
    static LOCAL: RefCell<Vec<Arc<Container>>> = const { RefCell::new(Vec::new()) };
}

/// The service container.
#[derive(Default)]
pub struct Container {
    bindings: RwLock<HashMap<TypeId, Binding>>,
    instances: RwLock<HashMap<TypeId, AnyArc>>,
    scoped_instances: RwLock<Vec<TypeId>>,
    resolved: RwLock<HashMap<TypeId, bool>>,
    resolving: RwLock<HashMap<TypeId, Vec<Callback>>>,
    extenders: RwLock<HashMap<TypeId, Vec<Extender>>>,
    names: RwLock<HashMap<TypeId, &'static str>>,
}

/// Thrown when the container is asked for something it doesn't know how to build.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Target [{target}] is not instantiable. Did you forget to bind it in a service provider?")]
pub struct BindingResolutionException {
    pub target: String,
}

/// Types the container can build on its own, without an explicit binding.
///
/// This is Rust's take on Laravel's "zero configuration" autowiring:
/// implement `Injectable` (by hand, or with `#[derive(Injectable)]`) and the
/// container can construct your type by resolving each of its dependencies.
pub trait Injectable: Send + Sync + Sized + 'static {
    fn inject(container: &Container) -> Self;
}

impl Container {
    /// Create a new, empty container.
    pub fn new() -> Self {
        Self::default()
    }

    // ------------------------------------------------------------------
    // The global instance
    // ------------------------------------------------------------------

    /// Get the globally available container instance.
    ///
    /// A thread-local instance (see [`Container::set_local_instance`]) takes
    /// precedence, which keeps parallel tests isolated from each other.
    pub fn get_instance() -> Arc<Container> {
        if let Some(local) = LOCAL.with(|l| l.borrow().last().cloned()) {
            return local;
        }
        if let Some(global) = GLOBAL.read().unwrap().as_ref() {
            return global.clone();
        }
        let mut global = GLOBAL.write().unwrap();
        global.get_or_insert_with(|| Arc::new(Container::new())).clone()
    }

    /// Set the globally available container instance.
    pub fn set_instance(container: Arc<Container>) {
        *GLOBAL.write().unwrap() = Some(container);
    }

    /// Determine if a global (or thread-local) container has been set.
    pub fn has_instance() -> bool {
        LOCAL.with(|l| !l.borrow().is_empty()) || GLOBAL.read().unwrap().is_some()
    }

    /// Make the given container the current instance for this thread until
    /// the returned guard is dropped.
    pub fn set_local_instance(container: Arc<Container>) -> LocalInstanceGuard {
        LOCAL.with(|l| l.borrow_mut().push(container));
        LocalInstanceGuard { _private: () }
    }

    // ------------------------------------------------------------------
    // Registering bindings
    // ------------------------------------------------------------------

    /// Register a binding: the factory runs every time the type is resolved.
    pub fn bind<T: ?Sized + Send + Sync + 'static>(
        &self,
        factory: impl Fn(&Container) -> Arc<T> + Send + Sync + 'static,
    ) {
        self.register::<T>(factory, false, false);
    }

    /// Register a shared binding: the factory runs once, and the same
    /// instance is returned on every subsequent resolution.
    pub fn singleton<T: ?Sized + Send + Sync + 'static>(
        &self,
        factory: impl Fn(&Container) -> Arc<T> + Send + Sync + 'static,
    ) {
        self.register::<T>(factory, true, false);
    }

    /// Register a shared binding if it hasn't already been registered.
    pub fn singleton_if<T: ?Sized + Send + Sync + 'static>(
        &self,
        factory: impl Fn(&Container) -> Arc<T> + Send + Sync + 'static,
    ) {
        if !self.bound::<T>() {
            self.singleton::<T>(factory);
        }
    }

    /// Register a binding if it hasn't already been registered.
    pub fn bind_if<T: ?Sized + Send + Sync + 'static>(
        &self,
        factory: impl Fn(&Container) -> Arc<T> + Send + Sync + 'static,
    ) {
        if !self.bound::<T>() {
            self.bind::<T>(factory);
        }
    }

    /// Register a scoped binding: shared until [`Container::forget_scoped_instances`]
    /// is called (at the end of every request or job).
    pub fn scoped<T: ?Sized + Send + Sync + 'static>(
        &self,
        factory: impl Fn(&Container) -> Arc<T> + Send + Sync + 'static,
    ) {
        self.register::<T>(factory, true, true);
    }

    fn register<T: ?Sized + Send + Sync + 'static>(
        &self,
        factory: impl Fn(&Container) -> Arc<T> + Send + Sync + 'static,
        shared: bool,
        scoped: bool,
    ) {
        let id = TypeId::of::<T>();
        self.instances.write().unwrap().remove(&id);
        self.names.write().unwrap().insert(id, type_name::<T>());
        let factory: Factory = Arc::new(move |c| Box::new(factory(c)) as AnyArc);
        self.bindings.write().unwrap().insert(
            id,
            Binding {
                factory,
                shared,
                scoped,
            },
        );
        if scoped {
            self.scoped_instances.write().unwrap().push(id);
        }
    }

    /// Register an existing instance as shared in the container.
    pub fn instance<T: Send + Sync + 'static>(&self, value: T) -> Arc<T> {
        let arc = Arc::new(value);
        self.instance_arc::<T>(arc.clone());
        arc
    }

    /// Register an existing shared handle in the container.
    pub fn instance_arc<T: ?Sized + Send + Sync + 'static>(&self, value: Arc<T>) {
        let id = TypeId::of::<T>();
        self.names.write().unwrap().insert(id, type_name::<T>());
        self.instances
            .write()
            .unwrap()
            .insert(id, Box::new(value) as AnyArc);
    }

    /// "Extend" a type in the container, decorating the resolved instance.
    pub fn extend<T: ?Sized + Send + Sync + 'static>(
        &self,
        extender: impl Fn(Arc<T>, &Container) -> Arc<T> + Send + Sync + 'static,
    ) {
        let id = TypeId::of::<T>();
        let wrapped: Extender = Arc::new(move |boxed, c| {
            let arc = boxed
                .downcast::<Arc<T>>()
                .map(|b| *b)
                .expect("container extender received a mismatched type");
            Box::new(extender(arc, c)) as AnyArc
        });

        // Extending an already-resolved shared instance applies immediately.
        let existing = self.instances.write().unwrap().remove(&id);
        match existing {
            Some(instance) => {
                let extended = wrapped(instance, self);
                self.instances.write().unwrap().insert(id, extended);
            }
            None => self
                .extenders
                .write()
                .unwrap()
                .entry(id)
                .or_default()
                .push(wrapped),
        }
    }

    /// Register a callback to run whenever the type is resolved.
    pub fn resolving<T: ?Sized + Send + Sync + 'static>(
        &self,
        callback: impl Fn(&Arc<T>, &Container) + Send + Sync + 'static,
    ) {
        let callback: Callback = Arc::new(move |any, c| {
            if let Some(arc) = any.downcast_ref::<Arc<T>>() {
                callback(arc, c);
            }
        });
        self.resolving
            .write()
            .unwrap()
            .entry(TypeId::of::<T>())
            .or_default()
            .push(callback);
    }

    // ------------------------------------------------------------------
    // Resolving
    // ------------------------------------------------------------------

    /// Resolve the given type from the container.
    ///
    /// # Panics
    ///
    /// Panics when the type has not been bound. Use [`Container::try_make`]
    /// when the binding is optional.
    pub fn make<T: ?Sized + Send + Sync + 'static>(&self) -> Arc<T> {
        match self.try_make::<T>() {
            Ok(value) => value,
            Err(e) => panic!("{e}"),
        }
    }

    /// Attempt to resolve the given type from the container.
    pub fn try_make<T: ?Sized + Send + Sync + 'static>(
        &self,
    ) -> Result<Arc<T>, BindingResolutionException> {
        let id = TypeId::of::<T>();

        if let Some(instance) = self.instances.read().unwrap().get(&id)
            && let Some(arc) = instance.downcast_ref::<Arc<T>>() {
                return Ok(arc.clone());
            }

        let (factory, shared) = {
            let bindings = self.bindings.read().unwrap();
            match bindings.get(&id) {
                Some(binding) => (binding.factory.clone(), binding.shared),
                None => {
                    return Err(BindingResolutionException {
                        target: type_name::<T>().to_string(),
                    });
                }
            }
        };

        // The factory runs without any locks held so it may resolve its own
        // dependencies from the container.
        let mut object = factory(self);

        let extenders = self.extenders.read().unwrap().get(&id).cloned();
        if let Some(extenders) = extenders {
            for extender in extenders {
                object = extender(object, self);
            }
        }

        let callbacks = self.resolving.read().unwrap().get(&id).cloned();
        if let Some(callbacks) = callbacks {
            for callback in callbacks {
                callback(object.as_ref(), self);
            }
        }

        self.resolved.write().unwrap().insert(id, true);

        if shared {
            let mut instances = self.instances.write().unwrap();
            // Another thread may have won the race; the first instance wins.
            let stored = instances.entry(id).or_insert(object);
            return Ok(stored
                .downcast_ref::<Arc<T>>()
                .expect("container binding produced a mismatched type")
                .clone());
        }

        Ok(object
            .downcast::<Arc<T>>()
            .map(|b| *b)
            .expect("container binding produced a mismatched type"))
    }

    /// Build an [`Injectable`] type, resolving its dependencies.
    pub fn build<T: Injectable>(&self) -> T {
        T::inject(self)
    }

    /// Resolve the type if bound, otherwise build it as an [`Injectable`].
    pub fn make_or_build<T: Injectable>(&self) -> Arc<T> {
        self.try_make::<T>()
            .unwrap_or_else(|_| Arc::new(T::inject(self)))
    }

    /// Determine if the given type has been bound (or has an instance).
    pub fn bound<T: ?Sized + 'static>(&self) -> bool {
        let id = TypeId::of::<T>();
        self.bindings.read().unwrap().contains_key(&id)
            || self.instances.read().unwrap().contains_key(&id)
    }

    /// Determine if the given type has been resolved.
    pub fn resolved<T: ?Sized + 'static>(&self) -> bool {
        let id = TypeId::of::<T>();
        self.resolved.read().unwrap().contains_key(&id)
            || self.instances.read().unwrap().contains_key(&id)
    }

    /// Remove a resolved instance from the instance cache.
    pub fn forget_instance<T: ?Sized + 'static>(&self) {
        self.instances.write().unwrap().remove(&TypeId::of::<T>());
    }

    /// Clear all of the scoped instances from the container.
    pub fn forget_scoped_instances(&self) {
        let scoped = self.scoped_instances.read().unwrap().clone();
        let mut instances = self.instances.write().unwrap();
        for id in scoped {
            instances.remove(&id);
        }
    }

    /// Flush the container of all bindings and resolved instances.
    pub fn flush(&self) {
        self.bindings.write().unwrap().clear();
        self.instances.write().unwrap().clear();
        self.scoped_instances.write().unwrap().clear();
        self.resolved.write().unwrap().clear();
        self.resolving.write().unwrap().clear();
        self.extenders.write().unwrap().clear();
    }

    /// The names of every bound type (useful for debugging).
    pub fn bound_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.names.read().unwrap().values().copied().collect();
        names.sort_unstable();
        names
    }

    /// Whether the binding for `T` is shared.
    pub fn is_shared<T: ?Sized + 'static>(&self) -> bool {
        let id = TypeId::of::<T>();
        self.instances.read().unwrap().contains_key(&id)
            || self
                .bindings
                .read()
                .unwrap()
                .get(&id)
                .is_some_and(|b| b.shared && !b.scoped)
    }
}

/// Service providers are the central place of all application bootstrapping.
///
/// `register` should only bind things into the container; `boot` runs after
/// every provider has been registered, so it may use any other service.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::{Container, ServiceProvider};
///
/// struct Riak;
///
/// struct RiakServiceProvider;
///
/// impl ServiceProvider for RiakServiceProvider {
///     fn register(&self, app: &Container) {
///         app.singleton::<Riak>(|_| Arc::new(Riak));
///     }
/// }
/// ```
pub trait ServiceProvider: Send + Sync + 'static {
    /// Register any application services.
    fn register(&self, _app: &Container) {}

    /// Bootstrap any application services.
    fn boot(&self, _app: &Container) {}

    /// The provider's fully qualified type name (its identity).
    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// The provider's name, for display in `about` and debugging.
    fn name(&self) -> String {
        let full = std::any::type_name::<Self>();
        full.rsplit("::").next().unwrap_or(full).to_string()
    }
}

/// Restores the previous thread-local container when dropped.
pub struct LocalInstanceGuard {
    _private: (),
}

impl Drop for LocalInstanceGuard {
    fn drop(&mut self) {
        LOCAL.with(|l| {
            l.borrow_mut().pop();
        });
    }
}

/// Resolve a service from the current container.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::{app, Container};
///
/// struct Counter(u32);
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Counter(7));
///
/// assert_eq!(app::<Counter>().0, 7);
/// ```
pub fn app<T: ?Sized + Send + Sync + 'static>() -> Arc<T> {
    Container::get_instance().make::<T>()
}

/// Resolve a service from the current container, if it is bound.
pub fn try_app<T: ?Sized + Send + Sync + 'static>() -> Option<Arc<T>> {
    Container::get_instance().try_make::<T>().ok()
}

/// Alias of [`app`].
pub fn resolve<T: ?Sized + Send + Sync + 'static>() -> Arc<T> {
    app::<T>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Config {
        name: String,
    }

    struct Mailer {
        from: String,
    }

    #[test]
    fn it_resolves_bindings_fresh_each_time() {
        let container = Container::new();
        let built = Arc::new(AtomicUsize::new(0));
        let counter = built.clone();
        container.bind::<Config>(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Arc::new(Config { name: "Laravel".into() })
        });
        let a = container.make::<Config>();
        let b = container.make::<Config>();
        assert_eq!(a.name, "Laravel");
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(built.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn singletons_are_shared() {
        let container = Container::new();
        container.singleton::<Config>(|_| Arc::new(Config { name: "Laravel".into() }));
        let a = container.make::<Config>();
        let b = container.make::<Config>();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn factories_can_resolve_dependencies() {
        let container = Container::new();
        container.instance(Config { name: "Taylor".into() });
        container.singleton::<Mailer>(|c| {
            Arc::new(Mailer {
                from: c.make::<Config>().name.clone(),
            })
        });
        assert_eq!(container.make::<Mailer>().from, "Taylor");
    }

    #[test]
    fn trait_objects_can_be_bound() {
        trait Shape: Send + Sync {
            fn sides(&self) -> u32;
        }
        struct Square;
        impl Shape for Square {
            fn sides(&self) -> u32 {
                4
            }
        }
        let container = Container::new();
        container.bind::<dyn Shape>(|_| Arc::new(Square));
        assert_eq!(container.make::<dyn Shape>().sides(), 4);
    }

    #[test]
    fn extenders_decorate_instances() {
        let container = Container::new();
        container.singleton::<Config>(|_| Arc::new(Config { name: "Laravel".into() }));
        container.extend::<Config>(|config, _| {
            Arc::new(Config {
                name: format!("{} Framework", config.name),
            })
        });
        assert_eq!(container.make::<Config>().name, "Laravel Framework");
    }

    #[test]
    fn unbound_types_report_a_helpful_error() {
        let container = Container::new();
        let error = container.try_make::<Config>().err().unwrap();
        assert!(error.to_string().contains("is not instantiable"));
    }

    #[test]
    fn scoped_instances_can_be_forgotten() {
        let container = Container::new();
        container.scoped::<Config>(|_| Arc::new(Config { name: "Laravel".into() }));
        let a = container.make::<Config>();
        assert!(Arc::ptr_eq(&a, &container.make::<Config>()));
        container.forget_scoped_instances();
        assert!(!Arc::ptr_eq(&a, &container.make::<Config>()));
    }

    #[test]
    fn injectables_are_built_from_their_dependencies() {
        struct Service {
            config: Arc<Config>,
        }
        impl Injectable for Service {
            fn inject(c: &Container) -> Self {
                Self { config: c.make() }
            }
        }
        let container = Container::new();
        container.instance(Config { name: "Laravel".into() });
        assert_eq!(container.make_or_build::<Service>().config.name, "Laravel");
    }

    #[test]
    fn local_instances_take_precedence() {
        let container = Arc::new(Container::new());
        container.instance(Config { name: "Local".into() });
        {
            let _guard = Container::set_local_instance(container.clone());
            assert_eq!(app::<Config>().name, "Local");
        }
    }
}
