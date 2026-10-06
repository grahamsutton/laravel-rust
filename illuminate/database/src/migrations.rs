//! Migrations: version control for your database.
//!
//! ```no_run
//! use illuminate_database::migrations::{Migration, Migrator};
//! use illuminate_database::{migrations, Schema};
//! use illuminate_support::Result;
//!
//! struct CreateFlightsTable;
//!
//! #[async_trait::async_trait]
//! impl Migration for CreateFlightsTable {
//!     async fn up(&self) -> Result<()> {
//!         Schema::create("flights", |table| {
//!             table.id();
//!             table.string("name");
//!             table.string("airline");
//!             table.timestamps();
//!         })
//!         .await
//!     }
//!
//!     async fn down(&self) -> Result<()> {
//!         Schema::drop("flights").await
//!     }
//! }
//!
//! # async fn example() -> Result<()> {
//! let migrator = Migrator::resolve(migrations![
//!     "2024_01_01_000000_create_flights_table" => CreateFlightsTable,
//! ]);
//! let ran = migrator.run(Default::default()).await?;
//! # Ok(()) }
//! ```

use std::sync::{Arc, RwLock};
use std::time::Instant;

use async_trait::async_trait;
use illuminate_container::try_app;
use illuminate_support::{Result, Value, ValueExt, json};
use indexmap::IndexMap;

use crate::connection::Connection;
use crate::manager::DatabaseManager;
use crate::query::Builder;

/// A database migration.
#[async_trait]
pub trait Migration: Send + Sync {
    /// Run the migrations.
    async fn up(&self) -> Result<()>;

    /// Reverse the migrations.
    async fn down(&self) -> Result<()> {
        Ok(())
    }

    /// The database connection the migration should use (`None` for the default).
    fn connection(&self) -> Option<String> {
        None
    }

    /// Whether the migration runs inside a transaction (on drivers that
    /// support transactional DDL, i.e. PostgreSQL).
    fn within_transaction(&self) -> bool {
        true
    }

    /// Determine if this migration should run.
    fn should_run(&self) -> bool {
        true
    }
}

/// Build a migration registry from `"name" => migration` pairs.
///
/// ```
/// use illuminate_database::migrations;
/// use illuminate_database::migrations::Migration;
///
/// struct CreateUsersTable;
///
/// #[async_trait::async_trait]
/// impl Migration for CreateUsersTable {
///     async fn up(&self) -> illuminate_support::Result<()> {
///         Ok(())
///     }
/// }
///
/// let registry = migrations![
///     "0001_01_01_000000_create_users_table" => CreateUsersTable,
/// ];
/// assert_eq!(registry.len(), 1);
/// assert_eq!(registry[0].0, "0001_01_01_000000_create_users_table");
/// ```
#[macro_export]
macro_rules! migrations {
    ($($name:expr => $migration:expr),* $(,)?) => {
        ::std::vec![$(
            (
                ::std::string::ToString::to_string(&$name),
                ::std::boxed::Box::new($migration) as ::std::boxed::Box<dyn $crate::migrations::Migration>,
            )
        ),*]
    };
}

/// A shared migration.
pub type MigrationRef = Arc<dyn Migration>;

/// The application's migrations, registered with the container so the
/// `migrate` commands can find them.
#[derive(Default)]
pub struct MigrationRegistry {
    migrations: RwLock<Vec<(String, MigrationRef)>>,
}

impl MigrationRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a migration under the given name.
    pub fn register(&self, name: impl Into<String>, migration: impl Migration + 'static) {
        self.add(name.into(), Arc::new(migration));
    }

    fn add(&self, name: String, migration: MigrationRef) {
        let mut migrations = self.migrations.write().unwrap();
        migrations.retain(|(existing, _)| *existing != name);
        migrations.push((name, migration));
        migrations.sort_by(|a, b| a.0.cmp(&b.0));
    }

    /// Register many migrations (the output of [`migrations!`]).
    pub fn extend(&self, migrations: impl IntoIterator<Item = (String, Box<dyn Migration>)>) {
        for (name, migration) in migrations {
            self.add(name, Arc::from(migration));
        }
    }

    /// All registered migrations, sorted by name.
    pub fn all(&self) -> Vec<(String, MigrationRef)> {
        self.migrations.read().unwrap().clone()
    }

    /// The names of the registered migrations.
    pub fn names(&self) -> Vec<String> {
        self.all().into_iter().map(|(name, _)| name).collect()
    }
}

