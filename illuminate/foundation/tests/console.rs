use illuminate_foundation::Application;
use illuminate_foundation::testing::TestApp;
use illuminate_routing::Route;

fn test_app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let builder = Application::configure_detached(dir.path()).with_routing(|routing| {
        routing
            .web(|| {
                Route::get("/", || async { "Home" }).name("home");
                Route::get("/users/{user}", || async { "User" }).name("users.show");
            })
            .commands(|| {
                illuminate_console::Artisan::command("greet {name}", |cmd| async move {
                    cmd.info(format!("Hello {}!", cmd.argument("name").unwrap_or_default()));
                    Ok(())
                })
                .purpose("Greet someone");
            });
    });
    (TestApp::new(builder), dir)
}

#[tokio::test]
async fn framework_commands_are_available() {
    let (app, _dir) = test_app();

    app.artisan("inspire").assert_successful().await;
    app.artisan("env")
        .expects_output_to_contain("The application environment is [testing].")
        .assert_successful()
        .await;
    app.artisan("route:list")
        .expects_output_to_contain("users/{user}")
        .expects_output_to_contain("users.show")
        .expects_output_to_contain("Showing [2] routes")
        .assert_successful()
        .await;
    app.artisan("about")
        .expects_output_to_contain("Laravel Version")
        .assert_successful()
        .await;
    app.artisan("key:generate --show")
        .expects_output_to_contain("base64:")
        .assert_successful()
        .await;
    app.artisan("greet Taylor")
        .expects_output("Hello Taylor!")
        .assert_successful()
        .await;
}

#[tokio::test]
async fn maintenance_mode_can_be_toggled() {
    let (mut app, _dir) = test_app();

    app.artisan("down --secret=letmein")
        .expects_output_to_contain("Application is now in maintenance mode.")
        .assert_successful()
        .await;
    app.get("/").await.assert_service_unavailable();
    app.get("/letmein").await.assert_redirect(Some("/"));
    app.get("/").await.assert_ok().assert_see("Home");

    app.artisan("up")
        .expects_output_to_contain("Application is now live.")
        .assert_successful()
        .await;
    app.flush_cookies();
    app.get("/").await.assert_ok();
}
