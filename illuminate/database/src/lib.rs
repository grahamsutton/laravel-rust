//! # Illuminate Database
//!
//! Laravel makes interacting with databases extremely simple across a
//! variety of supported databases using raw SQL, a fluent query builder,
//! and a schema builder with migrations and seeders. SQLite, MySQL, MariaDB
//! and PostgreSQL are supported out of the box.
//!
//! ```
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> illuminate_support::Result<()> {
//! use illuminate_database::{DatabaseManager, Schema};
//! use illuminate_support::json;
//!
//! let db = DatabaseManager::from_config(json!({
//!     "default": "sqlite",
//!     "connections": {"sqlite": {"driver": "sqlite", "database": ":memory:"}},
//! }));
//! let connection = db.connection("sqlite");
//!
//! connection.get_schema_builder().create("users", |table| {
//!     table.id();
//!     table.string("name");
//!     table.integer("votes").default(0);
//! }).await?;
//!
//! connection.table("users").insert(json!([
//!     {"name": "Taylor", "votes": 10},
//!     {"name": "Abigail", "votes": 5},
//! ])).await?;
//!
//! let popular = connection.table("users").where_op("votes", ">", 6).pluck("name").await?;
//! assert_eq!(popular.all(), &[json!("Taylor")]);
//! # Ok(()) }
//! ```

pub mod connection;
pub mod de;
mod driver;
pub mod eloquent;
pub mod error;
pub mod expression;
mod facade;
pub mod manager;
pub mod migrations;
pub mod query;
pub mod schema;
pub mod seeder;

pub use connection::{Connection, QueryExecuted, QueryListener, QueryLog, TransactionCallback};
pub use de::{from_row, from_value};
pub use driver::Driver;
pub use error::{
    MultipleColumnsSelectedException, MultipleRecordsFoundException, QueryException,
    RecordNotFoundException, RecordsNotFoundException, UnsupportedOperation,
};
pub use expression::{
    Expression, Ident, IntoBindings, IntoColumns, IntoRecord, IntoRecords, Operand, Record, raw,
};
pub use facade::DB;
pub use manager::{DatabaseManager, DatabaseServiceProvider};
pub use migrations::{
    DatabaseMigrationRepository, MigrateOptions, Migration, MigrationEvent, MigrationRegistry,
    MigrationStatus, Migrator, RollbackOptions,
};
pub use query::{Builder, JoinClause, QueryGrammar};
pub use schema::{
    Blueprint, ColumnDefinition, ForeignKeyDefinition, Schema, SchemaBuilder, SchemaGrammar,
};
pub use seeder::{Seeder, SeederRegistry};

/// Re-exported for implementing [`Migration`] and [`Seeder`].
pub use async_trait::async_trait;

/// Build Eloquent [`Attributes`](eloquent::Attributes) from `key => value`
/// pairs of mixed types (dates are stored in storage format).
///
/// ```
/// use illuminate_database::attrs;
/// use illuminate_support::json;
///
/// let attributes = attrs! { "name" => "Taylor", "votes" => 10, "admin" => true };
/// assert_eq!(attributes.get("votes"), Some(&json!(10)));
/// ```
#[macro_export]
macro_rules! attrs {
    ($($key:expr => $value:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut attributes = $crate::eloquent::Attributes::new();
        $( attributes.insert($key, $value); )*
        attributes
    }};
}

/// Re-exports used by the code `#[derive(Model)]` generates. Not public API.
#[doc(hidden)]
pub mod __private {
    pub use illuminate_support as support;
    pub use inventory;
}

/// The most commonly used database types, for glob imports.
pub mod prelude {
    pub use crate::migrations::Migration;
    pub use crate::query::{Builder, JoinClause};
    pub use crate::schema::{Blueprint, Schema};
    pub use crate::seeder::Seeder;
    pub use crate::{DB, Expression, raw};
    pub use illuminate_support::{Conditionable, Tappable};
}
