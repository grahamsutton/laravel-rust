//! `schema:dump`, loading the dump in `migrate`, and `db:monitor`.

use std::path::Path;

use illuminate_console::Artisan;
use illuminate_database::migrations::{Migration, MigrationsPruned};
use illuminate_database::schema::{SchemaDumped, SchemaLoaded};
use illuminate_database::{DB, Schema, async_trait, migrations};
use illuminate_events::Event;
use illuminate_foundation::Application;
use illuminate_foundation::console::commands::{DatabaseBusy, DbMonitorCommand};
use illuminate_foundation::testing::TestApp;
use illuminate_support::{Result, json};

struct CreateUsersTable;

#[async_trait]
impl Migration for CreateUsersTable {
    async fn up(&self) -> Result<()> {
        Schema::create("users", |table| {
            table.id();
            table.string("email").unique();
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
            table.foreign_id("user_id").constrained();
            table.string("airline");
        })
        .await
    }

    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("flights").await
    }
}

struct AddGateToFlightsTable;

#[async_trait]
impl Migration for AddGateToFlightsTable {
    async fn up(&self) -> Result<()> {
        Schema::table("flights", |table| {
            table.string("gate").nullable();
        })
        .await
    }

    async fn down(&self) -> Result<()> {
        Schema::drop_columns("flights", "gate").await
    }
}

/// An application on a SQLite database file within `base`.
fn app(base: &Path, database: &str, all_migrations: bool) -> TestApp {
    std::fs::create_dir_all(base.join("database")).unwrap();
    let database = base.join("database").join(database);
    if !database.exists() {
        std::fs::write(&database, "").unwrap();
    }
    let builder = Application::configure_detached(base).with_migrations(if all_migrations {
        migrations![
            "0001_01_01_000000_create_users_table" => CreateUsersTable,
            "2024_06_01_000000_create_flights_table" => CreateFlightsTable,
        ]
    } else {
        // The application after `schema:dump --prune` and a rebuild, with a
        // migration created since.
        migrations!["2025_01_01_000000_add_gate_to_flights_table" => AddGateToFlightsTable]
    });
    let app = TestApp::new(builder);
    app.app()
        .override_config("database.connections.sqlite.database", database.to_string_lossy().into_owned());
    app
}

async fn migrations_ran() -> Vec<String> {
    DB::table("migrations")
        .order_by("id", "asc")
        .pluck("migration")
        .await
        .unwrap()
        .all()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn the_schema_is_dumped_with_the_migrations_that_ran() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "database.sqlite", true);
    Event::fake_only::<SchemaDumped>();

    app.artisan("migrate").assert_successful().await;
    app.artisan("schema:dump")
        .expects_output_to_contain("Database schema dumped successfully.")
        .assert_successful()
        .await;

    let schema = std::fs::read_to_string(dir.path().join("database/schema/sqlite-schema.sql")).unwrap();
    assert!(schema.contains("CREATE TABLE \"users\""), "{schema}");
    assert!(schema.contains("CREATE UNIQUE INDEX \"users_email_unique\""), "{schema}");
    assert!(schema.contains("CREATE TABLE \"flights\""), "{schema}");
    assert!(schema.contains("INSERT INTO \"migrations\" VALUES(1,'0001_01_01_000000_create_users_table',1);"), "{schema}");
    assert!(schema.contains("INSERT INTO \"migrations\" VALUES(2,'2024_06_01_000000_create_flights_table',1);"), "{schema}");
    Event::assert_dispatched_with::<SchemaDumped>(|event| {
        event.connection_name == "sqlite" && event.path.ends_with("database/schema/sqlite-schema.sql")
    });

    let path = dir.path().join("custom/schema.sql");
    app.artisan(&format!("schema:dump --path={} --without-migration-data", path.display()))
        .assert_successful()
        .await;
    let custom = std::fs::read_to_string(&path).unwrap();
    assert!(custom.contains("CREATE TABLE \"migrations\""));
    assert!(!custom.contains("INSERT INTO"));
}

#[tokio::test]
async fn migrations_can_be_pruned_after_dumping() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "database.sqlite", true);
    let migrations = dir.path().join("database/migrations");
    std::fs::create_dir_all(&migrations).unwrap();
    std::fs::write(migrations.join("mod.rs"), "laravel::discover_migrations!();\n").unwrap();
    std::fs::write(migrations.join("0001_01_01_000000_create_users_table.rs"), "").unwrap();
    std::fs::write(migrations.join("2024_06_01_000000_create_flights_table.rs"), "").unwrap();
    Event::fake_only::<MigrationsPruned>();

    app.artisan("migrate").assert_successful().await;
    app.artisan("schema:dump --prune")
        .expects_output_to_contain("Database schema dumped and pruned successfully.")
        .assert_successful()
        .await;

    let remaining: Vec<String> = std::fs::read_dir(&migrations)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(remaining, ["mod.rs"]);
    Event::assert_dispatched_with::<MigrationsPruned>(|event| event.path == migrations);
}

