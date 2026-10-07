//! `config:publish`, `vendor:publish`, `lang:publish`, and `stub:publish`.

use std::path::Path;

use illuminate_container::{Container, Publishable, PublishRoot, ServiceProvider};
use illuminate_events::Event;
use illuminate_foundation::Application;
use illuminate_foundation::console::commands::{VendorTagPublished, publish::CONFIG_FILES};
use illuminate_foundation::testing::TestApp;

/// A package publishing its configuration, a migration, views and assets.
struct CourierServiceProvider {
    assets: std::path::PathBuf,
}

impl ServiceProvider for CourierServiceProvider {
    fn boot(&self, app: &Container) {
        self.publishes(app, [Publishable::config("courier.rs", "pub fn config() {}\n")], "courier-config");
        self.publishes_migrations(
            app,
            [Publishable::migration("2024_01_01_000000_create_couriers_table.rs", "// couriers\n")],
            "courier-migrations",
        );
        self.publishes(
            app,
            [
                Publishable::view("vendor/courier/mail.blade.html", "<p>Courier</p>\n"),
                Publishable::path(self.assets.clone(), PublishRoot::Public, "vendor/courier"),
            ],
            ["courier", "courier-assets"],
        );
    }
}

fn app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("app");
    std::fs::create_dir_all(base.join("config")).unwrap();
    std::fs::write(base.join("config/mod.rs"), "laravel::config_files![app, database];\n").unwrap();
    std::fs::write(base.join("config/app.rs"), "// mine\n").unwrap();

    let assets = dir.path().join("assets");
    std::fs::create_dir_all(assets.join("css")).unwrap();
    std::fs::write(assets.join("css/courier.css"), "body {}\n").unwrap();
    std::fs::write(assets.join("courier.js"), "// js\n").unwrap();

    let builder = Application::configure_detached(&base).with_provider(CourierServiceProvider { assets });
    (TestApp::new(builder), dir)
}

fn read(base: &Path, path: &str) -> String {
    std::fs::read_to_string(base.join(path)).unwrap_or_else(|_| panic!("[{path}] is missing"))
}

#[tokio::test]
async fn framework_configuration_files_can_be_published() {
    let (app, dir) = app();
    let base = dir.path().join("app");

    app.artisan("config:publish cors")
        .expects_output_to_contain("Published 'cors' configuration file.")
        .assert_successful()
        .await;
    let cors = read(&base, "config/cors.rs");
    assert!(cors.contains("| Cross-Origin Resource Sharing (CORS) Configuration"));
    assert!(cors.contains("\"paths\": [\"api/*\", \"sanctum/csrf-cookie\"],"));
    assert_eq!(read(&base, "config/mod.rs"), "laravel::config_files![app, cors, database];\n");

    app.artisan("config:publish cors")
        .expects_output_to_contain("The 'cors' configuration file already exists.")
        .assert_successful()
        .await;

    std::fs::write(base.join("config/cors.rs"), "// changed\n").unwrap();
    app.artisan("config:publish cors --force").assert_successful().await;
    assert!(read(&base, "config/cors.rs").contains("supports_credentials"));

    app.artisan("config:publish nothing")
        .expects_output_to_contain("Unrecognized configuration file.")
        .assert_failed()
        .await;

    let names: Vec<&str> = CONFIG_FILES.iter().map(|(name, _)| *name).collect();
    app.artisan("config:publish")
        .expects_choice("Which configuration file would you like to publish?", "hashing", names)
        .expects_output_to_contain("Published 'hashing' configuration file.")
        .assert_successful()
        .await;
    let hashing = read(&base, "config/hashing.rs");
    assert!(hashing.contains("\"driver\": env(\"HASH_DRIVER\", \"bcrypt\"),"));
    assert!(hashing.contains("\"rounds\": env(\"BCRYPT_ROUNDS\", 12),"));
}

