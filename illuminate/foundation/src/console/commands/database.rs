//! Database commands: `migrate*`, `db:seed`, and `db:wipe`.

use std::sync::Arc;

use illuminate_console::{Command, Components, Console, async_trait};
use illuminate_database::migrations::{MigrateOptions, MigrationEvent, RollbackOptions, render_task};
use illuminate_database::schema::SchemaLoaded;
use illuminate_database::seeder::{SeederEvent, SeederOutput, run_seeder};
use illuminate_database::{DatabaseManager, Migrator, SeederRegistry};
use illuminate_events::Event;
use illuminate_support::Result;

use crate::application::Application;

/// Ask before running a destructive command in production.
fn confirm_to_proceed(cmd: &Console) -> bool {
    let app = Application::current();
    if !app.is_production() || cmd.option_bool("force") {
        return true;
    }
    cmd.components().warn("Application In Production.");
    let confirmed = cmd.confirm("Are you sure you want to run this command?", false);
    if !confirmed {
        cmd.components().warn("Command cancelled.");
    }
    confirmed
}

/// A migrator that prints Laravel-style progress to the command's output.
fn migrator(cmd: &Console) -> Migrator {
    let output = cmd.output().clone();
    Migrator::from_app()
        .with_connection(cmd.option("database").as_deref())
        .with_output(move |event: &MigrationEvent| match event {
            MigrationEvent::Started { .. } => {}
            MigrationEvent::Info(message) => Components::new(&output).info(message),
            other => output.writeln(colorize(&other.render(output.width()))),
        })
}

/// Color a task line the way Laravel's task component does: gray dots and
/// run time, then the status.
fn colorize(line: &str) -> String {
    let statuses = [
        (" DONE", "<fg=green;options=bold>DONE</>"),
        (" FAIL", "<fg=red;options=bold>FAIL</>"),
        (" SKIPPED", "<fg=yellow;options=bold>SKIPPED</>"),
        (" RUNNING", "<fg=yellow;options=bold>RUNNING</>"),
    ];
    for (suffix, status) in statuses {
        if let Some(stripped) = line.strip_suffix(suffix) {
            // "  name ....... 9.80ms" → gray from the dots on.
            return match stripped.find(" .") {
                Some(at) => format!("{} <fg=gray>{}</> {status}", &stripped[..at], stripped[at + 1..].trim_end()),
                None => format!("{stripped} {status}"),
            };
        }
    }
    line.to_string()
}

/// Load the stored schema dump (`schema:dump`) into a database no
/// migration has run on yet — Laravel's `loadSchemaState`.
async fn load_schema_state(cmd: &Console, migrator: &Migrator) -> Result<()> {
    if migrator.has_run_any_migrations().await? {
        return Ok(());
    }
    let app = Application::current();
    let connection = migrator.connection_name();
    let path = match cmd.option("schema-path").filter(|path| !path.is_empty()) {
        Some(path) => std::path::PathBuf::from(path),
        None => Migrator::schema_path(app.database_path(""), &connection),
    };
    if !path.is_file() {
        return Ok(());
    }

    cmd.components().info("Loading stored database schemas.");
    let display = crate::console::generators::relative(&app, &path);
    cmd.components()
        .task(display, || async { migrator.load_schema_state(&path).await })
        .await?;
    cmd.new_line(1);

    Event::dispatch(SchemaLoaded { connection_name: connection, path }).await
}

/// Run a seeder by name, printing progress.
async fn seed(cmd: &Console, class: &str) -> Result<()> {
    let registry = Application::current().make::<SeederRegistry>();
    let Some(seeder) = registry.resolve(class) else {
        return cmd.fail(format!("Target class [{class}] does not exist."));
    };
    let output = cmd.output().clone();
    cmd.new_line(1);
    cmd.components().info("Seeding database.");
    let printer: SeederOutput = Arc::new(move |event: &SeederEvent| {
        let line = match event {
            SeederEvent::Running { name } => render_task(name, None, "RUNNING", output.width()),
            SeederEvent::Done { name, duration_ms } => render_task(name, Some(*duration_ms), "DONE", output.width()),
        };
        output.writeln(colorize(&line));
    });
    run_seeder(seeder.as_ref(), Some(printer)).await?;
    cmd.new_line(1);
    Ok(())
}