/// Whether a migration is being run or reversed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationMethod {
    /// `up`
    Up,
    /// `down`
    Down,
}

/// The outcome of running a single migration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationResult {
    /// The migration ran (`DONE`).
    Success,
    /// The migration failed (`FAIL`).
    Failure,
    /// The migration's `should_run` returned false (`SKIPPED`).
    Skipped,
}

impl MigrationResult {
    /// The label Laravel prints.
    pub fn label(&self) -> &'static str {
        match self {
            MigrationResult::Success => "DONE",
            MigrationResult::Failure => "FAIL",
            MigrationResult::Skipped => "SKIPPED",
        }
    }
}

/// Progress reported by the migrator, so commands can print Laravel-style output.
#[derive(Clone, Debug, PartialEq)]
pub enum MigrationEvent {
    /// An informational line ("Running migrations.", "Nothing to migrate.", ...).
    Info(String),
    /// A migration is about to run.
    Started {
        name: String,
        method: MigrationMethod,
    },
    /// A migration finished (or failed, or was skipped).
    Finished {
        name: String,
        method: MigrationMethod,
        duration_ms: f64,
        result: MigrationResult,
    },
    /// A ran migration could not be found while rolling back.
    NotFound { name: String },
    /// The queries a migration would run (`--pretend`).
    Pretended {
        name: String,
        method: MigrationMethod,
        queries: Vec<String>,
    },
}

impl MigrationEvent {
    /// Render the event the way Laravel's console components would, without
    /// colors, for a terminal of the given width:
    ///
    /// ```
    /// use illuminate_database::migrations::{MigrationEvent, MigrationMethod, MigrationResult};
    ///
    /// let event = MigrationEvent::Finished {
    ///     name: "2024_01_01_000000_create_users_table".into(),
    ///     method: MigrationMethod::Up,
    ///     duration_ms: 3.21,
    ///     result: MigrationResult::Success,
    /// };
    /// let line = event.render(80);
    /// assert!(line.starts_with("  2024_01_01_000000_create_users_table ...."));
    /// assert!(line.ends_with(" 3.21ms DONE"));
    /// ```
    pub fn render(&self, width: usize) -> String {
        match self {
            MigrationEvent::Info(message) => format!("\n   INFO  {message}\n"),
            MigrationEvent::Started { name, .. } => format!("  {name} "),
            MigrationEvent::Finished {
                name,
                duration_ms,
                result,
                ..
            } => render_task(name, Some(*duration_ms), result.label(), width),
            MigrationEvent::NotFound { name } => {
                render_task(name, None, "Migration not found", width)
            }
            MigrationEvent::Pretended { name, queries, .. } => {
                let mut out = render_task(name, None, "", width);
                for query in queries {
                    out.push_str(&format!("\n  ⇂ {query}"));
                }
                out
            }
        }
    }
}

/// Render a two-column task line: `  name ........ 3.21ms DONE`.
pub fn render_task(
    description: &str,
    duration_ms: Option<f64>,
    status: &str,
    width: usize,
) -> String {
    let run_time = duration_ms
        .map(format_run_time)
        .map(|t| format!(" {t}"))
        .unwrap_or_default();
    let width = width.min(150);
    let used = description.chars().count() + run_time.chars().count() + 10;
    let dots = width.saturating_sub(used);
    let status = if status.is_empty() {
        String::new()
    } else {
        format!(" {status}")
    };
    format!("  {description} {}{run_time}{status}", ".".repeat(dots))
}

/// Format a duration the way Laravel's `runTimeForHumans` does.
pub fn format_run_time(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{ms:.2}ms")
    } else {
        format!("{:.2}s", ms / 1000.0)
    }
}

