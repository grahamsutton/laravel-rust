//! The low-level glue between the framework and `sqlx`: drivers, pools,
//! bindings and row decoding.
//!
//! SQLite queries are prepared and bound natively. MySQL / MariaDB and
//! PostgreSQL queries bind their values client-side (just like PDO's
//! "emulated prepares"), so the database infers the type of every value from
//! its context — exactly like the PHP framework, and unlike typed binary
//! parameters, which can't coerce a string into a `timestamp` or `uuid`.

use std::str::FromStr;

use futures::future::BoxFuture;
use futures::stream::{BoxStream, StreamExt};
use std::time::Duration;

use illuminate_support::{Map, Value, ValueExt};
use serde_json::Number;
use sqlx::mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgRow};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow,
    SqliteSynchronous,
};
use sqlx::{Column, Decode, Executor, MySql, Postgres, Row, Sqlite, TypeInfo, ValueRef};

/// The database drivers the framework speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Driver {
    /// SQLite 3.
    Sqlite,
    /// MySQL 5.7+ / 8.
    MySql,
    /// MariaDB 10.3+.
    MariaDb,
    /// PostgreSQL 10+.
    Postgres,
}

impl Driver {
    /// Parse a driver name as used in `config/database.php`.
    pub fn from_name(name: &str) -> Option<Driver> {
        match name.to_ascii_lowercase().as_str() {
            "sqlite" | "sqlite3" => Some(Driver::Sqlite),
            "mysql" => Some(Driver::MySql),
            "mariadb" => Some(Driver::MariaDb),
            "pgsql" | "postgres" | "postgresql" => Some(Driver::Postgres),
            _ => None,
        }
    }

    /// The driver's configuration name (`sqlite`, `mysql`, `mariadb`, `pgsql`).
    pub fn name(&self) -> &'static str {
        match self {
            Driver::Sqlite => "sqlite",
            Driver::MySql => "mysql",
            Driver::MariaDb => "mariadb",
            Driver::Postgres => "pgsql",
        }
    }

    /// Whether the driver speaks the MySQL dialect (MySQL or MariaDB).
    pub fn is_mysql_family(&self) -> bool {
        matches!(self, Driver::MySql | Driver::MariaDb)
    }
}

/// A connection pool for one of the supported drivers.
#[derive(Clone, Debug)]
pub(crate) enum Pool {
    Sqlite(SqlitePool),
    MySql(MySqlPool),
    Postgres(PgPool),
}

impl Pool {
    pub(crate) async fn acquire(&self) -> Result<RawConnection, sqlx::Error> {
        Ok(match self {
            Pool::Sqlite(pool) => RawConnection::Sqlite(pool.acquire().await?),
            Pool::MySql(pool) => RawConnection::MySql(pool.acquire().await?),
            Pool::Postgres(pool) => RawConnection::Postgres(pool.acquire().await?),
        })
    }

    /// The maximum number of connections the pool holds.
    pub(crate) fn max_connections(&self) -> u32 {
        match self {
            Pool::Sqlite(pool) => pool.options().get_max_connections(),
            Pool::MySql(pool) => pool.options().get_max_connections(),
            Pool::Postgres(pool) => pool.options().get_max_connections(),
        }
    }

    /// Stream the rows of a query, one at a time.
    pub(crate) fn stream<'a>(
        &'a self,
        sql: &'a str,
        bindings: &'a [Value],
    ) -> BoxStream<'a, Result<Value, sqlx::Error>> {
        match self {
            Pool::Sqlite(pool) => bind_sqlite(sqlx::query(sql), bindings)
                .fetch(pool)
                .map(|row| row.map(|row| decode_sqlite_row(&row)))
                .boxed(),
            Pool::MySql(pool) => pool
                .fetch(sqlx::raw_sql(sql))
                .map(|row| row.map(|row| decode_mysql_row(&row)))
                .boxed(),
            Pool::Postgres(pool) => pool
                .fetch(sqlx::raw_sql(sql))
                .map(|row| row.map(|row| decode_postgres_row(&row)))
                .boxed(),
        }
    }

    pub(crate) async fn close(&self) {
        match self {
            Pool::Sqlite(pool) => pool.close().await,
            Pool::MySql(pool) => pool.close().await,
            Pool::Postgres(pool) => pool.close().await,
        }
    }
}