fn migrate_signature(name: &str, extra: &str) -> String {
    format!(
        "{name}
            {{--database= : The database connection to use}}
            {{--force : Force the operation to run when in production}}
            {extra}"
    )
}

/// `migrate` — Run the database migrations.
pub struct MigrateCommand {
    signature: String,
}

impl Default for MigrateCommand {
    fn default() -> Self {
        Self {
            signature: migrate_signature(
                "migrate",
                "{--schema-path= : The path to a schema dump file}
                 {--pretend : Dump the SQL queries that would be run}
                 {--seed : Indicates if the seed task should be re-run}
                 {--seeder= : The class name of the root seeder}
                 {--step : Force the migrations to be run so they can be rolled back individually}
                 {--graceful : Return a successful exit code even if an error occurs}",
            ),
        }
    }
}

#[async_trait]
impl Command for MigrateCommand {
    fn signature(&self) -> &str {
        &self.signature
    }

    fn description(&self) -> &str {
        "Run the database migrations"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        if !confirm_to_proceed(&cmd) {
            return cmd.exit(1);
        }

        match run_migrations(&cmd).await {
            Err(error) if cmd.option_bool("graceful") => {
                cmd.components().warn(error.to_string());
                Ok(())
            }
            result => result,
        }
    }
}

/// Prepare the database, then run the outstanding migrations (and seeders).
async fn run_migrations(cmd: &Console) -> Result<()> {
    let migrator = migrator(cmd);
    if !migrator.repository_exists().await? {
        cmd.new_line(1);
        cmd.components().info("Preparing database.");
        cmd.components()
            .task("Creating migration table", || async {
                migrator.install().await
            })
            .await?;
        cmd.new_line(1);
    }
    if !cmd.option_bool("pretend") {
        load_schema_state(cmd, &migrator).await?;
    }
    migrator
        .run(MigrateOptions {
            pretend: cmd.option_bool("pretend"),
            step: cmd.option_bool("step"),
        })
        .await?;
    cmd.new_line(1);

    if cmd.option_bool("seed") && !cmd.option_bool("pretend") {
        let class = cmd.option("seeder").unwrap_or_else(|| "DatabaseSeeder".into());
        seed(cmd, &class).await?;
    }
    Ok(())
}

/// `migrate:install` — Create the migration repository.
pub struct MigrateInstallCommand;

#[async_trait]
impl Command for MigrateInstallCommand {
    fn signature(&self) -> &str {
        "migrate:install {--database= : The database connection to use}"
    }

    fn description(&self) -> &str {
        "Create the migration repository"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let migrator = migrator(&cmd);
        migrator.install().await?;
        cmd.components().info("Migration table created successfully.");
        Ok(())
    }
}

/// `migrate:rollback` — Rollback the last database migration.
pub struct MigrateRollbackCommand {
    signature: String,
}

impl Default for MigrateRollbackCommand {
    fn default() -> Self {
        Self {
            signature: migrate_signature(
                "migrate:rollback",
                "{--step= : The number of migrations to be reverted}
                 {--batch= : The batch of migrations (identified by their batch number) to be reverted}
                 {--pretend : Dump the SQL queries that would be run}",
            ),
        }
    }
}

#[async_trait]
impl Command for MigrateRollbackCommand {
    fn signature(&self) -> &str {
        &self.signature
    }

