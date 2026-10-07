//! The Sanctum service provider.

use std::sync::Arc;

use illuminate_auth::{AuthManager, Guard};
use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};
use illuminate_routing::Router;
use illuminate_support::{Map, Value};

use crate::config;
use crate::facade::{Sanctum, SanctumState};
use crate::guard::SanctumGuard;
use crate::http::define_routes;

/// Registers Sanctum: its configuration defaults, the `sanctum` guard
/// driver *and* the `sanctum` guard itself (so `auth:sanctum` works without
/// touching `config/auth.php`), and the `/sanctum/csrf-cookie` route.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_auth::{Auth, AuthServiceProvider};
/// use illuminate_config::Repository;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_support::json;
/// use laravel_sanctum::{SanctumGuard, SanctumServiceProvider};
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({})));
/// AuthServiceProvider.register(&container);
/// SanctumServiceProvider.register(&container);
///
/// let guard = Auth::guard("sanctum");
/// assert!(guard.downcast_ref::<SanctumGuard>().is_some());
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct SanctumServiceProvider;

impl SanctumServiceProvider {
    /// Register the `sanctum` guard driver with the auth manager.
    pub fn register_guard(manager: &AuthManager) {
        manager.extend(Sanctum::GUARD, |_app, name, config| {
            Ok(Arc::new(SanctumGuard::new(name, config)) as Arc<dyn Guard>)
        });
    }

    /// Define the `auth.guards.sanctum` guard (merged under any
    /// configuration the application already has for it).
    fn configure_guard(config: &Repository) {
        let mut guard = Map::new();
        guard.insert("driver".into(), Value::String(Sanctum::GUARD.into()));
        guard.insert("provider".into(), Value::Null);
        if let Value::Object(existing) = config.get("auth.guards.sanctum") {
            guard.extend(existing);
        }
        config.set("auth.guards.sanctum", Value::Object(guard));
    }
}

impl ServiceProvider for SanctumServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton_if::<SanctumState>(|_| Arc::new(SanctumState::default()));

        app.singleton_if::<Repository>(|_| Arc::new(Repository::empty()));
        let config = app.make::<Repository>();
        Self::configure_guard(&config);
        config::merge_defaults(&config);

        // Like Laravel's `Auth::resolved(...)`: extend the auth manager
        // whenever it's built (or right away, if it already was).
        app.resolving::<AuthManager>(|manager, _| Self::register_guard(manager));
        if app.resolved::<AuthManager>()
            && let Ok(manager) = app.try_make::<AuthManager>()
        {
            Self::register_guard(&manager);
        }
    }

    fn boot(&self, app: &Container) {
        if let Ok(manager) = app.try_make::<AuthManager>() {
            Self::register_guard(&manager);
        }
        app.singleton_if::<Router>(|_| Arc::new(Router::new()));
        let router = app.make::<Router>();

        // `auth:sanctum` routes can check abilities, and `stateful_api()`
        // runs first-party SPA requests through the session.
        for (alias, factory) in crate::http::middleware::middleware_aliases() {
            router.alias_middleware_factory(alias, factory);
        }
        router.alias_middleware_instance(
            "sanctum.stateful",
            Arc::new(crate::http::middleware::EnsureFrontendRequestsAreStateful::new()),
        );

        illuminate_console::Artisan::register(crate::console::PruneExpired);

        if config::routes_enabled() {
            define_routes(&router);
        }
    }
}
