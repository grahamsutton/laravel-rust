//! The schema builder: creating, altering and inspecting tables.
//!
//! ```no_run
//! # async fn example() -> illuminate_support::Result<()> {
//! use illuminate_database::Schema;
//!
//! Schema::create("flights", |table| {
//!     table.id();
//!     table.string("name");
//!     table.string("airline");
//!     table.timestamps();
//! })
//! .await?;
//! # Ok(()) }
//! ```

mod blueprint;
mod grammar;
mod state;

use std::future::Future;

use illuminate_support::{Result, Value, ValueExt};

pub use blueprint::{
    Blueprint, ColumnAttributes, ColumnDefault, ColumnDefinition, Command, CommandAttributes, CommandDefinition,
    ForeignKeyDefinition, IndexDefinition, IndexFlag, default_string_length, set_default_morph_key_type,
    set_default_string_length,
};
pub use grammar::SchemaGrammar;
pub use state::TableState;

use crate::connection::Connection;
use crate::driver::{self, Driver};
use crate::manager::DatabaseManager;

/// Types that can be used as a list of column names.
pub trait IntoColumnNames {
    /// Convert into column names.
    fn into_column_names(self) -> Vec<String>;
}

impl IntoColumnNames for &str {
    fn into_column_names(self) -> Vec<String> {
        vec![self.to_string()]
    }
}

impl IntoColumnNames for String {
    fn into_column_names(self) -> Vec<String> {
        vec![self]
    }
}

