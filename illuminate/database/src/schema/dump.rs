//! Schema dumps: squashing migrations into a single SQL file
//! (`cargo artisan schema:dump`), and loading that file into a fresh
//! database before the remaining migrations run.
//!
//! SQLite schemas are dumped and loaded in-process (the equivalent of the
//! `sqlite3` shell's `.schema` and `.dump`), so no command line client is
//! needed. MySQL and MariaDB use `mysqldump` / `mariadb-dump` and `mysql` /
//! `mariadb`, and PostgreSQL uses `pg_dump` and `psql` / `pg_restore`, exactly
//! like Laravel — so those clients must be installed.
//!
//! ```no_run
//! # async fn example(connection: illuminate_database::Connection) -> illuminate_support::Result<()> {
//! let path = std::path::Path::new("database/schema/sqlite-schema.sql");
//!
//! connection.get_schema_state().with_migration_table(Some("migrations")).dump(path).await?;
//! connection.get_schema_state().load(path).await?;
//! # Ok(()) }
//! ```

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use illuminate_support::error::RuntimeException;
use illuminate_support::{Result, Value, ValueExt};
use regex::Regex;

use crate::connection::Connection;
use crate::driver::{self, Driver};

/// Receives the output of the command line clients used for dumping and
/// loading.
pub type SchemaOutput = Arc<dyn Fn(&str) + Send + Sync>;

/// Dumps and loads a connection's schema — Laravel's `SchemaState`.
#[derive(Clone)]
pub struct SchemaState {
    connection: Connection,
    migration_table: Option<String>,
    output: Option<SchemaOutput>,
}

impl fmt::Debug for SchemaState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SchemaState")
            .field("connection", &self.connection.get_name())
            .field("migration_table", &self.migration_table)
            .finish()
    }
}

impl Connection {
    /// The schema state for the connection, used to dump its schema to a
    /// file and to load it back.
    pub fn get_schema_state(&self) -> SchemaState {
        SchemaState::new(self.clone())
    }
}

/// A schema was dumped (`schema:dump`) — Laravel's `SchemaDumped` event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaDumped {
    /// The connection's name.
    pub connection_name: String,
    /// The dump file.
    pub path: PathBuf,
}

/// A schema dump was loaded before migrating — Laravel's `SchemaLoaded`
/// event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaLoaded {
    /// The connection's name.
    pub connection_name: String,
    /// The dump file.
    pub path: PathBuf,
}

