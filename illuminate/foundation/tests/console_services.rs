//! `reload` and `cache:prune-stale-tags`.

use illuminate_cache::{CacheManager, RedisStore};
use illuminate_console::Artisan;
use illuminate_foundation::Application;
use illuminate_foundation::console::commands::ReloadCommands;
use illuminate_foundation::testing::TestApp;
use illuminate_redis::testing::RedisServer;
use illuminate_support::json;

fn app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    (TestApp::new(Application::configure_detached(dir.path())), dir)
}

#[tokio::test]
async fn running_services_are_reloaded() {
    let (app, _dir) = app();

    app.artisan("reload")
        .expects_output_to_contain("Reloading services.")
        .expects_output_to_contain("queue")
        .expects_output_to_contain("schedule")
        .expects_output_to_contain("DONE")
        .doesnt_expect_output_to_contain("FAIL")
        .assert_successful()
        .await;

    // Packages add their own reload commands.
    app.app().bootstrap_console();
    Artisan::command("courier:restart", |cmd| async move {
        cmd.info("Courier restarted.");
        Ok(())
    });
    Artisan::command("courier:broken", |cmd| async move { cmd.fail("Broken.") });
    ReloadCommands::reloads("courier:restart", "courier");
    ReloadCommands::reloads("courier:broken", "broken");

    app.artisan("reload --except=queue,schedule:interrupt,broken")
        .expects_output_to_contain("courier")
        .doesnt_expect_output_to_contain("queue")
        .doesnt_expect_output_to_contain("schedule")
        .doesnt_expect_output_to_contain("Courier restarted.")
        .assert_successful()
        .await;

    app.artisan("reload -e queue")
        .expects_output_to_contain("broken")
        .expects_output_to_contain("FAIL")
        .assert_successful()
        .await;
}

#[tokio::test]
async fn stale_tags_are_only_pruned_on_redis() {
    let (app, _dir) = app();

    app.artisan("cache:prune-stale-tags")
        .expects_output_to_contain("Stale cache tags pruned successfully.")
        .assert_successful()
        .await;

    app.artisan("cache:prune-stale-tags missing").assert_failed().await;
}

#[tokio::test]
async fn stale_tags_are_pruned_from_redis() {
    let server = RedisServer::shared();
    let (app, _dir) = app();
    app.app().override_config("database.redis", server.config());
    app.app().override_config("cache.stores.redis", json!({"driver": "redis", "connection": "cache"}));
    app.app().override_config("cache.prefix", "laravel-cache-");

    let cache = app.app().make::<CacheManager>().store("redis").unwrap();
    cache.tags(["people", "artists"]).unwrap().forever("John", "Lennon").await.unwrap();
    cache.tags(["people", "authors"]).unwrap().forever("Anne", "Rice").await.unwrap();
    cache.tags(["authors"]).unwrap().flush().await.unwrap();

    let redis = RedisStore::of(&cache.get_store()).unwrap();
    let connection = redis.connection().unwrap();
    let items = || async {
        connection
            .keys("laravel-cache-*")
            .await
            .unwrap()
            .into_iter()
            .filter(|key| !key.contains("tag:"))
            .count()
    };
    assert_eq!(items().await, 2);

    app.artisan("cache:prune-stale-tags redis")
        .expects_output_to_contain("Stale cache tags pruned successfully.")
        .assert_successful()
        .await;

    assert_eq!(items().await, 1);
    assert_eq!(
        cache.tags(["people", "artists"]).unwrap().get("John").await.unwrap(),
        Some(json!("Lennon"))
    );
    assert_eq!(redis.current_tags().await.unwrap(), ["artists", "people"]);
}
