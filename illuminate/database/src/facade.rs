//! The `DB` facade.

use std::future::Future;
use std::sync::Arc;

use illuminate_support::{Result, Value};

use crate::connection::{Connection, QueryExecuted, QueryLog};
use crate::expression::{Expression, Ident, IntoBindings};
use crate::manager::DatabaseManager;
use crate::query::Builder;

/// The `DB` facade: static access to the database manager and the default
/// connection, resolved from the container on every call.
///
/// ```no_run
/// # async fn example() -> illuminate_support::Result<()> {
/// use illuminate_database::DB;
///
/// let users = DB::select("select * from users where active = ?", (1,)).await?;
/// let user = DB::table("users").where_("name", "John").first().await?;
/// let count = DB::connection("sqlite").table("posts").count().await?;
/// # Ok(()) }
/// ```
pub struct DB;

impl DB {
    /// Get the database manager.
    pub fn manager() -> Arc<DatabaseManager> {
        DatabaseManager::resolve()
    }

    /// Get a database connection instance by name.
    pub fn connection(name: &str) -> Connection {
        Self::manager().connection(name)
    }

    /// Get the default database connection instance.
    pub fn default_connection() -> Connection {
        Self::manager().default_connection()
    }

    /// Get the default connection name.
    pub fn get_default_connection() -> String {
        Self::manager().get_default_connection()
    }

    /// Set the default connection name.
    pub fn set_default_connection(name: &str) {
        Self::manager().set_default_connection(name)
    }

    /// Begin a fluent query against a database table.
    pub fn table(table: impl Into<Ident>) -> Builder {
        Self::default_connection().table(table)
    }

    /// Get a new query builder instance.
    pub fn query() -> Builder {
        Self::default_connection().query()
    }

    /// Get a new raw query expression.
    pub fn raw(value: impl Into<String>) -> Expression {
        Expression::new(value)
    }

    /// Run a select statement against the database.
    pub async fn select(query: &str, bindings: impl IntoBindings) -> Result<Vec<Value>> {
        Self::default_connection().select(query, bindings).await
    }

    /// Run a select statement and return a single result.
    pub async fn select_one(query: &str, bindings: impl IntoBindings) -> Result<Option<Value>> {
        Self::default_connection().select_one(query, bindings).await
    }

    /// Run a select statement and return the first column of the first row.
    pub async fn scalar(query: &str, bindings: impl IntoBindings) -> Result<Value> {
        Self::default_connection().scalar(query, bindings).await
    }

    /// Run an insert statement against the database.
    pub async fn insert(query: &str, bindings: impl IntoBindings) -> Result<bool> {
        Self::default_connection().insert(query, bindings).await
    }

    /// Run an update statement against the database.
    pub async fn update(query: &str, bindings: impl IntoBindings) -> Result<u64> {
        Self::default_connection().update(query, bindings).await
    }

    /// Run a delete statement against the database.
    pub async fn delete(query: &str, bindings: impl IntoBindings) -> Result<u64> {
        Self::default_connection().delete(query, bindings).await
    }

    /// Execute an SQL statement and return the boolean result.
    pub async fn statement(query: &str, bindings: impl IntoBindings) -> Result<bool> {
        Self::default_connection().statement(query, bindings).await
    }

    /// Run an SQL statement and get the number of rows affected.
    pub async fn affecting_statement(query: &str, bindings: impl IntoBindings) -> Result<u64> {
        Self::default_connection().affecting_statement(query, bindings).await
    }

    /// Run a raw, unprepared query against the database.
    pub async fn unprepared(query: &str) -> Result<bool> {
        Self::default_connection().unprepared(query).await
    }

    /// Execute a closure within a transaction on the default connection.
    pub async fn transaction<F, Fut, T>(callback: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        Self::default_connection().transaction(callback).await
    }

    /// Execute a closure within a transaction, retrying on deadlocks.
    pub async fn transaction_with_attempts<F, Fut, T>(attempts: usize, callback: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        Self::default_connection()
            .transaction_with_attempts(attempts, callback)
            .await
    }

    /// Start a new database transaction.
    pub async fn begin_transaction() -> Result<()> {
        Self::default_connection().begin_transaction().await
    }

    /// Commit the active database transaction.
    pub async fn commit() -> Result<()> {
        Self::default_connection().commit().await
    }

    /// Rollback the active database transaction.
    pub async fn rollback() -> Result<()> {
        Self::default_connection().rollback().await
    }

    /// Alias of [`DB::rollback`].
    pub async fn roll_back() -> Result<()> {
        Self::rollback().await
    }

    /// Get the number of active transactions.
    pub fn transaction_level() -> usize {
        Self::default_connection().transaction_level()
    }

    /// Run the callback after the current transaction commits.
    pub fn after_commit(callback: impl FnOnce() + Send + 'static) {
        Self::default_connection().after_commit(callback)
    }

    /// Register a listener called for every executed query.
    pub fn listen(callback: impl Fn(&QueryExecuted) + Send + Sync + 'static) {
        Self::manager().listen(callback)
    }

    /// Enable the query log on the default connection.
    pub fn enable_query_log() {
        Self::default_connection().enable_query_log()
    }

    /// Disable the query log on the default connection.
    pub fn disable_query_log() {
        Self::default_connection().disable_query_log()
    }

    /// Get the query log of the default connection.
    pub fn get_query_log() -> Vec<QueryLog> {
        Self::default_connection().get_query_log()
    }

    /// Get the query log with the bindings embedded.
    pub fn get_raw_query_log() -> Vec<QueryLog> {
        Self::default_connection().get_raw_query_log()
    }

    /// Clear the query log of the default connection.
    pub fn flush_query_log() {
        Self::default_connection().flush_query_log()
    }

    /// Execute the callback in "dry run" mode, returning the queries.
    pub async fn pretend<F, Fut>(callback: F) -> Result<Vec<QueryLog>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        Self::default_connection().pretend(callback).await
    }

    /// Disconnect from the given database and remove it from the cache.
    pub async fn purge(name: &str) {
        Self::manager().purge(name).await
    }

    /// Disconnect from the given database.
    pub async fn disconnect(name: &str) {
        Self::manager().disconnect(name).await
    }

    /// Reconnect to the given database.
    pub async fn reconnect(name: &str) -> Result<Connection> {
        Self::manager().reconnect(name).await
    }

    /// Escape a value for safe SQL embedding.
    pub fn escape(value: &Value) -> Result<String> {
        Self::default_connection().escape(value)
    }

    /// Get the driver name of the default connection.
    pub fn get_driver_name() -> &'static str {
        Self::default_connection().get_driver_name()
    }

    /// Get the table prefix of the default connection.
    pub fn get_table_prefix() -> String {
        Self::default_connection().get_table_prefix()
    }
}