/// A single connection checked out of a pool (used by transactions).
pub(crate) enum RawConnection {
    Sqlite(PoolConnection<Sqlite>),
    MySql(PoolConnection<MySql>),
    Postgres(PoolConnection<Postgres>),
}

/// The outcome of a write statement.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Affected {
    pub rows: u64,
    pub last_insert_id: Option<i64>,
}

// ----------------------------------------------------------------------
// Connecting
// ----------------------------------------------------------------------

/// Build a lazily-connecting pool from a connection's configuration.
pub(crate) fn create_pool(driver: Driver, config: &Value) -> Result<Pool, sqlx::Error> {
    match driver {
        Driver::Sqlite => create_sqlite_pool(config),
        Driver::MySql | Driver::MariaDb => create_mysql_pool(config),
        Driver::Postgres => create_postgres_pool(config),
    }
}

fn config_str(config: &Value, key: &str) -> Option<String> {
    match config.get(key) {
        None | Some(Value::Null) => None,
        Some(value) => {
            let s = value.to_string_lossy();
            if s.is_empty() { None } else { Some(s) }
        }
    }
}

fn pool_size(config: &Value, default: u32) -> u32 {
    config
        .dot("pool.max_connections")
        .and_then(|v| v.to_i64_lossy())
        .map(|v| v.max(1) as u32)
        .unwrap_or(default)
}

/// Determine if a SQLite database name refers to an in-memory database.
pub(crate) fn is_memory_database(database: &str) -> bool {
    database == ":memory:"
        || database.contains("?mode=memory")
        || database.contains("&mode=memory")
        || database == "sqlite::memory:"
}

/// The SQLite database path from the `database` (or `url`) option.
pub(crate) fn sqlite_database(config: &Value) -> String {
    if let Some(url) = config_str(config, "url") {
        let path = url
            .trim_start_matches("sqlite://")
            .trim_start_matches("sqlite:")
            .to_string();
        if !path.is_empty() {
            return path;
        }
    }
    config_str(config, "database").unwrap_or_else(|| ":memory:".to_string())
}

fn create_sqlite_pool(config: &Value) -> Result<Pool, sqlx::Error> {
    let database = sqlite_database(config);
    let memory = is_memory_database(&database);

    let mut options = if memory {
        SqliteConnectOptions::from_str("sqlite::memory:")?
    } else {
        SqliteConnectOptions::new()
            .filename(&database)
            .create_if_missing(true)
    };

    let foreign_keys = config
        .get("foreign_key_constraints")
        .map(|v| match v {
            Value::String(s) => {
                matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes")
            }
            other => other.truthy(),
        })
        .unwrap_or(false);
    options = options.foreign_keys(foreign_keys);

    if let Some(timeout) = config.get("busy_timeout").and_then(|v| v.to_i64_lossy()) {
        options = options.busy_timeout(Duration::from_millis(timeout.max(0) as u64));
    }
    if let Some(mode) = config_str(config, "journal_mode")
        && let Ok(mode) = SqliteJournalMode::from_str(&mode)
    {
        options = options.journal_mode(mode);
    }
    if let Some(mode) = config_str(config, "synchronous")
        && let Ok(mode) = SqliteSynchronous::from_str(&mode)
    {
        options = options.synchronous(mode);
    }

    let pool = if memory {
        // Every query must see the same in-memory database, so the pool
        // holds exactly one connection that is never recycled.
        SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(0)
            .idle_timeout(None)
            .max_lifetime(None)
            .acquire_timeout(Duration::from_secs(30))
            .connect_lazy_with(options)
    } else {
        SqlitePoolOptions::new()
            .max_connections(pool_size(config, 5))
            .acquire_timeout(Duration::from_secs(30))
            .connect_lazy_with(options)
    };

    Ok(Pool::Sqlite(pool))
}

