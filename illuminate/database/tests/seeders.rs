//! Database seeders.

use std::sync::{Arc, Mutex};

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};
use illuminate_database::seeder::{
    Seeder, SeederEvent, SeederRegistry, call, call_many, call_once, call_silent, run_seeder,
};
use illuminate_database::{DB, DatabaseServiceProvider, Schema, async_trait};
use illuminate_support::{Result, json};

#[derive(Default)]
struct UserSeeder;

#[async_trait]
impl Seeder for UserSeeder {
    async fn run(&self) -> Result<()> {
        DB::table("users")
            .insert(json!([
                {"name": "Taylor", "email": "taylor@laravel.com"},
                {"name": "Abigail", "email": "abigail@laravel.com"},
            ]))
            .await?;
        Ok(())
    }
}

#[derive(Default)]
struct PostSeeder;

#[async_trait]
impl Seeder for PostSeeder {
    async fn run(&self) -> Result<()> {
        let user = DB::table("users")
            .where_("name", "Taylor")
            .value_as::<i64>("id")
            .await?;
        DB::table("posts")
            .insert(json!({"user_id": user, "title": "Hello"}))
            .await?;
        // A seeder may depend on another one only running once.
        call_once::<UserSeeder>().await
    }
}

#[derive(Default)]
struct DatabaseSeeder;

#[async_trait]
impl Seeder for DatabaseSeeder {
    async fn run(&self) -> Result<()> {
        call::<UserSeeder>().await?;
        call::<PostSeeder>().await?;
        call_silent::<QuietSeeder>().await
    }
}

#[derive(Default)]
struct QuietSeeder;

#[async_trait]
impl Seeder for QuietSeeder {
    async fn run(&self) -> Result<()> {
        DB::table("posts")
            .insert(json!({"user_id": 1, "title": "Quiet"}))
            .await?;
        Ok(())
    }
}

async fn app() -> (Arc<Container>, illuminate_container::LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({"database": {
        "default": "sqlite",
        "connections": {"sqlite": {"driver": "sqlite", "database": ":memory:"}},
    }})));
    DatabaseServiceProvider.register(&container);
    Schema::create("users", |table| {
        table.id();
        table.string("name");
        table.string("email");
    })
    .await
    .unwrap();
    Schema::create("posts", |table| {
        table.id();
        table.foreign_id("user_id");
        table.string("title");
    })
    .await
    .unwrap();
    (container, guard)
}

#[tokio::test]
async fn seeders_call_other_seeders_and_report_progress() {
    let (_container, _guard) = app().await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();

    run_seeder(
        &DatabaseSeeder,
        Some(Arc::new(move |event: &SeederEvent| {
            captured.lock().unwrap().push(event.clone())
        })),
    )
    .await
    .unwrap();

    assert_eq!(
        DB::table("users").count().await.unwrap(),
        2,
        "call_once skipped the second run"
    );
    assert_eq!(DB::table("posts").count().await.unwrap(), 2);

    let events = events.lock().unwrap();
    let names: Vec<String> = events
        .iter()
        .map(|e| match e {
            SeederEvent::Running { name } => format!("RUNNING {name}"),
            SeederEvent::Done { name, .. } => format!("DONE {name}"),
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "RUNNING UserSeeder",
            "DONE UserSeeder",
            "RUNNING PostSeeder",
            "DONE PostSeeder"
        ]
    );
}

#[tokio::test]
async fn seeders_can_be_called_directly() {
    let (_container, _guard) = app().await;
    call::<UserSeeder>().await.unwrap();
    call_many(vec![Box::new(UserSeeder), Box::new(PostSeeder)])
        .await
        .unwrap();
    // Without a seeding context, call_once always runs.
    assert_eq!(DB::table("users").count().await.unwrap(), 6);
    assert_eq!(UserSeeder.name(), "UserSeeder");
}

#[tokio::test]
async fn the_registry_resolves_seeders_by_name() {
    let (container, _guard) = app().await;
    let registry = container.make::<SeederRegistry>();
    registry.register::<DatabaseSeeder>();
    registry.register::<UserSeeder>();

    assert!(registry.has("DatabaseSeeder"));
    assert!(registry.has("Database\\Seeders\\UserSeeder"));
    assert!(!registry.has("MissingSeeder"));
    assert_eq!(registry.names(), vec!["DatabaseSeeder", "UserSeeder"]);

    let seeder = registry.resolve("UserSeeder").unwrap();
    run_seeder(seeder.as_ref(), None).await.unwrap();
    assert_eq!(DB::table("users").count().await.unwrap(), 2);
}
