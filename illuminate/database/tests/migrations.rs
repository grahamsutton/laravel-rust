//! Running migrations up and down against a live SQLite database.

use std::sync::{Arc, Mutex};

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};
use illuminate_database::migrations::{
    MigrateOptions, Migration, MigrationEvent, MigrationMethod, MigrationRegistry, MigrationResult,
    Migrator, RollbackOptions,
};
use illuminate_database::{DB, DatabaseServiceProvider, Schema, async_trait, migrations};
use illuminate_support::{Result, error::RuntimeException, json};

struct CreateUsersTable;

#[async_trait]
impl Migration for CreateUsersTable {
    async fn up(&self) -> Result<()> {
        Schema::create("users", |table| {
            table.id();
            table.string("name");
            table.string("email").unique();
            table.timestamps();
        })
        .await
    }

    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("users").await
    }
}

struct CreatePostsTable;

#[async_trait]
impl Migration for CreatePostsTable {
    async fn up(&self) -> Result<()> {
        Schema::create("posts", |table| {
            table.id();
            table
                .foreign_id("user_id")
                .constrained()
                .cascade_on_delete();
            table.string("title");
        })
        .await
    }

    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("posts").await
    }
}

struct AddVotesToUsersTable;

#[async_trait]
impl Migration for AddVotesToUsersTable {
    async fn up(&self) -> Result<()> {
        Schema::table("users", |table| {
            table.integer("votes").default(0);
        })
        .await
    }

    async fn down(&self) -> Result<()> {
        Schema::table("users", |table| {
            table.drop_column("votes");
        })
        .await
    }
}

struct SkippedMigration;

#[async_trait]
impl Migration for SkippedMigration {
    async fn up(&self) -> Result<()> {
        Schema::create("never", |table| {
            table.id();
        })
        .await
    }

    fn should_run(&self) -> bool {
        false
    }
}

struct CreateLogsOnSecondary;

#[async_trait]
impl Migration for CreateLogsOnSecondary {
    async fn up(&self) -> Result<()> {
        // `Schema` targets the migration's connection while it runs.
        Schema::create("logs", |table| {
            table.text("message");
        })
        .await
    }

    async fn down(&self) -> Result<()> {
        Schema::drop("logs").await
    }

    fn connection(&self) -> Option<String> {
        Some("secondary".into())
    }
}

struct FailingMigration;

#[async_trait]
impl Migration for FailingMigration {
    async fn up(&self) -> Result<()> {
        Err(RuntimeException::new("Migration exploded.").into())
    }
}

fn app() -> (Arc<Container>, illuminate_container::LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({"database": {
        "default": "sqlite",
        "connections": {
            "sqlite": {"driver": "sqlite", "database": ":memory:", "foreign_key_constraints": true},
            "secondary": {"driver": "sqlite", "database": ":memory:"},
        },
        "migrations": {"table": "migrations", "update_date_on_publish": true},
    }})));
    DatabaseServiceProvider.register(&container);
    (container, guard)
}

fn migrator() -> Migrator {
    Migrator::resolve(migrations![
        "2024_01_01_000001_create_posts_table" => CreatePostsTable,
        "2024_01_01_000000_create_users_table" => CreateUsersTable,
        "2024_01_02_000000_add_votes_to_users_table" => AddVotesToUsersTable,
    ])
}