/// Options for [`Migrator::run`].
#[derive(Clone, Debug, Default)]
pub struct MigrateOptions {
    /// Dump the SQL queries that would be run instead of running them.
    pub pretend: bool,
    /// Give every migration its own batch, so they can be rolled back individually.
    pub step: bool,
}

/// Options for [`Migrator::rollback`].
#[derive(Clone, Debug, Default)]
pub struct RollbackOptions {
    /// Roll back this many migrations (instead of the last batch).
    pub step: Option<usize>,
    /// Roll back a specific batch.
    pub batch: Option<i64>,
    /// Dump the SQL queries that would be run instead of running them.
    pub pretend: bool,
}

/// The status of a migration, as shown by `migrate:status`.
#[derive(Clone, Debug, PartialEq)]
pub struct MigrationStatus {
    /// The migration's name.
    pub name: String,
    /// The batch it ran in, if it has run.
    pub batch: Option<i64>,
    /// Whether the migration has run.
    pub ran: bool,
}

/// A record in the migrations table.
#[derive(Clone, Debug, PartialEq)]
pub struct MigrationRecord {
    /// The migration's name.
    pub migration: String,
    /// The batch it ran in.
    pub batch: i64,
}

/// Stores which migrations have run in a database table.
#[derive(Clone)]
pub struct DatabaseMigrationRepository {
    manager: Arc<DatabaseManager>,
    table: String,
    connection: Option<String>,
}

impl DatabaseMigrationRepository {
    /// Create a repository using the given table.
    pub fn new(manager: Arc<DatabaseManager>, table: impl Into<String>) -> Self {
        Self {
            manager,
            table: table.into(),
            connection: None,
        }
    }

    /// Set the connection the repository uses.
    pub fn set_source(&mut self, name: Option<String>) {
        self.connection = name;
    }

    /// Get the connection the repository uses.
    pub fn get_connection(&self) -> Connection {
        match &self.connection {
            Some(name) => self.manager.connection(name),
            None => self.manager.default_connection(),
        }
    }

    fn query(&self) -> Builder {
        self.get_connection()
            .table(self.table.as_str())
            .use_write_pdo()
    }