impl<T: Into<String>> IntoColumnNames for Vec<T> {
    fn into_column_names(self) -> Vec<String> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T: Into<String>, const N: usize> IntoColumnNames for [T; N] {
    fn into_column_names(self) -> Vec<String> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T: Into<String> + Clone> IntoColumnNames for &[T] {
    fn into_column_names(self) -> Vec<String> {
        self.iter().cloned().map(Into::into).collect()
    }
}

/// An index referenced by name, or by the columns it covers (in which case
/// Laravel's conventional name is generated).
#[derive(Clone, Debug, PartialEq)]
pub enum IndexName {
    /// The index's name.
    Name(String),
    /// The index's columns.
    Columns(Vec<String>),
}

impl From<&str> for IndexName {
    fn from(value: &str) -> Self {
        IndexName::Name(value.to_string())
    }
}

impl From<String> for IndexName {
    fn from(value: String) -> Self {
        IndexName::Name(value)
    }
}

impl<T: Into<String>> From<Vec<T>> for IndexName {
    fn from(value: Vec<T>) -> Self {
        IndexName::Columns(value.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<String>, const N: usize> From<[T; N]> for IndexName {
    fn from(value: [T; N]) -> Self {
        IndexName::Columns(value.into_iter().map(Into::into).collect())
    }
}

/// A table, as reported by [`SchemaBuilder::get_tables`].
#[derive(Clone, Debug, PartialEq)]
pub struct TableInfo {
    pub name: String,
    pub schema: Option<String>,
    pub schema_qualified_name: String,
    pub size: Option<i64>,
    pub comment: Option<String>,
    pub collation: Option<String>,
    pub engine: Option<String>,
}

/// A column, as reported by [`SchemaBuilder::get_columns`].
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnInfo {
    pub name: String,
    /// The type's name (`varchar`, `integer`, ...).
    pub type_name: String,
    /// The full type definition (`varchar(255)`).
    pub type_: String,
    pub collation: Option<String>,
    pub nullable: bool,
    pub default: Option<String>,
    pub auto_increment: bool,
    pub comment: Option<String>,
}

/// An index, as reported by [`SchemaBuilder::get_indexes`].
#[derive(Clone, Debug, PartialEq)]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub type_: Option<String>,
    pub unique: bool,
    pub primary: bool,
}

/// A foreign key, as reported by [`SchemaBuilder::get_foreign_keys`].
#[derive(Clone, Debug, PartialEq)]
pub struct ForeignKeyInfo {
    pub name: Option<String>,
    pub columns: Vec<String>,
    pub foreign_schema: Option<String>,
    pub foreign_table: String,
    pub foreign_columns: Vec<String>,
    pub on_update: String,
    pub on_delete: String,
}

fn string(row: &Value, key: &str) -> String {
    row.get(key).map(|v| v.to_string_lossy()).unwrap_or_default()
}

fn optional_string(row: &Value, key: &str) -> Option<String> {
    match row.get(key) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.to_string_lossy()).filter(|s| !s.is_empty()),
    }
}

fn boolean(row: &Value, key: &str) -> bool {
    match row.get(key) {
        Some(Value::String(s)) => matches!(s.to_lowercase().as_str(), "1" | "t" | "true" | "yes"),
        Some(v) => v.truthy(),
        None => false,
    }
}

fn split_list(value: String) -> Vec<String> {
    if value.is_empty() {
        Vec::new()
    } else {
        value.split(',').map(String::from).collect()
    }
}

fn foreign_action(driver: Driver, value: String) -> String {
    if driver == Driver::Postgres {
        return match value.as_str() {
            "a" => "no action",
            "r" => "restrict",
            "c" => "cascade",
            "n" => "set null",
            "d" => "set default",
            other => other,
        }
        .to_string();
    }
    value.to_lowercase()
}

/// The schema builder for a connection.
#[derive(Clone, Debug)]
pub struct SchemaBuilder {
    connection: Connection,
}

impl SchemaBuilder {
    /// Create a schema builder for the connection.
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    /// Get the database connection instance.
    pub fn get_connection(&self) -> &Connection {
        &self.connection
    }

    fn grammar(&self) -> SchemaGrammar {
        self.connection.schema_grammar()
    }

    fn prefixed(&self, table: &str) -> (Option<String>, String) {
        let (schema, table) = SchemaGrammar::parse_schema_and_table(table);
        (schema, format!("{}{table}", self.connection.get_table_prefix()))
    }

    /// Create a new blueprint for the table (index names honour `prefix_indexes`).
    pub fn blueprint(&self, table: &str) -> Blueprint {
        let index_prefix = if self.connection.get_config("prefix_indexes").truthy() {
            self.connection.get_table_prefix()
        } else {
            String::new()
        };
        Blueprint::with_prefix(table, &index_prefix)
    }

    /// Execute the blueprint against the database.
    pub async fn build(&self, mut blueprint: Blueprint) -> Result<()> {
        let grammar = self.grammar();
        blueprint.add_implied_commands(&grammar);
        let state = if blueprint.needs_state(&grammar) {
            Some(self.table_state(blueprint.get_table()).await?)
        } else {
            None
        };
        for statement in blueprint.to_sql_with_state(&grammar, state)? {
            self.connection.statement(&statement, ()).await?;
        }
        Ok(())
    }

    async fn table_state(&self, table: &str) -> Result<TableState> {
        let columns = self.get_columns(table).await?;
        let indexes = self.get_indexes(table).await?;
        let foreign_keys = self.get_foreign_keys(table).await?;
        let enabled = self
            .connection
            .scalar("pragma foreign_keys", ())
            .await?
            .truthy();
        Ok(TableState::new(columns, indexes, foreign_keys, enabled))
    }

    /// Create a new table on the schema.
    pub async fn create(&self, table: &str, callback: impl FnOnce(&mut Blueprint)) -> Result<()> {
        let mut blueprint = self.blueprint(table);
        blueprint.create();
        callback(&mut blueprint);
        self.build(blueprint).await
    }

    /// Modify a table on the schema.
    pub async fn table(&self, table: &str, callback: impl FnOnce(&mut Blueprint)) -> Result<()> {
        let mut blueprint = self.blueprint(table);
        callback(&mut blueprint);
        self.build(blueprint).await
    }

    /// Drop a table from the schema.
    pub async fn drop(&self, table: &str) -> Result<()> {
        let mut blueprint = self.blueprint(table);
        blueprint.drop();
        self.build(blueprint).await
    }

    /// Drop a table from the schema if it exists.
    pub async fn drop_if_exists(&self, table: &str) -> Result<()> {
        let mut blueprint = self.blueprint(table);
        blueprint.drop_if_exists();
        self.build(blueprint).await
    }

    /// Drop columns from a table.
    pub async fn drop_columns(&self, table: &str, columns: impl IntoColumnNames) -> Result<()> {
        let mut blueprint = self.blueprint(table);
        blueprint.drop_column(columns);
        self.build(blueprint).await
    }

    /// Rename a table on the schema.
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let mut blueprint = self.blueprint(from);
        blueprint.rename(to);
        self.build(blueprint).await
    }

    /// Determine if the given table exists.
    pub async fn has_table(&self, table: &str) -> Result<bool> {
        let (schema, table) = self.prefixed(table);
        let sql = self.grammar().compile_table_exists(schema.as_deref(), &table);
        Ok(boolean(
            &serde_json::json!({"exists": self.connection.scalar(&sql, ()).await?}),
            "exists",
        ))
    }

    /// Determine if the given view exists.
    pub async fn has_view(&self, view: &str) -> Result<bool> {
        let (_, view) = self.prefixed(view);
        Ok(self
            .get_views()
            .await?
            .iter()
            .any(|v| string(v, "name").eq_ignore_ascii_case(&view)))
    }

    /// Get the tables of the current schema.
    pub async fn get_tables(&self) -> Result<Vec<TableInfo>> {
        let rows = self.connection.select(&self.grammar().compile_tables(), ()).await?;
        Ok(rows
            .iter()
            .map(|row| {
                let name = string(row, "name");
                let schema = optional_string(row, "schema");
                TableInfo {
                    schema_qualified_name: match &schema {
                        Some(schema) => format!("{schema}.{name}"),
                        None => name.clone(),
                    },
                    name,
                    schema,
                    size: row.get("size").and_then(|s| s.to_i64_lossy()),
                    comment: optional_string(row, "comment"),
                    collation: optional_string(row, "collation"),
                    engine: optional_string(row, "engine"),
                }
            })
            .collect())
    }

    /// Get the names of the tables of the current schema.
    pub async fn get_table_listing(&self) -> Result<Vec<String>> {
        Ok(self.get_tables().await?.into_iter().map(|t| t.name).collect())
    }

    /// Get the views of the current schema.
    pub async fn get_views(&self) -> Result<Vec<Value>> {
        self.connection.select(&self.grammar().compile_views(), ()).await
    }

    /// Get the columns of a table.
    pub async fn get_columns(&self, table: &str) -> Result<Vec<ColumnInfo>> {
        let (schema, table) = self.prefixed(table);
        let driver = self.connection.driver();
        let rows = self
            .connection
            .select(&self.grammar().compile_columns(schema.as_deref(), &table), ())
            .await?;

        let sqlite_single_primary =
            rows.iter().filter(|r| r.get("primary").and_then(|p| p.to_i64_lossy()).unwrap_or(0) > 0).count() == 1;

        Ok(rows
            .iter()
            .map(|row| match driver {
                Driver::Sqlite => {
                    let type_ = string(row, "type").to_lowercase();
                    let type_name = type_.split('(').next().unwrap_or_default().to_string();
                    let primary = row.get("primary").and_then(|p| p.to_i64_lossy()).unwrap_or(0) > 0;
                    ColumnInfo {
                        name: string(row, "name"),
                        auto_increment: sqlite_single_primary && primary && type_ == "integer",
                        type_name,
                        type_,
                        collation: None,
                        nullable: boolean(row, "nullable"),
                        default: optional_string(row, "default"),
                        comment: None,
                    }
                }
                Driver::MySql | Driver::MariaDb => ColumnInfo {
                    name: string(row, "name"),
                    type_name: string(row, "type_name"),
                    type_: string(row, "type"),
                    collation: optional_string(row, "collation"),
                    nullable: string(row, "nullable") == "YES",
                    default: optional_string(row, "default"),
                    auto_increment: string(row, "extra") == "auto_increment",
                    comment: optional_string(row, "comment"),
                },
                Driver::Postgres => {
                    let default = optional_string(row, "default");
                    ColumnInfo {
                        name: string(row, "name"),
                        type_name: string(row, "type_name"),
                        type_: string(row, "type"),
                        collation: optional_string(row, "collation"),
                        nullable: boolean(row, "nullable"),
                        auto_increment: default.as_deref().is_some_and(|d| d.starts_with("nextval(")),
                        default,
                        comment: optional_string(row, "comment"),
                    }
                }
            })
            .collect())
    }

    /// Get the column names of a table.
    pub async fn get_column_listing(&self, table: &str) -> Result<Vec<String>> {
        Ok(self.get_columns(table).await?.into_iter().map(|c| c.name).collect())
    }

    /// Determine if the given table has a given column.
    pub async fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        let column = column.to_lowercase();
        Ok(self
            .get_column_listing(table)
            .await?
            .iter()
            .any(|c| c.to_lowercase() == column))
    }