#[tokio::test]
async fn a_fresh_database_is_built_from_the_schema_dump() {
    let dir = tempfile::tempdir().unwrap();
    {
        let app = app(dir.path(), "database.sqlite", true);
        app.artisan("migrate").assert_successful().await;
        app.artisan("schema:dump --prune").assert_successful().await;
    }

    let app = app(dir.path(), "fresh.sqlite", false);
    Event::fake_only::<SchemaLoaded>();

    // `--pretend` never touches the database.
    app.artisan("migrate --pretend")
        .doesnt_expect_output_to_contain("Loading stored database schemas.")
        .assert_successful()
        .await;

    app.artisan("migrate")
        .expects_output_to_contain("Loading stored database schemas.")
        .expects_output_to_contain("database/schema/sqlite-schema.sql")
        .expects_output_to_contain("2025_01_01_000000_add_gate_to_flights_table")
        .assert_successful()
        .await;
    Event::assert_dispatched_with::<SchemaLoaded>(|event| event.connection_name == "sqlite");

    assert!(Schema::has_table("users").await.unwrap());
    assert!(Schema::has_column("flights", "gate").await.unwrap());
    assert_eq!(
        migrations_ran().await,
        [
            "0001_01_01_000000_create_users_table",
            "2024_06_01_000000_create_flights_table",
            "2025_01_01_000000_add_gate_to_flights_table",
        ]
    );
    let batches: Vec<i64> = DB::table("migrations")
        .order_by("id", "asc")
        .pluck("batch")
        .await
        .unwrap()
        .all()
        .iter()
        .map(|value| value.as_i64().unwrap())
        .collect();
    assert_eq!(batches, [1, 1, 2]);

    // Once migrations have run, the dump is left alone.
    app.artisan("migrate")
        .doesnt_expect_output_to_contain("Loading stored database schemas.")
        .expects_output_to_contain("Nothing to migrate.")
        .assert_successful()
        .await;

    // `migrate:fresh` starts over from the dump.
    DB::table("users").insert(json!({"email": "taylor@laravel.com"})).await.unwrap();
    app.artisan("migrate:fresh")
        .expects_output_to_contain("Loading stored database schemas.")
        .assert_successful()
        .await;
    assert_eq!(DB::table("users").count().await.unwrap(), 0);
    assert_eq!(migrations_ran().await.len(), 3);
}

#[tokio::test]
async fn a_schema_file_can_be_given() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("schema.sql"),
        "CREATE TABLE \"migrations\" (\"id\" integer primary key autoincrement not null, \"migration\" varchar not null, \"batch\" integer not null);\nCREATE TABLE \"flights\" (\"id\" integer primary key autoincrement not null);\n",
    )
    .unwrap();
    let app = app(dir.path(), "database.sqlite", false);
    let path = dir.path().join("schema.sql");

    app.artisan(&format!("migrate --schema-path={}", path.display()))
        .expects_output_to_contain("Loading stored database schemas.")
        .assert_successful()
        .await;
    assert!(Schema::has_column("flights", "gate").await.unwrap());
    assert_eq!(migrations_ran().await, ["2025_01_01_000000_add_gate_to_flights_table"]);
}

#[tokio::test]
async fn database_connections_are_monitored() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), "database.sqlite", true);

    // SQLite has no server to count connections on.
    app.artisan("db:monitor")
        .expects_output_to_contain("Database name")
        .expects_output_to_contain("sqlite")
        .expects_output_to_contain("[] OK")
        .assert_successful()
        .await;

    app.app().bootstrap_console();
    Artisan::register(DbMonitorCommand::count_connections_using(|connection| async move {
        Ok(Some(if connection.get_name() == "sqlite" { 120 } else { 3 }))
    }));
    Event::fake_only::<DatabaseBusy>();

    app.artisan("db:monitor --databases=sqlite,replica --max=100")
        .expects_output_to_contain("[120] ALERT")
        .expects_output_to_contain("[3] OK")
        .assert_successful()
        .await;
    Event::assert_dispatched_times::<DatabaseBusy>(1);
    Event::assert_dispatched_with::<DatabaseBusy>(|event| {
        event.connection_name == "sqlite" && event.connections == 120
    });

    // Without `--max`, nothing is dispatched.
    app.artisan("db:monitor --databases=sqlite").assert_successful().await;
    Event::assert_dispatched_times::<DatabaseBusy>(1);
}

#[tokio::test]
async fn refresh_database_builds_the_database_from_the_schema_dump() {
    let dir = tempfile::tempdir().unwrap();
    {
        let app = app(dir.path(), "database.sqlite", true);
        app.artisan("migrate").assert_successful().await;
        app.artisan("schema:dump --prune").assert_successful().await;
    }

    let mut app = app(dir.path(), "refreshed.sqlite", false);
    Event::fake_only::<SchemaLoaded>();

    app.refresh_database().await;

    Event::assert_dispatched_with::<SchemaLoaded>(|event| event.connection_name == "sqlite");
    assert!(Schema::has_table("users").await.unwrap());
    assert!(Schema::has_column("flights", "gate").await.unwrap());
    assert_eq!(
        migrations_ran().await,
        [
            "0001_01_01_000000_create_users_table",
            "2024_06_01_000000_create_flights_table",
            "2025_01_01_000000_add_gate_to_flights_table",
        ]
    );
}
