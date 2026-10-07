//! The Sanctum service provider.

use std::sync::Arc;

use illuminate_auth::{AuthManager, Guard};
use illuminate_config::Repository;
use illuminate_container::{Container, Publishable, ServiceProvider};
use illuminate_routing::Router;
use illuminate_support::{Map, Value};

use crate::config;
use crate::facade::{Sanctum, SanctumState};
use crate::guard::SanctumGuard;
use crate::http::define_routes;

/// Sanctum's configuration file, published to `config/sanctum.rs` by
/// `cargo artisan vendor:publish --tag=sanctum-config`.
pub const CONFIG_STUB: &str = r#"use laravel::prelude::*;
use laravel::sanctum::Sanctum;

pub fn config() -> Value {
    json!({
        /*
        |--------------------------------------------------------------------------
        | Stateful Domains
        |--------------------------------------------------------------------------
        |
        | Requests from the following domains / hosts will receive stateful API
        | authentication cookies. Typically, these should include your local
        | and production domains which access your API via a frontend SPA.
        |
        */

        "stateful": env("SANCTUM_STATEFUL_DOMAINS", format!(
            "{}{}",
            "localhost,localhost:3000,127.0.0.1,127.0.0.1:8000,::1",
            Sanctum::current_application_url_with_port(),
            // Sanctum::current_request_host(),
        ))
        .to_string_lossy()
        .split(',')
        .map(str::to_string)
        .collect::<Vec<_>>(),

        /*
        |--------------------------------------------------------------------------
        | Sanctum Guards
        |--------------------------------------------------------------------------
        |
        | This array contains the authentication guards that will be checked when
        | Sanctum is trying to authenticate a request. If none of these guards
        | are able to authenticate the request, Sanctum will use the bearer
        | token that's present on an incoming request for authentication.
        |
        */

        "guard": ["web"],

        /*
        |--------------------------------------------------------------------------
        | Expiration Minutes
        |--------------------------------------------------------------------------
        |
        | This value controls the number of minutes until an issued token will be
        | considered expired. This will override any values set in the token's
        | "expires_at" attribute, but first-party sessions are not affected.
        |
        */

        "expiration": Value::Null,

        /*
        |--------------------------------------------------------------------------
        | Token Prefix
        |--------------------------------------------------------------------------
        |
        | Sanctum can prefix new tokens in order to take advantage of numerous
        | security scanning initiatives maintained by open source platforms
        | that notify developers if they commit tokens into repositories.
        |
        | See: https://docs.github.com/en/code-security/secret-scanning/about-secret-scanning
        |
        */

        "token_prefix": env("SANCTUM_TOKEN_PREFIX", ""),

        /*
        |--------------------------------------------------------------------------
        | Sanctum Middleware
        |--------------------------------------------------------------------------
        |
        | When authenticating your first-party SPA with Sanctum you may need to
        | customize some of the middleware Sanctum uses while processing the
        | request. Each entry may be `true` (Sanctum's middleware), `false`
        | (skip it), or the alias of a route middleware to run instead.
        |
        */

        "middleware": {
            "authenticate_session": true,
            "encrypt_cookies": true,
            "validate_csrf_token": true,
        },
    })
}
"#;

/// Sanctum's `personal_access_tokens` migration, published by
/// `cargo artisan vendor:publish --tag=sanctum-migrations`.
pub const MIGRATION_STUB: &str = r#"use laravel::prelude::*;

pub struct CreatePersonalAccessTokensTable;

#[async_trait]
impl Migration for CreatePersonalAccessTokensTable {
    /// Run the migrations.
    async fn up(&self) -> Result<()> {
        Schema::create("personal_access_tokens", |table| {
            table.id();
            table.morphs("tokenable");
            table.text("name");
            table.string_len("token", 64).unique();
            table.text("abilities").nullable();
            table.timestamp("last_used_at").nullable();
            table.timestamp("expires_at").nullable().index();
            table.timestamps();
        })
        .await
    }

    /// Reverse the migrations.
    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("personal_access_tokens").await
    }
}
"#;

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

        // `vendor:publish --tag=sanctum-config` / `--tag=sanctum-migrations`.
        self.publishes_migrations(
            app,
            [Publishable::migration(
                format!("{}.rs", crate::CreatePersonalAccessTokensTable::NAME),
                MIGRATION_STUB,
            )],
            "sanctum-migrations",
        );
        self.publishes(app, [Publishable::config("sanctum.rs", CONFIG_STUB)], "sanctum-config");
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

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_container::{PublishRegistry, PublishSource};
    use illuminate_support::json;

    #[test]
    fn the_configuration_and_migration_can_be_published() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container.clone());
        container.instance(Repository::new(json!({})));
        SanctumServiceProvider.register(&container);
        SanctumServiceProvider.boot(&container);

        let registry = PublishRegistry::resolve(&container);
        assert_eq!(registry.publishable_groups(), ["sanctum-config", "sanctum-migrations"]);
        assert!(registry.has_provider("SanctumServiceProvider"));

        let config = registry.paths_to_publish(None, Some("sanctum-config"));
        assert_eq!(config[0].destination(), "config/sanctum.rs");
        assert_eq!(config[0].source, PublishSource::Contents(CONFIG_STUB.into()));

        let migrations = registry.paths_to_publish(Some("laravel_sanctum::SanctumServiceProvider"), Some("sanctum-migrations"));
        assert!(migrations[0].migration);
        assert_eq!(
            migrations[0].destination(),
            "database/migrations/2019_12_14_000001_create_personal_access_tokens_table.rs"
        );
    }
}
