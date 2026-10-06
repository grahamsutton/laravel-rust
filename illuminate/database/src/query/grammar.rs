//! Query grammars: compiling a [`Builder`] into SQL for SQLite, MySQL /
//! MariaDB and PostgreSQL, mirroring Laravel's generated SQL.

use std::sync::LazyLock;

use illuminate_support::{Result, Value, ValueExt};
use regex::Regex;

use super::clauses::*;
use super::{Builder, JoinClause};
use crate::driver::{self, Driver};
use crate::error::UnsupportedOperation;
use crate::expression::{Ident, Operand, Record};

static AS_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s+as\s+").unwrap());
static ARRAY_KEYS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\[[^\]]+\])+$").unwrap());
static ARRAY_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]+)\]").unwrap());
static ESCAPED_QUOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\\+)?'").unwrap());
static LEADING_BOOLEAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)and |or ").unwrap());

/// A column referenced by an `upsert`'s update clause.
#[derive(Clone, Debug, PartialEq)]
pub enum UpsertColumn {
    /// Update the column with the value that was being inserted.
    Column(String),
    /// Update the column with the given value.
    Value(String, Operand),
}

/// Compiles query builders into SQL for a specific driver.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryGrammar {
    driver: Driver,
    prefix: String,
    use_upsert_alias: bool,
}

fn unsupported<T>(message: &str) -> Result<T> {
    Err(UnsupportedOperation(message.to_string()).into())
}

/// Determine if a string looks like an integer (PHP's `FILTER_VALIDATE_INT`).
fn is_int(value: &str) -> bool {
    value
        .parse::<i64>()
        .map(|i| i.to_string() == value)
        .unwrap_or(false)
}

/// Remove the leading boolean from a list of compiled clauses.
fn remove_leading_boolean(value: &str) -> String {
    LEADING_BOOLEAN.replacen(value, 1, "").into_owned()
}

/// Everything before the last occurrence of `search`.
fn before_last<'a>(subject: &'a str, search: &str) -> &'a str {
    match subject.rfind(search) {
        Some(index) => &subject[..index],
        None => subject,
    }
}

impl QueryGrammar {
    /// Create a grammar for the given driver and table prefix.
    pub fn new(driver: Driver, prefix: impl Into<String>) -> Self {
        Self {
            driver,
            prefix: prefix.into(),
            use_upsert_alias: false,
        }
    }

    /// Use MySQL's `as laravel_upsert_alias` upsert syntax (MySQL 8.0.19+).
    pub fn with_upsert_alias(mut self, value: bool) -> Self {
        self.use_upsert_alias = value;
        self
    }

    /// The driver this grammar compiles for.
    pub fn driver(&self) -> Driver {
        self.driver
    }

    /// The table prefix.
    pub fn table_prefix(&self) -> &str {
        &self.prefix
    }

