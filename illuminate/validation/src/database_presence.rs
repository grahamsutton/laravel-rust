//! The database presence verifier: the `unique` and `exists` rules, backed
//! by real tables.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use illuminate_database::{Builder, Connection, DatabaseManager};
use illuminate_support::{Result, Value};

use crate::presence::{PresenceVerifier, split_table};
use crate::rules::Condition;

/// Counts rows with the query builder — Laravel's `DatabasePresenceVerifier`.
///
/// The [`ValidationServiceProvider`](crate::ValidationServiceProvider) binds
/// one as the default `dyn PresenceVerifier` whenever a [`DatabaseManager`]
/// is registered, so `unique:users,email` and `exists:states,abbreviation`
/// just work.
///
/// A rule's table may name its connection (`unique:mysql.users,email`).
/// When the prefix isn't a configured connection, the whole name is used as
/// the table (handy for schema-qualified names such as `public.users`).
///
/// ```
/// use illuminate_database::DatabaseManager;
/// use illuminate_support::json;
/// use illuminate_validation::{DatabasePresenceVerifier, PresenceVerifier};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let db = DatabaseManager::from_config(json!({
///     "default": "sqlite",
///     "connections": {"sqlite": {"driver": "sqlite", "database": ":memory:"}},
/// }));
/// let connection = db.connection("sqlite");
/// connection.get_schema_builder().create("users", |table| {
///     table.id();
///     table.string("email");
/// }).await.unwrap();
/// connection.table("users").insert(json!({"email": "taylor@laravel.com"})).await.unwrap();
///
/// let verifier = DatabasePresenceVerifier::new(std::sync::Arc::new(db));
/// let count = verifier.count("users", "email", &json!("taylor@laravel.com"), None, None, &[]).await.unwrap();
///
/// assert_eq!(count, 1);
/// # });
/// ```
pub struct DatabasePresenceVerifier {
    db: Arc<DatabaseManager>,
    connection: RwLock<Option<String>>,
}

impl DatabasePresenceVerifier {
    /// Create a verifier querying the manager's connections.
    pub fn new(db: Arc<DatabaseManager>) -> Self {
        Self {
            db,
            connection: RwLock::new(None),
        }
    }

    /// Create a verifier for the database manager in the container.
    pub fn resolve() -> Self {
        Self::new(DatabaseManager::resolve())
    }

    /// Set the connection used for tables that don't name one (`None` for
    /// the default connection).
    pub fn set_connection(&self, connection: Option<&str>) {
        *self.connection.write().unwrap() = connection.map(str::to_string);
    }

    /// The connection used for tables that don't name one, if one was set.
    pub fn get_connection_name(&self) -> Option<String> {
        self.connection.read().unwrap().clone()
    }

    /// The connection and table a rule's table refers to.
    fn resolve_table(&self, table: &str) -> (Connection, String) {
        if let (Some(connection), name) = split_table(table)
            && self.db.get_config(connection).is_object()
        {
            return (self.db.connection(connection), name.to_string());
        }
        let connection = match self.get_connection_name() {
            Some(name) => self.db.connection(&name),
            None => self.db.default_connection(),
        };
        (connection, table.to_string())
    }

    /// A query builder for the given table, reading from the write connection.
    fn table(&self, table: &str) -> Builder {
        let (connection, table) = self.resolve_table(table);
        connection.table(table).use_write_pdo()
    }

    /// Add the rule's extra conditions to the query.
    ///
    /// String values follow Laravel's conventions: `NULL` means "is null",
    /// `NOT_NULL` means "is not null", and a `!` prefix means "not equal".
    fn add_conditions(query: Builder, conditions: &[Condition]) -> Builder {
        conditions
            .iter()
            .fold(query, |query, condition| match condition {
                Condition::Where(column, Value::String(value)) => {
                    Self::add_where(query, column, value)
                }
                Condition::Where(column, Value::Null) => query.where_null(column.as_str()),
                Condition::Where(column, value) => query.where_(column.as_str(), value.clone()),
                Condition::WhereNot(column, Value::Null) => query.where_not_null(column.as_str()),
                Condition::WhereNot(column, value) => {
                    query.where_op(column.as_str(), "!=", value.clone())
                }
                Condition::WhereNull(column) => query.where_null(column.as_str()),
                Condition::WhereNotNull(column) => query.where_not_null(column.as_str()),
                Condition::WhereIn(column, values) => {
                    query.where_in(column.as_str(), values.clone())
                }
                Condition::WhereNotIn(column, values) => {
                    query.where_not_in(column.as_str(), values.clone())
                }
            })
    }

    /// Add a "where" clause for a string condition value.
    fn add_where(query: Builder, column: &str, value: &str) -> Builder {
        match value {
            "NULL" => query.where_null(column),
            "NOT_NULL" => query.where_not_null(column),
            value => match value.strip_prefix('!') {
                Some(value) => query.where_op(column, "!=", value),
                None => query.where_(column, value),
            },
        }
    }

    /// Determine if an excluded ID means "exclude nothing".
    fn is_null_id(id: &Value) -> bool {
        matches!(id, Value::Null) || id.as_str() == Some("NULL")
    }

    fn to_count(count: i64) -> usize {
        usize::try_from(count).unwrap_or(0)
    }
}

#[async_trait]
impl PresenceVerifier for DatabasePresenceVerifier {
    async fn count(
        &self,
        table: &str,
        column: &str,
        value: &Value,
        exclude_id: Option<&Value>,
        id_column: Option<&str>,
        extra: &[Condition],
    ) -> Result<usize> {
        let mut query = self.table(table).where_op(column, "=", value.clone());

        if let Some(id) = exclude_id
            && !Self::is_null_id(id)
        {
            let id_column = id_column.filter(|c| !c.is_empty()).unwrap_or("id");
            query = query.where_op(id_column, "<>", id.clone());
        }

        let count = Self::add_conditions(query, extra).count().await?;
        Ok(Self::to_count(count))
    }

    async fn multi_count(
        &self,
        table: &str,
        column: &str,
        values: &[Value],
        extra: &[Condition],
    ) -> Result<usize> {
        let query = self.table(table).where_in(column, values.to_vec());
        let count = Self::add_conditions(query, extra)
            .distinct()
            .count_column(column)
            .await?;
        Ok(Self::to_count(count))
    }
}