fn create_mysql_pool(config: &Value) -> Result<Pool, sqlx::Error> {
    let mut options = match config_str(config, "url") {
        Some(url) => MySqlConnectOptions::from_str(&url)?,
        None => {
            let mut options = MySqlConnectOptions::new()
                .host(&config_str(config, "host").unwrap_or_else(|| "127.0.0.1".into()))
                .port(
                    config
                        .get("port")
                        .and_then(|v| v.to_i64_lossy())
                        .unwrap_or(3306) as u16,
                )
                .username(&config_str(config, "username").unwrap_or_else(|| "root".into()));
            if let Some(password) = config_str(config, "password") {
                options = options.password(&password);
            }
            if let Some(database) = config_str(config, "database") {
                options = options.database(&database);
            }
            if let Some(socket) = config_str(config, "unix_socket") {
                options = options.socket(socket);
            }
            options
        }
    };

    options = options.charset(&config_str(config, "charset").unwrap_or_else(|| "utf8mb4".into()));
    if let Some(collation) = config_str(config, "collation") {
        options = options.collation(&collation);
    }
    if let Some(timezone) = config_str(config, "timezone") {
        options = options.timezone(Some(timezone));
    }
    // Laravel concatenates with `concat()`, never `||`.
    options = options.pipes_as_concat(false);

    let sql_mode = mysql_sql_mode(config);

    let pool = MySqlPoolOptions::new()
        .max_connections(pool_size(config, 10))
        .acquire_timeout(Duration::from_secs(30))
        .after_connect(move |conn, _meta| {
            let sql_mode = sql_mode.clone();
            Box::pin(async move {
                if let Some(mode) = sql_mode {
                    conn.execute(sqlx::raw_sql(&format!("set session sql_mode='{mode}'")))
                        .await?;
                }
                Ok(())
            })
        })
        .connect_lazy_with(options);

    Ok(Pool::MySql(pool))
}

