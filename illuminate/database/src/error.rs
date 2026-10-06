//! The database component's exceptions.

use std::fmt;

use illuminate_support::{Value, ValueExt};

/// Thrown when a query fails to run.
///
/// The message mirrors Laravel's format, including the connection name and
/// the SQL with its bindings substituted:
///
/// ```text
/// SQLSTATE[HY000]: General error: 1 no such table: users (Connection: sqlite, SQL: select * from "users" where "id" = 1)
/// ```
#[derive(Debug)]
pub struct QueryException {
    /// The name of the connection the query ran on.
    pub connection_name: String,
    /// The SQL of the failed query.
    pub sql: String,
    /// The bindings of the failed query.
    pub bindings: Vec<Value>,
    /// The underlying driver error.
    pub source: Box<dyn std::error::Error + Send + Sync + 'static>,
    message: String,
}

impl QueryException {
    /// Create a new query exception.
    pub fn new(
        connection_name: impl Into<String>,
        sql: impl Into<String>,
        bindings: Vec<Value>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    ) -> Self {
        let source = source.into();
        let message = describe_error(source.as_ref());
        Self {
            connection_name: connection_name.into(),
            sql: sql.into(),
            bindings,
            source,
            message,
        }
    }

    /// Get the connection name for the query.
    pub fn connection_name(&self) -> &str {
        &self.connection_name
    }

    /// Get the SQL for the query.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// Get the bindings for the query.
    pub fn bindings(&self) -> &[Value] {
        &self.bindings
    }

    /// The driver's error message, without the connection / SQL details.
    pub fn driver_message(&self) -> &str {
        &self.message
    }

    /// The SQLSTATE (or driver) error code, when the database reported one.
    pub fn code(&self) -> Option<String> {
        database_error(self.source.as_ref()).and_then(|e| e.code().map(|c| c.into_owned()))
    }

    /// Determine if the query failed because of a unique constraint
    /// violation (Laravel's `UniqueConstraintViolationException`).
    pub fn is_unique_constraint_violation(&self) -> bool {
        if let Some(error) = database_error(self.source.as_ref()) {
            if error.is_unique_violation() {
                return true;
            }
        }
        let message = self.message.to_lowercase();
        message.contains("unique constraint failed")
            || message.contains("duplicate entry")
            || message.contains("duplicate key value violates unique constraint")
    }

    /// Determine if the query failed because of a deadlock or lock timeout.
    pub fn is_concurrency_error(&self) -> bool {
        caused_by_concurrency_error(&self.message)
    }

    /// Determine if the query failed because the connection was lost.
    pub fn is_lost_connection(&self) -> bool {
        let message = self.message.to_lowercase();
        [
            "server has gone away",
            "no connection to the server",
            "lost connection",
            "broken pipe",
            "connection reset",
            "connection refused",
            "pool timed out",
            "pool closed",
            "connection timed out",
        ]
        .iter()
        .any(|needle| message.contains(needle))
    }
}

impl fmt::Display for QueryException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (Connection: {}, SQL: {})",
            self.message,
            self.connection_name,
            replace_array(&self.sql, &self.bindings)
        )
    }
}

impl std::error::Error for QueryException {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// Determine if an error message describes a deadlock / lock timeout.
pub(crate) fn caused_by_concurrency_error(message: &str) -> bool {
    let message = message.to_lowercase();
    [
        "deadlock found when trying to get lock",
        "deadlock detected",
        "the database file is locked",
        "database is locked",
        "database table is locked",
        "a table in the database is locked",
        "has been chosen as the deadlock victim",
        "lock wait timeout exceeded; try restarting transaction",
        "could not serialize access",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn database_error<'a>(
    error: &'a (dyn std::error::Error + Send + Sync + 'static),
) -> Option<&'a dyn sqlx::error::DatabaseError> {
    match error.downcast_ref::<sqlx::Error>() {
        Some(sqlx::Error::Database(db)) => Some(db.as_ref()),
        _ => None,
    }
}

/// Describe a driver error the way PDO would.
fn describe_error(error: &(dyn std::error::Error + Send + Sync + 'static)) -> String {
    if let Some(db) = database_error(error) {
        let message = db.message();
        let code = db.code().map(|c| c.into_owned()).unwrap_or_default();
        // SQLite reports numeric result codes rather than SQLSTATEs.
        if let Ok(numeric) = code.parse::<i64>() {
            let (state, label) = if numeric & 0xff == 19 {
                ("23000", "Integrity constraint violation")
            } else {
                ("HY000", "General error")
            };
            return format!("SQLSTATE[{state}]: {label}: {} {message}", numeric & 0xff);
        }
        if code.is_empty() {
            return message.to_string();
        }
        return format!("SQLSTATE[{code}]: {message}");
    }
    error.to_string()
}

/// Replace each `?` in the SQL with the next binding (Laravel's `Str::replaceArray`).
fn replace_array(sql: &str, bindings: &[Value]) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut bindings = bindings.iter();
    for ch in sql.chars() {
        if ch == '?' {
            match bindings.next() {
                Some(Value::Bool(b)) => out.push_str(if *b { "1" } else { "0" }),
                Some(value) => out.push_str(&value.to_string_lossy()),
                None => out.push('?'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Thrown by `first_or_fail` when no record matches the query.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct RecordNotFoundException {
    /// The exception message.
    pub message: String,
}

impl RecordNotFoundException {
    /// Create a new exception with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Default for RecordNotFoundException {
    fn default() -> Self {
        Self::new("No record found for the given query.")
    }
}

/// Thrown by `sole` when no records match the query.
#[derive(Debug, Clone, Default, thiserror::Error)]
#[error("No records found for the given query.")]
pub struct RecordsNotFoundException;

/// Thrown by `sole` when more than one record matches the query.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{count} records were found.")]
pub struct MultipleRecordsFoundException {
    /// The number of records found.
    pub count: usize,
}

impl MultipleRecordsFoundException {
    /// Create a new exception for the given number of records.
    pub fn new(count: usize) -> Self {
        Self { count }
    }

    /// Get the number of records found.
    pub fn get_count(&self) -> usize {
        self.count
    }
}

/// Thrown when `scalar` is used on a query selecting more than one column.
#[derive(Debug, Clone, Default, thiserror::Error)]
#[error("The query selected more than one column.")]
pub struct MultipleColumnsSelectedException;

/// Thrown when a feature is not supported by the database driver in use.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct UnsupportedOperation(pub String);

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn messages_follow_laravels_format() {
        let error = QueryException::new(
            "sqlite",
            "select * from \"users\" where \"id\" = ? and \"active\" = ?",
            vec![json!(1), json!(true)],
            illuminate_support::error::RuntimeException::new("no such table: users"),
        );
        assert_eq!(
            error.to_string(),
            "no such table: users (Connection: sqlite, SQL: select * from \"users\" where \"id\" = 1 and \"active\" = 1)"
        );
    }

    #[test]
    fn concurrency_errors_are_detected() {
        assert!(caused_by_concurrency_error("SQLSTATE[40001]: Deadlock found when trying to get lock"));
        assert!(!caused_by_concurrency_error("syntax error"));
    }

    #[test]
    fn record_exceptions_have_laravel_messages() {
        assert_eq!(
            RecordNotFoundException::default().to_string(),
            "No record found for the given query."
        );
        assert_eq!(
            RecordsNotFoundException.to_string(),
            "No records found for the given query."
        );
        assert_eq!(MultipleRecordsFoundException::new(3).to_string(), "3 records were found.");
    }
}
