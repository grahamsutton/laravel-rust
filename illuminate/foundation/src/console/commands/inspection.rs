//! Database inspection commands: `db:show` and `db:table`.

use illuminate_console::{Command, Console, async_trait};
use illuminate_database::{Connection, DatabaseManager, Driver};
use illuminate_support::{Number, Result, Value, ValueExt, json};

/// The connection named by `--database`, or the default one.
fn connection(cmd: &Console) -> Connection {
    let manager = DatabaseManager::resolve();
    match cmd.option("database").filter(|name| !name.is_empty()) {
        Some(name) => manager.connection(&name),
        None => manager.default_connection(),
    }
}

/// The platform's name, like Laravel's `getDriverTitle`.
fn driver_title(driver: Driver) -> &'static str {
    match driver {
        Driver::Sqlite => "SQLite",
        Driver::MySql => "MySQL",
        Driver::MariaDb => "MariaDB",
        Driver::Postgres => "PostgreSQL",
    }
}

/// The database server's version.
async fn server_version(connection: &Connection) -> String {
    let query = match connection.driver() {
        Driver::Sqlite => "select sqlite_version()",
        Driver::Postgres => "show server_version",
        _ => "select version()",
    };
    connection
        .scalar(query, ())
        .await
        .map(|version| version.to_string_lossy())
        .unwrap_or_default()
}

/// The number of open connections to the server, where it can tell us.
async fn open_connections(connection: &Connection) -> Option<i64> {
    let query = match connection.driver() {
        Driver::Sqlite => return None,
        Driver::Postgres => "select count(*) from pg_stat_activity",
        _ => "select variable_value from performance_schema.global_status where variable_name = 'threads_connected'",
    };
    let value = connection.scalar(query, ()).await.ok()?;
    value.as_i64().or_else(|| value.to_string_lossy().parse().ok())
}

fn file_size(bytes: Option<i64>) -> Option<String> {
    bytes.map(|bytes| Number::file_size(bytes as f64, 2))
}

fn config_string(connection: &Connection, key: &str) -> String {
    match connection.get_config(key) {
        Value::Null => String::new(),
        value => value.to_string_lossy(),
    }
}

/// `db:show` — Display information about the given database.
pub struct ShowCommand;

#[async_trait]
impl Command for ShowCommand {
    fn signature(&self) -> &str {
        "db:show
            {--database= : The database connection}
            {--json : Output the database information as JSON}
            {--counts : Show the table row count <bg=red;options=bold> Note: This can be slow on large databases </>}
            {--views : Show the database views <bg=red;options=bold> Note: This can be slow on large databases </>}"
    }