/// The session `sql_mode` Laravel sets for the `strict` / `modes` options.
fn mysql_sql_mode(config: &Value) -> Option<String> {
    if let Some(Value::Array(modes)) = config.get("modes") {
        return Some(
            modes
                .iter()
                .map(|m| m.to_string_lossy().replace('\'', ""))
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    match config.get("strict") {
        Some(value) if value.truthy() => Some(
            "ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,NO_ZERO_IN_DATE,NO_ZERO_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_ENGINE_SUBSTITUTION"
                .to_string(),
        ),
        Some(Value::Bool(false)) => Some("NO_ENGINE_SUBSTITUTION".to_string()),
        _ => None,
    }
}

fn create_postgres_pool(config: &Value) -> Result<Pool, sqlx::Error> {
    let mut options = match config_str(config, "url") {
        Some(url) => PgConnectOptions::from_str(&url)?,
        None => {
            let mut options = PgConnectOptions::new()
                .host(&config_str(config, "host").unwrap_or_else(|| "127.0.0.1".into()))
                .port(
                    config
                        .get("port")
                        .and_then(|v| v.to_i64_lossy())
                        .unwrap_or(5432) as u16,
                )
                .username(&config_str(config, "username").unwrap_or_else(|| "postgres".into()));
            if let Some(password) = config_str(config, "password") {
                options = options.password(&password);
            }
            if let Some(database) = config_str(config, "database") {
                options = options.database(&database);
            }
            if let Some(mode) = config_str(config, "sslmode")
                && let Ok(mode) = sqlx::postgres::PgSslMode::from_str(&mode)
            {
                options = options.ssl_mode(mode);
            }
            options
        }
    };

    // Values are bound client-side, which requires standard conforming strings.
    let mut settings: Vec<(String, String)> =
        vec![("standard_conforming_strings".into(), "on".into())];
    match config.get("search_path").or_else(|| config.get("schema")) {
        Some(Value::Array(paths)) => settings.push((
            "search_path".into(),
            paths
                .iter()
                .map(|p| p.to_string_lossy())
                .collect::<Vec<_>>()
                .join(","),
        )),
        Some(Value::String(path)) if !path.is_empty() => {
            settings.push(("search_path".into(), path.clone()))
        }
        _ => {}
    }
    if let Some(timezone) = config_str(config, "timezone") {
        settings.push(("TimeZone".into(), timezone));
    }
    if let Some(name) = config_str(config, "application_name") {
        options = options.application_name(&name);
    }
    options = options.options(settings);

    let pool = PgPoolOptions::new()
        .max_connections(pool_size(config, 10))
        .acquire_timeout(Duration::from_secs(30))
        .connect_lazy_with(options);

    Ok(Pool::Postgres(pool))
}

// ----------------------------------------------------------------------
// Client-side binding (MySQL / PostgreSQL)
// ----------------------------------------------------------------------

/// Quote a value as a SQL literal for the given driver.
///
/// This is the escaping used for client-side ("emulated") binding: strings
/// are quoted, numbers and booleans are written as literals, and arrays /
/// objects are stored as JSON text.
pub(crate) fn quote_literal(driver: Driver, value: &Value) -> Result<String, String> {
    Ok(match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => match driver {
            Driver::Postgres => if *b { "true" } else { "false" }.to_string(),
            _ => if *b { "1" } else { "0" }.to_string(),
        },
        Value::Number(n) => match driver {
            // Untyped literals let PostgreSQL infer the type from context.
            Driver::Postgres => format!("'{}'", number_literal(n)),
            _ => number_literal(n),
        },
        Value::String(s) => quote_string(driver, s)?,
        other => quote_string(driver, &other.to_string())?,
    })
}

fn number_literal(n: &Number) -> String {
    if let Some(f) = n.as_f64().filter(|_| n.is_f64())
        && f.is_finite()
    {
        return format!("{f}");
    }
    n.to_string()
}

/// Quote a string literal for the given driver.
pub(crate) fn quote_string(driver: Driver, value: &str) -> Result<String, String> {
    if value.contains('\0') {
        return Err(
            "Strings with null bytes cannot be escaped. Use the binary escape option.".into(),
        );
    }
    Ok(match driver {
        Driver::Postgres => {
            if value.contains('\\') {
                // An escape string is immune to the `standard_conforming_strings` setting.
                format!("E'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
            } else {
                format!("'{}'", value.replace('\'', "''"))
            }
        }
        Driver::MySql | Driver::MariaDb => {
            // Doubled quotes are safe whether or not NO_BACKSLASH_ESCAPES is on.
            let mut out = String::with_capacity(value.len() + 2);
            out.push('\'');
            for ch in value.chars() {
                match ch {
                    '\\' => out.push_str("\\\\"),
                    '\'' => out.push_str("''"),
                    '\u{1a}' => out.push_str("\\Z"),
                    other => out.push(other),
                }
            }
            out.push('\'');
            out
        }
        Driver::Sqlite => format!("'{}'", value.replace('\'', "''")),
    })
}

/// Substitute bindings for the `?` placeholders of a query, skipping
/// placeholders inside quoted strings and identifiers.
pub(crate) fn interpolate(
    sql: &str,
    bindings: &[Value],
    mut literal: impl FnMut(&Value) -> Result<String, String>,
    unescape_double_question: bool,
) -> Result<String, String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len() + bindings.len() * 4);
    let mut quote: Option<char> = None;
    let mut index = 0;
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied();

        if let Some(q) = quote {
            out.push(ch);
            if ch == '\\'
                && q == '\''
                && let Some(n) = next
            {
                out.push(n);
                i += 2;
                continue;
            }
            if ch == q {
                if next == Some(q) {
                    out.push(q);
                    i += 2;
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }

        match ch {
            '\'' | '"' | '`' => {
                quote = Some(ch);
                out.push(ch);
            }
            '?' if next == Some('?') => {
                if unescape_double_question {
                    out.push('?');
                } else {
                    out.push_str("??");
                }
                i += 2;
                continue;
            }
            '?' => match bindings.get(index) {
                Some(value) => {
                    out.push_str(&literal(value)?);
                    index += 1;
                }
                None => out.push('?'),
            },
            other => out.push(other),
        }
        i += 1;
    }

    Ok(out)
}

// ----------------------------------------------------------------------
// Executing
// ----------------------------------------------------------------------

fn bind_sqlite<'q>(
    mut query: sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    bindings: &[Value],
) -> sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    for value in bindings {
        query = match value {
            Value::Null => query.bind(None::<String>),
            Value::Bool(b) => query.bind(*b as i64),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    query.bind(i)
                } else if let Some(u) = n.as_u64() {
                    query.bind(u.to_string())
                } else {
                    query.bind(n.as_f64().unwrap_or_default())
                }
            }
            Value::String(s) => query.bind(s.clone()),
            other => query.bind(other.to_string()),
        };
    }
    query
}