#[tokio::test]
async fn migrations_run_in_name_order_and_are_logged() {
    let (_container, _guard) = app();
    let migrator = migrator();
    assert!(!migrator.repository_exists().await.unwrap());

    let ran = migrator.run(MigrateOptions::default()).await.unwrap();
    assert_eq!(
        ran,
        vec![
            "2024_01_01_000000_create_users_table",
            "2024_01_01_000001_create_posts_table",
            "2024_01_02_000000_add_votes_to_users_table",
        ]
    );
    assert!(migrator.repository_exists().await.unwrap());
    assert!(Schema::has_table("posts").await.unwrap());
    assert!(Schema::has_column("users", "votes").await.unwrap());

    let records = DB::table("migrations")
        .order_by("id", "asc")
        .get()
        .await
        .unwrap();
    assert_eq!(records.count(), 3);
    assert_eq!(
        records[0]["migration"],
        json!("2024_01_01_000000_create_users_table")
    );
    assert!(records.iter().all(|r| r["batch"] == json!(1)));

    // Nothing left to run.
    assert!(
        migrator
            .run(MigrateOptions::default())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(migrator.has_run_any_migrations().await.unwrap());
}

#[tokio::test]
async fn new_migrations_run_in_a_new_batch_and_roll_back_by_batch() {
    let (container, _guard) = app();
    let first = Migrator::resolve(migrations![
        "2024_01_01_000000_create_users_table" => CreateUsersTable,
    ]);
    first.run(MigrateOptions::default()).await.unwrap();

    let all = migrator();
    let ran = all.run(MigrateOptions::default()).await.unwrap();
    assert_eq!(ran.len(), 2);
    let batches = all.get_repository().get_migration_batches().await.unwrap();
    assert_eq!(batches["2024_01_01_000000_create_users_table"], 1);
    assert_eq!(batches["2024_01_02_000000_add_votes_to_users_table"], 2);

    let rolled_back = all.rollback(RollbackOptions::default()).await.unwrap();
    assert_eq!(
        rolled_back,
        vec![
            "2024_01_02_000000_add_votes_to_users_table",
            "2024_01_01_000001_create_posts_table",
        ]
    );
    assert!(!Schema::has_table("posts").await.unwrap());
    assert!(!Schema::has_column("users", "votes").await.unwrap());
    assert!(Schema::has_table("users").await.unwrap());

    let status = all.status().await.unwrap();
    assert_eq!(status.iter().filter(|s| s.ran).count(), 1);
    assert_eq!(status[0].batch, Some(1));
    assert_eq!(status[1].batch, None);
    drop(container);
}

#[tokio::test]
async fn step_runs_each_migration_in_its_own_batch() {
    let (_container, _guard) = app();
    let migrator = migrator();
    migrator
        .run(MigrateOptions {
            step: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let batches = migrator
        .get_repository()
        .get_migration_batches()
        .await
        .unwrap();
    assert_eq!(batches.values().copied().collect::<Vec<_>>(), vec![1, 2, 3]);

    let rolled_back = migrator
        .rollback(RollbackOptions {
            step: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(rolled_back.len(), 2);
    assert!(Schema::has_table("users").await.unwrap());
    assert!(!Schema::has_table("posts").await.unwrap());

    let rolled_back = migrator
        .rollback(RollbackOptions {
            batch: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(rolled_back, vec!["2024_01_01_000000_create_users_table"]);
    assert!(!Schema::has_table("users").await.unwrap());
    assert!(
        migrator
            .rollback(RollbackOptions::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn reset_refresh_and_fresh() {
    let (_container, _guard) = app();
    let migrator = migrator();
    migrator.run(MigrateOptions::default()).await.unwrap();
    DB::table("users")
        .insert(json!({"name": "Taylor", "email": "t@laravel.com"}))
        .await
        .unwrap();

    let reset = migrator.reset(false).await.unwrap();
    assert_eq!(
        reset,
        vec![
            "2024_01_02_000000_add_votes_to_users_table",
            "2024_01_01_000001_create_posts_table",
            "2024_01_01_000000_create_users_table",
        ]
    );
    assert!(!Schema::has_table("users").await.unwrap());
    assert!(Schema::has_table("migrations").await.unwrap());
    assert_eq!(DB::table("migrations").count().await.unwrap(), 0);
    assert!(migrator.reset(false).await.unwrap().is_empty());

    migrator.run(MigrateOptions::default()).await.unwrap();
    DB::table("users")
        .insert(json!({"name": "Taylor", "email": "t@laravel.com"}))
        .await
        .unwrap();
    let ran = migrator.refresh(None).await.unwrap();
    assert_eq!(ran.len(), 3);
    assert_eq!(
        DB::table("users").count().await.unwrap(),
        0,
        "refresh rebuilds the tables"
    );

    DB::table("users")
        .insert(json!({"name": "Taylor", "email": "t@laravel.com"}))
        .await
        .unwrap();
    DB::statement("create table stray (id integer)", ())
        .await
        .unwrap();
    let ran = migrator.fresh(MigrateOptions::default()).await.unwrap();
    assert_eq!(ran.len(), 3);
    assert!(!Schema::has_table("stray").await.unwrap());
    assert_eq!(DB::table("users").count().await.unwrap(), 0);
    assert_eq!(
        DB::table("migrations").max("batch").await.unwrap(),
        json!(1)
    );

    let refreshed = migrator.refresh(Some(1)).await.unwrap();
    assert_eq!(
        refreshed,
        vec!["2024_01_02_000000_add_votes_to_users_table"]
    );
}

#[tokio::test]
async fn pretending_dumps_the_queries() {
    let (_container, _guard) = app();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let migrator =
        migrator().with_output(move |event| captured.lock().unwrap().push(event.clone()));

    let ran = migrator
        .run(MigrateOptions {
            pretend: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(ran.len(), 3);
    assert!(!Schema::has_table("users").await.unwrap());
    assert_eq!(DB::table("migrations").count().await.unwrap(), 0);

    let events = events.lock().unwrap();
    let pretended: Vec<&MigrationEvent> = events
        .iter()
        .filter(|e| matches!(e, MigrationEvent::Pretended { .. }))
        .collect();
    assert_eq!(pretended.len(), 3);
    match pretended[0] {
        MigrationEvent::Pretended {
            name,
            queries,
            method,
        } => {
            assert_eq!(name, "2024_01_01_000000_create_users_table");
            assert_eq!(*method, MigrationMethod::Up);
            assert_eq!(
                queries,
                &vec![
                    "create table \"users\" (\"id\" integer primary key autoincrement not null, \"name\" varchar not null, \"email\" varchar not null, \"created_at\" datetime, \"updated_at\" datetime)".to_string(),
                    "create unique index \"users_email_unique\" on \"users\" (\"email\")".to_string(),
                ]
            );
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn output_events_report_progress() {
    let (_container, _guard) = app();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let migrator = Migrator::resolve(migrations![
        "2024_01_01_000000_create_users_table" => CreateUsersTable,
        "2024_01_01_000001_skipped" => SkippedMigration,
    ])
    .with_output(move |event| captured.lock().unwrap().push(event.clone()));

    migrator.run(MigrateOptions::default()).await.unwrap();
    migrator.run(MigrateOptions::default()).await.unwrap();
    assert!(!Schema::has_table("never").await.unwrap());

    let events = events.lock().unwrap().clone();
    assert_eq!(
        events[0],
        MigrationEvent::Info("Running migrations.".into())
    );
    assert!(
        matches!(&events[1], MigrationEvent::Started { name, method: MigrationMethod::Up } if name == "2024_01_01_000000_create_users_table")
    );
    match &events[2] {
        MigrationEvent::Finished {
            name,
            result,
            duration_ms,
            ..
        } => {
            assert_eq!(name, "2024_01_01_000000_create_users_table");
            assert_eq!(*result, MigrationResult::Success);
            assert!(*duration_ms >= 0.0);
            let line = events[2].render(100);
            assert!(line.starts_with("  2024_01_01_000000_create_users_table ."));
            assert!(line.ends_with("DONE"));
        }
        other => panic!("unexpected event {other:?}"),
    }
    assert!(matches!(
        &events[3],
        MigrationEvent::Finished {
            result: MigrationResult::Skipped,
            ..
        }
    ));
    // The second run: the skipped migration is still pending, but skipped again.
    assert_eq!(
        events[4],
        MigrationEvent::Info("Running migrations.".into())
    );
}

#[tokio::test]
async fn failures_stop_the_run_and_are_reported() {
    let (_container, _guard) = app();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let migrator = Migrator::resolve(migrations![
        "2024_01_01_000000_create_users_table" => CreateUsersTable,
        "2024_01_01_000001_failing" => FailingMigration,
        "2024_01_01_000002_create_posts_table" => CreatePostsTable,
    ])
    .with_output(move |event| captured.lock().unwrap().push(event.clone()));

    let error = migrator.run(MigrateOptions::default()).await.unwrap_err();
    assert_eq!(error.to_string(), "Migration exploded.");
    assert!(Schema::has_table("users").await.unwrap());
    assert!(!Schema::has_table("posts").await.unwrap());
    assert_eq!(
        migrator.get_repository().get_ran().await.unwrap(),
        vec!["2024_01_01_000000_create_users_table"]
    );
    assert!(events.lock().unwrap().iter().any(|e| matches!(
        e,
        MigrationEvent::Finished {
            result: MigrationResult::Failure,
            ..
        }
    )));
}

#[tokio::test]
async fn migrations_can_target_another_connection() {
    let (_container, _guard) = app();
    let migrator = Migrator::resolve(migrations![
        "2024_01_01_000000_create_logs_table" => CreateLogsOnSecondary,
    ]);
    migrator.run(MigrateOptions::default()).await.unwrap();
    assert!(
        Schema::connection("secondary")
            .has_table("logs")
            .await
            .unwrap()
    );
    assert!(!Schema::has_table("logs").await.unwrap());
    // The repository lives on the migrator's (default) connection.
    assert!(Schema::has_table("migrations").await.unwrap());

    migrator.reset(false).await.unwrap();
    assert!(
        !Schema::connection("secondary")
            .has_table("logs")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn the_migrator_can_use_a_non_default_connection() {
    let (_container, _guard) = app();
    let migrator = migrator().with_connection(Some("secondary"));
    migrator.run(MigrateOptions::default()).await.unwrap();
    assert!(
        Schema::connection("secondary")
            .has_table("users")
            .await
            .unwrap()
    );
    assert!(
        Schema::connection("secondary")
            .has_table("migrations")
            .await
            .unwrap()
    );
    assert!(!Schema::has_table("users").await.unwrap());
}

#[tokio::test]
async fn migrations_can_be_registered_with_the_container() {
    let (container, _guard) = app();
    let registry = container.make::<MigrationRegistry>();
    registry.register("2024_01_01_000000_create_users_table", CreateUsersTable);
    registry.extend(migrations!["2024_01_01_000001_create_posts_table" => CreatePostsTable]);
    assert_eq!(
        registry.names(),
        vec![
            "2024_01_01_000000_create_users_table",
            "2024_01_01_000001_create_posts_table"
        ]
    );

    let migrator = Migrator::from_app();
    assert_eq!(migrator.migration_names().len(), 2);
    let status = migrator.status().await.unwrap();
    assert!(status.iter().all(|s| !s.ran));
    migrator.install().await.unwrap();
    assert!(migrator.repository_exists().await.unwrap());
    migrator.run(MigrateOptions::default()).await.unwrap();
    assert!(
        migrator
            .status()
            .await
            .unwrap()
            .iter()
            .all(|s| s.ran && s.batch == Some(1))
    );
}

#[test]
fn task_lines_render_like_laravel() {
    let line = illuminate_database::migrations::render_task(
        "2024_01_01_000000_create_users_table",
        Some(3.21),
        "DONE",
        80,
    );
    assert_eq!(line.len(), 2 + 38 + 1 + (80 - 38 - 7 - 10) + 7 + 5);
    assert!(line.ends_with(" 3.21ms DONE"));
    assert_eq!(
        illuminate_database::migrations::format_run_time(1520.0),
        "1.52s"
    );
}
