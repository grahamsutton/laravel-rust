use illuminate_database::migrations::Migration;
use illuminate_database::seeder::{Seeder, call};
use illuminate_database::{DB, Schema, async_trait, migrations};
use illuminate_foundation::Application;
use illuminate_foundation::testing::TestApp;
use illuminate_support::{Result, json};

struct CreateUsersTable;

#[async_trait]
impl Migration for CreateUsersTable {
    async fn up(&self) -> Result<()> {
        Schema::create("users", |table| {
            table.id();
            table.string("name");
            table.string("email");
            table.timestamps();
        })
        .await
    }

    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("users").await
    }
}

struct CreateFlightsTable;

#[async_trait]
impl Migration for CreateFlightsTable {
    async fn up(&self) -> Result<()> {
        Schema::create("flights", |table| {
            table.id();
            table.string("airline");
        })
        .await
    }

    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("flights").await
    }
}

#[derive(Default)]
struct UserSeeder;

#[async_trait]
impl Seeder for UserSeeder {
    async fn run(&self) -> Result<()> {
        DB::table("users")
            .insert(json!({"name": "Taylor", "email": "taylor@laravel.com"}))
            .await?;
        Ok(())
    }
}

#[derive(Default)]
struct DatabaseSeeder;

#[async_trait]
impl Seeder for DatabaseSeeder {
    async fn run(&self) -> Result<()> {
        call::<UserSeeder>().await
    }
}

fn test_app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let builder = Application::configure_detached(dir.path())
        .with_migrations(migrations![
            "0001_01_01_000000_create_users_table" => CreateUsersTable,
            "2024_06_01_000000_create_flights_table" => CreateFlightsTable,
        ])
        .with_seeders(|seeders| {
            seeders.register::<DatabaseSeeder>();
            seeders.register::<UserSeeder>();
        });
    (TestApp::new(builder), dir)
}

#[tokio::test]
async fn migrations_can_be_run_and_rolled_back() {
    let (app, _dir) = test_app();

    app.artisan("migrate")
        .expects_output_to_contain("Preparing database.")
        .expects_output_to_contain("Creating migration table")
        .expects_output_to_contain("Running migrations.")
        .expects_output_to_contain("0001_01_01_000000_create_users_table")
        .expects_output_to_contain("2024_06_01_000000_create_flights_table")
        .assert_successful()
        .await;
    assert!(Schema::has_table("users").await.unwrap());
    assert!(Schema::has_table("flights").await.unwrap());

    app.artisan("migrate")
        .expects_output_to_contain("Nothing to migrate.")
        .assert_successful()
        .await;

    app.artisan("migrate:status")
        .expects_output_to_contain("0001_01_01_000000_create_users_table")
        .expects_output_to_contain("[1] Ran")
        .assert_successful()
        .await;

    app.artisan("migrate:rollback")
        .expects_output_to_contain("Rolling back migrations.")
        .assert_successful()
        .await;
    assert!(!Schema::has_table("users").await.unwrap());

    app.artisan("migrate:status --pending")
        .expects_output_to_contain("Pending")
        .assert_successful()
        .await;

    app.artisan("migrate --step").assert_successful().await;
    app.artisan("migrate:rollback --step=1").assert_successful().await;
    assert!(Schema::has_table("users").await.unwrap());
    assert!(!Schema::has_table("flights").await.unwrap());
}

#[tokio::test]
async fn the_database_can_be_seeded() {
    let (app, _dir) = test_app();

    app.artisan("migrate --seed")
        .expects_output_to_contain("Seeding database.")
        .expects_output_to_contain("UserSeeder")
        .assert_successful()
        .await;
    app.assert_database_has("users", json!({"email": "taylor@laravel.com"}))
        .await
        .assert_database_count("users", 1)
        .await;

    app.artisan("db:seed UserSeeder").assert_successful().await;
    app.assert_database_count("users", 2).await;

    app.artisan("db:seed MissingSeeder")
        .expects_output_to_contain("Target class [MissingSeeder] does not exist.")
        .assert_failed()
        .await;

    app.artisan("migrate:fresh")
        .expects_output_to_contain("Dropping all tables")
        .assert_successful()
        .await;
    app.assert_database_empty("users").await;

    app.artisan("db:wipe")
        .expects_output_to_contain("Dropped all tables successfully.")
        .assert_successful()
        .await;
    assert!(!Schema::has_table("users").await.unwrap());
}