pub(crate) fn sqlite_fetch<'a, E>(
    executor: E,
    sql: &'a str,
    bindings: &'a [Value],
) -> BoxFuture<'a, Result<Vec<Value>, sqlx::Error>>
where
    E: Executor<'a, Database = Sqlite> + 'a,
{
    Box::pin(async move {
        let rows = bind_sqlite(sqlx::query(sql), bindings)
            .fetch_all(executor)
            .await?;
        Ok(rows.iter().map(decode_sqlite_row).collect())
    })
}

pub(crate) fn sqlite_execute<'a, E>(
    executor: E,
    sql: &'a str,
    bindings: &'a [Value],
) -> BoxFuture<'a, Result<Affected, sqlx::Error>>
where
    E: Executor<'a, Database = Sqlite> + 'a,
{
    Box::pin(async move {
        let result = bind_sqlite(sqlx::query(sql), bindings)
            .execute(executor)
            .await?;
        Ok(Affected {
            rows: result.rows_affected(),
            last_insert_id: Some(result.last_insert_rowid()),
        })
    })
}

pub(crate) fn sqlite_unprepared<'a, E>(
    executor: E,
    sql: &'a str,
) -> BoxFuture<'a, Result<Affected, sqlx::Error>>
where
    E: Executor<'a, Database = Sqlite> + 'a,
{
    Box::pin(async move {
        let result = executor.execute(sqlx::raw_sql(sql)).await?;
        Ok(Affected {
            rows: result.rows_affected(),
            last_insert_id: Some(result.last_insert_rowid()),
        })
    })
}

pub(crate) fn mysql_fetch<'a, E>(
    executor: E,
    sql: &'a str,
) -> BoxFuture<'a, Result<Vec<Value>, sqlx::Error>>
where
    E: Executor<'a, Database = MySql> + 'a,
{
    Box::pin(async move {
        let rows = executor.fetch_all(sqlx::raw_sql(sql)).await?;
        Ok(rows.iter().map(decode_mysql_row).collect())
    })
}

pub(crate) fn mysql_execute<'a, E>(
    executor: E,
    sql: &'a str,
) -> BoxFuture<'a, Result<Affected, sqlx::Error>>
where
    E: Executor<'a, Database = MySql> + 'a,
{
    Box::pin(async move {
        let result = executor.execute(sqlx::raw_sql(sql)).await?;
        Ok(Affected {
            rows: result.rows_affected(),
            last_insert_id: Some(result.last_insert_id() as i64),
        })
    })
}

pub(crate) fn postgres_fetch<'a, E>(
    executor: E,
    sql: &'a str,
) -> BoxFuture<'a, Result<Vec<Value>, sqlx::Error>>
where
    E: Executor<'a, Database = Postgres> + 'a,
{
    Box::pin(async move {
        let rows = executor.fetch_all(sqlx::raw_sql(sql)).await?;
        Ok(rows.iter().map(decode_postgres_row).collect())
    })
}

pub(crate) fn postgres_execute<'a, E>(
    executor: E,
    sql: &'a str,
) -> BoxFuture<'a, Result<Affected, sqlx::Error>>
where
    E: Executor<'a, Database = Postgres> + 'a,
{
    Box::pin(async move {
        let result = executor.execute(sqlx::raw_sql(sql)).await?;
        Ok(Affected {
            rows: result.rows_affected(),
            last_insert_id: None,
        })
    })
}

// ----------------------------------------------------------------------
// Decoding rows
// ----------------------------------------------------------------------

fn float_value(f: f64) -> Value {
    Number::from_f64(f)
        .map(Value::Number)
        .unwrap_or_else(|| Value::String(f.to_string()))
}

/// Represent raw bytes: valid UTF-8 becomes a string, anything else a list of bytes.
fn bytes_value(bytes: Vec<u8>) -> Value {
    match String::from_utf8(bytes) {
        Ok(s) => Value::String(s),
        Err(e) => Value::Array(e.into_bytes().into_iter().map(Value::from).collect()),
    }
}