#[tokio::test]
async fn every_missing_configuration_file_can_be_published_at_once() {
    let (app, dir) = app();
    let base = dir.path().join("app");

    app.artisan("config:publish --all")
        .expects_output_to_contain("The 'app' configuration file already exists.")
        .expects_output_to_contain("Published 'broadcasting' configuration file.")
        .expects_output_to_contain("Published 'concurrency' configuration file.")
        .expects_output_to_contain("Published 'view' configuration file.")
        .assert_successful()
        .await;

    assert_eq!(read(&base, "config/app.rs"), "// mine\n");
    for (name, contents) in CONFIG_FILES.iter().filter(|(name, _)| *name != "app") {
        assert_eq!(read(&base, &format!("config/{name}.rs")), *contents);
    }
    assert!(read(&base, "config/view.rs").contains("\"compiled\": env(\"VIEW_COMPILED_PATH\", storage_path(\"framework/views\")),"));
    assert!(read(&base, "config/images.rs").contains("env(\"IMAGE_DRIVER\", \"image\")"));
    assert!(read(&base, "config/concurrency.rs").contains("env(\"CONCURRENCY_DRIVER\", \"tokio\")"));
    assert_eq!(
        read(&base, "config/mod.rs"),
        "laravel::config_files![app, auth, broadcasting, cache, concurrency, cors, database, filesystems, hashing, images, logging, mail, queue, services, session, view];\n"
    );
}

#[tokio::test]
async fn package_files_are_published_by_tag() {
    let (app, dir) = app();
    let base = dir.path().join("app");
    Event::fake_only::<VendorTagPublished>();

    app.artisan("vendor:publish --tag=courier-config")
        .expects_output_to_contain("Publishing [courier-config] assets.")
        .expects_output_to_contain("Copying file [courier.rs] to [config/courier.rs]")
        .assert_successful()
        .await;
    assert_eq!(read(&base, "config/courier.rs"), "pub fn config() {}\n");
    assert_eq!(read(&base, "config/mod.rs"), "laravel::config_files![app, courier, database];\n");
    Event::assert_dispatched_with::<VendorTagPublished>(|event| {
        event.tag.as_deref() == Some("courier-config") && event.paths.len() == 1
    });

    std::fs::write(base.join("config/courier.rs"), "// mine\n").unwrap();
    app.artisan("vendor:publish --tag=courier-config")
        .expects_output_to_contain("File [config/courier.rs] already exists")
        .expects_output_to_contain("SKIPPED")
        .assert_successful()
        .await;
    assert_eq!(read(&base, "config/courier.rs"), "// mine\n");

    app.artisan("vendor:publish --tag=courier-config --force").assert_successful().await;
    assert_eq!(read(&base, "config/courier.rs"), "pub fn config() {}\n");

    app.artisan("vendor:publish --tag=nothing")
        .expects_output_to_contain("No publishable resources for tag [nothing].")
        .assert_successful()
        .await;
}

#[tokio::test]
async fn published_migrations_are_dated_when_they_are_published() {
    let (app, dir) = app();
    let migrations = dir.path().join("app/database/migrations");

    app.artisan("vendor:publish --tag=courier-migrations").assert_successful().await;
    let files: Vec<String> = std::fs::read_dir(&migrations)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("_create_couriers_table.rs"));
    assert!(!files[0].starts_with("2024_01_01_000000"), "{}", files[0]);

    // Publishing again finds the migration under its new date.
    app.artisan("vendor:publish --tag=courier-migrations")
        .expects_output_to_contain("already exists")
        .assert_successful()
        .await;
    assert_eq!(std::fs::read_dir(&migrations).unwrap().count(), 1);
}

#[tokio::test]
async fn package_files_are_published_by_provider_and_directory() {
    let (app, dir) = app();
    let base = dir.path().join("app");

    app.artisan("vendor:publish --provider=CourierServiceProvider --tag=courier-assets")
        .expects_output_to_contain("Publishing [courier-assets] assets.")
        .expects_output_to_contain("Copying file [mail.blade.html] to [resources/views/vendor/courier/mail.blade.html]")
        .expects_output_to_contain("Copying directory")
        .assert_successful()
        .await;
    assert_eq!(read(&base, "resources/views/vendor/courier/mail.blade.html"), "<p>Courier</p>\n");
    assert_eq!(read(&base, "public/vendor/courier/css/courier.css"), "body {}\n");
    assert_eq!(read(&base, "public/vendor/courier/courier.js"), "// js\n");
    assert!(!base.join("config/courier.rs").exists());

    // `--existing` only refreshes what was already published.
    std::fs::write(base.join("public/vendor/courier/courier.js"), "// old\n").unwrap();
    std::fs::remove_file(base.join("resources/views/vendor/courier/mail.blade.html")).unwrap();
    app.artisan("vendor:publish --provider=CourierServiceProvider --existing")
        .expects_output_to_contain("File [resources/views/vendor/courier/mail.blade.html] does not exist")
        .expects_output_to_contain("File [config/courier.rs] does not exist")
        .assert_successful()
        .await;
    assert_eq!(read(&base, "public/vendor/courier/courier.js"), "// js\n");
    assert!(!base.join("resources/views/vendor/courier/mail.blade.html").exists());

    app.artisan("vendor:publish --provider=MissingServiceProvider")
        .expects_output_to_contain("No publishable resources for tag [].")
        .assert_successful()
        .await;
}