    fn description(&self) -> &str {
        "Display information about the given database"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let connection = connection(&cmd);
        let schema = connection.get_schema_builder();
        let counts = cmd.option_bool("counts");

        let mut tables = Vec::new();
        for table in schema.get_tables().await? {
            let rows = if counts {
                Some(connection.table(table.schema_qualified_name.as_str()).count().await?)
            } else {
                None
            };
            tables.push((table, rows));
        }

        let views = if cmd.option_bool("views") {
            schema.get_views().await?
        } else {
            Vec::new()
        };

        let platform_name = driver_title(connection.driver());
        let version = server_version(&connection).await;
        let open = open_connections(&connection).await;

        if cmd.option_bool("json") {
            let data = json!({
                "platform": {
                    "config": connection.get_config_array().as_object().map(|config| {
                        let mut config = config.clone();
                        config.remove("password");
                        Value::Object(config)
                    }),
                    "name": platform_name,
                    "connection": connection.get_name(),
                    "version": version,
                    "open_connections": open,
                },
                "tables": tables.iter().map(|(table, rows)| json!({
                    "table": table.name,
                    "schema": table.schema,
                    "schema_qualified_name": table.schema_qualified_name,
                    "size": table.size,
                    "rows": rows,
                    "engine": table.engine,
                    "collation": table.collation,
                    "comment": table.comment,
                })).collect::<Vec<_>>(),
                "views": if views.is_empty() { Value::Null } else { Value::Array(views) },
            });
            cmd.line(data.to_string());
            return Ok(());
        }

        let components = cmd.components();
        cmd.new_line(1);
        components.two_column_detail(format!("<fg=green;options=bold>{platform_name}</>"), &version);
        components.two_column_detail("Connection", connection.get_name());
        components.two_column_detail("Database", config_string(&connection, "database"));
        components.two_column_detail("Host", config_string(&connection, "host"));
        components.two_column_detail("Port", config_string(&connection, "port"));
        components.two_column_detail("Username", config_string(&connection, "username"));
        components.two_column_detail("URL", config_string(&connection, "url"));
        components.two_column_detail("Open Connections", open.map(|open| open.to_string()).unwrap_or_default());
        components.two_column_detail("Tables", tables.len().to_string());

        let total: i64 = tables.iter().filter_map(|(table, _)| table.size).sum();
        if total > 0 {
            components.two_column_detail("Total Size", Number::file_size(total as f64, 2));
        }
        cmd.new_line(1);

        if let Some((first, _)) = tables.first() {
            let has_schema = first.schema.is_some();
            components.two_column_detail(
                format!(
                    "{}<fg=green;options=bold>Table</>",
                    if has_schema { "<fg=green;options=bold>Schema</> <fg=gray;options=bold>/</> " } else { "" }
                ),
                format!("Size{}", if counts { " <fg=gray;options=bold>/</> <fg=yellow;options=bold>Rows</>" } else { "" }),
            );
            for (table, rows) in &tables {
                let name = match &table.schema {
                    Some(schema) => format!("{schema} <fg=gray;options=bold>/</> {}", table.name),
                    None => table.name.clone(),
                };
                let engine = match (&table.engine, cmd.is_verbose()) {
                    (Some(engine), true) => format!(" <fg=gray>{engine}</>"),
                    _ => String::new(),
                };
                let rows = match rows {
                    Some(rows) => format!(
                        " <fg=gray;options=bold>/</> <fg=yellow;options=bold>{}</>",
                        Number::format(*rows as f64, None)
                    ),
                    None => String::new(),
                };
                components.two_column_detail(
                    format!("{name}{engine}"),
                    format!("{}{rows}", file_size(table.size).unwrap_or_else(|| "—".into())),
                );
                if cmd.is_verbose()
                    && let Some(comment) = table.comment.as_ref().filter(|comment| !comment.is_empty())
                {
                    components.bullet_list([comment.as_str()]);
                }
            }
            cmd.new_line(1);
        }

        if !views.is_empty() {
            components.two_column_detail("<fg=green;options=bold>View</>", "");
            for view in &views {
                let name = view.get("name").map(ValueExt::to_string_lossy).unwrap_or_default();
                let name = match view.get("schema").filter(|schema| !schema.is_null()) {
                    Some(schema) => format!("{} <fg=gray;options=bold>/</> {name}", schema.to_string_lossy()),
                    None => name,
                };
                components.two_column_detail(name, "");
            }
            cmd.new_line(1);
        }
        Ok(())
    }
}

/// `db:table` — Display information about the given database table.
pub struct TableCommand;

#[async_trait]
impl Command for TableCommand {
    fn signature(&self) -> &str {
        "db:table
            {table? : The name of the table}
            {--database= : The database connection}
            {--json : Output the table information as JSON}"
    }