/// Decode a SQLite row into an object, preserving column order.
pub(crate) fn decode_sqlite_row(row: &SqliteRow) -> Value {
    let mut map = Map::new();
    for column in row.columns() {
        map.insert(
            column.name().to_string(),
            decode_sqlite_value(row, column.ordinal()),
        );
    }
    Value::Object(map)
}

fn sqlite_decode<'r, T: Decode<'r, Sqlite>>(row: &'r SqliteRow, index: usize) -> Option<T> {
    let raw = row.try_get_raw(index).ok()?;
    T::decode(raw).ok()
}

fn decode_sqlite_value(row: &SqliteRow, index: usize) -> Value {
    let kind = match row.try_get_raw(index) {
        Ok(raw) if raw.is_null() => return Value::Null,
        Ok(raw) => raw.type_info().name().to_string(),
        Err(_) => return Value::Null,
    };
    match kind.as_str() {
        "INTEGER" => {
            if let Some(i) = sqlite_decode::<i64>(row, index) {
                return Value::from(i);
            }
        }
        "REAL" => {
            if let Some(f) = sqlite_decode::<f64>(row, index) {
                return float_value(f);
            }
        }
        "TEXT" => {
            if let Some(s) = sqlite_decode::<String>(row, index) {
                return Value::String(s);
            }
        }
        "BLOB" => {
            if let Some(b) = sqlite_decode::<Vec<u8>>(row, index) {
                return bytes_value(b);
            }
        }
        _ => {}
    }
    // Leniently try each representation in turn.
    if let Some(i) = sqlite_decode::<i64>(row, index) {
        return Value::from(i);
    }
    if let Some(f) = sqlite_decode::<f64>(row, index) {
        return float_value(f);
    }
    if let Some(s) = sqlite_decode::<String>(row, index) {
        return Value::String(s);
    }
    if let Some(b) = sqlite_decode::<bool>(row, index) {
        return Value::Bool(b);
    }
    if let Some(b) = sqlite_decode::<Vec<u8>>(row, index) {
        return bytes_value(b);
    }
    Value::Null
}

/// Decode a MySQL row (text protocol) into an object.
pub(crate) fn decode_mysql_row(row: &MySqlRow) -> Value {
    let mut map = Map::new();
    for column in row.columns() {
        let value = match row.try_get_raw(column.ordinal()) {
            Ok(raw) if raw.is_null() => Value::Null,
            Ok(raw) => {
                let kind = raw.type_info().name().to_ascii_uppercase();
                match <&[u8] as Decode<MySql>>::decode(raw) {
                    Ok(bytes) => decode_text_value(&kind, bytes, false),
                    Err(_) => Value::Null,
                }
            }
            Err(_) => Value::Null,
        };
        map.insert(column.name().to_string(), value);
    }
    Value::Object(map)
}

/// Decode a PostgreSQL row (simple query protocol: text format) into an object.
pub(crate) fn decode_postgres_row(row: &PgRow) -> Value {
    let mut map = Map::new();
    for column in row.columns() {
        let value = match row.try_get_raw(column.ordinal()) {
            Ok(raw) if raw.is_null() => Value::Null,
            Ok(raw) => {
                let kind = raw.type_info().name().to_ascii_uppercase();
                match raw.as_bytes() {
                    Ok(bytes) => decode_text_value(&kind, bytes, true),
                    Err(_) => Value::Null,
                }
            }
            Err(_) => Value::Null,
        };
        map.insert(column.name().to_string(), value);
    }
    Value::Object(map)
}