impl SchemaState {
    /// Create a schema state for the connection. The `migrations` table's
    /// rows are included in dumps unless told otherwise.
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            migration_table: Some("migrations".to_string()),
            output: None,
        }
    }

    /// The connection whose schema is dumped and loaded.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Include the rows of the given migrations table in dumps (`None` dumps
    /// the schema without any migration data).
    pub fn with_migration_table(mut self, table: Option<&str>) -> Self {
        self.migration_table = table.filter(|table| !table.is_empty()).map(str::to_string);
        self
    }

    /// Hand the output of the command line clients to the given callback.
    pub fn handle_output_using(mut self, output: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.output = Some(Arc::new(output));
        self
    }

    /// Determine if the migrations table exists (and should be dumped).
    pub async fn has_migration_table(&self) -> Result<bool> {
        match &self.migration_table {
            Some(table) => self.connection.get_schema_builder().has_table(table).await,
            None => Ok(false),
        }
    }

    /// The prefixed name of the migrations table.
    fn migration_table(&self) -> String {
        format!(
            "{}{}",
            self.connection.get_table_prefix(),
            self.migration_table.as_deref().unwrap_or("migrations")
        )
    }

    /// Dump the database's schema (and the migrations table's rows) to the
    /// given file, creating its directory if needed.
    pub async fn dump(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(directory) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            std::fs::create_dir_all(directory)?;
        }
        match self.connection.driver() {
            Driver::Sqlite => self.dump_sqlite(path).await,
            Driver::MySql | Driver::MariaDb => self.dump_mysql(path).await,
            Driver::Postgres => self.dump_postgres(path).await,
        }
    }

    /// Load a schema dump into the database.
    pub async fn load(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if !path.is_file() {
            return Err(RuntimeException::new(format!("Schema file [{}] does not exist.", path.display())).into());
        }
        match self.connection.driver() {
            Driver::Sqlite => {
                let schema = std::fs::read_to_string(path)?;
                if !schema.trim().is_empty() {
                    self.connection.unprepared(&schema).await?;
                }
                Ok(())
            }
            Driver::MySql | Driver::MariaDb => self.load_mysql(path).await,
            Driver::Postgres => self.load_postgres(path).await,
        }
    }

    // ------------------------------------------------------------------
    // SQLite
    // ------------------------------------------------------------------

    /// The `sqlite3` shell's `.schema`: every table, index, view and trigger,
    /// in the order they were created, without SQLite's internal tables.
    async fn dump_sqlite(&self, path: &Path) -> Result<()> {
        let objects = self
            .connection
            .select(
                "select type, name, sql from sqlite_master where sql is not null and name not like 'sqlite_%' order by rowid",
                (),
            )
            .await?;
        let shadow = self.sqlite_shadow_tables().await;

        let mut schema = String::new();
        for object in objects {
            let name = object.get("name").map(ValueExt::to_string_lossy).unwrap_or_default();
            if shadow.contains(&name) {
                continue;
            }
            let sql = object.get("sql").map(ValueExt::to_string_lossy).unwrap_or_default();
            let sql = sql.trim().trim_end_matches(';');
            if !sql.is_empty() {
                schema.push_str(sql);
                schema.push_str(";\n");
            }
        }

        if self.has_migration_table().await? {
            schema.push_str(&self.sqlite_migration_data().await?);
        }
        std::fs::write(path, schema)?;
        Ok(())
    }

    /// The shadow tables of virtual tables (FTS indexes and the like), which
    /// SQLite creates by itself.
    async fn sqlite_shadow_tables(&self) -> Vec<String> {
        self.connection
            .select("pragma main.table_list", ())
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|table| table.get("type").and_then(Value::as_str) == Some("shadow"))
            .filter_map(|table| table.get("name").map(ValueExt::to_string_lossy))
            .collect()
    }

    /// The `sqlite3` shell's `.dump 'migrations'`, keeping only the inserts.
    async fn sqlite_migration_data(&self) -> Result<String> {
        let table = self.migration_table();
        let rows = self
            .connection
            .select(&format!("select * from {} order by rowid", quote_identifier(&table)), ())
            .await?;
        let mut data = String::new();
        for row in rows {
            let Value::Object(columns) = row else { continue };
            let values = columns
                .values()
                .map(|value| driver::quote_literal(Driver::Sqlite, value))
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(RuntimeException::new)?;
            data.push_str(&format!("INSERT INTO {} VALUES({});\n", quote_identifier(&table), values.join(",")));
        }
        Ok(data)
    }

    // ------------------------------------------------------------------
    // MySQL & MariaDB
    // ------------------------------------------------------------------

    fn is_maria(&self) -> bool {
        self.connection.driver() == Driver::MariaDb
    }

    async fn dump_mysql(&self, path: &Path) -> Result<()> {
        let version = self.detect_mysql_client_version().await;
        let mut variables = self.mysql_variables();
        variables.push(("LARAVEL_LOAD_PATH".into(), path.to_string_lossy().into_owned()));
        let command = format!(
            "{} --routines --result-file=\"$LARAVEL_LOAD_PATH\" --no-data",
            self.mysql_base_dump_command(&version)
        );
        let output = self.run_mysql_dump(command, &variables, 0).await?;
        self.write_output(&output);

        // Remove the auto-incrementing state, so the dump doesn't change
        // every time a row is inserted.
        let pattern = Regex::new(r"(?i)\s+AUTO_INCREMENT=[0-9]+").expect("valid regex");
        let schema = std::fs::read_to_string(path)?;
        std::fs::write(path, pattern.replace_all(&schema, "").as_ref())?;

        if self.has_migration_table().await? {
            let command = format!(
                "{} {} --no-create-info --skip-extended-insert --skip-routines --compact --complete-insert",
                self.mysql_base_dump_command(&version),
                shell_quote(&self.migration_table()),
            );
            let data = self.run_mysql_dump(command, &self.mysql_variables(), 0).await?;
            append(path, &data)?;
        }
        Ok(())
    }

    /// Run a dump command, dropping the options older clients don't know.
    async fn run_mysql_dump(&self, command: String, variables: &[(String, String)], depth: u8) -> Result<String> {
        match run_process(&command, variables).await {
            Ok(output) => Ok(output),
            Err(error) if depth < 30 => {
                let message = error.to_string();
                if message.contains("column-statistics") || message.contains("column_statistics") {
                    let command = command.replace(" --column-statistics=0", "");
                    return Box::pin(self.run_mysql_dump(command, variables, depth + 1)).await;
                }
                if message.contains("set-gtid-purged") {
                    let command = command.replace(" --set-gtid-purged=OFF", "");
                    return Box::pin(self.run_mysql_dump(command, variables, depth + 1)).await;
                }
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    async fn load_mysql(&self, path: &Path) -> Result<()> {
        let version = self.detect_mysql_client_version().await;
        let client = if self.is_maria() { "mariadb" } else { "mysql" };
        let command = format!(
            "{client} {} --database=\"$LARAVEL_LOAD_DATABASE\" < \"$LARAVEL_LOAD_PATH\"",
            self.mysql_connection_string(&version)
        );
        let mut variables = self.mysql_variables();
        variables.push(("LARAVEL_LOAD_PATH".into(), path.to_string_lossy().into_owned()));
        let output = run_process(&command, &variables).await?;
        self.write_output(&output);
        Ok(())
    }

    /// The `mysqldump` (or `mariadb-dump`) command, before its options.
    fn mysql_base_dump_command(&self, version: &ClientVersion) -> String {
        if self.is_maria() {
            return format!(
                "mariadb-dump {} --no-tablespaces --skip-add-locks --skip-comments --skip-set-charset --tz-utc \"$LARAVEL_LOAD_DATABASE\"",
                self.mysql_connection_string(version)
            );
        }
        let mut command = format!(
            "mysqldump {} --no-tablespaces --skip-add-locks --skip-comments --skip-set-charset --tz-utc --column-statistics=0",
            self.mysql_connection_string(version)
        );
        if !version.maria {
            command.push_str(" --set-gtid-purged=OFF");
        }
        command.push_str(" \"$LARAVEL_LOAD_DATABASE\"");
        command
    }

    fn mysql_connection_string(&self, version: &ClientVersion) -> String {
        let mut value = " --user=\"$LARAVEL_LOAD_USER\" --password=\"$LARAVEL_LOAD_PASSWORD\"".to_string();
        if self.config_string("unix_socket").is_empty() {
            value.push_str(" --host=\"$LARAVEL_LOAD_HOST\" --port=\"$LARAVEL_LOAD_PORT\"");
        } else {
            value.push_str(" --socket=\"$LARAVEL_LOAD_SOCKET\"");
        }
        if !self.config_string("options.ssl_ca").is_empty() {
            value.push_str(" --ssl-ca=\"$LARAVEL_LOAD_SSL_CA\"");
        }
        if !self.config_string("options.ssl_cert").is_empty() {
            value.push_str(" --ssl-cert=\"$LARAVEL_LOAD_SSL_CERT\"");
        }
        if !self.config_string("options.ssl_key").is_empty() {
            value.push_str(" --ssl-key=\"$LARAVEL_LOAD_SSL_KEY\"");
        }
        if self.connection.get_config("options.ssl_verify_server_cert") == Value::Bool(false) {
            if version_at_least(&version.version, "5.7.11") && !version.maria {
                value.push_str(" --ssl-mode=DISABLED");
            } else {
                value.push_str(" --ssl=off");
            }
        }
        value
    }

    fn mysql_variables(&self) -> Vec<(String, String)> {
        vec![
            ("LARAVEL_LOAD_SOCKET".into(), self.config_string("unix_socket")),
            ("LARAVEL_LOAD_HOST".into(), self.host()),
            ("LARAVEL_LOAD_PORT".into(), self.config_string("port")),
            ("LARAVEL_LOAD_USER".into(), self.config_string("username")),
            ("LARAVEL_LOAD_PASSWORD".into(), self.config_string("password")),
            ("LARAVEL_LOAD_DATABASE".into(), self.config_string("database")),
            ("LARAVEL_LOAD_SSL_CA".into(), self.config_string("options.ssl_ca")),
            ("LARAVEL_LOAD_SSL_CERT".into(), self.config_string("options.ssl_cert")),
            ("LARAVEL_LOAD_SSL_KEY".into(), self.config_string("options.ssl_key")),
        ]
    }

    /// The installed client's version (`mysql --version`).
    async fn detect_mysql_client_version(&self) -> ClientVersion {
        let (client, fallback) = if self.is_maria() { ("mariadb", "10.5.2") } else { ("mysql", "8.0.0") };
        let output = run_process(&format!("{client} --version"), &[]).await.unwrap_or_default();
        let version = Regex::new(r"(\d+\.\d+\.\d+)")
            .expect("valid regex")
            .captures(&output)
            .map(|captures| captures[1].to_string())
            .unwrap_or_else(|| fallback.to_string());
        ClientVersion {
            version,
            maria: self.is_maria() || output.to_lowercase().contains("mariadb"),
        }
    }

    // ------------------------------------------------------------------
    // PostgreSQL
    // ------------------------------------------------------------------

    async fn dump_postgres(&self, path: &Path) -> Result<()> {
        let mut variables = self.postgres_variables();
        variables.push(("LARAVEL_LOAD_PATH".into(), path.to_string_lossy().into_owned()));

        let output = run_process(
            &format!("{} --schema-only > \"$LARAVEL_LOAD_PATH\"", postgres_base_dump_command()),
            &variables,
        )
        .await?;
        self.write_output(&output);

        if self.has_migration_table().await? {
            variables.push(("LARAVEL_LOAD_MIGRATION_TABLE".into(), self.postgres_migration_table()));
            let output = run_process(
                &format!(
                    "{} -t \"$LARAVEL_LOAD_MIGRATION_TABLE\" --data-only >> \"$LARAVEL_LOAD_PATH\"",
                    postgres_base_dump_command()
                ),
                &variables,
            )
            .await?;
            self.write_output(&output);
        }
        Ok(())
    }

    async fn load_postgres(&self, path: &Path) -> Result<()> {
        let command = if path.extension().is_some_and(|extension| extension == "sql") {
            "psql --file=\"$LARAVEL_LOAD_PATH\" --host=\"$LARAVEL_LOAD_HOST\" --port=\"$LARAVEL_LOAD_PORT\" --username=\"$LARAVEL_LOAD_USER\" --dbname=\"$LARAVEL_LOAD_DATABASE\""
        } else {
            "pg_restore --no-owner --no-acl --clean --if-exists --host=\"$LARAVEL_LOAD_HOST\" --port=\"$LARAVEL_LOAD_PORT\" --username=\"$LARAVEL_LOAD_USER\" --dbname=\"$LARAVEL_LOAD_DATABASE\" \"$LARAVEL_LOAD_PATH\""
        };
        let mut variables = self.postgres_variables();
        variables.push(("LARAVEL_LOAD_PATH".into(), path.to_string_lossy().into_owned()));
        let output = run_process(command, &variables).await?;
        self.write_output(&output);
        Ok(())
    }

    /// The schema-qualified migrations table (`public.migrations`).
    fn postgres_migration_table(&self) -> String {
        let table = self.migration_table();
        if table.contains('.') {
            return table;
        }
        let schema = match self.connection.get_config("search_path") {
            Value::Array(paths) => paths.first().map(ValueExt::to_string_lossy).unwrap_or_default(),
            Value::String(paths) => paths.split(',').next().unwrap_or_default().to_string(),
            _ => String::new(),
        };
        let schema = schema.trim().trim_matches('"');
        let schema = if schema.is_empty() || schema == "$user" { "public" } else { schema };
        format!("{schema}.{table}")
    }

    fn postgres_variables(&self) -> Vec<(String, String)> {
        let mut variables = vec![
            ("LARAVEL_LOAD_HOST".into(), self.host()),
            ("LARAVEL_LOAD_PORT".into(), self.config_string("port")),
            ("LARAVEL_LOAD_USER".into(), self.config_string("username")),
            ("PGPASSWORD".into(), self.config_string("password")),
        ];
        let sslmode = self.config_string("sslmode");
        if !sslmode.is_empty() {
            variables.push(("PGSSLMODE".into(), sslmode));
        }
        variables.push(("LARAVEL_LOAD_DATABASE".into(), self.config_string("database")));
        variables
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn config_string(&self, key: &str) -> String {
        match self.connection.get_config(key) {
            Value::Null => String::new(),
            value => value.to_string_lossy(),
        }
    }

    /// The host to connect to (the first, for read / write hosts).
    fn host(&self) -> String {
        match self.connection.get_config("host") {
            Value::Array(hosts) => hosts.first().map(ValueExt::to_string_lossy).unwrap_or_default(),
            Value::Null => String::new(),
            host => host.to_string_lossy(),
        }
    }

    fn write_output(&self, output: &str) {
        if let Some(callback) = &self.output
            && !output.is_empty()
        {
            callback(output);
        }
    }
}

/// The base `pg_dump` command.
fn postgres_base_dump_command() -> &'static str {
    "pg_dump --no-owner --no-acl --host=\"$LARAVEL_LOAD_HOST\" --port=\"$LARAVEL_LOAD_PORT\" --username=\"$LARAVEL_LOAD_USER\" --dbname=\"$LARAVEL_LOAD_DATABASE\""
}

/// The installed MySQL client's version.
#[derive(Clone, Debug)]
struct ClientVersion {
    version: String,
    maria: bool,
}

/// Compare dotted version numbers.
fn version_at_least(version: &str, minimum: &str) -> bool {
    let parse = |value: &str| -> Vec<u64> { value.split('.').map(|part| part.parse().unwrap_or(0)).collect() };
    parse(version) >= parse(minimum)
}

/// Quote an SQLite identifier, like the `sqlite3` shell does.
fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Quote a shell argument.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn append(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}

/// Run a shell command with the given environment, returning its output or
/// failing with its error output, like Symfony's `mustRun`.
async fn run_process(command: &str, variables: &[(String, String)]) -> Result<String> {
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .envs(variables.iter().map(|(key, value)| (key.as_str(), value.as_str())))
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|error| RuntimeException::new(format!("The command \"{command}\" failed.\n\n{error}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(RuntimeException::new(format!(
        "The command \"{command}\" failed.\n\nExit Code: {}\n\nOutput:\n================\n{stdout}\n\nError Output:\n================\n{stderr}",
        output.status.code().map(|code| code.to_string()).unwrap_or_else(|| "(signal)".into()),
    ))
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn sqlite(path: &Path) -> Connection {
        Connection::new("sqlite", json!({"driver": "sqlite", "database": path.to_string_lossy()}))
    }

    async fn migrate(connection: &Connection) {
        connection
            .get_schema_builder()
            .create("migrations", |table| {
                table.increments("id");
                table.string("migration");
                table.integer("batch");
            })
            .await
            .unwrap();
        connection
            .get_schema_builder()
            .create("flights", |table| {
                table.id();
                table.string("name").index();
                table.timestamps();
            })
            .await
            .unwrap();
        connection
            .insert("insert into migrations (migration, batch) values (?, ?), (?, ?)", (
                "0001_01_01_000000_create_flights_table",
                1,
                "2024_01_01_000000_add_o'hare_to_flights",
                2,
            ))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn sqlite_schemas_are_dumped_and_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let connection = sqlite(&dir.path().join("database.sqlite"));
        std::fs::write(dir.path().join("database.sqlite"), "").unwrap();
        migrate(&connection).await;

        let path = dir.path().join("schema/sqlite-schema.sql");
        connection.get_schema_state().dump(&path).await.unwrap();
        let dump = std::fs::read_to_string(&path).unwrap();
        assert!(dump.contains("CREATE TABLE \"flights\""), "{dump}");
        assert!(dump.contains("CREATE INDEX \"flights_name_index\""), "{dump}");
        assert!(!dump.contains("sqlite_sequence"), "{dump}");
        assert!(dump.contains("INSERT INTO \"migrations\" VALUES(1,'0001_01_01_000000_create_flights_table',1);"), "{dump}");
        assert!(dump.contains("'2024_01_01_000000_add_o''hare_to_flights'"), "{dump}");

        // Load it into a fresh database.
        std::fs::write(dir.path().join("fresh.sqlite"), "").unwrap();
        let fresh = sqlite(&dir.path().join("fresh.sqlite"));
        fresh.get_schema_state().load(&path).await.unwrap();
        let schema = fresh.get_schema_builder();
        assert!(schema.has_table("flights").await.unwrap());
        assert!(schema.has_index("flights", "flights_name_index", None).await.unwrap());
        let migrations = fresh.select("select migration, batch from migrations order by id", ()).await.unwrap();
        assert_eq!(migrations.len(), 2);
        assert_eq!(migrations[1]["batch"], json!(2));

        // New migrations keep counting from the loaded ones.
        fresh.insert("insert into migrations (migration, batch) values (?, ?)", ("next", 3)).await.unwrap();
        assert_eq!(fresh.scalar("select max(id) from migrations", ()).await.unwrap(), json!(3));
    }

    #[tokio::test]
    async fn the_migration_data_can_be_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let connection = sqlite(&dir.path().join("database.sqlite"));
        std::fs::write(dir.path().join("database.sqlite"), "").unwrap();
        migrate(&connection).await;

        let path = dir.path().join("schema.sql");
        connection.get_schema_state().with_migration_table(None).dump(&path).await.unwrap();
        let dump = std::fs::read_to_string(&path).unwrap();
        assert!(dump.contains("CREATE TABLE \"migrations\""));
        assert!(!dump.contains("INSERT INTO"));
    }

    #[tokio::test]
    async fn loading_a_missing_file_fails() {
        let connection = Connection::new("sqlite", json!({"driver": "sqlite", "database": ":memory:"}));
        let error = connection.get_schema_state().load("/nowhere/schema.sql").await.unwrap_err();
        assert!(error.to_string().contains("does not exist"));
    }

    #[test]
    fn mysql_and_postgres_commands_use_the_connection_configuration() {
        let mysql = Connection::new(
            "mysql",
            json!({"driver": "mysql", "host": ["10.0.0.1", "10.0.0.2"], "port": 3306, "username": "root", "password": "secret", "database": "laravel"}),
        );
        let state = mysql.get_schema_state();
        let command = state.mysql_base_dump_command(&ClientVersion { version: "8.0.32".into(), maria: false });
        assert!(command.starts_with("mysqldump  --user=\"$LARAVEL_LOAD_USER\""));
        assert!(command.contains("--host=\"$LARAVEL_LOAD_HOST\" --port=\"$LARAVEL_LOAD_PORT\""));
        assert!(command.contains("--set-gtid-purged=OFF"));
        assert!(command.ends_with("\"$LARAVEL_LOAD_DATABASE\""));
        let variables = state.mysql_variables();
        assert!(variables.contains(&("LARAVEL_LOAD_HOST".into(), "10.0.0.1".into())));
        assert!(variables.contains(&("LARAVEL_LOAD_PASSWORD".into(), "secret".into())));

        let socket = Connection::new("mariadb", json!({"driver": "mariadb", "unix_socket": "/tmp/mysql.sock"}));
        let command = socket
            .get_schema_state()
            .mysql_base_dump_command(&ClientVersion { version: "10.11.0".into(), maria: true });
        assert!(command.starts_with("mariadb-dump"));
        assert!(command.contains("--socket=\"$LARAVEL_LOAD_SOCKET\""));

        let pgsql = Connection::new(
            "pgsql",
            json!({"driver": "pgsql", "host": "127.0.0.1", "port": 5432, "username": "root", "password": "secret", "database": "laravel", "search_path": "app", "sslmode": "prefer", "prefix": "x_"}),
        );
        let state = pgsql.get_schema_state();
        assert_eq!(state.postgres_migration_table(), "app.x_migrations");
        let variables = state.postgres_variables();
        assert!(variables.contains(&("PGPASSWORD".into(), "secret".into())));
        assert!(variables.contains(&("PGSSLMODE".into(), "prefer".into())));
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(version_at_least("8.0.32", "5.7.11"));
        assert!(version_at_least("5.7.11", "5.7.11"));
        assert!(!version_at_least("5.6.40", "5.7.11"));
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