    fn description(&self) -> &str {
        "Display information about the given database table"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let connection = connection(&cmd);
        let schema = connection.get_schema_builder();
        let tables = schema.get_tables().await?;

        let name = match cmd.argument("table").filter(|table| !table.is_empty()) {
            Some(name) => name,
            None => cmd.anticipate(
                "Which table would you like to inspect?",
                tables.iter().map(|table| table.schema_qualified_name.clone()),
            ),
        };

        let Some(table) = tables
            .iter()
            .find(|table| table.schema_qualified_name == name)
            .or_else(|| tables.iter().find(|table| table.name == name))
        else {
            cmd.components().warn(format!("Table [{name}] doesn't exist."));
            return cmd.exit(1);
        };

        let qualified = table.schema_qualified_name.as_str();
        let columns = schema.get_columns(qualified).await?;
        let indexes = schema.get_indexes(qualified).await?;
        let foreign_keys = schema.get_foreign_keys(qualified).await?;

        let column_attributes = |column: &illuminate_database::schema::ColumnInfo| {
            [
                Some(column.type_name.clone()),
                column.auto_increment.then(|| "autoincrement".to_string()),
                column.nullable.then(|| "nullable".to_string()),
                column.collation.clone(),
            ]
            .into_iter()
            .flatten()
            .filter(|attribute| !attribute.is_empty())
            .collect::<Vec<_>>()
        };
        let index_attributes = |index: &illuminate_database::schema::IndexInfo| {
            [
                index.type_.clone(),
                (index.columns.len() > 1).then(|| "compound".to_string()),
                (index.unique && !index.primary).then(|| "unique".to_string()),
                index.primary.then(|| "primary".to_string()),
            ]
            .into_iter()
            .flatten()
            .filter(|attribute| !attribute.is_empty())
            .collect::<Vec<_>>()
        };

        if cmd.option_bool("json") {
            let data = json!({
                "table": {
                    "schema": table.schema,
                    "name": table.name,
                    "schema_qualified_name": table.schema_qualified_name,
                    "columns": columns.len(),
                    "size": table.size,
                    "comment": table.comment,
                    "collation": table.collation,
                    "engine": table.engine,
                },
                "columns": columns.iter().map(|column| json!({
                    "column": column.name,
                    "attributes": column_attributes(column),
                    "default": column.default,
                    "type": column.type_,
                })).collect::<Vec<_>>(),
                "indexes": indexes.iter().map(|index| json!({
                    "name": index.name,
                    "columns": index.columns,
                    "attributes": index_attributes(index),
                })).collect::<Vec<_>>(),
                "foreign_keys": foreign_keys.iter().map(|key| json!({
                    "name": key.name,
                    "columns": key.columns,
                    "foreign_schema": key.foreign_schema,
                    "foreign_table": key.foreign_table,
                    "foreign_columns": key.foreign_columns,
                    "on_update": key.on_update,
                    "on_delete": key.on_delete,
                })).collect::<Vec<_>>(),
            });
            cmd.line(data.to_string());
            return Ok(());
        }

        let components = cmd.components();
        cmd.new_line(1);
        components.two_column_detail(
            format!("<fg=green;options=bold>{}</>", table.schema_qualified_name),
            table
                .comment
                .as_ref()
                .filter(|comment| !comment.is_empty())
                .map(|comment| format!("<fg=gray>{comment}</>"))
                .unwrap_or_default(),
        );
        components.two_column_detail("Columns", columns.len().to_string());
        if let Some(size) = file_size(table.size) {
            components.two_column_detail("Size", size);
        }
        if let Some(engine) = table.engine.as_ref().filter(|engine| !engine.is_empty()) {
            components.two_column_detail("Engine", engine);
        }
        if let Some(collation) = table.collation.as_ref().filter(|collation| !collation.is_empty()) {
            components.two_column_detail("Collation", collation);
        }
        cmd.new_line(1);

        if !columns.is_empty() {
            components.two_column_detail("<fg=green;options=bold>Column</>", "Type");
            for column in &columns {
                let default = column
                    .default
                    .as_ref()
                    .map(|default| format!("<fg=gray>{default}</> "))
                    .unwrap_or_default();
                components.two_column_detail(
                    format!("{} <fg=gray>{}</>", column.name, column_attributes(column).join(", ")),
                    format!("{default}{}", column.type_),
                );
            }
            cmd.new_line(1);
        }

        if !indexes.is_empty() {
            components.two_column_detail("<fg=green;options=bold>Index</>", "");
            for index in &indexes {
                components.two_column_detail(
                    format!("{} <fg=gray>{}</>", index.name, index.columns.join(", ")),
                    index_attributes(index).join(", "),
                );
            }
            cmd.new_line(1);
        }

        if !foreign_keys.is_empty() {
            components.two_column_detail("<fg=green;options=bold>Foreign Key</>", "On Update / On Delete");
            for key in &foreign_keys {
                components.two_column_detail(
                    format!(
                        "{} <fg=gray;options=bold>{} references {} on {}</>",
                        key.name.clone().unwrap_or_default(),
                        key.columns.join(", "),
                        key.foreign_columns.join(", "),
                        key.foreign_table
                    ),
                    format!("{} / {}", key.on_update, key.on_delete),
                );
            }
            cmd.new_line(1);
        }
        Ok(())
    }
}