    /// Determine if the given table has all of the given columns.
    pub async fn has_columns(&self, table: &str, columns: impl IntoColumnNames) -> Result<bool> {
        let listing: Vec<String> = self
            .get_column_listing(table)
            .await?
            .into_iter()
            .map(|c| c.to_lowercase())
            .collect();
        Ok(columns
            .into_column_names()
            .iter()
            .all(|c| listing.contains(&c.to_lowercase())))
    }

    /// Get the data type of a column (`varchar`, `integer`, ...).
    pub async fn get_column_type(&self, table: &str, column: &str) -> Result<String> {
        self.get_columns(table)
            .await?
            .into_iter()
            .find(|c| c.name.eq_ignore_ascii_case(column))
            .map(|c| c.type_name)
            .ok_or_else(|| {
                illuminate_support::error::InvalidArgumentException::new(format!(
                    "There is no column with name '{column}' on table '{table}'."
                ))
                .into()
            })
    }

    /// Get the indexes of a table.
    pub async fn get_indexes(&self, table: &str) -> Result<Vec<IndexInfo>> {
        let (schema, table) = self.prefixed(table);
        let driver = self.connection.driver();
        let rows = self
            .connection
            .select(&self.grammar().compile_indexes(schema.as_deref(), &table), ())
            .await?;
        let mut indexes: Vec<IndexInfo> = rows
            .iter()
            .map(|row| {
                let name = string(row, "name").to_lowercase();
                IndexInfo {
                    columns: split_list(string(row, "columns")),
                    type_: optional_string(row, "type").map(|t| t.to_lowercase()),
                    unique: boolean(row, "unique"),
                    primary: match driver {
                        Driver::MySql | Driver::MariaDb => name == "primary",
                        _ => boolean(row, "primary"),
                    },
                    name,
                }
            })
            .collect();
        if driver == Driver::Sqlite && indexes.iter().filter(|i| i.primary).count() > 1 {
            indexes.retain(|i| i.name != "primary");
        }
        Ok(indexes)
    }

