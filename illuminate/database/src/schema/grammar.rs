//! Schema grammars: compiling blueprints into DDL for each driver.

use illuminate_support::{Result, Value, ValueExt};

use super::blueprint::{Blueprint, ColumnAttributes, ColumnDefault, CommandAttributes, CommandDefinition};
use super::state::TableState;
use crate::driver::Driver;
use crate::error::UnsupportedOperation;
use crate::expression::{Expression, Ident};
use crate::query::QueryGrammar;

const SERIALS: [&str; 5] = ["bigInteger", "integer", "mediumInteger", "smallInteger", "tinyInteger"];

fn unsupported<T>(message: &str) -> Result<T> {
    Err(UnsupportedOperation(message.to_string()).into())
}

/// Compiles schema blueprints into SQL statements.
#[derive(Clone, Debug)]
pub struct SchemaGrammar {
    driver: Driver,
    prefix: String,
    config: Value,
    query: QueryGrammar,
}

impl SchemaGrammar {
    /// Create a schema grammar for the driver, table prefix and connection
    /// configuration (used for `charset`, `collation`, `engine`, ...).
    pub fn new(driver: Driver, prefix: impl Into<String>, config: Value) -> Self {
        let prefix = prefix.into();
        Self {
            driver,
            query: QueryGrammar::new(driver, prefix.clone()),
            prefix,
            config,
        }
    }

    /// The driver this grammar compiles for.
    pub fn driver(&self) -> Driver {
        self.driver
    }

    /// The table prefix.
    pub fn table_prefix(&self) -> &str {
        &self.prefix
    }

