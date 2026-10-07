//! The resources' "static" state: the wrapping key, per-resource wrappers
//! and forced wrapping.
//!
//! Laravel keeps these in static properties of `JsonResource`. Here they
//! live in the service container, so every application (and every test
//! with its own container) gets an isolated copy.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use illuminate_container::{Container, ServiceProvider, try_app};

/// The default key the outermost resource is wrapped in.
pub const DEFAULT_WRAPPER: &str = "data";

/// Wrapping configuration for one application container.
#[derive(Debug)]
pub(crate) struct ResourceState {
    wrapper: RwLock<Option<String>>,
    wrappers: RwLock<HashMap<TypeId, Option<String>>>,
    force_wrapping: AtomicBool,
}

impl Default for ResourceState {
    fn default() -> Self {
        Self {
            wrapper: RwLock::new(Some(DEFAULT_WRAPPER.to_string())),
            wrappers: RwLock::new(HashMap::new()),
            force_wrapping: AtomicBool::new(false),
        }
    }
}

impl ResourceState {
    /// Resolve the state from the current container, registering it on
    /// first use.
    pub(crate) fn resolve() -> Arc<Self> {
        if let Some(state) = try_app::<Self>() {
            return state;
        }
        let container = Container::get_instance();
        container.singleton_if::<Self>(|_| Arc::new(Self::default()));
        container.make::<Self>()
    }

    /// The global wrapper (`None` after `without_wrapping`).
    pub(crate) fn global_wrapper(&self) -> Option<String> {
        self.wrapper.read().unwrap().clone()
    }

    /// Set (or clear) the global wrapper.
    pub(crate) fn set_global_wrapper(&self, wrapper: Option<String>) {
        *self.wrapper.write().unwrap() = wrapper;
    }

    /// The wrapper for the given resource type: its own, else the global one.
    pub(crate) fn wrapper_for<T: 'static>(&self) -> Option<String> {
        match self.wrappers.read().unwrap().get(&TypeId::of::<T>()) {
            Some(wrapper) => wrapper.clone(),
            None => self.global_wrapper(),
        }
    }

    /// Set (or clear) the wrapper of one resource type.
    pub(crate) fn set_wrapper_for<T: 'static>(&self, wrapper: Option<String>) {
        self.wrappers
            .write()
            .unwrap()
            .insert(TypeId::of::<T>(), wrapper);
    }

    /// Whether data is wrapped even when it already holds the wrapper key.
    pub(crate) fn force_wrapping(&self) -> bool {
        self.force_wrapping.load(Ordering::SeqCst)
    }

    /// Force (or stop forcing) wrapping.
    pub(crate) fn set_force_wrapping(&self, force: bool) {
        self.force_wrapping.store(force, Ordering::SeqCst);
    }

    /// Reset everything to Laravel's defaults.
    pub(crate) fn flush(&self) {
        self.set_global_wrapper(Some(DEFAULT_WRAPPER.to_string()));
        self.wrappers.write().unwrap().clear();
        self.set_force_wrapping(false);
    }
}

/// Registers the resources' wrapping state with the container.
///
/// Resources work without it (the state is registered on first use), but
/// registering it up front keeps the container's bindings explicit.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_http_resources::HttpResourcesServiceProvider;
///
/// let container = Arc::new(Container::new());
/// HttpResourcesServiceProvider.register(&container);
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct HttpResourcesServiceProvider;

impl ServiceProvider for HttpResourcesServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton_if::<ResourceState>(|_| Arc::new(ResourceState::default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct First;
    struct Second;

    #[test]
    fn it_defaults_to_the_data_wrapper() {
        let state = ResourceState::default();
        assert_eq!(state.global_wrapper().as_deref(), Some("data"));
        assert_eq!(state.wrapper_for::<First>().as_deref(), Some("data"));
        assert!(!state.force_wrapping());
    }

    #[test]
    fn per_type_wrappers_win_over_the_global_one() {
        let state = ResourceState::default();
        state.set_wrapper_for::<First>(Some("user".into()));
        state.set_global_wrapper(None);
        assert_eq!(state.wrapper_for::<First>().as_deref(), Some("user"));
        assert_eq!(state.wrapper_for::<Second>(), None);

        state.set_wrapper_for::<Second>(None);
        state.set_global_wrapper(Some("payload".into()));
        assert_eq!(state.wrapper_for::<Second>(), None);
    }

    #[test]
    fn it_can_be_flushed() {
        let state = ResourceState::default();
        state.set_wrapper_for::<First>(Some("user".into()));
        state.set_global_wrapper(None);
        state.set_force_wrapping(true);
        state.flush();
        assert_eq!(state.wrapper_for::<First>().as_deref(), Some("data"));
        assert!(!state.force_wrapping());
    }

    #[test]
    fn the_state_lives_in_the_container() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container.clone());
        HttpResourcesServiceProvider.register(&container);
        ResourceState::resolve().set_global_wrapper(None);
        assert_eq!(container.make::<ResourceState>().global_wrapper(), None);

        let other = Arc::new(Container::new());
        let _other = Container::set_local_instance(other);
        assert_eq!(
            ResourceState::resolve().global_wrapper().as_deref(),
            Some("data")
        );
    }
}