    /// Get the names of the indexes of a table.
    pub async fn get_index_listing(&self, table: &str) -> Result<Vec<String>> {
        Ok(self.get_indexes(table).await?.into_iter().map(|i| i.name).collect())
    }

    /// Determine if the table has an index (by name, or by its columns),
    /// optionally of a given type (`primary`, `unique`, ...).
    pub async fn has_index(&self, table: &str, index: impl Into<IndexName>, kind: Option<&str>) -> Result<bool> {
        let index = index.into();
        let kind = kind.map(|k| k.to_lowercase());
        Ok(self.get_indexes(table).await?.iter().any(|value| {
            let type_matches = match kind.as_deref() {
                None => true,
                Some("primary") => value.primary,
                Some("unique") => value.unique,
                Some(other) => value.type_.as_deref() == Some(other),
            };
            let name_matches = match &index {
                IndexName::Name(name) => value.name == name.to_lowercase(),
                IndexName::Columns(columns) => &value.columns == columns,
            };
            name_matches && type_matches
        }))
    }

    /// Get the foreign keys of a table.
    pub async fn get_foreign_keys(&self, table: &str) -> Result<Vec<ForeignKeyInfo>> {
        let (schema, table) = self.prefixed(table);
        let driver = self.connection.driver();
        let rows = self
            .connection
            .select(&self.grammar().compile_foreign_keys(schema.as_deref(), &table), ())
            .await?;
        Ok(rows
            .iter()
            .map(|row| ForeignKeyInfo {
                name: optional_string(row, "name"),
                columns: split_list(string(row, "columns")),
                foreign_schema: optional_string(row, "foreign_schema"),
                foreign_table: string(row, "foreign_table"),
                foreign_columns: split_list(string(row, "foreign_columns")),
                on_update: foreign_action(driver, string(row, "on_update")),
                on_delete: foreign_action(driver, string(row, "on_delete")),
            })
            .collect())
    }

    /// Determine if the table has a foreign key (by name, or by its columns).
    pub async fn has_foreign_key(&self, table: &str, key: impl Into<IndexName>) -> Result<bool> {
        let key = key.into();
        Ok(self.get_foreign_keys(table).await?.iter().any(|fk| match &key {
            IndexName::Name(name) => fk.name.as_deref() == Some(name.as_str()),
            IndexName::Columns(columns) => &fk.columns == columns,
        }))
    }