    fn description(&self) -> &str {
        "Rollback the last database migration"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        if !confirm_to_proceed(&cmd) {
            return cmd.exit(1);
        }
        let migrator = migrator(&cmd);
        if !migrator.repository_exists().await? {
            cmd.components().error("Migration table not found.");
            return cmd.exit(1);
        }
        migrator
            .rollback(RollbackOptions {
                step: cmd.option("step").and_then(|s| s.parse().ok()),
                batch: cmd.option("batch").and_then(|b| b.parse().ok()),
                pretend: cmd.option_bool("pretend"),
            })
            .await?;
        cmd.new_line(1);
        Ok(())
    }
}

/// `migrate:reset` — Rollback all database migrations.
pub struct MigrateResetCommand {
    signature: String,
}

impl Default for MigrateResetCommand {
    fn default() -> Self {
        Self {
            signature: migrate_signature("migrate:reset", "{--pretend : Dump the SQL queries that would be run}"),
        }
    }
}

#[async_trait]
impl Command for MigrateResetCommand {
    fn signature(&self) -> &str {
        &self.signature
    }

    fn description(&self) -> &str {
        "Rollback all database migrations"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        if !confirm_to_proceed(&cmd) {
            return cmd.exit(1);
        }
        let migrator = migrator(&cmd);
        if !migrator.repository_exists().await? {
            cmd.components().warn("Migration table not found.");
            return Ok(());
        }
        migrator.reset(cmd.option_bool("pretend")).await?;
        cmd.new_line(1);
        Ok(())
    }
}

/// `migrate:refresh` — Reset and re-run all migrations.
pub struct MigrateRefreshCommand {
    signature: String,
}

impl Default for MigrateRefreshCommand {
    fn default() -> Self {
        Self {
            signature: migrate_signature(
                "migrate:refresh",
                "{--seed : Indicates if the seed task should be re-run}
                 {--seeder= : The class name of the root seeder}
                 {--step= : The number of migrations to be reverted & re-run}",
            ),
        }
    }
}

#[async_trait]
impl Command for MigrateRefreshCommand {
    fn signature(&self) -> &str {
        &self.signature
    }

    fn description(&self) -> &str {
        "Reset and re-run all migrations"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        if !confirm_to_proceed(&cmd) {
            return cmd.exit(1);
        }
        let migrator = migrator(&cmd);
        if !migrator.repository_exists().await? {
            migrator.install().await?;
        }
        match cmd.option("step").and_then(|s| s.parse::<usize>().ok()).filter(|step| *step > 0) {
            Some(step) => {
                migrator
                    .rollback(RollbackOptions {
                        step: Some(step),
                        ..Default::default()
                    })
                    .await?;
            }
            None => {
                migrator.reset(false).await?;
            }
        }
        load_schema_state(&cmd, &migrator).await?;
        migrator.run(MigrateOptions::default()).await?;
        cmd.new_line(1);
        if cmd.option_bool("seed") {
            let class = cmd.option("seeder").unwrap_or_else(|| "DatabaseSeeder".into());
            seed(&cmd, &class).await?;
        }
        Ok(())
    }
}

/// `migrate:fresh` — Drop all tables and re-run all migrations.
pub struct MigrateFreshCommand {
    signature: String,
}

impl Default for MigrateFreshCommand {
    fn default() -> Self {
        Self {
            signature: migrate_signature(
                "migrate:fresh",
                "{--drop-views : Drop all tables and views}
                 {--schema-path= : The path to a schema dump file}
                 {--seed : Indicates if the seed task should be re-run}
                 {--seeder= : The class name of the root seeder}
                 {--step : Force the migrations to be run so they can be rolled back individually}",
            ),
        }
    }
}

#[async_trait]
impl Command for MigrateFreshCommand {
    fn signature(&self) -> &str {
        &self.signature
    }

