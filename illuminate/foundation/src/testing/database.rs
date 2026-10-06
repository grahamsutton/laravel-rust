//! Database testing helpers: `RefreshDatabase` and the `assertDatabase*`
//! assertions.

use illuminate_database::seeder::run_seeder;
use illuminate_database::{DatabaseManager, MigrateOptions, Migrator, Seeder, SeederRegistry};
use illuminate_support::{Value, json};

use super::TestApp;

impl TestApp {
    /// Run every migration against a fresh database — Laravel's
    /// `RefreshDatabase` trait. Tests use an in-memory SQLite database by
    /// default, so each test starts from a clean slate.
    pub async fn refresh_database(&mut self) -> &mut Self {
        let migrator = Migrator::from_app();
        let connection = migrator.get_repository().get_connection();
        connection
            .get_schema_builder()
            .drop_all_tables()
            .await
            .expect("Unable to drop the database tables");
        migrator
            .run(MigrateOptions::default())
            .await
            .expect("Unable to run the database migrations");
        self
    }

    /// Seed the database with the given seeder.
    pub async fn seed<S: Seeder + Default + 'static>(&mut self) -> &mut Self {
        run_seeder(&S::default(), None).await.expect("Unable to seed the database");
        self
    }

    /// Seed the database with a seeder registered under the given name
    /// (`"DatabaseSeeder"`).
    pub async fn seed_named(&mut self, seeder: &str) -> &mut Self {
        let registry = self.app().make::<SeederRegistry>();
        let seeder = registry
            .resolve(seeder)
            .unwrap_or_else(|| panic!("Target class [{seeder}] does not exist."));
        run_seeder(seeder.as_ref(), None).await.expect("Unable to seed the database");
        self
    }

    /// Assert that a table contains a row matching the given attributes.
    pub async fn assert_database_has(&self, table: &str, data: Value) -> &Self {
        let count = matching(table, &data).await;
        if count == 0 {
            panic!(
                "Failed asserting that a row in the table [{table}] matches the attributes {}.\n\n{}",
                pretty(&data),
                similar_results(table).await
            );
        }
        self
    }

    /// Assert that a table doesn't contain a row matching the given attributes.
    pub async fn assert_database_missing(&self, table: &str, data: Value) -> &Self {
        let count = matching(table, &data).await;
        if count > 0 {
            panic!(
                "Failed asserting that any existing row in the table [{table}] matches the attributes {}.\n\nFound: {count} matching row(s).",
                pretty(&data)
            );
        }
        self
    }

    /// Assert the number of rows in a table.
    pub async fn assert_database_count(&self, table: &str, expected: i64) -> &Self {
        let actual = connection()
            .table(table)
            .count()
            .await
            .unwrap_or_else(|error| panic!("Unable to count the rows of [{table}]: {error}"));
        assert_eq!(
            actual, expected,
            "Failed asserting that table [{table}] matches expected entries count of {expected}. Entries found: {actual}."
        );
        self
    }

    /// Assert that a table is empty.
    pub async fn assert_database_empty(&self, table: &str) -> &Self {
        self.assert_database_count(table, 0).await
    }
}

fn connection() -> illuminate_database::Connection {
    DatabaseManager::resolve().default_connection()
}

async fn matching(table: &str, data: &Value) -> i64 {
    let data = match data {
        Value::Object(_) => data.clone(),
        _ => json!({}),
    };
    connection()
        .table(table)
        .where_map(data)
        .count()
        .await
        .unwrap_or_else(|error| panic!("Unable to query the table [{table}]: {error}"))
}

async fn similar_results(table: &str) -> String {
    let rows = connection().table(table).limit(3).get().await.map(|rows| rows.all().to_vec());
    match rows {
        Ok(rows) if rows.is_empty() => "The table is empty.".to_string(),
        Ok(rows) => format!("Found similar results: {}", pretty(&Value::Array(rows))),
        Err(error) => format!("Unable to query the table: {error}"),
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}
