//! `schema:dump` and `db:monitor`.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use illuminate_console::{Command, Console, async_trait};
use illuminate_database::migrations::MigrationsPruned;
use illuminate_database::schema::SchemaDumped;
use illuminate_database::{Connection, DatabaseManager};
use illuminate_events::Event;
use illuminate_support::Result;

use crate::application::Application;

/// The connection named by `--database`, or the default one.
fn connection(manager: &DatabaseManager, name: Option<String>) -> Connection {
    match name.filter(|name| !name.is_empty()) {
        Some(name) => manager.connection(&name),
        None => manager.default_connection(),
    }
}

/// `schema:dump` — Dump the given database schema.
///
/// The schema is written to `database/schema/{connection}-schema.sql`, and
/// `migrate` loads it into a fresh database before running the migrations
/// created after it. `--prune` deletes the migration files (keeping
/// `database/migrations/mod.rs`, which discovers them): migrations are
/// compiled into your application, so the pruned ones disappear once it is
/// rebuilt, and fresh databases are built from the dump from then on.
pub struct DumpCommand;

#[async_trait]
impl Command for DumpCommand {
    fn signature(&self) -> &str {
        "schema:dump
            {--database= : The database connection to use}
            {--path= : The path where the schema dump file should be stored}
            {--prune : Delete all existing migration files}
            {--without-migration-data : Dump the schema without the migration data}"
    }

    fn description(&self) -> &str {
        "Dump the given database schema"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let manager = DatabaseManager::resolve();
        let connection = connection(&manager, cmd.option("database"));

        let path = cmd
            .option("path")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(app.database_path(&format!("schema/{}-schema.sql", connection.get_name())))
            });

        let table = manager.migrations_table();
        let migration_table = (!cmd.option_bool("without-migration-data")).then_some(table.as_str());
        let output = cmd.output().clone();
        connection
            .get_schema_state()
            .with_migration_table(migration_table)
            .handle_output_using(move |buffer| output.write(buffer))
            .dump(&path)
            .await?;

        Event::dispatch(SchemaDumped {
            connection_name: connection.get_name().to_string(),
            path: path.clone(),
        })
        .await?;

        let mut info = "Database schema dumped".to_string();
        if cmd.option_bool("prune") {
            let migrations = PathBuf::from(app.database_path("migrations"));
            prune_migrations(&migrations)?;
            info.push_str(" and pruned");
            Event::dispatch(MigrationsPruned {
                connection_name: connection.get_name().to_string(),
                path: migrations,
            })
            .await?;
        }

        cmd.components().info(format!("{info} successfully."));
        Ok(())
    }
}

/// Delete every migration in the directory, keeping the `mod.rs` that
/// discovers them so the application still compiles.
fn prune_migrations(directory: &std::path::Path) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Ok(());
    };
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else if path.file_name().is_some_and(|name| name != "mod.rs") {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// A database has more open connections than `db:monitor --max` allows —
/// Laravel's `DatabaseBusy` event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseBusy {
    /// The connection's name.
    pub connection_name: String,
    /// How many connections are open.
    pub connections: i64,
}

/// Counts the open connections of a database connection.
pub type ThreadCounter = Arc<dyn Fn(Connection) -> BoxFuture<'static, Result<Option<i64>>> + Send + Sync>;

/// The number of connections open on the connection's server — Laravel's
/// `Connection::threadCount()`. SQLite has no server, so there is no count.
pub async fn thread_count(connection: &Connection) -> Result<Option<i64>> {
    Ok(super::inspection::open_connections(connection).await)
}

/// `db:monitor` — Monitor the number of connections on the specified
/// database, dispatching [`DatabaseBusy`] when there are `--max` or more.
#[derive(Default)]
pub struct MonitorCommand {
    counter: Option<ThreadCounter>,
}

impl MonitorCommand {
    /// Count open connections with the given callback instead of asking the
    /// database server.
    pub fn count_connections_using<F, Fut>(counter: F) -> Self
    where
        F: Fn(Connection) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<i64>>> + Send + 'static,
    {
        Self {
            counter: Some(Arc::new(move |connection| Box::pin(counter(connection)))),
        }
    }

    async fn count(&self, connection: Connection) -> Result<Option<i64>> {
        match &self.counter {
            Some(counter) => counter(connection).await,
            None => thread_count(&connection).await,
        }
    }
}

#[async_trait]
impl Command for MonitorCommand {
    fn signature(&self) -> &str {
        "db:monitor
            {--databases= : The database connections to monitor}
            {--max= : The maximum number of connections that can be open before an event is dispatched}"
    }

    fn description(&self) -> &str {
        "Monitor the number of connections on the specified database"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let manager = DatabaseManager::resolve();
        let max = cmd
            .option("max")
            .and_then(|max| max.trim().parse::<i64>().ok())
            .filter(|max| *max > 0);

        let mut databases = Vec::new();
        for database in cmd.option("databases").unwrap_or_default().split(',') {
            let name = match database.trim() {
                "" => manager.get_default_connection(),
                name => name.to_string(),
            };
            let connections = self.count(manager.connection(&name)).await?;
            let busy = matches!((max, connections), (Some(max), Some(count)) if count >= max);
            databases.push((name, connections, busy));
        }

        cmd.new_line(1);
        cmd.components()
            .two_column_detail("<fg=gray>Database name</>", "<fg=gray>Connections</>");
        for (name, connections, busy) in &databases {
            let status = if *busy { "<fg=yellow;options=bold>ALERT</>" } else { "<fg=green;options=bold>OK</>" };
            let count = connections.map(|count| count.to_string()).unwrap_or_default();
            cmd.components().two_column_detail(name, format!("[{count}] {status}"));
        }
        cmd.new_line(1);

        if max.is_some() {
            for (name, connections, busy) in databases {
                if busy {
                    Event::dispatch(DatabaseBusy {
                        connection_name: name,
                        connections: connections.unwrap_or_default(),
                    })
                    .await?;
                }
            }
        }
        Ok(())
    }
}