    /// Drop all tables from the database.
    pub async fn drop_all_tables(&self) -> Result<()> {
        let mut tables = self.get_table_listing().await?;
        if self.connection.driver() == Driver::Postgres {
            let excluded: Vec<String> = match self.connection.get_config("dont_drop") {
                Value::Array(items) => items.iter().map(|i| i.to_string_lossy()).collect(),
                _ => vec!["spatial_ref_sys".to_string()],
            };
            tables.retain(|t| !excluded.contains(t));
        }
        if tables.is_empty() {
            return Ok(());
        }
        let grammar = self.grammar();
        match self.connection.driver() {
            Driver::Sqlite => {
                self.disable_foreign_key_constraints().await?;
                for table in &tables {
                    self.connection
                        .statement(&grammar.compile_drop_all_tables(std::slice::from_ref(table)), ())
                        .await?;
                }
                self.enable_foreign_key_constraints().await?;
            }
            Driver::MySql | Driver::MariaDb => {
                self.disable_foreign_key_constraints().await?;
                let result = self
                    .connection
                    .statement(&grammar.compile_drop_all_tables(&tables), ())
                    .await;
                self.enable_foreign_key_constraints().await?;
                result?;
            }
            Driver::Postgres => {
                self.connection
                    .statement(&grammar.compile_drop_all_tables(&tables), ())
                    .await?;
            }
        }
        Ok(())
    }

    /// Drop all views from the database.
    pub async fn drop_all_views(&self) -> Result<()> {
        let views: Vec<String> = self.get_views().await?.iter().map(|v| string(v, "name")).collect();
        if views.is_empty() {
            return Ok(());
        }
        let grammar = self.grammar();
        if self.connection.driver() == Driver::Sqlite {
            for view in &views {
                self.connection
                    .statement(&grammar.compile_drop_all_views(std::slice::from_ref(view)), ())
                    .await?;
            }
            return Ok(());
        }
        self.connection
            .statement(&grammar.compile_drop_all_views(&views), ())
            .await?;
        Ok(())
    }

    /// Enable foreign key constraints.
    pub async fn enable_foreign_key_constraints(&self) -> Result<bool> {
        self.connection
            .statement(&self.grammar().compile_enable_foreign_key_constraints(), ())
            .await
    }

    /// Disable foreign key constraints.
    pub async fn disable_foreign_key_constraints(&self) -> Result<bool> {
        self.connection
            .statement(&self.grammar().compile_disable_foreign_key_constraints(), ())
            .await
    }

    /// Run the callback with foreign key constraints disabled.
    pub async fn without_foreign_key_constraints<F, Fut, T>(&self, callback: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.disable_foreign_key_constraints().await?;
        let result = callback().await;
        self.enable_foreign_key_constraints().await?;
        result
    }

    /// Create a database (a file for SQLite).
    pub async fn create_database(&self, name: &str) -> Result<bool> {
        match self.connection.driver() {
            Driver::Sqlite => Ok(std::fs::write(name, b"").is_ok()),
            _ => {
                let wrapped = self.connection.query_grammar().wrap_value(name);
                self.connection.statement(&format!("create database {wrapped}"), ()).await
            }
        }
    }

    /// Drop a database if it exists (deleting the file for SQLite).
    pub async fn drop_database_if_exists(&self, name: &str) -> Result<bool> {
        match self.connection.driver() {
            Driver::Sqlite => {
                if driver::is_memory_database(name) || !std::path::Path::new(name).exists() {
                    return Ok(true);
                }
                Ok(std::fs::remove_file(name).is_ok())
            }
            _ => {
                let wrapped = self.connection.query_grammar().wrap_value(name);
                self.connection
                    .statement(&format!("drop database if exists {wrapped}"), ())
                    .await
            }
        }
    }
}

/// The `Schema` facade: the schema builder of the default connection.
pub struct Schema;

impl Schema {
    /// Get a schema builder instance for a connection.
    pub fn connection(name: &str) -> SchemaBuilder {
        DatabaseManager::resolve().connection(name).get_schema_builder()
    }

    fn builder() -> SchemaBuilder {
        DatabaseManager::resolve().default_connection().get_schema_builder()
    }

    /// Set the default string length for migrations.
    pub fn default_string_length(length: u32) {
        set_default_string_length(length);
    }