    /// The format dates are stored in.
    pub fn date_format(&self) -> &'static str {
        "Y-m-d H:i:s"
    }

    fn is_mysql(&self) -> bool {
        self.driver.is_mysql_family()
    }

    // ------------------------------------------------------------------
    // Wrapping
    // ------------------------------------------------------------------

    /// Wrap a single string in keyword identifiers.
    pub fn wrap_value(&self, value: &str) -> String {
        if value == "*" {
            return value.to_string();
        }
        if self.is_mysql() {
            format!("`{}`", value.replace('`', "``"))
        } else {
            format!("\"{}\"", value.replace('"', "\"\""))
        }
    }

    /// Wrap a table in keyword identifiers, applying the table prefix.
    pub fn wrap_table(&self, table: &Ident) -> String {
        match table {
            Ident::Raw(expression) => expression.value().to_string(),
            Ident::Name(name) => self.wrap_table_str(name),
        }
    }

    /// Wrap a table name in keyword identifiers, applying the table prefix.
    pub fn wrap_table_str(&self, table: &str) -> String {
        self.wrap_table_with_prefix(table, &self.prefix)
    }

    pub(crate) fn wrap_table_with_prefix(&self, table: &str, prefix: &str) -> String {
        if table.to_lowercase().contains(" as ") {
            let segments: Vec<&str> = AS_SPLIT.split(table).collect();
            return format!(
                "{} as {}",
                self.wrap_table_with_prefix(segments[0], prefix),
                self.wrap_value(&format!("{prefix}{}", segments.get(1).copied().unwrap_or("")))
            );
        }
        if let Some(index) = table.rfind('.') {
            let prefixed = format!("{}.{prefix}{}", &table[..index], &table[index + 1..]);
            return prefixed
                .split('.')
                .map(|segment| self.wrap_value(segment))
                .collect::<Vec<_>>()
                .join(".");
        }
        self.wrap_value(&format!("{prefix}{table}"))
    }

    /// Wrap a value (column, `table.column`, `column as alias`, JSON path)
    /// in keyword identifiers.
    pub fn wrap(&self, value: &Ident) -> String {
        match value {
            Ident::Raw(expression) => expression.value().to_string(),
            Ident::Name(name) => self.wrap_str(name),
        }
    }

    /// Wrap a column name in keyword identifiers.
    pub fn wrap_str(&self, value: &str) -> String {
        if value.to_lowercase().contains(" as ") {
            let segments: Vec<&str> = AS_SPLIT.split(value).collect();
            return format!(
                "{} as {}",
                self.wrap_str(segments[0]),
                self.wrap_value(segments.get(1).copied().unwrap_or(""))
            );
        }
        if self.is_json_selector(value) {
            return self.wrap_json_selector(value);
        }
        self.wrap_segments(&value.split('.').collect::<Vec<_>>())
    }

    fn wrap_segments(&self, segments: &[&str]) -> String {
        segments
            .iter()
            .enumerate()
            .map(|(i, segment)| {
                if i == 0 && segments.len() > 1 {
                    self.wrap_table_str(segment)
                } else {
                    self.wrap_value(segment)
                }
            })
            .collect::<Vec<_>>()
            .join(".")
    }

    /// Convert a list of column names into a delimited string.
    pub fn columnize(&self, columns: &[Ident]) -> String {
        columns.iter().map(|c| self.wrap(c)).collect::<Vec<_>>().join(", ")
    }

    fn columnize_str(&self, columns: &[String]) -> String {
        columns.iter().map(|c| self.wrap_str(c)).collect::<Vec<_>>().join(", ")
    }

    /// Get the appropriate query parameter place-holder for a value.
    pub fn parameter(&self, value: &Operand) -> String {
        match value {
            Operand::Raw(expression) => expression.value().to_string(),
            Operand::Value(_) => "?".to_string(),
        }
    }

    /// Create query parameter place-holders for a list of values.
    pub fn parameterize(&self, values: &[Operand]) -> String {
        values.iter().map(|v| self.parameter(v)).collect::<Vec<_>>().join(", ")
    }

    /// Quote the given string literal (unescaped, like Laravel's `quoteString`).
    pub fn quote_string(&self, value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }

    // ------------------------------------------------------------------
    // JSON paths
    // ------------------------------------------------------------------

    /// Determine if the given string is a JSON selector (`options->language`).
    pub fn is_json_selector(&self, value: &str) -> bool {
        value.contains("->")
    }

    pub(crate) fn wrap_json_field_and_path(&self, column: &str) -> (String, String) {
        let mut parts = column.splitn(2, "->");
        let field = self.wrap_str(parts.next().unwrap_or(""));
        let path = match parts.next() {
            Some(path) => format!(", {}", self.wrap_json_path(path, "->")),
            None => String::new(),
        };
        (field, path)
    }

    pub(crate) fn wrap_json_path(&self, value: &str, delimiter: &str) -> String {
        let value = ESCAPED_QUOTE.replace_all(value, "''");
        let path = value
            .split(delimiter)
            .map(|segment| self.wrap_json_path_segment(segment))
            .collect::<Vec<_>>()
            .join(".");
        let separator = if path.starts_with('[') { "" } else { "." };
        format!("'${separator}{path}'")
    }

    fn wrap_json_path_segment(&self, segment: &str) -> String {
        if let Some(found) = ARRAY_KEYS.find(segment) {
            let key = before_last(segment, found.as_str());
            if !key.is_empty() {
                return format!("\"{key}\"{}", found.as_str());
            }
            return found.as_str().to_string();
        }
        format!("\"{segment}\"")
    }

    /// Wrap a JSON selector for the driver.
    pub fn wrap_json_selector(&self, value: &str) -> String {
        match self.driver {
            Driver::Sqlite => {
                let (field, path) = self.wrap_json_field_and_path(value);
                format!("json_extract({field}{path})")
            }
            Driver::MySql => {
                let (field, path) = self.wrap_json_field_and_path(value);
                format!("json_unquote(json_extract({field}{path}))")
            }
            Driver::MariaDb => {
                let (field, path) = self.wrap_json_field_and_path(value);
                format!("json_value({field}{path})")
            }
            Driver::Postgres => {
                let mut path: Vec<&str> = value.split("->").collect();
                let first = path.remove(0);
                let field = self.wrap_segments(&first.split('.').collect::<Vec<_>>());
                let mut wrapped = self.wrap_json_path_attributes(&path, "'");
                let attribute = wrapped.pop().unwrap_or_default();
                if wrapped.is_empty() {
                    format!("{field}->>{attribute}")
                } else {
                    format!("{field}->{}->>{attribute}", wrapped.join("->"))
                }
            }
        }
    }

    fn wrap_json_path_attributes(&self, path: &[&str], quote: &str) -> Vec<String> {
        path.iter()
            .flat_map(|attribute| self.parse_json_path_array_keys(attribute))
            .map(|attribute| {
                if is_int(&attribute) {
                    return attribute;
                }
                let mut attribute = attribute.replace('\'', "''");
                if quote != "'" {
                    attribute = attribute.replace(quote, &format!("{quote}{quote}"));
                }
                format!("{quote}{attribute}{quote}")
            })
            .collect()
    }

    fn parse_json_path_array_keys(&self, attribute: &str) -> Vec<String> {
        if let Some(found) = ARRAY_KEYS.find(attribute) {
            let key = before_last(attribute, found.as_str());
            let mut keys = vec![key.to_string()];
            keys.extend(
                ARRAY_KEY
                    .captures_iter(found.as_str())
                    .map(|c| c[1].to_string()),
            );
            return keys.into_iter().filter(|k| !k.is_empty()).collect();
        }
        vec![attribute.to_string()]
    }

    fn wrap_json_boolean_selector(&self, value: &str) -> String {
        match self.driver {
            Driver::MySql | Driver::MariaDb => {
                let (field, path) = self.wrap_json_field_and_path(value);
                format!("json_extract({field}{path})")
            }
            Driver::Postgres => format!("({})::jsonb", self.wrap_json_selector(value).replace("->>", "->")),
            Driver::Sqlite => self.wrap_json_selector(value),
        }
    }

    fn wrap_json_boolean_value(&self, value: &str) -> String {
        match self.driver {
            Driver::Postgres => format!("'{value}'::jsonb"),
            _ => value.to_string(),
        }
    }

    /// Prepare a binding for a JSON contains clause.
    pub fn prepare_binding_for_json_contains(&self, value: &Value) -> Value {
        match self.driver {
            Driver::Sqlite => value.clone(),
            _ => Value::String(value.to_string()),
        }
    }

    /// Prepare the binding of a `where_like` clause.
    pub fn prepare_where_like_binding(&self, value: &Value, case_sensitive: bool) -> Value {
        if self.driver == Driver::Sqlite && case_sensitive {
            if let Value::String(s) = value {
                let mut out = String::with_capacity(s.len());
                for ch in s.chars() {
                    match ch {
                        '*' => out.push_str("[*]"),
                        '?' => out.push_str("[?]"),
                        '%' => out.push('*'),
                        '_' => out.push('?'),
                        other => out.push(other),
                    }
                }
                return Value::String(out);
            }
        }
        value.clone()
    }

    // ------------------------------------------------------------------
    // Select statements
    // ------------------------------------------------------------------

    /// Compile a select query into SQL.
    pub fn compile_select(&self, query: &Builder) -> Result<String> {
        if (!query.unions.is_empty() || !query.havings.is_empty()) && query.aggregate.is_some() {
            return self.compile_union_aggregate(query);
        }

        let mut sql = self.concatenate(self.compile_components(query)?);

        if !query.unions.is_empty() {
            sql = format!("{} {}", self.wrap_union(&sql), self.compile_unions(query)?);
        }

        Ok(sql)
    }

    fn concatenate(&self, segments: Vec<String>) -> String {
        segments
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string()
    }

    fn compile_components(&self, query: &Builder) -> Result<Vec<String>> {
        let mut sql = Vec::new();
        if let Some(aggregate) = &query.aggregate {
            sql.push(self.compile_aggregate(query, aggregate));
        } else {
            sql.push(self.compile_columns(query));
        }
        if let Some(from) = &query.from {
            sql.push(format!("from {}", self.wrap_table(from)));
        }
        if !query.joins.is_empty() {
            sql.push(self.compile_joins(&query.joins)?);
        }
        sql.push(self.compile_wheres(query)?);
        if !query.groups.is_empty() {
            sql.push(format!("group by {}", self.columnize(&query.groups)));
        }
        if !query.havings.is_empty() {
            sql.push(self.compile_havings(&query.havings)?);
        }
        sql.push(self.compile_orders(&query.orders));
        if let Some(limit) = query.limit {
            sql.push(format!("limit {limit}"));
        }
        if let Some(offset) = query.offset {
            sql.push(format!("offset {offset}"));
        }
        if let Some(lock) = &query.lock {
            sql.push(self.compile_lock(lock));
        }
        Ok(sql)
    }

    fn compile_aggregate(&self, query: &Builder, aggregate: &Aggregate) -> String {
        let mut column = self.columnize(&aggregate.columns);
        match &query.distinct {
            Distinct::Columns(columns) => column = format!("distinct {}", self.columnize(columns)),
            Distinct::Yes if column != "*" => column = format!("distinct {column}"),
            _ => {}
        }
        format!(
            "select {}({column}) as {}",
            aggregate.function,
            self.wrap_value("aggregate")
        )
    }

    fn compile_columns(&self, query: &Builder) -> String {
        let star = [Ident::from("*")];
        let columns = query.columns.as_deref().unwrap_or(&star);
        let select = match (&query.distinct, self.driver) {
            (Distinct::Columns(on), Driver::Postgres) => {
                format!("select distinct on ({}) ", self.columnize(on))
            }
            (Distinct::No, _) => "select ".to_string(),
            _ => "select distinct ".to_string(),
        };
        format!("{select}{}", self.columnize(columns))
    }

    /// Compile the join clauses of a query.
    pub fn compile_joins(&self, joins: &[JoinClause]) -> Result<String> {
        let mut out = Vec::new();
        for join in joins {
            let table = self.wrap_table(&join.table);
            let wheres = self.compile_wheres(&join.query)?;
            out.push(format!("{} join {table} {wheres}", join.kind).trim().to_string());
        }
        Ok(out.join(" "))
    }

    /// Compile the where clauses of a query (`where ...`, or `on ...` for joins).
    pub fn compile_wheres(&self, query: &Builder) -> Result<String> {
        match self.compile_where_list(query)? {
            Some(list) => {
                let conjunction = if query.is_join_clause { "on" } else { "where" };
                Ok(format!("{conjunction} {list}"))
            }
            None => Ok(String::new()),
        }
    }

    fn compile_where_list(&self, query: &Builder) -> Result<Option<String>> {
        if query.wheres.is_empty() {
            return Ok(None);
        }
        let mut parts = Vec::with_capacity(query.wheres.len());
        for clause in &query.wheres {
            parts.push(format!("{} {}", clause.boolean, self.compile_where(query, clause)?));
        }
        Ok(Some(remove_leading_boolean(&parts.join(" "))))
    }

    fn compile_where(&self, _query: &Builder, clause: &Where) -> Result<String> {
        Ok(match &clause.kind {
            WhereKind::Basic {
                column,
                operator,
                value,
            } => self.where_basic(column, operator, value),
            WhereKind::JsonBoolean {
                column,
                operator,
                value,
            } => format!(
                "{} {operator} {}",
                self.wrap_json_boolean_selector(column),
                self.wrap_json_boolean_value(if *value { "true" } else { "false" })
            ),
            WhereKind::Bitwise {
                column,
                operator,
                value,
            } => match self.driver {
                Driver::Postgres => format!(
                    "({} {} {})::bool",
                    self.wrap(column),
                    operator.replace('?', "??"),
                    self.parameter(value)
                ),
                _ => self.where_basic(column, operator, value),
            },
            WhereKind::NullSafeEquals { column, value } => {
                let column = self.wrap(column);
                let value = self.parameter(value);
                match self.driver {
                    Driver::Sqlite => format!("{column} is {value}"),
                    Driver::MySql | Driver::MariaDb => format!("{column} <=> {value}"),
                    Driver::Postgres => format!("{column} is not distinct from {value}"),
                }
            }
            WhereKind::Raw { sql } => sql.clone(),
            WhereKind::In { column, values, not } => {
                if values.is_empty() {
                    if *not { "1 = 1" } else { "0 = 1" }.to_string()
                } else {
                    format!(
                        "{} {}in ({})",
                        self.wrap(column),
                        if *not { "not " } else { "" },
                        self.parameterize(values)
                    )
                }
            }
            WhereKind::InRaw { column, values, not } => {
                if values.is_empty() {
                    if *not { "1 = 1" } else { "0 = 1" }.to_string()
                } else {
                    format!(
                        "{} {}in ({})",
                        self.wrap(column),
                        if *not { "not " } else { "" },
                        values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")
                    )
                }
            }
            WhereKind::Null { column, not } => self.where_null(column, *not),
            WhereKind::Between {
                column,
                min,
                max,
                not,
            } => format!(
                "{} {} {} and {}",
                self.wrap(column),
                if *not { "not between" } else { "between" },
                self.parameter(min),
                self.parameter(max)
            ),
            WhereKind::BetweenColumns {
                column,
                min,
                max,
                not,
            } => format!(
                "{} {} {} and {}",
                self.wrap(column),
                if *not { "not between" } else { "between" },
                self.wrap(min),
                self.wrap(max)
            ),
            WhereKind::Date {
                part,
                column,
                operator,
                value,
            } => self.where_date(*part, column, operator, value),
            WhereKind::Column {
                first,
                operator,
                second,
            } => format!(
                "{} {} {}",
                self.wrap(first),
                operator.replace('?', "??"),
                self.wrap(second)
            ),
            WhereKind::Nested { query } => {
                format!("({})", self.compile_where_list(query)?.unwrap_or_default())
            }
            WhereKind::Sub {
                column,
                operator,
                query,
            } => format!("{} {operator} ({})", self.wrap(column), self.compile_select(query)?),
            WhereKind::Exists { query, not } => format!(
                "{}exists ({})",
                if *not { "not " } else { "" },
                self.compile_select(query)?
            ),
            WhereKind::Like {
                column,
                value,
                case_sensitive,
                not,
            } => {
                let operator = match self.driver {
                    Driver::Sqlite => match (*case_sensitive, *not) {
                        (false, false) => "like",
                        (false, true) => "not like",
                        (true, false) => "glob",
                        (true, true) => "not glob",
                    },
                    Driver::MySql | Driver::MariaDb => match (*case_sensitive, *not) {
                        (false, false) => "like",
                        (false, true) => "not like",
                        (true, false) => "like binary",
                        (true, true) => "not like binary",
                    },
                    Driver::Postgres => match (*case_sensitive, *not) {
                        (false, false) => "ilike",
                        (false, true) => "not ilike",
                        (true, false) => "like",
                        (true, true) => "not like",
                    },
                };
                self.where_basic(column, operator, value)
            }
            WhereKind::JsonContains { column, value, not } => format!(
                "{}{}",
                if *not { "not " } else { "" },
                self.compile_json_contains(column, &self.parameter(value))
            ),
            WhereKind::JsonContainsKey { column, not } => format!(
                "{}{}",
                if *not { "not " } else { "" },
                self.compile_json_contains_key(column)
            ),
            WhereKind::JsonLength {
                column,
                operator,
                value,
            } => self.compile_json_length(column, operator, &self.parameter(value)),
            WhereKind::FullText {
                columns,
                value,
                options,
            } => self.where_full_text(columns, value, options)?,
        })
    }

    fn where_basic(&self, column: &Ident, operator: &str, value: &Operand) -> String {
        let operator = operator.replace('?', "??");
        if self.driver == Driver::Postgres && operator.to_lowercase().contains("like") {
            return format!("{}::text {operator} {}", self.wrap(column), self.parameter(value));
        }
        format!("{} {operator} {}", self.wrap(column), self.parameter(value))
    }

    fn where_null(&self, column: &Ident, not: bool) -> String {
        if self.is_mysql() {
            if let Ident::Name(name) = column {
                if self.is_json_selector(name) {
                    let (field, path) = self.wrap_json_field_and_path(name);
                    return if not {
                        format!(
                            "(json_extract({field}{path}) is not null AND json_type(json_extract({field}{path})) != 'NULL')"
                        )
                    } else {
                        format!(
                            "(json_extract({field}{path}) is null OR json_type(json_extract({field}{path})) = 'NULL')"
                        )
                    };
                }
            }
        }
        format!("{} is {}null", self.wrap(column), if not { "not " } else { "" })
    }

    fn where_date(&self, part: DatePart, column: &Ident, operator: &str, value: &Operand) -> String {
        let parameter = self.parameter(value);
        let wrapped = self.wrap(column);
        match self.driver {
            Driver::Sqlite => {
                let format = match part {
                    DatePart::Date => "%Y-%m-%d",
                    DatePart::Time => "%H:%M:%S",
                    DatePart::Day => "%d",
                    DatePart::Month => "%m",
                    DatePart::Year => "%Y",
                };
                format!("strftime('{format}', {wrapped}) {operator} cast({parameter} as text)")
            }
            Driver::MySql | Driver::MariaDb => {
                let function = match part {
                    DatePart::Date => "date",
                    DatePart::Time => "time",
                    DatePart::Day => "day",
                    DatePart::Month => "month",
                    DatePart::Year => "year",
                };
                format!("{function}({wrapped}) {operator} {parameter}")
            }
            Driver::Postgres => {
                let is_json = column.as_name().is_some_and(|n| self.is_json_selector(n));
                let cast_column = if is_json { format!("({wrapped})") } else { wrapped.clone() };
                match part {
                    DatePart::Date => format!("{cast_column}::date {operator} {parameter}"),
                    DatePart::Time => format!("{cast_column}::time {operator} {parameter}"),
                    DatePart::Day => format!("extract(day from {wrapped}) {operator} {parameter}"),
                    DatePart::Month => format!("extract(month from {wrapped}) {operator} {parameter}"),
                    DatePart::Year => format!("extract(year from {wrapped}) {operator} {parameter}"),
                }
            }
        }
    }

    fn compile_json_contains(&self, column: &str, value: &str) -> String {
        match self.driver {
            Driver::Sqlite => {
                let (field, path) = self.wrap_json_field_and_path(column);
                format!(
                    "exists (select 1 from json_each({field}{path}) where {} is {value})",
                    self.wrap_str("json_each.value")
                )
            }
            Driver::MySql | Driver::MariaDb => {
                let (field, path) = self.wrap_json_field_and_path(column);
                format!("json_contains({field}, {value}{path})")
            }
            Driver::Postgres => {
                let column = self.wrap_str(column).replace("->>", "->");
                format!("({column})::jsonb @> {value}")
            }
        }
    }

    fn compile_json_contains_key(&self, column: &str) -> String {
        match self.driver {
            Driver::Sqlite => {
                let (field, path) = self.wrap_json_field_and_path(column);
                format!("json_type({field}{path}) is not null")
            }
            Driver::MySql | Driver::MariaDb => {
                let (field, path) = self.wrap_json_field_and_path(column);
                format!("ifnull(json_contains_path({field}, 'one'{path}), 0)")
            }
            Driver::Postgres => {
                let mut segments: Vec<String> = column.split("->").map(String::from).collect();
                let last = segments.pop().unwrap_or_default();
                let mut index: Option<i64> = None;
                if is_int(&last) {
                    index = last.parse().ok();
                } else if let Some(caps) = Regex::new(r"\[(-?[0-9]+)\]$").unwrap().captures(&last) {
                    segments.push(before_last(&last, &caps[0]).to_string());
                    index = caps[1].parse().ok();
                }
                let wrapped = self.wrap_str(&segments.join("->")).replace("->>", "->");
                match index {
                    Some(i) => format!(
                        "case when jsonb_typeof(({wrapped})::jsonb) = 'array' then jsonb_array_length(({wrapped})::jsonb) >= {} else false end",
                        if i < 0 { i.abs() } else { i + 1 }
                    ),
                    None => format!(
                        "coalesce(({wrapped})::jsonb ?? '{}', false)",
                        last.replace('\'', "''")
                    ),
                }
            }
        }
    }

    fn compile_json_length(&self, column: &str, operator: &str, value: &str) -> String {
        match self.driver {
            Driver::Sqlite => {
                let (field, path) = self.wrap_json_field_and_path(column);
                format!("json_array_length({field}{path}) {operator} {value}")
            }
            Driver::MySql | Driver::MariaDb => {
                let (field, path) = self.wrap_json_field_and_path(column);
                format!("json_length({field}{path}) {operator} {value}")
            }
            Driver::Postgres => {
                let column = self.wrap_str(column).replace("->>", "->");
                format!("jsonb_array_length(({column})::jsonb) {operator} {value}")
            }
        }
    }

    fn where_full_text(&self, columns: &[String], value: &Operand, options: &FullTextOptions) -> Result<String> {
        match self.driver {
            Driver::MySql | Driver::MariaDb => {
                let boolean = options.mode.as_deref() == Some("boolean");
                let mode = if boolean { " in boolean mode" } else { " in natural language mode" };
                let expanded = if options.expanded && !boolean { " with query expansion" } else { "" };
                Ok(format!(
                    "match ({}) against ({}{mode}{expanded})",
                    self.columnize_str(columns),
                    self.parameter(value)
                ))
            }
            Driver::Postgres => {
                const LANGUAGES: [&str; 22] = [
                    "simple", "arabic", "danish", "dutch", "english", "finnish", "french", "german",
                    "hungarian", "indonesian", "irish", "italian", "lithuanian", "nepali", "norwegian",
                    "portuguese", "romanian", "russian", "spanish", "swedish", "tamil", "turkish",
                ];
                let language = options
                    .language
                    .as_deref()
                    .filter(|l| LANGUAGES.contains(l))
                    .unwrap_or("english");
                let columns = columns
                    .iter()
                    .map(|c| format!("to_tsvector('{language}', {})", self.wrap_str(c)))
                    .collect::<Vec<_>>()
                    .join(" || ");
                let mode = match options.mode.as_deref() {
                    Some("phrase") => "phraseto_tsquery",
                    Some("websearch") => "websearch_to_tsquery",
                    Some("raw") => "to_tsquery",
                    _ => "plainto_tsquery",
                };
                Ok(format!(
                    "({columns}) @@ {mode}('{language}', {})",
                    self.parameter(value)
                ))
            }
            Driver::Sqlite => unsupported("This database engine does not support fulltext search operations."),
        }
    }

    /// Compile the having clauses of a query.
    pub fn compile_havings(&self, havings: &[Having]) -> Result<String> {
        Ok(format!("having {}", self.compile_having_list(havings)?))
    }

    fn compile_having_list(&self, havings: &[Having]) -> Result<String> {
        let mut parts = Vec::with_capacity(havings.len());
        for having in havings {
            let sql = match &having.kind {
                HavingKind::Raw { sql } => sql.clone(),
                HavingKind::Between {
                    column,
                    min,
                    max,
                    not,
                } => format!(
                    "{} {} {} and {}",
                    self.wrap(column),
                    if *not { "not between" } else { "between" },
                    self.parameter(min),
                    self.parameter(max)
                ),
                HavingKind::Null { column, not } => {
                    format!("{} is {}null", self.wrap(column), if *not { "not " } else { "" })
                }
                HavingKind::Bitwise {
                    column,
                    operator,
                    value,
                } if self.driver == Driver::Postgres => format!(
                    "({} {operator} {})::bool",
                    self.wrap(column),
                    self.parameter(value)
                ),
                HavingKind::Bitwise {
                    column,
                    operator,
                    value,
                }
                | HavingKind::Basic {
                    column,
                    operator,
                    value,
                } => format!("{} {operator} {}", self.wrap(column), self.parameter(value)),
                HavingKind::Nested { query } => {
                    format!("({})", self.compile_having_list(&query.havings)?)
                }
            };
            parts.push(format!("{} {sql}", having.boolean));
        }
        Ok(remove_leading_boolean(&parts.join(" ")))
    }

    /// Compile the order by clauses of a query.
    pub fn compile_orders(&self, orders: &[Order]) -> String {
        if orders.is_empty() {
            return String::new();
        }
        let orders = orders
            .iter()
            .map(|order| match order {
                Order::Raw { sql } => sql.clone(),
                Order::Column { column, direction } => {
                    format!("{} {}", self.wrap(column), direction.as_str())
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("order by {orders}")
    }

    /// Compile the random statement into SQL.
    pub fn compile_random(&self, seed: &str) -> String {
        match self.driver {
            Driver::MySql | Driver::MariaDb => {
                if seed.is_empty() {
                    "RAND()".to_string()
                } else {
                    format!("RAND({})", seed.parse::<i64>().unwrap_or_default())
                }
            }
            _ => "RANDOM()".to_string(),
        }
    }

    fn compile_lock(&self, lock: &Lock) -> String {
        match (self.driver, lock) {
            (Driver::Sqlite, _) => String::new(),
            (_, Lock::Raw(sql)) => sql.clone(),
            (_, Lock::Update) => "for update".to_string(),
            (Driver::Postgres, Lock::Shared) => "for share".to_string(),
            (_, Lock::Shared) => "lock in share mode".to_string(),
        }
    }

    fn wrap_union(&self, sql: &str) -> String {
        match self.driver {
            Driver::Sqlite => format!("select * from ({sql})"),
            _ => format!("({sql})"),
        }
    }

    fn compile_unions(&self, query: &Builder) -> Result<String> {
        let mut sql = String::new();
        for union in &query.unions {
            let conjunction = if union.all { " union all " } else { " union " };
            sql.push_str(conjunction);
            sql.push_str(&self.wrap_union(&self.compile_select(&union.query)?));
        }
        if !query.union_orders.is_empty() {
            sql.push(' ');
            sql.push_str(&self.compile_orders(&query.union_orders));
        }
        if let Some(limit) = query.union_limit {
            sql.push_str(&format!(" limit {limit}"));
        }
        if let Some(offset) = query.union_offset {
            sql.push_str(&format!(" offset {offset}"));
        }
        Ok(sql.trim_start().to_string())
    }

    fn compile_union_aggregate(&self, query: &Builder) -> Result<String> {
        let aggregate = query.aggregate.clone().unwrap_or(Aggregate {
            function: "count".into(),
            columns: vec![Ident::from("*")],
        });
        let sql = self.compile_aggregate(query, &aggregate);
        let mut inner = query.clone();
        inner.aggregate = None;
        Ok(format!(
            "{sql} from ({}) as {}",
            self.compile_select(&inner)?,
            self.wrap_table_str("temp_table")
        ))
    }

    /// Compile an exists statement into SQL.
    pub fn compile_exists(&self, query: &Builder) -> Result<String> {
        Ok(format!(
            "select exists({}) as {}",
            self.compile_select(query)?,
            self.wrap_value("exists")
        ))
    }

    // ------------------------------------------------------------------
    // Inserts
    // ------------------------------------------------------------------

    fn from_table(&self, query: &Builder) -> String {
        query
            .from
            .as_ref()
            .map(|table| self.wrap_table(table))
            .unwrap_or_default()
    }

    /// Compile an insert statement into SQL.
    pub fn compile_insert(&self, query: &Builder, records: &[Record]) -> String {
        let table = self.from_table(query);
        let empty = records.is_empty() || records.iter().all(|r| r.is_empty());
        if empty {
            return if self.is_mysql() {
                format!("insert into {table} () values ()")
            } else {
                format!("insert into {table} default values")
            };
        }
        let columns = records[0]
            .iter()
            .map(|(column, _)| self.wrap_str(column))
            .collect::<Vec<_>>()
            .join(", ");
        let parameters = records
            .iter()
            .map(|record| {
                let values: Vec<Operand> = record.iter().map(|(_, v)| v.clone()).collect();
                format!("({})", self.parameterize(&values))
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("insert into {table} ({columns}) values {parameters}")
    }

    /// Compile an insert ignore statement into SQL.
    pub fn compile_insert_or_ignore(&self, query: &Builder, records: &[Record]) -> String {
        let insert = self.compile_insert(query, records);
        match self.driver {
            Driver::Sqlite => insert.replacen("insert", "insert or ignore", 1),
            Driver::MySql | Driver::MariaDb => insert.replacen("insert", "insert ignore", 1),
            Driver::Postgres => format!("{insert} on conflict do nothing"),
        }
    }

    /// Compile an insert and get ID statement into SQL.
    pub fn compile_insert_get_id(&self, query: &Builder, records: &[Record], sequence: &str) -> String {
        let insert = self.compile_insert(query, records);
        match self.driver {
            Driver::Postgres => format!("{insert} returning {}", self.wrap_str(sequence)),
            _ => insert,
        }
    }

    /// Compile an insert statement using a sub-query into SQL.
    pub fn compile_insert_using(&self, query: &Builder, columns: &[String], sql: &str) -> String {
        let table = self.from_table(query);
        if columns.is_empty() || (columns.len() == 1 && columns[0] == "*") {
            return format!("insert into {table} {sql}");
        }
        format!("insert into {table} ({}) {sql}", self.columnize_str(columns))
    }

    /// Compile an insert-or-ignore statement using a sub-query into SQL.
    pub fn compile_insert_or_ignore_using(&self, query: &Builder, columns: &[String], sql: &str) -> String {
        let insert = self.compile_insert_using(query, columns, sql);
        match self.driver {
            Driver::Sqlite => insert.replacen("insert", "insert or ignore", 1),
            Driver::MySql | Driver::MariaDb => insert.replacen("insert", "insert ignore", 1),
            Driver::Postgres => format!("{insert} on conflict do nothing"),
        }
    }

    /// Compile an "upsert" statement into SQL.
    pub fn compile_upsert(
        &self,
        query: &Builder,
        records: &[Record],
        unique_by: &[String],
        update: &[UpsertColumn],
    ) -> String {
        let mut sql = self.compile_insert(query, records);
        match self.driver {
            Driver::MySql | Driver::MariaDb => {
                if self.use_upsert_alias {
                    sql.push_str(" as laravel_upsert_alias");
                }
                sql.push_str(" on duplicate key update ");
                let columns = update
                    .iter()
                    .map(|column| match column {
                        UpsertColumn::Value(column, value) => {
                            format!("{} = {}", self.wrap_str(column), self.parameter(value))
                        }
                        UpsertColumn::Column(column) if self.use_upsert_alias => format!(
                            "{} = {}.{}",
                            self.wrap_str(column),
                            self.wrap_str("laravel_upsert_alias"),
                            self.wrap_str(column)
                        ),
                        UpsertColumn::Column(column) => format!(
                            "{} = values({})",
                            self.wrap_str(column),
                            self.wrap_str(column)
                        ),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                sql.push_str(&columns);
            }
            _ => {
                sql.push_str(&format!(
                    " on conflict ({}) do update set ",
                    self.columnize_str(unique_by)
                ));
                let columns = update
                    .iter()
                    .map(|column| match column {
                        UpsertColumn::Value(column, value) => {
                            format!("{} = {}", self.wrap_str(column), self.parameter(value))
                        }
                        UpsertColumn::Column(column) => format!(
                            "{} = {}.{}",
                            self.wrap_str(column),
                            self.wrap_value("excluded"),
                            self.wrap_str(column)
                        ),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                sql.push_str(&columns);
            }
        }
        sql
    }

    // ------------------------------------------------------------------
    // Updates
    // ------------------------------------------------------------------

    /// Compile an update statement into SQL.
    pub fn compile_update(&self, query: &Builder, values: &Record) -> Result<String> {
        let table = self.from_table(query);
        let columns = self.compile_update_columns(values);

        match self.driver {
            Driver::Sqlite | Driver::Postgres if !query.joins.is_empty() || query.limit.is_some() => {
                let row_id = if self.driver == Driver::Sqlite { "rowid" } else { "ctid" };
                let select = self.row_id_select(query, row_id)?;
                Ok(format!(
                    "update {table} set {columns} where {} in ({select})",
                    self.wrap_str(row_id)
                ))
            }
            Driver::MySql | Driver::MariaDb => {
                let wheres = self.compile_wheres(query)?;
                if !query.joins.is_empty() {
                    let joins = self.compile_joins(&query.joins)?;
                    return Ok(format!("update {table} {joins} set {columns} {wheres}")
                        .trim()
                        .to_string());
                }
                let mut sql = format!("update {table} set {columns} {wheres}").trim().to_string();
                if !query.orders.is_empty() {
                    sql = format!("{sql} {}", self.compile_orders(&query.orders));
                }
                if let Some(limit) = query.limit {
                    sql = format!("{sql} limit {limit}");
                }
                Ok(sql)
            }
            _ => {
                let wheres = self.compile_wheres(query)?;
                Ok(format!("update {table} set {columns} {wheres}").trim().to_string())
            }
        }
    }

    /// `select "alias"."rowid" from ...`, used by updates / deletes with joins or limits.
    fn row_id_select(&self, query: &Builder, row_id: &str) -> Result<String> {
        let from = query
            .from
            .as_ref()
            .and_then(|f| f.as_name().map(String::from))
            .unwrap_or_default();
        let alias = AS_SPLIT.split(&from).last().unwrap_or("").to_string();
        let mut select = query.clone();
        select.columns = Some(vec![Ident::Name(format!("{alias}.{row_id}"))]);
        select.aggregate = None;
        self.compile_select(&select)
    }

    fn compile_update_columns(&self, values: &Record) -> String {
        match self.driver {
            Driver::Sqlite => {
                let groups = self.group_json_columns_for_update(values);
                let mut parts: Vec<String> = values
                    .iter()
                    .filter(|(key, _)| !self.is_json_selector(key))
                    .map(|(key, value)| {
                        let column = key.rsplit('.').next().unwrap_or(key);
                        format!("{} = {}", self.wrap_str(column), self.parameter(value))
                    })
                    .collect();
                for (column, _) in groups {
                    parts.push(format!(
                        "{} = json_patch(ifnull({}, json('{{}}')), json(?))",
                        self.wrap_str(&column),
                        self.wrap_str(&column)
                    ));
                }
                parts.join(", ")
            }
            Driver::MySql | Driver::MariaDb => values
                .iter()
                .map(|(key, value)| {
                    if self.is_json_selector(key) {
                        let value = match value {
                            Operand::Value(Value::Bool(b)) => if *b { "true" } else { "false" }.to_string(),
                            Operand::Value(Value::Array(_)) | Operand::Value(Value::Object(_)) => {
                                "cast(? as json)".to_string()
                            }
                            other => self.parameter(other),
                        };
                        let (field, path) = self.wrap_json_field_and_path(key);
                        format!("{field} = json_set({field}{path}, {value})")
                    } else {
                        format!("{} = {}", self.wrap_str(key), self.parameter(value))
                    }
                })
                .collect::<Vec<_>>()
                .join(", "),
            Driver::Postgres => values
                .iter()
                .map(|(key, value)| {
                    let column = key.rsplit('.').next().unwrap_or(key);
                    if self.is_json_selector(key) {
                        let mut segments: Vec<&str> = column.split("->").collect();
                        let field = self.wrap_str(segments.remove(0));
                        let path = format!("'{{{}}}'", self.wrap_json_path_attributes(&segments, "\"").join(","));
                        format!("{field} = jsonb_set({field}::jsonb, {path}, {})", self.parameter(value))
                    } else {
                        format!("{} = {}", self.wrap_str(column), self.parameter(value))
                    }
                })
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// Group `column->path` update values into a JSON patch per column (SQLite).
    fn group_json_columns_for_update(&self, values: &Record) -> Vec<(String, Value)> {
        let mut groups: Vec<(String, Value)> = Vec::new();
        for (key, value) in values {
            if !self.is_json_selector(key) {
                continue;
            }
            let after_table = match key.find('.') {
                Some(index) if index < key.find("->").unwrap_or(usize::MAX) => &key[index + 1..],
                _ => key.as_str(),
            };
            let mut path = after_table.split("->");
            let column = path.next().unwrap_or("").to_string();
            let keys: Vec<&str> = path.collect();
            let value = value.as_value().cloned().unwrap_or(Value::Null);

            let index = match groups.iter().position(|(c, _)| *c == column) {
                Some(index) => index,
                None => {
                    groups.push((column, Value::Object(Default::default())));
                    groups.len() - 1
                }
            };
            let mut target = &mut groups[index].1;
            for (i, key) in keys.iter().enumerate() {
                if !target.is_object() {
                    *target = Value::Object(Default::default());
                }
                let map = target.as_object_mut().expect("object");
                if i == keys.len() - 1 {
                    map.insert(key.to_string(), value.clone());
                    break;
                }
                target = map
                    .entry(key.to_string())
                    .or_insert_with(|| Value::Object(Default::default()));
            }
        }
        groups
    }

    /// Prepare the bindings for an update statement.
    pub fn prepare_bindings_for_update(&self, bindings: &Bindings, values: &Record) -> Vec<Value> {
        let encode = |value: &Value| match value {
            Value::Array(_) | Value::Object(_) => Value::String(value.to_string()),
            other => other.clone(),
        };
        match self.driver {
            Driver::Sqlite => {
                let mut out: Vec<Value> = values
                    .iter()
                    .filter(|(key, _)| !self.is_json_selector(key))
                    .filter_map(|(_, v)| v.as_value().map(encode))
                    .collect();
                for (_, group) in self.group_json_columns_for_update(values) {
                    out.push(Value::String(group.to_string()));
                }
                out.extend(bindings.flatten_except(&[BindingType::Select]));
                out
            }
            Driver::MySql | Driver::MariaDb => {
                let mut out = bindings.join.clone();
                out.extend(
                    values
                        .iter()
                        .filter(|(key, value)| {
                            !(self.is_json_selector(key) && matches!(value, Operand::Value(Value::Bool(_))))
                        })
                        .filter_map(|(_, v)| v.as_value().map(encode)),
                );
                out.extend(bindings.flatten_except(&[BindingType::Select, BindingType::Join]));
                out
            }
            Driver::Postgres => {
                let mut out: Vec<Value> = values
                    .iter()
                    .filter_map(|(key, value)| {
                        value.as_value().map(|v| {
                            if self.is_json_selector(key) || v.is_array() || v.is_object() {
                                Value::String(v.to_string())
                            } else {
                                v.clone()
                            }
                        })
                    })
                    .collect();
                out.extend(bindings.flatten_except(&[BindingType::Select]));
                out
            }
        }
    }

    // ------------------------------------------------------------------
    // Deletes
    // ------------------------------------------------------------------

    /// Compile a delete statement into SQL.
    pub fn compile_delete(&self, query: &Builder) -> Result<String> {
        let table = self.from_table(query);
        match self.driver {
            Driver::Sqlite | Driver::Postgres if !query.joins.is_empty() || query.limit.is_some() => {
                let row_id = if self.driver == Driver::Sqlite { "rowid" } else { "ctid" };
                let select = self.row_id_select(query, row_id)?;
                Ok(format!("delete from {table} where {} in ({select})", self.wrap_str(row_id)))
            }
            Driver::MySql | Driver::MariaDb => {
                let wheres = self.compile_wheres(query)?;
                let mut sql = if query.joins.is_empty() {
                    format!("delete from {table} {wheres}").trim().to_string()
                } else {
                    let alias = table.rsplit(" as ").next().unwrap_or(&table).to_string();
                    let joins = self.compile_joins(&query.joins)?;
                    format!("delete {alias} from {table} {joins} {wheres}").trim().to_string()
                };
                if !query.orders.is_empty() {
                    sql = format!("{sql} {}", self.compile_orders(&query.orders));
                }
                if let Some(limit) = query.limit {
                    sql = format!("{sql} limit {limit}");
                }
                Ok(sql)
            }
            _ => {
                let wheres = self.compile_wheres(query)?;
                Ok(format!("delete from {table} {wheres}").trim().to_string())
            }
        }
    }

    /// Prepare the bindings for a delete statement.
    pub fn prepare_bindings_for_delete(&self, bindings: &Bindings) -> Vec<Value> {
        bindings.flatten_except(&[BindingType::Select])
    }

    /// Compile a truncate table statement into SQL (one or more statements
    /// with their bindings).
    pub fn compile_truncate(&self, query: &Builder) -> Vec<(String, Vec<Value>)> {
        let table = self.from_table(query);
        match self.driver {
            Driver::Sqlite => {
                let name = query
                    .from
                    .as_ref()
                    .and_then(|f| f.as_name())
                    .unwrap_or_default()
                    .to_string();
                let (schema, name) = match name.split_once('.') {
                    Some((schema, name)) => (format!("{}.", self.wrap_value(schema)), name.to_string()),
                    None => (String::new(), name),
                };
                vec![
                    (
                        format!("delete from {schema}sqlite_sequence where name = ?"),
                        vec![Value::String(format!("{}{name}", self.prefix))],
                    ),
                    (format!("delete from {table}"), Vec::new()),
                ]
            }
            Driver::MySql | Driver::MariaDb => vec![(format!("truncate table {table}"), Vec::new())],
            Driver::Postgres => vec![(format!("truncate {table} restart identity cascade"), Vec::new())],
        }
    }

    // ------------------------------------------------------------------
    // Raw SQL helpers
    // ------------------------------------------------------------------

    /// Escape a value for safe SQL embedding.
    pub fn escape(&self, value: &Value) -> std::result::Result<String, String> {
        match value {
            Value::Null => Ok("null".to_string()),
            Value::Bool(b) => Ok(match self.driver {
                Driver::Postgres => if *b { "true" } else { "false" },
                _ => if *b { "1" } else { "0" },
            }
            .to_string()),
            Value::Number(n) => Ok(Value::Number(n.clone()).to_string_lossy()),
            Value::String(s) => driver::quote_string(self.driver, s),
            other => driver::quote_string(self.driver, &other.to_string()),
        }
    }

    /// Substitute the given bindings into the given raw SQL query.
    pub fn substitute_bindings_into_raw_sql(&self, sql: &str, bindings: &[Value]) -> String {
        driver::interpolate(
            sql,
            bindings,
            |value| self.escape(value),
            self.driver == Driver::Postgres,
        )
        .unwrap_or_else(|_| sql.to_string())
    }

    /// Convert `?` placeholders into PostgreSQL's numbered `$1, $2, ...`
    /// placeholders, leaving quoted strings and identifiers untouched.
    ///
    /// ```
    /// use illuminate_database::{Driver, QueryGrammar};
    ///
    /// let grammar = QueryGrammar::new(Driver::Postgres, "");
    /// assert_eq!(
    ///     grammar.numbered_placeholders("select * from \"t\" where a = ? and b = '?' and c = ?"),
    ///     "select * from \"t\" where a = $1 and b = '?' and c = $2"
    /// );
    /// ```
    pub fn numbered_placeholders(&self, sql: &str) -> String {
        let count = sql.matches('?').count();
        let placeholders: Vec<Value> = (1..=count).map(|i| Value::String(format!("${i}"))).collect();
        driver::interpolate(sql, &placeholders, |v| Ok(v.to_string_lossy()), true).unwrap_or_else(|_| sql.to_string())
    }
}