#[tokio::test]
async fn tests_can_refresh_the_database() {
    let (mut app, _dir) = test_app();

    app.refresh_database().await.seed::<DatabaseSeeder>().await;

    app.assert_database_has("users", json!({"name": "Taylor"}))
        .await
        .assert_database_missing("users", json!({"name": "Abigail"}))
        .await;
}

#[tokio::test]
#[should_panic(expected = "Failed asserting that a row in the table [users] matches the attributes")]
async fn database_assertions_fail_loudly() {
    let (mut app, _dir) = test_app();

    app.refresh_database().await;
    app.assert_database_has("users", json!({"name": "Nobody"})).await;
}

#[tokio::test]
async fn migrations_and_seeders_can_be_generated() {
    let (app, dir) = test_app();

    app.artisan("make:migration create_flights_table")
        .expects_output_to_contain("Migration [database/migrations/")
        .assert_successful()
        .await;
    app.artisan("make:migration AddVotesToUsersTable").assert_successful().await;
    app.artisan("make:seeder UserSeeder")
        .expects_output_to_contain("Seeder [database/seeders/user_seeder.rs] created successfully.")
        .assert_successful()
        .await;

    let migration = |suffix: &str| {
        std::fs::read_dir(dir.path().join("database/migrations"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .find(|entry| entry.file_name().to_string_lossy().ends_with(suffix))
            .map(|entry| std::fs::read_to_string(entry.path()).unwrap())
            .unwrap_or_else(|| panic!("No migration ending in {suffix}"))
    };

    let create = migration("_create_flights_table.rs");
    assert!(create.contains("pub struct CreateFlightsTable;"));
    assert!(create.contains(r#"Schema::create("flights", |table| {"#));
    assert!(create.contains(r#"Schema::drop_if_exists("flights")"#));

    let update = migration("_add_votes_to_users_table.rs");
    assert!(update.contains("pub struct AddVotesToUsersTable;"));
    assert!(update.contains(r#"Schema::table("users", |table| {"#));

    let seeder = std::fs::read_to_string(dir.path().join("database/seeders/user_seeder.rs")).unwrap();
    assert!(seeder.contains("impl Seeder for UserSeeder"));
}

#[tokio::test]
async fn databases_and_tables_can_be_inspected() {
    let (app, _dir) = test_app();
    app.artisan("migrate").assert_successful().await;
    DB::table("users")
        .insert(json!({"name": "Taylor", "email": "taylor@laravel.com"}))
        .await
        .unwrap();

    app.artisan("db:show --counts")
        .expects_output_to_contain("SQLite")
        .expects_output_to_contain("Connection")
        .expects_output_to_contain("Tables")
        .expects_output_to_contain("flights")
        .expects_output_to_contain("users")
        .expects_output_to_contain("migrations")
        .assert_successful()
        .await;

    app.artisan("db:table users")
        .expects_output_to_contain("Columns")
        .expects_output_to_contain("email")
        .expects_output_to_contain("autoincrement")
        .expects_output_to_contain("Index")
        .assert_successful()
        .await;

    app.artisan("db:table missing")
        .expects_output_to_contain("Table [missing] doesn't exist.")
        .assert_failed()
        .await;

    let output = app.artisan("db:table users --json").run().await.output;
    let data: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(data["table"]["name"], "users");
    assert_eq!(data["table"]["columns"], 5);
    assert_eq!(data["columns"][1]["column"], "name");
}
