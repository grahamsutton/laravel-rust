//! The service provider: configuration, the guard, and the routes.

use illuminate_support::json;
use std::sync::Arc;

use illuminate_auth::{Auth, AuthManager, AuthServiceProvider};
use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};
use laravel_sanctum::{SanctumGuard, SanctumServiceProvider, config};

use crate::support::app;

#[tokio::test]
async fn the_provider_defines_the_sanctum_guard_and_config() {
    let app = app().await;
    let config = app.config();

    assert_eq!(
        config.get("auth.guards.sanctum"),
        json!({"driver": "sanctum", "provider": null})
    );
    assert_eq!(config.get("sanctum.guard"), json!(["web"]));
    assert_eq!(config.get("sanctum.expiration"), json!(null));
    assert_eq!(config.get("sanctum.token_prefix"), json!(""));
    assert_eq!(
        config.get("sanctum.stateful"),
        json!([
            "localhost",
            "localhost:3000",
            "127.0.0.1",
            "127.0.0.1:8000",
            "::1"
        ])
    );
    assert_eq!(
        config.get("sanctum.middleware"),
        json!({
            "authenticate_session": true,
            "encrypt_cookies": true,
            "validate_csrf_token": true,
        })
    );
    assert!(
        Auth::guard("sanctum")
            .downcast_ref::<SanctumGuard>()
            .is_some()
    );
}

#[test]
fn the_guard_driver_is_registered_after_the_auth_manager_was_built() {
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({"auth": {"guards": {
        "api": {"driver": "sanctum", "provider": "users"},
    }}})));
    AuthServiceProvider.register(&container);
    let manager = container.make::<AuthManager>();

    SanctumServiceProvider.register(&container);
    SanctumServiceProvider.boot(&container);

    let api = manager.guard(Some("api")).unwrap();
    let api = api.downcast_ref::<SanctumGuard>().unwrap();
    assert_eq!(api.provider_name(), Some("users"));
    assert!(manager.guard(Some("sanctum")).is_ok());
}

#[test]
fn the_guard_driver_is_registered_before_the_auth_manager_is_built() {
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({})));

    SanctumServiceProvider.register(&container);
    AuthServiceProvider.register(&container);

    assert!(Auth::try_guard("sanctum").is_ok());
    assert_eq!(config::guards(), ["web"]);
}

#[test]
fn the_application_config_wins() {
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({
        "auth": {"guards": {"sanctum": {"provider": "admins"}}},
        "sanctum": {"guard": ["admin"], "stateful": ["spa.test"]},
    })));

    SanctumServiceProvider.register(&container);

    let config = container.make::<Repository>();
    assert_eq!(
        config.get("auth.guards.sanctum"),
        json!({"driver": "sanctum", "provider": "admins"})
    );
    assert_eq!(config.get("sanctum.guard"), json!(["admin"]));
    assert_eq!(config.get("sanctum.stateful"), json!(["spa.test"]));
    assert_eq!(config.get("sanctum.token_prefix"), json!(""));
}
