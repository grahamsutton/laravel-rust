//! The `session` cache store: each visitor gets their own cache.

use illuminate_cache::Cache;
use illuminate_foundation::Application;
use illuminate_foundation::testing::TestApp;
use illuminate_routing::Route;
use illuminate_support::json;

fn app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let builder = Application::configure_detached(dir.path()).with_routing(|routing| {
        routing.web(|| {
            Route::get("/visits", || async {
                let cache = Cache::store("session")?;
                let visits = cache.increment("visits").await?;
                Ok::<_, illuminate_support::Error>(format!("Visit {visits}"))
            });
        });
    });
    let app = TestApp::new(builder);
    app.app().override_config("session.driver", "array");
    (app, dir)
}

#[tokio::test]
async fn items_are_cached_in_the_visitors_session() {
    let (mut app, _dir) = app();

    app.get("/visits").await.assert_see("Visit 1");
    app.get("/visits").await.assert_see("Visit 2").assert_session_has("_cache.visits", Some(json!({"value": 2, "expiresAt": 0.0})));

    // Another visitor starts from scratch.
    app.flush_cookies();
    app.get("/visits").await.assert_see("Visit 1");
}

#[tokio::test]
async fn the_store_needs_a_session() {
    let (_app, _dir) = app();

    let error = Cache::store("session").unwrap().get("visits").await.unwrap_err();
    assert_eq!(error.to_string(), "Session store not set on request.");
}