#[tokio::test]
async fn everything_can_be_published_from_the_prompt() {
    let (app, dir) = app();
    let base = dir.path().join("app");

    app.artisan("vendor:publish")
        .expects_question("Which provider or tag's files would you like to publish?", "All providers and tags")
        .expects_output_to_contain("Publishing assets.")
        .assert_successful()
        .await;
    assert!(base.join("config/courier.rs").exists());
    assert!(base.join("public/vendor/courier/courier.js").exists());

    std::fs::remove_file(base.join("config/courier.rs")).unwrap();
    app.artisan("vendor:publish --all").assert_successful().await;
    assert!(base.join("config/courier.rs").exists());
}

#[tokio::test]
async fn the_framework_language_files_can_be_published() {
    let (app, dir) = app();
    let lang = dir.path().join("app/lang/en");

    app.artisan("lang:publish")
        .expects_output_to_contain("Language files published successfully.")
        .assert_successful()
        .await;
    for group in ["auth", "pagination", "passwords", "validation"] {
        let json: serde_json::Value = serde_json::from_str(&read(&lang, &format!("{group}.json"))).unwrap();
        assert!(json.is_object(), "{group}");
    }
    assert!(read(&lang, "auth.json").contains("These credentials do not match our records."));

    std::fs::write(lang.join("auth.json"), "{}").unwrap();
    std::fs::remove_file(lang.join("passwords.json")).unwrap();
    app.artisan("lang:publish --existing").assert_successful().await;
    assert!(read(&lang, "auth.json").contains("credentials"));
    assert!(!lang.join("passwords.json").exists());

    std::fs::write(lang.join("auth.json"), "{}").unwrap();
    app.artisan("lang:publish").assert_successful().await;
    assert_eq!(read(&lang, "auth.json"), "{}");
    assert!(lang.join("passwords.json").exists());
    app.artisan("lang:publish --force").assert_successful().await;
    assert!(read(&lang, "auth.json").contains("credentials"));
}

#[tokio::test]
async fn generators_prefer_the_applications_published_stubs() {
    let (app, dir) = app();
    let base = dir.path().join("app");

    app.artisan("stub:publish")
        .expects_output_to_contain("Stubs published successfully.")
        .assert_successful()
        .await;
    let stubs = base.join("stubs");
    for name in ["controller.plain.stub", "controller.stub", "model.stub", "migration.create.stub", "mail.stub", "view.stub"] {
        assert!(stubs.join(name).exists(), "{name}");
    }
    assert_eq!(
        std::fs::read_dir(&stubs).unwrap().count(),
        illuminate_foundation::console::generators::stubs::ALL.len()
    );
    assert!(read(&stubs, "controller.plain.stub").contains("pub struct {{ class }};"));

    std::fs::write(stubs.join("controller.plain.stub"), "// A custom controller: {{ class }}\n").unwrap();
    std::fs::write(stubs.join("migration.create.stub"), "// create {{ table }}\n").unwrap();
    app.artisan("make:controller PhotoController").assert_successful().await;
    assert_eq!(
        read(&base, "app/http/controllers/photo_controller.rs"),
        "// A custom controller: PhotoController\n"
    );
    app.artisan("make:migration create_flights_table").assert_successful().await;
    let migration = std::fs::read_dir(base.join("database/migrations"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with("_create_flights_table.rs"))
        .unwrap();
    assert_eq!(std::fs::read_to_string(migration).unwrap(), "// create flights\n");

    // Published stubs are only replaced when asked to.
    app.artisan("stub:publish").assert_successful().await;
    assert!(read(&stubs, "controller.plain.stub").starts_with("// A custom controller"));
    app.artisan("stub:publish --force").assert_successful().await;
    assert!(read(&stubs, "controller.plain.stub").contains("pub struct {{ class }};"));
}