    fn description(&self) -> &str {
        "Drop all tables and re-run all migrations"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        if !confirm_to_proceed(&cmd) {
            return cmd.exit(1);
        }
        let migrator = migrator(&cmd);
        let schema = migrator.get_repository().get_connection().get_schema_builder();
        cmd.new_line(1);
        cmd.components()
            .task("Dropping all tables", || async {
                if cmd.option_bool("drop-views") {
                    schema.drop_all_views().await?;
                }
                schema.drop_all_tables().await
            })
            .await?;
        cmd.new_line(1);
        cmd.components().info("Preparing database.");
        cmd.components()
            .task("Creating migration table", || async { migrator.install().await })
            .await?;
        cmd.new_line(1);
        load_schema_state(&cmd, &migrator).await?;
        migrator
            .run(MigrateOptions {
                pretend: false,
                step: cmd.option_bool("step"),
            })
            .await?;
        cmd.new_line(1);
        if cmd.option_bool("seed") {
            let class = cmd.option("seeder").unwrap_or_else(|| "DatabaseSeeder".into());
            seed(&cmd, &class).await?;
        }
        Ok(())
    }
}

/// `migrate:status` — Show the status of each migration.
pub struct MigrateStatusCommand;

#[async_trait]
impl Command for MigrateStatusCommand {
    fn signature(&self) -> &str {
        "migrate:status
            {--database= : The database connection to use}
            {--pending : Only list pending migrations}"
    }

    fn description(&self) -> &str {
        "Show the status of each migration"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let migrator = migrator(&cmd);
        if !migrator.repository_exists().await? {
            cmd.components().error("Migration table not found.");
            return cmd.exit(1);
        }
        let mut statuses = migrator.status().await?;
        if cmd.option_bool("pending") {
            statuses.retain(|status| !status.ran);
        }
        if statuses.is_empty() {
            cmd.components().info("No migrations found");
            return Ok(());
        }
        cmd.new_line(1);
        cmd.components()
            .two_column_detail("<fg=gray>Migration name</>", "<fg=gray>Batch / Status</>");
        for status in statuses {
            let state = match status.batch {
                Some(batch) if status.ran => format!("[{batch}] <fg=green;options=bold>Ran</>"),
                _ => "<fg=yellow;options=bold>Pending</>".to_string(),
            };
            cmd.components().two_column_detail(status.name, state);
        }
        cmd.new_line(1);
        Ok(())
    }
}

/// `db:seed` — Seed the database with records.
pub struct SeedCommand;

#[async_trait]
impl Command for SeedCommand {
    fn signature(&self) -> &str {
        "db:seed
            {class? : The class name of the root seeder}
            {--class=DatabaseSeeder : The class name of the root seeder}
            {--database= : The database connection to seed}
            {--force : Force the operation to run when in production}"
    }

    fn description(&self) -> &str {
        "Seed the database with records"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        if !confirm_to_proceed(&cmd) {
            return cmd.exit(1);
        }
        let class = cmd
            .argument("class")
            .or_else(|| cmd.option("class"))
            .unwrap_or_else(|| "DatabaseSeeder".into());
        match cmd.option("database") {
            Some(connection) => {
                let manager = DatabaseManager::resolve();
                let previous = manager.get_default_connection();
                manager.set_default_connection(&connection);
                let result = seed(&cmd, &class).await;
                manager.set_default_connection(&previous);
                result
            }
            None => seed(&cmd, &class).await,
        }
    }
}

/// `db:wipe` — Drop all tables, views, and types.
pub struct WipeCommand;

#[async_trait]
impl Command for WipeCommand {
    fn signature(&self) -> &str {
        "db:wipe
            {--database= : The database connection to use}
            {--force : Force the operation to run when in production}"
    }

    fn description(&self) -> &str {
        "Drop all tables, views, and types"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        if !confirm_to_proceed(&cmd) {
            return cmd.exit(1);
        }
        let manager = DatabaseManager::resolve();
        let connection = match cmd.option("database") {
            Some(name) => manager.connection(&name),
            None => manager.default_connection(),
        };
        connection.get_schema_builder().drop_all_tables().await?;
        cmd.components().info("Dropped all tables successfully.");
        Ok(())
    }
}