    fn records(rows: impl IntoIterator<Item = Value>) -> Vec<MigrationRecord> {
        rows.into_iter()
            .map(|row| MigrationRecord {
                migration: row
                    .get("migration")
                    .map(|m| m.to_string_lossy())
                    .unwrap_or_default(),
                batch: row
                    .get("batch")
                    .and_then(|b| b.to_i64_lossy())
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// Get the completed migrations, in the order they ran.
    pub async fn get_ran(&self) -> Result<Vec<String>> {
        Ok(self
            .query()
            .order_by("batch", "asc")
            .order_by("migration", "asc")
            .pluck("migration")
            .await?
            .into_iter()
            .map(|m| m.to_string_lossy())
            .collect())
    }

    /// Get the last `steps` migrations that ran.
    pub async fn get_migrations(&self, steps: usize) -> Result<Vec<MigrationRecord>> {
        let rows = self
            .query()
            .where_op("batch", ">=", 1)
            .order_by("batch", "desc")
            .order_by("migration", "desc")
            .limit(steps as i64)
            .get()
            .await?;
        Ok(Self::records(rows))
    }

    /// Get the migrations of a given batch.
    pub async fn get_migrations_by_batch(&self, batch: i64) -> Result<Vec<MigrationRecord>> {
        let rows = self
            .query()
            .where_("batch", batch)
            .order_by("migration", "desc")
            .get()
            .await?;
        Ok(Self::records(rows))
    }

    /// Get the migrations of the last batch.
    pub async fn get_last(&self) -> Result<Vec<MigrationRecord>> {
        let last = self.get_last_batch_number().await?;
        self.get_migrations_by_batch(last).await
    }

    /// Get every ran migration with its batch number.
    pub async fn get_migration_batches(&self) -> Result<IndexMap<String, i64>> {
        Ok(self
            .query()
            .order_by("batch", "asc")
            .order_by("migration", "asc")
            .pluck_with_key("batch", "migration")
            .await?
            .into_iter()
            .map(|(name, batch)| (name, batch.to_i64_lossy().unwrap_or_default()))
            .collect())
    }

    /// Log that a migration was run.
    pub async fn log(&self, migration: &str, batch: i64) -> Result<()> {
        self.query()
            .insert(json!({"migration": migration, "batch": batch}))
            .await?;
        Ok(())
    }

    /// Remove a migration from the log.
    pub async fn delete(&self, migration: &str) -> Result<()> {
        self.query().where_("migration", migration).delete().await?;
        Ok(())
    }

    /// Get the next migration batch number.
    pub async fn get_next_batch_number(&self) -> Result<i64> {
        Ok(self.get_last_batch_number().await? + 1)
    }

    /// Get the last migration batch number.
    pub async fn get_last_batch_number(&self) -> Result<i64> {
        Ok(self.query().max("batch").await?.to_i64_lossy().unwrap_or(0))
    }

    /// Create the migration repository table.
    pub async fn create_repository(&self) -> Result<()> {
        self.get_connection()
            .get_schema_builder()
            .create(&self.table, |table| {
                table.increments("id");
                table.string("migration");
                table.integer("batch");
            })
            .await
    }

    /// Determine if the migration repository exists.
    pub async fn repository_exists(&self) -> Result<bool> {
        self.get_connection()
            .get_schema_builder()
            .has_table(&self.table)
            .await
    }

    /// Delete the migration repository table.
    pub async fn delete_repository(&self) -> Result<()> {
        self.get_connection()
            .get_schema_builder()
            .drop(&self.table)
            .await
    }
}

/// A migration output callback.
pub type MigrationOutput = Arc<dyn Fn(&MigrationEvent) + Send + Sync>;

/// Runs migrations up and down.
pub struct Migrator {
    manager: Arc<DatabaseManager>,
    repository: DatabaseMigrationRepository,
    migrations: Vec<(String, MigrationRef)>,
    connection: Option<String>,
    output: Option<MigrationOutput>,
}

impl Migrator {
    /// Create a migrator for the given migrations (sorted by name).
    pub fn new(
        manager: Arc<DatabaseManager>,
        migrations: impl IntoIterator<Item = (String, Box<dyn Migration>)>,
    ) -> Self {
        let registry = MigrationRegistry::new();
        registry.extend(migrations);
        Self::from_registry(manager, &registry)
    }

    /// Create a migrator for the migrations of a registry.
    pub fn from_registry(manager: Arc<DatabaseManager>, registry: &MigrationRegistry) -> Self {
        let table = manager.migrations_table();
        Self {
            repository: DatabaseMigrationRepository::new(manager.clone(), table),
            manager,
            migrations: registry.all(),
            connection: None,
            output: None,
        }
    }

    /// Create a migrator using the container's database manager, for the
    /// given migrations.
    pub fn resolve(migrations: impl IntoIterator<Item = (String, Box<dyn Migration>)>) -> Self {
        Self::new(DatabaseManager::resolve(), migrations)
    }

    /// Create a migrator for the migrations registered in the container's
    /// [`MigrationRegistry`].
    pub fn from_app() -> Self {
        let manager = DatabaseManager::resolve();
        match try_app::<MigrationRegistry>() {
            Some(registry) => Self::from_registry(manager, &registry),
            None => Self::from_registry(manager, &MigrationRegistry::new()),
        }
    }

    /// Use the given connection (`None` for the default) for the repository
    /// and for migrations that don't name a connection.
    pub fn set_connection(&mut self, name: Option<&str>) {
        self.connection = name.map(String::from);
        self.repository.set_source(self.connection.clone());
    }

    /// Builder-style [`Migrator::set_connection`].
    pub fn with_connection(mut self, name: Option<&str>) -> Self {
        self.set_connection(name);
        self
    }

    /// Set the callback receiving progress events.
    pub fn set_output(&mut self, output: impl Fn(&MigrationEvent) + Send + Sync + 'static) {
        self.output = Some(Arc::new(output));
    }

    /// Builder-style [`Migrator::set_output`].
    pub fn with_output(mut self, output: impl Fn(&MigrationEvent) + Send + Sync + 'static) -> Self {
        self.set_output(output);
        self
    }

    fn write(&self, event: MigrationEvent) {
        if let Some(output) = &self.output {
            output(&event);
        }
    }

    /// Get the migration repository.
    pub fn get_repository(&self) -> &DatabaseMigrationRepository {
        &self.repository
    }

    /// The names of every known migration, sorted.
    pub fn migration_names(&self) -> Vec<String> {
        self.migrations
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn find(&self, name: &str) -> Option<MigrationRef> {
        self.migrations
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, m)| m.clone())
    }

    /// Determine if the migration repository exists.
    pub async fn repository_exists(&self) -> Result<bool> {
        self.repository.repository_exists().await
    }

    /// Create the migration repository (`migrate:install`).
    pub async fn install(&self) -> Result<()> {
        self.repository.create_repository().await
    }

    /// Delete the migration repository.
    pub async fn delete_repository(&self) -> Result<()> {
        self.repository.delete_repository().await
    }

    /// Determine if any migrations have been run.
    pub async fn has_run_any_migrations(&self) -> Result<bool> {
        Ok(self.repository_exists().await? && !self.repository.get_ran().await?.is_empty())
    }

    async fn ensure_repository(&self) -> Result<()> {
        if !self.repository_exists().await? {
            self.install().await?;
        }
        Ok(())
    }

    /// Run the pending migrations, returning the names of the ones that ran.
    pub async fn run(&self, options: MigrateOptions) -> Result<Vec<String>> {
        self.ensure_repository().await?;
        let ran = self.repository.get_ran().await?;
        let pending: Vec<(String, MigrationRef)> = self
            .migrations
            .iter()
            .filter(|(name, _)| !ran.contains(name))
            .cloned()
            .collect();

        if pending.is_empty() {
            self.write(MigrationEvent::Info("Nothing to migrate.".into()));
            return Ok(Vec::new());
        }

        let mut batch = self.repository.get_next_batch_number().await?;
        self.write(MigrationEvent::Info("Running migrations.".into()));

        let mut names = Vec::new();
        for (name, migration) in pending {
            if options.pretend {
                self.pretend_to_run(&name, &migration, MigrationMethod::Up)
                    .await?;
                names.push(name);
                continue;
            }
            if !migration.should_run() {
                self.write(MigrationEvent::Finished {
                    name: name.clone(),
                    method: MigrationMethod::Up,
                    duration_ms: 0.0,
                    result: MigrationResult::Skipped,
                });
                continue;
            }
            self.run_migration(&name, &migration, MigrationMethod::Up)
                .await?;
            self.repository.log(&name, batch).await?;
            names.push(name);
            if options.step {
                batch += 1;
            }
        }
        Ok(names)
    }

    /// Roll back the last batch (or the given steps / batch) of migrations,
    /// returning the names of the ones rolled back.
    pub async fn rollback(&self, options: RollbackOptions) -> Result<Vec<String>> {
        if !self.repository_exists().await? {
            self.write(MigrationEvent::Info("Nothing to rollback.".into()));
            return Ok(Vec::new());
        }
        let records = match (options.step, options.batch) {
            (Some(step), _) if step > 0 => self.repository.get_migrations(step).await?,
            (_, Some(batch)) if batch > 0 => self.repository.get_migrations_by_batch(batch).await?,
            _ => self.repository.get_last().await?,
        };
        if records.is_empty() {
            self.write(MigrationEvent::Info("Nothing to rollback.".into()));
            return Ok(Vec::new());
        }
        let names = records.into_iter().map(|r| r.migration).collect();
        self.rollback_migrations(names, options.pretend).await
    }

    async fn rollback_migrations(&self, names: Vec<String>, pretend: bool) -> Result<Vec<String>> {
        self.write(MigrationEvent::Info("Rolling back migrations.".into()));
        let mut rolled_back = Vec::new();
        for name in names {
            let Some(migration) = self.find(&name) else {
                self.write(MigrationEvent::NotFound { name });
                continue;
            };
            if pretend {
                self.pretend_to_run(&name, &migration, MigrationMethod::Down)
                    .await?;
            } else {
                self.run_migration(&name, &migration, MigrationMethod::Down)
                    .await?;
                self.repository.delete(&name).await?;
            }
            rolled_back.push(name);
        }
        Ok(rolled_back)
    }

    /// Roll back every migration that has run (`migrate:reset`).
    pub async fn reset(&self, pretend: bool) -> Result<Vec<String>> {
        if !self.repository_exists().await? {
            self.write(MigrationEvent::Info("Nothing to rollback.".into()));
            return Ok(Vec::new());
        }
        let mut ran = self.repository.get_ran().await?;
        if ran.is_empty() {
            self.write(MigrationEvent::Info("Nothing to rollback.".into()));
            return Ok(Vec::new());
        }
        ran.reverse();
        self.rollback_migrations(ran, pretend).await
    }

    /// Roll back every migration (or the last `step` migrations) and run them
    /// all again (`migrate:refresh`). Returns the migrations that ran.
    pub async fn refresh(&self, step: Option<usize>) -> Result<Vec<String>> {
        match step {
            Some(step) if step > 0 => {
                self.rollback(RollbackOptions {
                    step: Some(step),
                    ..Default::default()
                })
                .await?;
            }
            _ => {
                self.reset(false).await?;
            }
        }
        self.run(MigrateOptions::default()).await
    }

    /// Drop every table and run all migrations (`migrate:fresh`).
    pub async fn fresh(&self, options: MigrateOptions) -> Result<Vec<String>> {
        self.repository
            .get_connection()
            .get_schema_builder()
            .drop_all_tables()
            .await?;
        self.write(MigrationEvent::Info("Dropping all tables.".into()));
        self.run(options).await
    }

    /// The status of every known migration (`migrate:status`).
    pub async fn status(&self) -> Result<Vec<MigrationStatus>> {
        let batches = if self.repository_exists().await? {
            self.repository.get_migration_batches().await?
        } else {
            IndexMap::new()
        };
        Ok(self
            .migrations
            .iter()
            .map(|(name, _)| MigrationStatus {
                name: name.clone(),
                batch: batches.get(name).copied(),
                ran: batches.contains_key(name),
            })
            .collect())
    }

    fn connection_for(&self, migration: &MigrationRef) -> String {
        migration
            .connection()
            .or_else(|| self.connection.clone())
            .unwrap_or_else(|| self.manager.get_default_connection())
    }

    async fn run_migration(
        &self,
        name: &str,
        migration: &MigrationRef,
        method: MigrationMethod,
    ) -> Result<()> {
        let connection_name = self.connection_for(migration);
        let connection = self.manager.connection(&connection_name);
        self.write(MigrationEvent::Started {
            name: name.to_string(),
            method,
        });
        let start = Instant::now();

        let run = || {
            let migration = migration.clone();
            let connection_name = connection_name.clone();
            let manager = self.manager.clone();
            async move {
                manager
                    .using_connection(&connection_name, async {
                        match method {
                            MigrationMethod::Up => migration.up().await,
                            MigrationMethod::Down => migration.down().await,
                        }
                    })
                    .await
            }
        };

        let result = if connection.schema_grammar().supports_schema_transactions()
            && migration.within_transaction()
        {
            connection.transaction(run).await
        } else {
            run().await
        };

        let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
        self.write(MigrationEvent::Finished {
            name: name.to_string(),
            method,
            duration_ms,
            result: if result.is_ok() {
                MigrationResult::Success
            } else {
                MigrationResult::Failure
            },
        });
        result
    }

    async fn pretend_to_run(
        &self,
        name: &str,
        migration: &MigrationRef,
        method: MigrationMethod,
    ) -> Result<()> {
        let connection_name = self.connection_for(migration);
        let connection = self.manager.connection(&connection_name);
        let manager = self.manager.clone();
        let queries = connection
            .pretend(|| async {
                manager
                    .using_connection(&connection_name, async {
                        match method {
                            MigrationMethod::Up => migration.up().await,
                            MigrationMethod::Down => migration.down().await,
                        }
                    })
                    .await
            })
            .await?;
        self.write(MigrationEvent::Pretended {
            name: name.to_string(),
            method,
            queries: queries.into_iter().map(|q| q.query).collect(),
        });
        Ok(())
    }
}