    fn config(&self, key: &str) -> Option<String> {
        match self.config.get(key) {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.to_string_lossy()).filter(|s| !s.is_empty()),
        }
    }

    /// Whether DDL statements can run inside a transaction.
    pub fn supports_schema_transactions(&self) -> bool {
        self.driver == Driver::Postgres
    }

    /// Commands added for every column (`autoIncrementStartingValues`, `comment`).
    pub fn fluent_commands(&self) -> &'static [&'static str] {
        match self.driver {
            Driver::MySql | Driver::MariaDb => &["autoIncrementStartingValues"],
            Driver::Postgres => &["autoIncrementStartingValues", "comment"],
            Driver::Sqlite => &[],
        }
    }

    fn wrap_table(&self, table: &str) -> String {
        self.query.wrap_table_str(table)
    }

    fn wrap(&self, column: &str) -> String {
        self.query.wrap_str(column)
    }

    fn wrap_ident(&self, column: &Ident) -> String {
        self.query.wrap(column)
    }

    fn wrap_value(&self, value: &str) -> String {
        self.query.wrap_value(value)
    }

    fn columnize(&self, columns: &[Ident]) -> String {
        columns.iter().map(|c| self.wrap_ident(c)).collect::<Vec<_>>().join(", ")
    }

    fn columnize_str(&self, columns: &[String]) -> String {
        columns.iter().map(|c| self.wrap(c)).collect::<Vec<_>>().join(", ")
    }

    fn quote_string(&self, value: &str) -> String {
        self.query.quote_string(value)
    }

    fn quote_strings(&self, values: &[String]) -> String {
        values.iter().map(|v| self.quote_string(v)).collect::<Vec<_>>().join(", ")
    }

    /// Format a value as a column default.
    pub fn get_default_value(&self, value: &ColumnDefault) -> String {
        match value {
            ColumnDefault::Raw(expression) => expression.value().to_string(),
            ColumnDefault::Value(Value::Bool(b)) => format!("'{}'", *b as i32),
            ColumnDefault::Value(value) => format!("'{}'", value.to_string_lossy().replace('\'', "''")),
        }
    }

    /// Split a possibly schema-qualified table into its schema and name.
    pub fn parse_schema_and_table(reference: &str) -> (Option<String>, String) {
        match reference.split_once('.') {
            Some((schema, table)) => (Some(schema.to_string()), table.to_string()),
            None => (None, reference.to_string()),
        }
    }

    // ------------------------------------------------------------------
    // Dispatch
    // ------------------------------------------------------------------

    /// Compile a single blueprint command into zero or more statements.
    pub(crate) fn compile_command(
        &self,
        blueprint: &Blueprint,
        command: &CommandDefinition,
        state: Option<&TableState>,
    ) -> Result<Vec<String>> {
        let attributes = command.attributes();
        let table = blueprint.get_table();
        let one = |sql: String| Ok(vec![sql]);
        match attributes.name.as_str() {
            "create" => one(self.compile_create(blueprint)?),
            "add" => match &attributes.column {
                Some(column) => one(self.compile_add(blueprint, &column.attributes())),
                None => Ok(Vec::new()),
            },
            "change" => match &attributes.column {
                Some(column) => self.compile_change(blueprint, &column.attributes()),
                None => Ok(Vec::new()),
            },
            "alter" => match state {
                Some(state) => Ok(self.compile_alter(blueprint, state)),
                None => Ok(Vec::new()),
            },
            "primary" => self.compile_primary(table, &attributes),
            "unique" => self.compile_unique(table, &attributes),
            "index" => one(self.compile_index(table, &attributes)),
            "fulltext" => self.compile_fulltext(table, &attributes).map(|s| vec![s]),
            "foreign" => self.compile_foreign(table, &attributes),
            "drop" => one(format!("drop table {}", self.wrap_table(table))),
            "dropIfExists" => one(format!("drop table if exists {}", self.wrap_table(table))),
            "dropColumn" => Ok(self.compile_drop_column(table, &attributes)),
            "dropPrimary" => Ok(self.compile_drop_primary(table)),
            "dropUnique" => one(self.compile_drop_unique(table, &attributes)),
            "dropIndex" => one(self.compile_drop_index(table, &attributes)),
            "dropFullText" => match self.driver {
                Driver::Sqlite => unsupported("This database driver does not support fulltext index removal."),
                _ => one(self.compile_drop_index(table, &attributes)),
            },
            "dropForeign" => self.compile_drop_foreign(table, &attributes),
            "rename" => one(self.compile_rename(table, attributes.to.as_deref().unwrap_or_default())),
            "renameColumn" => one(format!(
                "alter table {} rename column {} to {}",
                self.wrap_table(table),
                self.wrap(attributes.from.as_deref().unwrap_or_default()),
                self.wrap(attributes.to.as_deref().unwrap_or_default())
            )),
            "renameIndex" => self.compile_rename_index(table, &attributes, state),
            "tableComment" => Ok(self.compile_table_comment(table, &attributes).into_iter().collect()),
            "comment" => Ok(self.compile_column_comment(table, &attributes).into_iter().collect()),
            "autoIncrementStartingValues" => Ok(self
                .compile_auto_increment_starting_values(table, &attributes)
                .into_iter()
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    // ------------------------------------------------------------------
    // Tables
    // ------------------------------------------------------------------

    fn compile_create(&self, blueprint: &Blueprint) -> Result<String> {
        let table = blueprint.get_table();
        let create = if blueprint.temporary { "create temporary" } else { "create" };
        let mut columns = self.get_columns(blueprint);

        match self.driver {
            Driver::Sqlite => {
                let foreign_keys: String = blueprint
                    .commands_named("foreign")
                    .iter()
                    .map(|f| self.sqlite_foreign_key(&f.attributes()))
                    .collect();
                let primary = blueprint
                    .commands_named("primary")
                    .first()
                    .map(|p| format!(", primary key ({})", self.columnize(&p.attributes().columns)))
                    .unwrap_or_default();
                Ok(format!(
                    "{create} table {} ({}{foreign_keys}{primary})",
                    self.wrap_table(table),
                    columns.join(", ")
                ))
            }
            Driver::MySql | Driver::MariaDb => {
                if let Some(primary) = blueprint.commands_named("primary").first() {
                    let attributes = primary.attributes();
                    columns.push(format!(
                        "primary key {}({})",
                        attributes.algorithm.map(|a| format!("using {a}")).unwrap_or_default(),
                        self.columnize(&attributes.columns)
                    ));
                    primary.lock().should_be_skipped = true;
                }
                let mut sql = format!("{create} table {} ({})", self.wrap_table(table), columns.join(", "));
                if let Some(charset) = blueprint.charset.clone().or_else(|| self.config("charset")) {
                    sql.push_str(&format!(" default character set {charset}"));
                }
                if let Some(collation) = blueprint.collation.clone().or_else(|| self.config("collation")) {
                    sql.push_str(&format!(" collate '{collation}'"));
                }
                if let Some(engine) = blueprint.engine.clone().or_else(|| self.config("engine")) {
                    sql.push_str(&format!(" engine = {engine}"));
                }
                Ok(sql)
            }
            Driver::Postgres => Ok(format!(
                "{create} table {} ({})",
                self.wrap_table(table),
                columns.join(", ")
            )),
        }
    }

    fn sqlite_foreign_key(&self, foreign: &CommandAttributes) -> String {
        let mut sql = format!(
            ", foreign key({}) references {}({})",
            self.columnize(&foreign.columns),
            self.wrap_table(foreign.on.as_deref().unwrap_or_default()),
            self.columnize_str(&foreign.references)
        );
        if let Some(action) = &foreign.on_delete {
            sql.push_str(&format!(" on delete {action}"));
        }
        if let Some(action) = &foreign.on_update {
            sql.push_str(&format!(" on update {action}"));
        }
        sql
    }

    fn compile_add(&self, blueprint: &Blueprint, column: &ColumnAttributes) -> String {
        let table = self.wrap_table(blueprint.get_table());
        let definition = self.get_column(blueprint, column);
        match self.driver {
            Driver::MySql | Driver::MariaDb => format!("alter table {table} add {definition}"),
            _ => format!("alter table {table} add column {definition}"),
        }
    }

    fn compile_change(&self, blueprint: &Blueprint, column: &ColumnAttributes) -> Result<Vec<String>> {
        let table = self.wrap_table(blueprint.get_table());
        match self.driver {
            // SQLite rebuilds the table in the `alter` command.
            Driver::Sqlite => Ok(Vec::new()),
            Driver::MySql | Driver::MariaDb => {
                let column = self.with_type_defaults(column);
                let sql = format!("alter table {table} modify {} {}", self.wrap(&column.name), self.get_type(&column));
                Ok(vec![self.add_modifiers(sql, blueprint, &column)])
            }
            Driver::Postgres => {
                let column = self.with_type_defaults(column);
                let mut changes = vec![format!(
                    "type {}{}{}",
                    self.get_type(&column),
                    self.modify_collate(&column),
                    column.using.as_ref().map(|u| format!(" using {u}")).unwrap_or_default()
                )];
                changes.push(if column.nullable.unwrap_or(false) {
                    "drop not null".to_string()
                } else {
                    "set not null".to_string()
                });
                if !column.auto_increment || column.generated_as.is_some() {
                    changes.push(match &column.default {
                        Some(default) => format!("set default {}", self.get_default_value(default)),
                        None => "drop default".to_string(),
                    });
                }
                if column.generated_as.is_some() || !column.auto_increment {
                    changes.push("drop identity if exists".to_string());
                }
                if let Some(generated) = self.modify_generated_as(&column) {
                    changes.push(format!("add {}", generated.trim_start()));
                }
                let wrapped = self.wrap(&column.name);
                Ok(vec![format!(
                    "alter table {table} {}",
                    changes
                        .iter()
                        .map(|c| format!("alter column {wrapped} {c}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )])
            }
        }
    }

    /// Rebuild a SQLite table from its (updated) state.
    fn compile_alter(&self, blueprint: &Blueprint, state: &TableState) -> Vec<String> {
        let mut column_names = Vec::new();
        let mut auto_increment_column = None;
        let columns: Vec<String> = state
            .columns
            .iter()
            .map(|column| {
                let column = self.with_type_defaults(column);
                if column.auto_increment {
                    auto_increment_column = Some(column.name.clone());
                }
                if column.virtual_as.is_none() && column.stored_as.is_none() {
                    column_names.push(self.wrap(&column.name));
                }
                let kind = column
                    .full_type_definition
                    .clone()
                    .unwrap_or_else(|| self.get_type(&column));
                self.add_modifiers(format!("{} {kind}", self.wrap(&column.name)), blueprint, &column)
            })
            .collect();

        let table = blueprint.get_table();
        let (_, table_name) = Self::parse_schema_and_table(table);
        let temp_table = self
            .query
            .wrap_table_with_prefix(table, &format!("__temp__{}", self.prefix));
        let wrapped = self.wrap_table(table);
        let column_names = column_names.join(", ");

        let foreign_keys: String = state.foreign_keys.iter().map(|f| self.sqlite_foreign_key(f)).collect();
        let primary = if auto_increment_column.is_some() {
            String::new()
        } else {
            state
                .primary
                .as_ref()
                .map(|p| format!(", primary key ({})", self.columnize(&p.columns)))
                .unwrap_or_default()
        };

        let mut statements = Vec::new();
        if state.foreign_keys_enabled {
            statements.push(self.compile_disable_foreign_key_constraints());
        }
        statements.push(format!(
            "create table {temp_table} ({}{foreign_keys}{primary})",
            columns.join(", ")
        ));
        statements.push(format!(
            "insert into {temp_table} ({column_names}) select {column_names} from {wrapped}"
        ));
        statements.push(format!("drop table {wrapped}"));
        statements.push(format!(
            "alter table {temp_table} rename to {}",
            self.wrap_table(&table_name)
        ));
        for index in &state.indexes {
            if index.index.as_deref().is_some_and(|name| name.starts_with("sqlite_")) {
                continue;
            }
            let sql = match index.name.as_str() {
                "unique" => self.compile_unique(table, index).unwrap_or_default(),
                _ => vec![self.compile_index(table, index)],
            };
            statements.extend(sql);
        }
        if state.foreign_keys_enabled {
            statements.push(self.compile_enable_foreign_key_constraints());
        }
        statements
    }

    fn compile_rename(&self, from: &str, to: &str) -> String {
        match self.driver {
            Driver::MySql | Driver::MariaDb => format!("rename table {} to {}", self.wrap_table(from), self.wrap_table(to)),
            _ => format!("alter table {} rename to {}", self.wrap_table(from), self.wrap_table(to)),
        }
    }

    fn compile_table_comment(&self, table: &str, command: &CommandAttributes) -> Option<String> {
        let comment = command.comment.as_deref()?.replace('\'', "''");
        match self.driver {
            Driver::MySql | Driver::MariaDb => Some(format!("alter table {} comment = '{comment}'", self.wrap_table(table))),
            Driver::Postgres => Some(format!("comment on table {} is '{comment}'", self.wrap_table(table))),
            Driver::Sqlite => None,
        }
    }

    fn compile_column_comment(&self, table: &str, command: &CommandAttributes) -> Option<String> {
        let column = command.column.as_ref()?.attributes();
        if column.comment.is_none() && !column.change {
            return None;
        }
        Some(format!(
            "comment on column {}.{} is {}",
            self.wrap_table(table),
            self.wrap(&column.name),
            match &column.comment {
                Some(comment) => format!("'{}'", comment.replace('\'', "''")),
                None => "NULL".to_string(),
            }
        ))
    }

    fn compile_auto_increment_starting_values(&self, table: &str, command: &CommandAttributes) -> Option<String> {
        let column = command.column.as_ref()?.attributes();
        let value = column.starting_value.filter(|_| column.auto_increment)?;
        match self.driver {
            Driver::MySql | Driver::MariaDb => {
                Some(format!("alter table {} auto_increment = {value}", self.wrap_table(table)))
            }
            Driver::Postgres => Some(format!(
                "select setval(pg_get_serial_sequence({}, {}), {value}, false)",
                self.quote_string(&self.wrap_table(table)),
                self.quote_string(&column.name)
            )),
            Driver::Sqlite => None,
        }
    }

    // ------------------------------------------------------------------
    // Indexes & foreign keys
    // ------------------------------------------------------------------

    fn index_name(command: &CommandAttributes) -> String {
        command.index.clone().unwrap_or_default()
    }

    fn compile_primary(&self, table: &str, command: &CommandAttributes) -> Result<Vec<String>> {
        let columns = self.columnize(&command.columns);
        Ok(match self.driver {
            Driver::Sqlite => Vec::new(),
            Driver::MySql | Driver::MariaDb => vec![format!(
                "alter table {} add primary key {}({columns})",
                self.wrap_table(table),
                command.algorithm.as_ref().map(|a| format!("using {a}")).unwrap_or_default()
            )],
            Driver::Postgres => vec![format!("alter table {} add primary key ({columns})", self.wrap_table(table))],
        })
    }

    fn compile_unique(&self, table: &str, command: &CommandAttributes) -> Result<Vec<String>> {
        let index = self.wrap(&Self::index_name(command));
        let columns = self.columnize(&command.columns);
        Ok(match self.driver {
            Driver::Sqlite => {
                let (schema, name) = Self::parse_schema_and_table(table);
                vec![format!(
                    "create unique index {}{index} on {} ({columns})",
                    schema.map(|s| format!("{}.", self.wrap_value(&s))).unwrap_or_default(),
                    self.wrap_table(&name)
                )]
            }
            Driver::MySql | Driver::MariaDb => vec![self.mysql_key(table, command, "unique")],
            Driver::Postgres => {
                let mut sql = format!(
                    "alter table {} add constraint {index} unique ({columns})",
                    self.wrap_table(table)
                );
                if let Some(deferrable) = command.deferrable {
                    sql.push_str(if deferrable { " deferrable" } else { " not deferrable" });
                    if deferrable {
                        if let Some(immediate) = command.initially_immediate {
                            sql.push_str(if immediate { " initially immediate" } else { " initially deferred" });
                        }
                    }
                }
                vec![sql]
            }
        })
    }

    fn mysql_key(&self, table: &str, command: &CommandAttributes, kind: &str) -> String {
        format!(
            "alter table {} add {kind} {}{}({})",
            self.wrap_table(table),
            self.wrap(&Self::index_name(command)),
            command.algorithm.as_ref().map(|a| format!(" using {a}")).unwrap_or_default(),
            self.columnize(&command.columns)
        )
    }

    fn compile_index(&self, table: &str, command: &CommandAttributes) -> String {
        let index = self.wrap(&Self::index_name(command));
        let columns = self.columnize(&command.columns);
        match self.driver {
            Driver::Sqlite => {
                let (schema, name) = Self::parse_schema_and_table(table);
                format!(
                    "create index {}{index} on {} ({columns})",
                    schema.map(|s| format!("{}.", self.wrap_value(&s))).unwrap_or_default(),
                    self.wrap_table(&name)
                )
            }
            Driver::MySql | Driver::MariaDb => self.mysql_key(table, command, "index"),
            Driver::Postgres => format!(
                "create index {}{index} on {}{} ({columns})",
                if command.online { "concurrently " } else { "" },
                self.wrap_table(table),
                command.algorithm.as_ref().map(|a| format!(" using {a}")).unwrap_or_default()
            ),
        }
    }

    fn compile_fulltext(&self, table: &str, command: &CommandAttributes) -> Result<String> {
        match self.driver {
            Driver::Sqlite => unsupported("This database driver does not support fulltext index creation."),
            Driver::MySql | Driver::MariaDb => Ok(self.mysql_key(table, command, "fulltext")),
            Driver::Postgres => {
                let language = command.language.clone().unwrap_or_else(|| "english".into());
                let columns = command
                    .columns
                    .iter()
                    .map(|c| format!("to_tsvector({}, {})", self.quote_string(&language), self.wrap_ident(c)))
                    .collect::<Vec<_>>()
                    .join(" || ");
                Ok(format!(
                    "create index {}{} on {} using gin (({columns}))",
                    if command.online { "concurrently " } else { "" },
                    self.wrap(&Self::index_name(command)),
                    self.wrap_table(table)
                ))
            }
        }
    }

    fn compile_foreign(&self, table: &str, command: &CommandAttributes) -> Result<Vec<String>> {
        if self.driver == Driver::Sqlite {
            // Foreign keys are created inline (or by a table rebuild).
            return Ok(Vec::new());
        }
        let mut sql = format!(
            "alter table {} add constraint {} foreign key ({}) references {} ({})",
            self.wrap_table(table),
            self.wrap(&Self::index_name(command)),
            self.columnize(&command.columns),
            self.wrap_table(command.on.as_deref().unwrap_or_default()),
            self.columnize_str(&command.references)
        );
        if let Some(action) = &command.on_delete {
            sql.push_str(&format!(" on delete {action}"));
        }
        if let Some(action) = &command.on_update {
            sql.push_str(&format!(" on update {action}"));
        }
        if self.driver == Driver::Postgres {
            if let Some(deferrable) = command.deferrable {
                sql.push_str(if deferrable { " deferrable" } else { " not deferrable" });
                if deferrable {
                    if let Some(immediate) = command.initially_immediate {
                        sql.push_str(if immediate { " initially immediate" } else { " initially deferred" });
                    }
                }
            }
            if command.not_valid {
                sql.push_str(" not valid");
            }
        }
        Ok(vec![sql])
    }

    fn compile_drop_column(&self, table: &str, command: &CommandAttributes) -> Vec<String> {
        let wrapped = self.wrap_table(table);
        let columns: Vec<String> = command.column_names().iter().map(|c| self.wrap(c)).collect();
        match self.driver {
            Driver::Sqlite => columns
                .iter()
                .map(|c| format!("alter table {wrapped} drop column {c}"))
                .collect(),
            Driver::MySql | Driver::MariaDb => vec![format!(
                "alter table {wrapped} {}",
                columns.iter().map(|c| format!("drop {c}")).collect::<Vec<_>>().join(", ")
            )],
            Driver::Postgres => vec![format!(
                "alter table {wrapped} {}",
                columns.iter().map(|c| format!("drop column {c}")).collect::<Vec<_>>().join(", ")
            )],
        }
    }

    fn compile_drop_primary(&self, table: &str) -> Vec<String> {
        match self.driver {
            Driver::Sqlite => Vec::new(),
            Driver::MySql | Driver::MariaDb => vec![format!("alter table {} drop primary key", self.wrap_table(table))],
            Driver::Postgres => {
                let (_, name) = Self::parse_schema_and_table(table);
                vec![format!(
                    "alter table {} drop constraint {}",
                    self.wrap_table(table),
                    self.wrap(&format!("{}{name}_pkey", self.prefix))
                )]
            }
        }
    }

    fn compile_drop_unique(&self, table: &str, command: &CommandAttributes) -> String {
        match self.driver {
            Driver::Postgres => format!(
                "alter table {} drop constraint {}",
                self.wrap_table(table),
                self.wrap(&Self::index_name(command))
            ),
            _ => self.compile_drop_index(table, command),
        }
    }

    fn compile_drop_index(&self, table: &str, command: &CommandAttributes) -> String {
        let index = self.wrap(&Self::index_name(command));
        match self.driver {
            Driver::Sqlite => {
                let (schema, _) = Self::parse_schema_and_table(table);
                format!(
                    "drop index {}{index}",
                    schema.map(|s| format!("{}.", self.wrap_value(&s))).unwrap_or_default()
                )
            }
            Driver::MySql | Driver::MariaDb => format!("alter table {} drop index {index}", self.wrap_table(table)),
            Driver::Postgres => format!("drop index {index}"),
        }
    }

    fn compile_drop_foreign(&self, table: &str, command: &CommandAttributes) -> Result<Vec<String>> {
        let index = self.wrap(&Self::index_name(command));
        match self.driver {
            Driver::Sqlite => {
                if command.columns.is_empty() {
                    return unsupported("This database driver does not support dropping foreign keys by name.");
                }
                Ok(Vec::new())
            }
            Driver::MySql | Driver::MariaDb => Ok(vec![format!(
                "alter table {} drop foreign key {index}",
                self.wrap_table(table)
            )]),
            Driver::Postgres => Ok(vec![format!(
                "alter table {} drop constraint {index}",
                self.wrap_table(table)
            )]),
        }
    }

    fn compile_rename_index(
        &self,
        table: &str,
        command: &CommandAttributes,
        state: Option<&TableState>,
    ) -> Result<Vec<String>> {
        let from = command.from.clone().unwrap_or_default();
        let to = command.to.clone().unwrap_or_default();
        match self.driver {
            Driver::MySql | Driver::MariaDb => Ok(vec![format!(
                "alter table {} rename index {} to {}",
                self.wrap_table(table),
                self.wrap(&from),
                self.wrap(&to)
            )]),
            Driver::Postgres => Ok(vec![format!("alter index {} rename to {}", self.wrap(&from), self.wrap(&to))]),
            Driver::Sqlite => {
                let Some(state) = state else {
                    return unsupported("Renaming an index on SQLite requires inspecting the table.");
                };
                if state.primary.as_ref().and_then(|p| p.index.as_deref()) == Some(from.as_str()) {
                    return unsupported("SQLite does not support altering primary keys.");
                }
                let Some(index) = state
                    .original_indexes
                    .iter()
                    .find(|i| i.index.as_deref() == Some(from.as_str()))
                else {
                    return Err(illuminate_support::error::RuntimeException::new(format!(
                        "Index [{from}] does not exist."
                    ))
                    .into());
                };
                let drop = CommandAttributes {
                    index: Some(from.clone()),
                    ..Default::default()
                };
                let create = CommandAttributes {
                    index: Some(to),
                    columns: index.columns.clone(),
                    ..Default::default()
                };
                let mut statements = vec![self.compile_drop_index(table, &drop)];
                if index.name == "unique" {
                    statements.extend(self.compile_unique(table, &create)?);
                } else {
                    statements.push(self.compile_index(table, &create));
                }
                Ok(statements)
            }
        }
    }

    // ------------------------------------------------------------------
    // Columns
    // ------------------------------------------------------------------

    fn get_columns(&self, blueprint: &Blueprint) -> Vec<String> {
        blueprint
            .get_added_columns()
            .iter()
            .map(|column| self.get_column(blueprint, &column.attributes()))
            .collect()
    }

    fn get_column(&self, blueprint: &Blueprint, column: &ColumnAttributes) -> String {
        let column = self.with_type_defaults(column);
        let sql = format!("{} {}", self.wrap(&column.name), self.get_type(&column));
        self.add_modifiers(sql, blueprint, &column)
    }

    /// Apply the defaults implied by a column's type (`use_current`, ...).
    fn with_type_defaults(&self, column: &ColumnAttributes) -> ColumnAttributes {
        let mut column = column.clone();
        let current = match (self.driver, column.precision) {
            (Driver::MySql | Driver::MariaDb, Some(p)) if p > 0 => format!("CURRENT_TIMESTAMP({p})"),
            _ => "CURRENT_TIMESTAMP".to_string(),
        };
        match column.kind.as_str() {
            "date" if column.use_current => {
                column.default = Some(ColumnDefault::Raw(Expression::new(match self.driver {
                    Driver::MySql | Driver::MariaDb => "(CURDATE())",
                    _ => "CURRENT_DATE",
                })));
            }
            "dateTime" | "dateTimeTz" | "timestamp" | "timestampTz" => {
                if column.use_current {
                    column.default = Some(ColumnDefault::Raw(Expression::new(current.clone())));
                }
                if column.use_current_on_update && self.driver.is_mysql_family() {
                    column.on_update = Some(Expression::new(current));
                }
            }
            "year" if column.use_current => {
                column.default = Some(ColumnDefault::Raw(Expression::new(match self.driver {
                    Driver::Sqlite => "(CAST(strftime('%Y', 'now') AS INTEGER))",
                    Driver::MySql | Driver::MariaDb => "(YEAR(CURDATE()))",
                    Driver::Postgres => "EXTRACT(YEAR FROM CURRENT_DATE)",
                })));
            }
            _ => {}
        }
        column
    }

    fn precision_suffix(precision: Option<u32>) -> String {
        precision.map(|p| format!("({p})")).unwrap_or_default()
    }

    /// Get the SQL type for a column.
    pub fn get_type(&self, column: &ColumnAttributes) -> String {
        let kind = column.kind.as_str();
        if kind == "raw" {
            return column.definition.clone().unwrap_or_default();
        }
        match self.driver {
            Driver::Sqlite => match kind {
                "char" | "string" | "uuid" | "ipAddress" | "macAddress" => "varchar".into(),
                "tinyText" | "text" | "mediumText" | "longText" => "text".into(),
                "integer" | "bigInteger" | "mediumInteger" | "tinyInteger" | "smallInteger" | "year" => {
                    "integer".into()
                }
                "float" => "float".into(),
                "double" => "double".into(),
                "decimal" => "numeric".into(),
                "boolean" => "tinyint(1)".into(),
                "enum" => format!(
                    "varchar check (\"{}\" in ({}))",
                    column.name,
                    self.quote_strings(&column.allowed)
                ),
                "json" => if self.config.get("use_native_json").is_some_and(|v| v.truthy()) { "json" } else { "text" }.into(),
                "jsonb" => if self.config.get("use_native_jsonb").is_some_and(|v| v.truthy()) { "jsonb" } else { "text" }.into(),
                "date" => "date".into(),
                "dateTime" | "dateTimeTz" | "timestamp" | "timestampTz" => "datetime".into(),
                "time" | "timeTz" => "time".into(),
                "binary" => "blob".into(),
                other => other.to_string(),
            },
            Driver::MySql | Driver::MariaDb => match kind {
                "char" => format!("char({})", column.length.unwrap_or(255)),
                "string" => format!("varchar({})", column.length.unwrap_or(255)),
                "tinyText" => "tinytext".into(),
                "text" => "text".into(),
                "mediumText" => "mediumtext".into(),
                "longText" => "longtext".into(),
                "bigInteger" => "bigint".into(),
                "integer" => "int".into(),
                "mediumInteger" => "mediumint".into(),
                "tinyInteger" => "tinyint".into(),
                "smallInteger" => "smallint".into(),
                "float" => match column.precision {
                    Some(p) if p > 0 => format!("float({p})"),
                    _ => "float".into(),
                },
                "double" => "double".into(),
                "decimal" => format!("decimal({}, {})", column.total.unwrap_or(8), column.places.unwrap_or(2)),
                "boolean" => "tinyint(1)".into(),
                "enum" => format!("enum({})", self.quote_strings(&column.allowed)),
                "set" => format!("set({})", self.quote_strings(&column.allowed)),
                "json" | "jsonb" => "json".into(),
                "date" => "date".into(),
                "dateTime" | "dateTimeTz" => match column.precision {
                    Some(p) if p > 0 => format!("datetime({p})"),
                    _ => "datetime".into(),
                },
                "time" | "timeTz" => match column.precision {
                    Some(p) if p > 0 => format!("time({p})"),
                    _ => "time".into(),
                },
                "timestamp" | "timestampTz" => match column.precision {
                    Some(p) if p > 0 => format!("timestamp({p})"),
                    _ => "timestamp".into(),
                },
                "year" => "year".into(),
                "binary" => match column.length {
                    Some(length) if column.fixed => format!("binary({length})"),
                    Some(length) => format!("varbinary({length})"),
                    None => "blob".into(),
                },
                "uuid" => if self.driver == Driver::MariaDb { "uuid" } else { "char(36)" }.into(),
                "ipAddress" => "varchar(45)".into(),
                "macAddress" => "varchar(17)".into(),
                other => other.to_string(),
            },
            Driver::Postgres => {
                let serial = column.auto_increment && column.generated_as.is_none() && !column.change;
                match kind {
                    "char" => match column.length {
                        Some(length) => format!("char({length})"),
                        None => "char".into(),
                    },
                    "string" => match column.length {
                        Some(length) => format!("varchar({length})"),
                        None => "varchar".into(),
                    },
                    "tinyText" => "varchar(255)".into(),
                    "text" | "mediumText" | "longText" => "text".into(),
                    "integer" | "mediumInteger" | "year" => if serial && kind != "year" { "serial" } else { "integer" }.into(),
                    "bigInteger" => if serial { "bigserial" } else { "bigint" }.into(),
                    "smallInteger" | "tinyInteger" => if serial { "smallserial" } else { "smallint" }.into(),
                    "float" => match column.precision {
                        Some(p) if p > 0 => format!("float({p})"),
                        _ => "float".into(),
                    },
                    "double" => "double precision".into(),
                    "decimal" => format!("decimal({}, {})", column.total.unwrap_or(8), column.places.unwrap_or(2)),
                    "boolean" => "boolean".into(),
                    "enum" => format!(
                        "varchar(255) check (\"{}\" in ({}))",
                        column.name,
                        self.quote_strings(&column.allowed)
                    ),
                    "json" => "json".into(),
                    "jsonb" => "jsonb".into(),
                    "date" => "date".into(),
                    "dateTime" | "timestamp" => {
                        format!("timestamp{} without time zone", Self::precision_suffix(column.precision))
                    }
                    "dateTimeTz" | "timestampTz" => {
                        format!("timestamp{} with time zone", Self::precision_suffix(column.precision))
                    }
                    "time" => format!("time{} without time zone", Self::precision_suffix(column.precision)),
                    "timeTz" => format!("time{} with time zone", Self::precision_suffix(column.precision)),
                    "binary" => "bytea".into(),
                    "uuid" => "uuid".into(),
                    "ipAddress" => "inet".into(),
                    "macAddress" => "macaddr".into(),
                    other => other.to_string(),
                }
            }
        }
    }

    fn add_modifiers(&self, mut sql: String, blueprint: &Blueprint, column: &ColumnAttributes) -> String {
        let generated = column.virtual_as.is_some() || column.stored_as.is_some();
        match self.driver {
            Driver::Sqlite => {
                if SERIALS.contains(&column.kind.as_str()) && column.auto_increment {
                    sql.push_str(" primary key autoincrement");
                }
                if !generated {
                    if !column.nullable.unwrap_or(false) {
                        sql.push_str(" not null");
                    }
                } else if column.nullable == Some(false) {
                    sql.push_str(" not null");
                }
                if let Some(default) = column.default.as_ref().filter(|_| !generated) {
                    sql.push_str(&format!(" default {}", self.get_default_value(default)));
                }
                if let Some(collation) = &column.collation {
                    sql.push_str(&format!(" collate '{collation}'"));
                }
                if let Some(expression) = &column.virtual_as {
                    sql.push_str(&format!(" as ({expression})"));
                }
                if let Some(expression) = &column.stored_as {
                    sql.push_str(&format!(" as ({expression}) stored"));
                }
            }
            Driver::MySql | Driver::MariaDb => {
                if column.unsigned {
                    sql.push_str(" unsigned");
                }
                if let Some(charset) = &column.charset {
                    sql.push_str(&format!(" character set {charset}"));
                }
                if let Some(collation) = &column.collation {
                    sql.push_str(&format!(" collate '{collation}'"));
                }
                if let Some(expression) = &column.virtual_as {
                    sql.push_str(&format!(" as ({expression})"));
                }
                if let Some(expression) = &column.stored_as {
                    sql.push_str(&format!(" as ({expression}) stored"));
                }
                if !generated {
                    sql.push_str(if column.nullable.unwrap_or(false) { " null" } else { " not null" });
                } else if column.nullable == Some(false) {
                    sql.push_str(" not null");
                }
                if let Some(default) = &column.default {
                    sql.push_str(&format!(" default {}", self.get_default_value(default)));
                }
                if let Some(on_update) = &column.on_update {
                    sql.push_str(&format!(" on update {}", on_update.value()));
                }
                if column.invisible {
                    sql.push_str(" invisible");
                }
                if SERIALS.contains(&column.kind.as_str()) && column.auto_increment {
                    let primary_elsewhere = blueprint.has_command("primary")
                        || (column.change && column.primary.is_none());
                    sql.push_str(if primary_elsewhere { " auto_increment" } else { " auto_increment primary key" });
                }
                if let Some(comment) = &column.comment {
                    sql.push_str(&format!(" comment '{}'", add_slashes(comment)));
                }
                if let Some(after) = &column.after {
                    sql.push_str(&format!(" after {}", self.wrap(after)));
                }
                if column.first {
                    sql.push_str(" first");
                }
            }
            Driver::Postgres => {
                sql.push_str(&self.modify_collate(column));
                sql.push_str(if column.nullable.unwrap_or(false) { " null" } else { " not null" });
                if let Some(default) = &column.default {
                    sql.push_str(&format!(" default {}", self.get_default_value(default)));
                }
                if let Some(expression) = &column.virtual_as {
                    sql.push_str(&format!(" generated always as ({expression}) virtual"));
                }
                if let Some(expression) = &column.stored_as {
                    sql.push_str(&format!(" generated always as ({expression}) stored"));
                }
                if let Some(generated) = self.modify_generated_as(column) {
                    sql.push_str(&generated);
                }
                if !column.change
                    && !blueprint.has_command("primary")
                    && (SERIALS.contains(&column.kind.as_str()) || column.generated_as.is_some())
                    && column.auto_increment
                {
                    sql.push_str(" primary key");
                }
            }
        }
        sql
    }

    fn modify_collate(&self, column: &ColumnAttributes) -> String {
        match &column.collation {
            Some(collation) if self.driver == Driver::Postgres => format!(" collate {}", self.wrap_value(collation)),
            Some(collation) => format!(" collate '{collation}'"),
            None => String::new(),
        }
    }

    fn modify_generated_as(&self, column: &ColumnAttributes) -> Option<String> {
        let generated = column.generated_as.as_ref()?;
        Some(format!(
            " generated {} as identity{}",
            if column.always { "always" } else { "by default" },
            if generated.is_empty() { String::new() } else { format!(" ({generated})") }
        ))
    }

    // ------------------------------------------------------------------
    // Introspection
    // ------------------------------------------------------------------

    /// Compile the query to determine if a table exists.
    pub fn compile_table_exists(&self, schema: Option<&str>, table: &str) -> String {
        match self.driver {
            Driver::Sqlite => format!(
                "select exists (select 1 from {}.sqlite_master where name = {} and type = 'table') as \"exists\"",
                self.wrap_value(schema.unwrap_or("main")),
                self.quote_string(table)
            ),
            Driver::MySql | Driver::MariaDb => format!(
                "select exists (select 1 from information_schema.tables where table_schema = {} and table_name = {} and table_type in ('BASE TABLE', 'SYSTEM VERSIONED')) as `exists`",
                schema.map(|s| self.quote_string(s)).unwrap_or_else(|| "schema()".into()),
                self.quote_string(table)
            ),
            Driver::Postgres => format!(
                "select exists (select 1 from pg_class c, pg_namespace n where n.nspname = {} and c.relname = {} and c.relkind in ('r', 'p') and n.oid = c.relnamespace)",
                schema.map(|s| self.quote_string(s)).unwrap_or_else(|| "current_schema()".into()),
                self.quote_string(table)
            ),
        }
    }

    /// Compile the query to list the tables of the current schema.
    pub fn compile_tables(&self) -> String {
        match self.driver {
            Driver::Sqlite => "select tl.name as name, tl.schema as schema from pragma_table_list as tl where tl.schema = 'main' and tl.type in ('table', 'virtual') and tl.name not like 'sqlite\\_%' escape '\\' order by tl.schema, tl.name".to_string(),
            Driver::MySql | Driver::MariaDb => "select table_name as `name`, table_schema as `schema`, (data_length + index_length) as `size`, table_comment as `comment`, engine as `engine`, table_collation as `collation` from information_schema.tables where table_type in ('BASE TABLE', 'SYSTEM VERSIONED') and table_schema = schema() order by table_schema, table_name".to_string(),
            Driver::Postgres => "select c.relname as name, n.nspname as schema, pg_total_relation_size(c.oid) as size, obj_description(c.oid, 'pg_class') as comment from pg_class c, pg_namespace n where c.relkind in ('r', 'p') and n.oid = c.relnamespace and n.nspname = current_schema() order by n.nspname, c.relname".to_string(),
        }
    }

    /// Compile the query to list the views of the current schema.
    pub fn compile_views(&self) -> String {
        match self.driver {
            Driver::Sqlite => "select name, 'main' as schema, sql as definition from \"main\".sqlite_master where type = 'view' order by name".to_string(),
            Driver::MySql | Driver::MariaDb => "select table_name as `name`, table_schema as `schema`, view_definition as `definition` from information_schema.views where table_schema = schema() order by table_schema, table_name".to_string(),
            Driver::Postgres => "select viewname as name, schemaname as schema, definition from pg_views where schemaname = current_schema() order by schemaname, viewname".to_string(),
        }
    }

    /// Compile the query to list a table's columns.
    pub fn compile_columns(&self, schema: Option<&str>, table: &str) -> String {
        match self.driver {
            Driver::Sqlite => format!(
                "select name, type, not \"notnull\" as \"nullable\", dflt_value as \"default\", pk as \"primary\", hidden as \"extra\" from pragma_table_xinfo({}, {}) order by cid asc",
                self.quote_string(table),
                self.quote_string(schema.unwrap_or("main"))
            ),
            Driver::MySql | Driver::MariaDb => format!(
                "select column_name as `name`, data_type as `type_name`, column_type as `type`, collation_name as `collation`, is_nullable as `nullable`, column_default as `default`, column_comment as `comment`, generation_expression as `expression`, extra as `extra` from information_schema.columns where table_schema = {} and table_name = {} order by ordinal_position asc",
                schema.map(|s| self.quote_string(s)).unwrap_or_else(|| "schema()".into()),
                self.quote_string(table)
            ),
            Driver::Postgres => format!(
                "select a.attname as name, t.typname as type_name, format_type(a.atttypid, a.atttypmod) as type, (select tc.collcollate from pg_catalog.pg_collation tc where tc.oid = a.attcollation) as collation, not a.attnotnull as nullable, (select pg_get_expr(adbin, adrelid) from pg_attrdef where c.oid = pg_attrdef.adrelid and pg_attrdef.adnum = a.attnum) as default, a.attgenerated as generated, col_description(c.oid, a.attnum) as comment from pg_attribute a, pg_class c, pg_type t, pg_namespace n where c.relname = {} and n.nspname = {} and a.attnum > 0 and a.attrelid = c.oid and a.atttypid = t.oid and n.oid = c.relnamespace order by a.attnum",
                self.quote_string(table),
                schema.map(|s| self.quote_string(s)).unwrap_or_else(|| "current_schema()".into())
            ),
        }
    }

    /// Compile the query to list a table's indexes.
    pub fn compile_indexes(&self, schema: Option<&str>, table: &str) -> String {
        match self.driver {
            Driver::Sqlite => {
                let table = self.quote_string(table);
                let schema = self.quote_string(schema.unwrap_or("main"));
                format!(
                    "select 'primary' as name, group_concat(col) as columns, 1 as \"unique\", 1 as \"primary\" from (select name as col from pragma_table_xinfo({table}, {schema}) where pk > 0 order by pk, cid) group by name union select name, group_concat(col) as columns, \"unique\", origin = 'pk' as \"primary\" from (select il.*, ii.name as col from pragma_index_list({table}, {schema}) il, pragma_index_info(il.name, {schema}) ii order by il.seq, ii.seqno) group by name, \"unique\", \"primary\""
                )
            }
            Driver::MySql | Driver::MariaDb => format!(
                "select index_name as `name`, group_concat(column_name order by seq_in_index) as `columns`, index_type as `type`, not non_unique as `unique` from information_schema.statistics where table_schema = {} and table_name = {} group by index_name, index_type, non_unique",
                schema.map(|s| self.quote_string(s)).unwrap_or_else(|| "schema()".into()),
                self.quote_string(table)
            ),
            Driver::Postgres => format!(
                "select ic.relname as name, string_agg(a.attname, ',' order by indseq.ord) as columns, am.amname as \"type\", i.indisunique as \"unique\", i.indisprimary as \"primary\" from pg_index i join pg_class tc on tc.oid = i.indrelid join pg_namespace tn on tn.oid = tc.relnamespace join pg_class ic on ic.oid = i.indexrelid join pg_am am on am.oid = ic.relam join lateral unnest(i.indkey) with ordinality as indseq(num, ord) on true left join pg_attribute a on a.attrelid = i.indrelid and a.attnum = indseq.num where tc.relname = {} and tn.nspname = {} group by ic.relname, am.amname, i.indisunique, i.indisprimary",
                self.quote_string(table),
                schema.map(|s| self.quote_string(s)).unwrap_or_else(|| "current_schema()".into())
            ),
        }
    }

    /// Compile the query to list a table's foreign keys.
    pub fn compile_foreign_keys(&self, schema: Option<&str>, table: &str) -> String {
        match self.driver {
            Driver::Sqlite => {
                let schema = self.quote_string(schema.unwrap_or("main"));
                format!(
                    "select group_concat(\"from\") as columns, {schema} as foreign_schema, \"table\" as foreign_table, group_concat(\"to\") as foreign_columns, on_update, on_delete from (select * from pragma_foreign_key_list({}, {schema}) order by id desc, seq) group by id, \"table\", on_update, on_delete",
                    self.quote_string(table)
                )
            }
            Driver::MySql | Driver::MariaDb => format!(
                "select kc.constraint_name as `name`, group_concat(kc.column_name order by kc.ordinal_position) as `columns`, kc.referenced_table_schema as `foreign_schema`, kc.referenced_table_name as `foreign_table`, group_concat(kc.referenced_column_name order by kc.ordinal_position) as `foreign_columns`, rc.update_rule as `on_update`, rc.delete_rule as `on_delete` from information_schema.key_column_usage kc join information_schema.referential_constraints rc on kc.constraint_schema = rc.constraint_schema and kc.constraint_name = rc.constraint_name where kc.table_schema = {} and kc.table_name = {} and kc.referenced_table_name is not null group by kc.constraint_name, kc.referenced_table_schema, kc.referenced_table_name, rc.update_rule, rc.delete_rule",
                schema.map(|s| self.quote_string(s)).unwrap_or_else(|| "schema()".into()),
                self.quote_string(table)
            ),
            Driver::Postgres => format!(
                "select c.conname as name, string_agg(la.attname, ',' order by conseq.ord) as columns, fn.nspname as foreign_schema, fc.relname as foreign_table, string_agg(fa.attname, ',' order by conseq.ord) as foreign_columns, c.confupdtype as on_update, c.confdeltype as on_delete from pg_constraint c join pg_class tc on c.conrelid = tc.oid join pg_namespace tn on tn.oid = tc.relnamespace join pg_class fc on c.confrelid = fc.oid join pg_namespace fn on fn.oid = fc.relnamespace join lateral unnest(c.conkey) with ordinality as conseq(num, ord) on true join pg_attribute la on la.attrelid = c.conrelid and la.attnum = conseq.num join pg_attribute fa on fa.attrelid = c.confrelid and fa.attnum = c.confkey[conseq.ord] where c.contype = 'f' and tc.relname = {} and tn.nspname = {} group by c.conname, fn.nspname, fc.relname, c.confupdtype, c.confdeltype",
                self.quote_string(table),
                schema.map(|s| self.quote_string(s)).unwrap_or_else(|| "current_schema()".into())
            ),
        }
    }

    /// Compile the command to enable foreign key constraints.
    pub fn compile_enable_foreign_key_constraints(&self) -> String {
        match self.driver {
            Driver::Sqlite => "pragma foreign_keys = 1".into(),
            Driver::MySql | Driver::MariaDb => "SET FOREIGN_KEY_CHECKS=1;".into(),
            Driver::Postgres => "SET CONSTRAINTS ALL IMMEDIATE;".into(),
        }
    }

    /// Compile the command to disable foreign key constraints.
    pub fn compile_disable_foreign_key_constraints(&self) -> String {
        match self.driver {
            Driver::Sqlite => "pragma foreign_keys = 0".into(),
            Driver::MySql | Driver::MariaDb => "SET FOREIGN_KEY_CHECKS=0;".into(),
            Driver::Postgres => "SET CONSTRAINTS ALL DEFERRED;".into(),
        }
    }

    /// Compile the statement that drops the given tables.
    pub fn compile_drop_all_tables(&self, tables: &[String]) -> String {
        let names = self.escape_names(tables);
        match self.driver {
            Driver::Postgres => format!("drop table {} cascade", names.join(", ")),
            _ => format!("drop table {}", names.join(", ")),
        }
    }

    /// Compile the statement that drops the given views.
    pub fn compile_drop_all_views(&self, views: &[String]) -> String {
        let names = self.escape_names(views);
        match self.driver {
            Driver::Postgres => format!("drop view {} cascade", names.join(", ")),
            _ => format!("drop view {}", names.join(", ")),
        }
    }

    fn escape_names(&self, names: &[String]) -> Vec<String> {
        names
            .iter()
            .map(|name| name.split('.').map(|s| self.wrap_value(s)).collect::<Vec<_>>().join("."))
            .collect()
    }
}

/// PHP's `addslashes`.
fn add_slashes(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\'' | '"' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            '\0' => out.push_str("\\0"),
            other => out.push(other),
        }
    }
    out
}