/// Decode a value returned in text format, guided by its type name.
fn decode_text_value(kind: &str, bytes: &[u8], postgres: bool) -> Value {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return bytes_value(bytes.to_vec()),
    };

    let integer = matches!(
        kind,
        "INT2"
            | "INT4"
            | "INT8"
            | "OID"
            | "SMALLINT"
            | "INT"
            | "INTEGER"
            | "BIGINT"
            | "TINYINT"
            | "MEDIUMINT"
            | "YEAR"
            | "BOOLEAN"
            | "SMALLSERIAL"
            | "SERIAL"
            | "BIGSERIAL"
    ) || (kind.ends_with("INT UNSIGNED")
        || kind.starts_with("TINYINT")
        || kind.starts_with("SMALLINT")
        || kind.starts_with("MEDIUMINT")
        || kind.starts_with("BIGINT")
        || kind.starts_with("INT "));

    if postgres && kind == "BOOL" {
        return Value::Bool(text == "t" || text == "true");
    }
    if integer {
        if let Ok(i) = text.parse::<i64>() {
            return Value::from(i);
        }
        if let Ok(u) = text.parse::<u64>() {
            return Value::from(u);
        }
    }
    if (matches!(kind, "FLOAT4" | "FLOAT8" | "FLOAT" | "DOUBLE" | "REAL")
        || kind.starts_with("FLOAT")
        || kind.starts_with("DOUBLE"))
        && let Ok(f) = text.parse::<f64>()
    {
        return float_value(f);
    }
    if postgres
        && kind == "BYTEA"
        && let Some(hex) = text.strip_prefix("\\x")
        && let Some(bytes) = decode_hex(hex)
    {
        return bytes_value(bytes);
    }
    Value::String(text.to_string())
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn drivers_parse_from_config_names() {
        assert_eq!(Driver::from_name("pgsql"), Some(Driver::Postgres));
        assert_eq!(Driver::from_name("mariadb"), Some(Driver::MariaDb));
        assert_eq!(Driver::from_name("sqlsrv"), None);
        assert!(Driver::MariaDb.is_mysql_family());
    }

    #[test]
    fn strings_are_quoted_per_driver() {
        assert_eq!(
            quote_string(Driver::Sqlite, "O'Brien").unwrap(),
            "'O''Brien'"
        );
        assert_eq!(
            quote_string(Driver::Postgres, "O'Brien").unwrap(),
            "'O''Brien'"
        );
        assert_eq!(
            quote_string(Driver::Postgres, "a\\'b").unwrap(),
            "E'a\\\\''b'"
        );
        assert_eq!(quote_string(Driver::MySql, "a\\'b").unwrap(), "'a\\\\''b'");
        assert!(quote_string(Driver::MySql, "a\0b").is_err());
    }

    #[test]
    fn literals_are_typed_per_driver() {
        assert_eq!(
            quote_literal(Driver::Postgres, &json!(true)).unwrap(),
            "true"
        );
        assert_eq!(quote_literal(Driver::MySql, &json!(true)).unwrap(), "1");
        assert_eq!(quote_literal(Driver::Postgres, &json!(5)).unwrap(), "'5'");
        assert_eq!(quote_literal(Driver::MySql, &json!(5)).unwrap(), "5");
        assert_eq!(quote_literal(Driver::MySql, &json!(null)).unwrap(), "NULL");
        assert_eq!(
            quote_literal(Driver::MySql, &json!({"a": 1})).unwrap(),
            "'{\"a\":1}'"
        );
    }

    #[test]
    fn interpolation_skips_quoted_placeholders() {
        let sql = "select * from \"a?\" where x = ? and y = '?' and z = ?";
        let out = interpolate(
            sql,
            &[json!(1), json!("b")],
            |v| quote_literal(Driver::Sqlite, v),
            true,
        )
        .unwrap();
        assert_eq!(
            out,
            "select * from \"a?\" where x = 1 and y = '?' and z = 'b'"
        );

        let out = interpolate(
            "a ?? b and c = ?",
            &[json!(1)],
            |v| quote_literal(Driver::Postgres, v),
            true,
        )
        .unwrap();
        assert_eq!(out, "a ? b and c = '1'");
    }

    #[test]
    fn text_values_decode_by_type() {
        assert_eq!(decode_text_value("INT8", b"42", true), json!(42));
        assert_eq!(decode_text_value("BOOL", b"t", true), json!(true));
        assert_eq!(decode_text_value("FLOAT8", b"1.5", true), json!(1.5));
        assert_eq!(decode_text_value("NUMERIC", b"1.50", true), json!("1.50"));
        assert_eq!(decode_text_value("BIGINT UNSIGNED", b"7", false), json!(7));
        assert_eq!(decode_text_value("BYTEA", b"\\x6869", true), json!("hi"));
        assert_eq!(
            decode_text_value("TIMESTAMP", b"2024-01-01 00:00:00", true),
            json!("2024-01-01 00:00:00")
        );
    }
}
