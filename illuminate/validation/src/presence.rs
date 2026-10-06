//! Presence verifiers power the `unique` and `exists` rules.

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;

use illuminate_support::{Result, Value, ValueExt};

use crate::php::loose_eq;
use crate::rules::Condition;

/// Counts matching rows for the `unique` and `exists` rules — Laravel's
/// `PresenceVerifierInterface`.
///
/// The database component binds an implementation as `dyn PresenceVerifier`
/// in the container. `table` is exactly as written in the rule and may be
/// prefixed with a connection name (`mysql.users`); use [`split_table`] to
/// separate the two.
#[async_trait]
pub trait PresenceVerifier: Send + Sync {
    /// Count the rows where `column = value`, excluding the row whose
    /// `id_column` (default `id`) equals `exclude_id`, and applying the
    /// extra conditions.
    async fn count(
        &self,
        table: &str,
        column: &str,
        value: &Value,
        exclude_id: Option<&Value>,
        id_column: Option<&str>,
        extra: &[Condition],
    ) -> Result<usize>;

    /// Count the distinct values of `column` among `values`, applying the
    /// extra conditions.
    async fn multi_count(&self, table: &str, column: &str, values: &[Value], extra: &[Condition]) -> Result<usize>;
}

/// Split `connection.table` into its connection and table.
///
/// ```
/// use illuminate_validation::split_table;
///
/// assert_eq!(split_table("mysql.users"), (Some("mysql"), "users"));
/// assert_eq!(split_table("users"), (None, "users"));
/// ```
pub fn split_table(table: &str) -> (Option<&str>, &str) {
    match table.split_once('.') {
        Some((connection, table)) => (Some(connection), table),
        None => (None, table),
    }
}

/// An in-memory presence verifier, perfect for tests.
///
/// ```
/// use illuminate_validation::{ArrayPresenceVerifier, PresenceVerifier};
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let verifier = ArrayPresenceVerifier::new()
///     .with_table("users", vec![json!({"id": 1, "email": "taylor@laravel.com"})]);
///
/// assert_eq!(verifier.count("users", "email", &json!("taylor@laravel.com"), None, None, &[]).await.unwrap(), 1);
/// # });
/// ```
#[derive(Debug, Default)]
pub struct ArrayPresenceVerifier {
    tables: RwLock<HashMap<String, Vec<Value>>>,
}

impl ArrayPresenceVerifier {
    /// Create an empty verifier.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a table with the given rows, fluently.
    pub fn with_table(self, table: &str, rows: Vec<Value>) -> Self {
        self.set_table(table, rows);
        self
    }

    /// Replace a table's rows.
    pub fn set_table(&self, table: &str, rows: Vec<Value>) {
        self.tables.write().unwrap().insert(table.to_string(), rows);
    }

    /// Insert a row into a table.
    pub fn insert(&self, table: &str, row: Value) {
        self.tables.write().unwrap().entry(table.to_string()).or_default().push(row);
    }

    fn rows(&self, table: &str) -> Vec<Value> {
        let tables = self.tables.read().unwrap();
        if let Some(rows) = tables.get(table) {
            return rows.clone();
        }
        tables.get(split_table(table).1).cloned().unwrap_or_default()
    }

    fn column<'a>(row: &'a Value, column: &str) -> &'a Value {
        row.get(column).unwrap_or(&Value::Null)
    }

    fn matches(row: &Value, condition: &Condition) -> bool {
        let equals = |a: &Value, b: &Value| !a.is_null() && (loose_eq(a, b) || a.to_string_lossy() == b.to_string_lossy());
        match condition {
            Condition::Where(column, value) => equals(Self::column(row, column), value),
            Condition::WhereNot(column, value) => !equals(Self::column(row, column), value),
            Condition::WhereNull(column) => Self::column(row, column).is_null(),
            Condition::WhereNotNull(column) => !Self::column(row, column).is_null(),
            Condition::WhereIn(column, values) => values.iter().any(|v| equals(Self::column(row, column), v)),
            Condition::WhereNotIn(column, values) => !values.iter().any(|v| equals(Self::column(row, column), v)),
        }
    }
}

#[async_trait]
impl PresenceVerifier for ArrayPresenceVerifier {
    async fn count(
        &self,
        table: &str,
        column: &str,
        value: &Value,
        exclude_id: Option<&Value>,
        id_column: Option<&str>,
        extra: &[Condition],
    ) -> Result<usize> {
        let mut conditions = vec![Condition::Where(column.to_string(), value.clone())];
        if let Some(id) = exclude_id {
            if id.to_string_lossy() != "NULL" {
                conditions.push(Condition::WhereNot(id_column.unwrap_or("id").to_string(), id.clone()));
            }
        }
        conditions.extend(extra.iter().cloned());
        Ok(self
            .rows(table)
            .iter()
            .filter(|row| conditions.iter().all(|c| Self::matches(row, c)))
            .count())
    }

    async fn multi_count(&self, table: &str, column: &str, values: &[Value], extra: &[Condition]) -> Result<usize> {
        let mut conditions = vec![Condition::WhereIn(column.to_string(), values.to_vec())];
        conditions.extend(extra.iter().cloned());
        let mut seen: Vec<String> = Vec::new();
        for row in self.rows(table) {
            if conditions.iter().all(|c| Self::matches(&row, c)) {
                let value = Self::column(&row, column).to_string_lossy();
                if !seen.contains(&value) {
                    seen.push(value);
                }
            }
        }
        Ok(seen.len())
    }
}
