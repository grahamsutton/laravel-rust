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
        .expects_output_to_contain("storage.local")
        .expects_output_to_contain("Showing [3] routes")
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

#[tokio::test]
async fn generators_create_and_register_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("app/http")).unwrap();
    std::fs::write(dir.path().join("app/mod.rs"), "pub mod http;\n").unwrap();
    std::fs::write(dir.path().join("app/http/mod.rs"), "").unwrap();
    std::fs::create_dir_all(dir.path().join("bootstrap")).unwrap();
    std::fs::write(
        dir.path().join("bootstrap/providers.rs"),
        "pub fn providers() -> Vec<Box<dyn ServiceProvider>> {\n    vec![\n        Box::new(crate::app::providers::AppServiceProvider),\n    ]\n}\n",
    )
    .unwrap();
    let app = TestApp::new(Application::configure_detached(dir.path()));

    app.artisan("make:controller Admin/PhotoController --resource")
        .expects_output_to_contain("Controller [app/http/controllers/admin/photo_controller.rs] created successfully.")
        .assert_successful()
        .await;
    let controller = std::fs::read_to_string(dir.path().join("app/http/controllers/admin/photo_controller.rs")).unwrap();
    assert!(controller.contains("impl ResourceController for PhotoController"));
    assert!(std::fs::read_to_string(dir.path().join("app/http/mod.rs")).unwrap().contains("pub mod controllers;"));
    assert!(std::fs::read_to_string(dir.path().join("app/http/controllers/admin/mod.rs")).unwrap().contains("pub use photo_controller::PhotoController;"));

    app.artisan("make:controller Admin/PhotoController")
        .expects_output_to_contain("Controller already exists.")
        .assert_failed()
        .await;

    app.artisan("make:command SendEmails").assert_successful().await;
    let command = std::fs::read_to_string(dir.path().join("app/console/commands/send_emails.rs")).unwrap();
    assert!(command.contains("\"app:send-emails\""));

    app.artisan("make:provider RiakServiceProvider").assert_successful().await;
    let providers = std::fs::read_to_string(dir.path().join("bootstrap/providers.rs")).unwrap();
    assert!(providers.contains("Box::new(crate::app::providers::RiakServiceProvider),"));

    app.artisan("make:view users.index").assert_successful().await;
    assert!(dir.path().join("resources/views/users/index.blade.html").exists());
}