    /// Set the default morph key type (`int`, `uuid` or `ulid`).
    pub fn default_morph_key_type(kind: &str) -> Result<()> {
        set_default_morph_key_type(kind)
    }

    /// Use UUIDs for polymorphic relation keys.
    pub fn morph_using_uuids() {
        let _ = set_default_morph_key_type("uuid");
    }

    /// Use ULIDs for polymorphic relation keys.
    pub fn morph_using_ulids() {
        let _ = set_default_morph_key_type("ulid");
    }

    /// Create a new table on the schema.
    pub async fn create(table: &str, callback: impl FnOnce(&mut Blueprint)) -> Result<()> {
        Self::builder().create(table, callback).await
    }

    /// Modify a table on the schema.
    pub async fn table(table: &str, callback: impl FnOnce(&mut Blueprint)) -> Result<()> {
        Self::builder().table(table, callback).await
    }

    /// Drop a table from the schema.
    pub async fn drop(table: &str) -> Result<()> {
        Self::builder().drop(table).await
    }

    /// Drop a table from the schema if it exists.
    pub async fn drop_if_exists(table: &str) -> Result<()> {
        Self::builder().drop_if_exists(table).await
    }

    /// Drop columns from a table.
    pub async fn drop_columns(table: &str, columns: impl IntoColumnNames) -> Result<()> {
        Self::builder().drop_columns(table, columns).await
    }

    /// Rename a table on the schema.
    pub async fn rename(from: &str, to: &str) -> Result<()> {
        Self::builder().rename(from, to).await
    }

    /// Determine if the given table exists.
    pub async fn has_table(table: &str) -> Result<bool> {
        Self::builder().has_table(table).await
    }

    /// Determine if the given table has a given column.
    pub async fn has_column(table: &str, column: &str) -> Result<bool> {
        Self::builder().has_column(table, column).await
    }

    /// Determine if the given table has all of the given columns.
    pub async fn has_columns(table: &str, columns: impl IntoColumnNames) -> Result<bool> {
        Self::builder().has_columns(table, columns).await
    }

    /// Determine if the given table has an index.
    pub async fn has_index(table: &str, index: impl Into<IndexName>) -> Result<bool> {
        Self::builder().has_index(table, index, None).await
    }

    /// Get the column names of a table.
    pub async fn get_column_listing(table: &str) -> Result<Vec<String>> {
        Self::builder().get_column_listing(table).await
    }

    /// Get the columns of a table.
    pub async fn get_columns(table: &str) -> Result<Vec<ColumnInfo>> {
        Self::builder().get_columns(table).await
    }

    /// Get the data type of a column.
    pub async fn get_column_type(table: &str, column: &str) -> Result<String> {
        Self::builder().get_column_type(table, column).await
    }

    /// Get the indexes of a table.
    pub async fn get_indexes(table: &str) -> Result<Vec<IndexInfo>> {
        Self::builder().get_indexes(table).await
    }

    /// Get the foreign keys of a table.
    pub async fn get_foreign_keys(table: &str) -> Result<Vec<ForeignKeyInfo>> {
        Self::builder().get_foreign_keys(table).await
    }

    /// Get the tables of the current schema.
    pub async fn get_tables() -> Result<Vec<TableInfo>> {
        Self::builder().get_tables().await
    }

    /// Get the names of the tables of the current schema.
    pub async fn get_table_listing() -> Result<Vec<String>> {
        Self::builder().get_table_listing().await
    }

    /// Drop all tables from the database.
    pub async fn drop_all_tables() -> Result<()> {
        Self::builder().drop_all_tables().await
    }

    /// Drop all views from the database.
    pub async fn drop_all_views() -> Result<()> {
        Self::builder().drop_all_views().await
    }

    /// Enable foreign key constraints.
    pub async fn enable_foreign_key_constraints() -> Result<bool> {
        Self::builder().enable_foreign_key_constraints().await
    }

    /// Disable foreign key constraints.
    pub async fn disable_foreign_key_constraints() -> Result<bool> {
        Self::builder().disable_foreign_key_constraints().await
    }

    /// Run the callback with foreign key constraints disabled.
    pub async fn without_foreign_key_constraints<F, Fut, T>(callback: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        Self::builder().without_foreign_key_constraints(callback).await
    }
}
