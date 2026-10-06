//! The fluent query builder.

use std::future::Future;

use indexmap::IndexMap;
use serde::de::DeserializeOwned;

use anyhow::bail;
use illuminate_support::{Collection, Conditionable, Result, Tappable, Value, ValueExt};

use super::clauses::*;
use super::grammar::{QueryGrammar, UpsertColumn};
use super::join::JoinClause;
use crate::connection::Connection;
use crate::de::from_value;
use crate::error::{MultipleRecordsFoundException, RecordNotFoundException, RecordsNotFoundException};
use crate::expression::{
    Expression, Ident, IntoBindings, IntoColumns, IntoRecord, IntoRecords, Operand, Record, to_binding,
};

/// The operators every grammar understands.
const OPERATORS: &[&str] = &[
    "=", "<", ">", "<=", ">=", "<>", "!=", "<=>", "like", "like binary", "not like", "ilike", "&", "|",
    "^", "<<", ">>", "&~", "is", "is not", "rlike", "not rlike", "regexp", "not regexp", "~", "~*", "!~",
    "!~*", "similar to", "not similar to", "not ilike", "~~*", "!~~*",
];

/// Operators only understood by PostgreSQL.
const POSTGRES_OPERATORS: &[&str] = &[
    "between", "#", "<<=", ">>=", "&&", "@>", "<@", "?", "?|", "?&", "||", "-", "@?", "@@", "#-",
    "is distinct from", "is not distinct from",
];

/// Bitwise operators.
const BITWISE_OPERATORS: &[&str] = &["&", "|", "^", "<<", ">>", "&~"];
const POSTGRES_BITWISE_OPERATORS: &[&str] = &["~", "&", "|", "#", "<<", ">>", "<<=", ">>="];

/// A query, or something that can become one (a closure receiving a fresh
/// builder for the sub-query).
pub trait IntoQuery {
    /// Build the query, using the parent for its connection.
    fn into_query(self, parent: &Builder) -> Builder;
}

impl IntoQuery for Builder {
    fn into_query(self, _parent: &Builder) -> Builder {
        self
    }
}

impl IntoQuery for &Builder {
    fn into_query(self, _parent: &Builder) -> Builder {
        self.clone()
    }
}

impl<F: FnOnce(Builder) -> Builder> IntoQuery for F {
    fn into_query(self, parent: &Builder) -> Builder {
        self(parent.for_sub_query())
    }
}

/// A sub-query: a query builder, a closure building one, or raw SQL.
pub enum SubQuery {
    /// A query builder.
    Query(Builder),
    /// Raw SQL with its bindings.
    Raw(String, Vec<Value>),
}

/// Types usable as a sub-query (`select_sub`, `from_sub`, `join_sub`, ...).
pub trait IntoSubQuery {
    /// Convert into a sub-query.
    fn into_sub_query(self, parent: &Builder) -> SubQuery;
}

impl<T: IntoQuery> IntoSubQuery for T {
    fn into_sub_query(self, parent: &Builder) -> SubQuery {
        SubQuery::Query(self.into_query(parent))
    }
}

impl IntoSubQuery for &str {
    fn into_sub_query(self, _parent: &Builder) -> SubQuery {
        SubQuery::Raw(self.to_string(), Vec::new())
    }
}

impl IntoSubQuery for String {
    fn into_sub_query(self, _parent: &Builder) -> SubQuery {
        SubQuery::Raw(self, Vec::new())
    }
}

impl IntoSubQuery for Expression {
    fn into_sub_query(self, _parent: &Builder) -> SubQuery {
        SubQuery::Raw(self.value().to_string(), Vec::new())
    }
}

/// The values of a `where_in` clause: a list of values, or a sub-query.
pub enum InValues {
    /// A list of values.
    List(Vec<Operand>),
    /// A sub-query.
    Query(SubQuery),
}

/// Types usable as the values of a `where_in` clause.
pub trait WhereInValues {
    /// Convert into in-values.
    fn into_in_values(self, parent: &Builder) -> InValues;
}

impl<T: Into<Operand>> WhereInValues for Vec<T> {
    fn into_in_values(self, _parent: &Builder) -> InValues {
        InValues::List(self.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<Operand>, const N: usize> WhereInValues for [T; N] {
    fn into_in_values(self, _parent: &Builder) -> InValues {
        InValues::List(self.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<Operand> + Clone> WhereInValues for &[T] {
    fn into_in_values(self, _parent: &Builder) -> InValues {
        InValues::List(self.iter().cloned().map(Into::into).collect())
    }
}

impl<T: Into<Operand>> WhereInValues for Collection<T> {
    fn into_in_values(self, _parent: &Builder) -> InValues {
        InValues::List(self.into_iter().map(Into::into).collect())
    }
}

impl WhereInValues for Value {
    fn into_in_values(self, _parent: &Builder) -> InValues {
        match self {
            Value::Array(items) => InValues::List(items.into_iter().map(Operand::Value).collect()),
            Value::Null => InValues::List(Vec::new()),
            other => InValues::List(vec![Operand::Value(other)]),
        }
    }
}

impl WhereInValues for Builder {
    fn into_in_values(self, _parent: &Builder) -> InValues {
        InValues::Query(SubQuery::Query(self))
    }
}

impl WhereInValues for &Builder {
    fn into_in_values(self, _parent: &Builder) -> InValues {
        InValues::Query(SubQuery::Query(self.clone()))
    }
}

impl<F: FnOnce(Builder) -> Builder> WhereInValues for F {
    fn into_in_values(self, parent: &Builder) -> InValues {
        InValues::Query(SubQuery::Query(self(parent.for_sub_query())))
    }
}

/// The two bounds of a `between` clause.
pub trait BetweenValues<T> {
    /// The lower and upper bounds.
    fn bounds(self) -> (T, T);
}

impl<T: Into<Operand>> BetweenValues<Operand> for [T; 2] {
    fn bounds(self) -> (Operand, Operand) {
        let [min, max] = self;
        (min.into(), max.into())
    }
}

impl<A: Into<Operand>, B: Into<Operand>> BetweenValues<Operand> for (A, B) {
    fn bounds(self) -> (Operand, Operand) {
        (self.0.into(), self.1.into())
    }
}

impl<T: Into<Operand>> BetweenValues<Operand> for Vec<T> {
    fn bounds(self) -> (Operand, Operand) {
        let mut items: Vec<Operand> = self.into_iter().map(Into::into).collect();
        let max = items.pop().unwrap_or(Operand::Value(Value::Null));
        let min = if items.is_empty() { max.clone() } else { items.remove(0) };
        (min, max)
    }
}

impl<T: Into<Ident>> BetweenValues<Ident> for [T; 2] {
    fn bounds(self) -> (Ident, Ident) {
        let [min, max] = self;
        (min.into(), max.into())
    }
}

/// The query builder.
///
/// Builders are cheap to clone and own a handle to their connection, so they
/// can be built in one place and executed in another. Every clause method
/// consumes the builder and returns it, so queries read naturally:
///
/// ```
/// use illuminate_database::Connection;
/// use illuminate_support::json;
///
/// let db = Connection::new("sqlite", json!({"driver": "sqlite", "database": ":memory:"}));
///
/// let query = db
///     .table("users")
///     .where_op("votes", ">", 100)
///     .or_where("name", "John")
///     .order_by("name", "asc");
///
/// assert_eq!(
///     query.to_sql(),
///     "select * from \"users\" where \"votes\" > ? or \"name\" = ? order by \"name\" asc"
/// );
/// assert_eq!(query.get_bindings(), vec![json!(100), json!("John")]);
/// ```
#[derive(Clone, Debug)]
pub struct Builder {
    /// The connection the query runs on.
    pub connection: Connection,
    /// An aggregate function and column to be run.
    pub aggregate: Option<Aggregate>,
    /// The columns that should be returned (`None` selects `*`).
    pub columns: Option<Vec<Ident>>,
    /// Whether the query returns distinct results.
    pub distinct: Distinct,
    /// The table (or raw expression) the query targets.
    pub from: Option<Ident>,
    /// The table joins.
    pub joins: Vec<JoinClause>,
    /// The where constraints.
    pub wheres: Vec<Where>,
    /// The groupings.
    pub groups: Vec<Ident>,
    /// The having constraints.
    pub havings: Vec<Having>,
    /// The orderings.
    pub orders: Vec<Order>,
    /// The maximum number of records to return.
    pub limit: Option<i64>,
    /// The number of records to skip.
    pub offset: Option<i64>,
    /// The unions.
    pub unions: Vec<Union>,
    /// The maximum number of union records to return.
    pub union_limit: Option<i64>,
    /// The number of union records to skip.
    pub union_offset: Option<i64>,
    /// The orderings for the union.
    pub union_orders: Vec<Order>,
    /// The pessimistic lock.
    pub lock: Option<Lock>,
    /// The current query value bindings.
    pub bindings: Bindings,
    /// Whether to use the write connection for reads.
    pub use_write_connection: bool,
    /// Whether this builder holds the conditions of a join (`on` instead of `where`).
    pub is_join_clause: bool,
    /// An error raised while building, reported when the query is compiled.
    pub error: Option<String>,
}

impl Conditionable for Builder {}
impl Tappable for Builder {}

impl Builder {
    /// Create a new query builder for the connection.
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            aggregate: None,
            columns: None,
            distinct: Distinct::No,
            from: None,
            joins: Vec::new(),
            wheres: Vec::new(),
            groups: Vec::new(),
            havings: Vec::new(),
            orders: Vec::new(),
            limit: None,
            offset: None,
            unions: Vec::new(),
            union_limit: None,
            union_offset: None,
            union_orders: Vec::new(),
            lock: None,
            bindings: Bindings::default(),
            use_write_connection: false,
            is_join_clause: false,
            error: None,
        }
    }

    /// Get a new instance of the query builder on the same connection.
    pub fn new_query(&self) -> Builder {
        Builder::new(self.connection.clone())
    }

    /// Create a new query instance for a sub-query.
    pub fn for_sub_query(&self) -> Builder {
        self.new_query()
    }

    /// Create a new query instance for a nested where condition.
    pub fn for_nested_where(&self) -> Builder {
        let mut query = self.new_query();
        query.from = self.from.clone();
        query
    }

    /// Get the database connection instance.
    pub fn get_connection(&self) -> &Connection {
        &self.connection
    }

    /// Get the query grammar instance.
    pub fn get_grammar(&self) -> QueryGrammar {
        self.connection.query_grammar()
    }

    fn fail(mut self, message: impl Into<String>) -> Self {
        if self.error.is_none() {
            self.error = Some(message.into());
        }
        self
    }

    /// The table name the query targets, if it is a plain table.
    pub fn table_name(&self) -> Option<&str> {
        self.from.as_ref().and_then(|f| f.as_name())
    }

    // ------------------------------------------------------------------
    // Selects
    // ------------------------------------------------------------------

    /// Set the columns to be selected.
    pub fn select(mut self, columns: impl IntoColumns) -> Self {
        self.columns = Some(columns.into_columns());
        self.bindings.select.clear();
        self
    }

    /// Add a new "raw" select expression to the query.
    pub fn select_raw(mut self, expression: &str, bindings: impl IntoBindings) -> Self {
        self = self.add_select(Expression::new(expression));
        self.bindings.select.extend(bindings.into_bindings());
        self
    }

    /// Add a sub-select expression to the query.
    pub fn select_sub(mut self, query: impl IntoSubQuery, alias: &str) -> Self {
        let (sql, bindings) = self.create_sub(query);
        let wrapped = self.get_grammar().wrap_str(alias);
        self.select_raw(&format!("({sql}) as {wrapped}"), bindings)
    }

    /// Add new columns to the select (skipping ones already selected).
    pub fn add_select(mut self, columns: impl IntoColumns) -> Self {
        let existing = self.columns.get_or_insert_with(Vec::new);
        for column in columns.into_columns() {
            if !column.is_raw() && existing.contains(&column) {
                continue;
            }
            existing.push(column);
        }
        self
    }

    /// Force the query to only return distinct results.
    pub fn distinct(mut self) -> Self {
        self.distinct = Distinct::Yes;
        self
    }

    /// Force the query to return results distinct on the given columns
    /// (`distinct on` for PostgreSQL).
    pub fn distinct_on(mut self, columns: impl IntoColumns) -> Self {
        self.distinct = Distinct::Columns(columns.into_columns());
        self
    }

    /// Set the table which the query is targeting.
    pub fn from(mut self, table: impl Into<Ident>) -> Self {
        self.from = Some(table.into());
        self
    }

    /// Set the table which the query is targeting, with an alias.
    pub fn from_as(mut self, table: &str, alias: &str) -> Self {
        self.from = Some(Ident::Name(format!("{table} as {alias}")));
        self
    }

    /// Add a raw from clause to the query.
    pub fn from_raw(mut self, expression: &str, bindings: impl IntoBindings) -> Self {
        self.from = Some(Ident::Raw(Expression::new(expression)));
        self.bindings.from.extend(bindings.into_bindings());
        self
    }

    /// Make a sub-query the table of the query.
    pub fn from_sub(mut self, query: impl IntoSubQuery, alias: &str) -> Self {
        let (sql, bindings) = self.create_sub(query);
        let wrapped = self.get_grammar().wrap_table_str(alias);
        self.from_raw(&format!("({sql}) as {wrapped}"), bindings)
    }

    /// Turn a sub-query into SQL and bindings, remembering any compile error.
    fn create_sub(&mut self, query: impl IntoSubQuery) -> (String, Vec<Value>) {
        match query.into_sub_query(self) {
            SubQuery::Raw(sql, bindings) => (sql, bindings),
            SubQuery::Query(query) => match query.try_to_sql() {
                Ok(sql) => (sql, query.get_bindings()),
                Err(error) => {
                    self.error.get_or_insert(error.to_string());
                    (String::new(), Vec::new())
                }
            },
        }
    }

    // ------------------------------------------------------------------
    // Joins
    // ------------------------------------------------------------------

    fn push_join(mut self, join: JoinClause) -> Self {
        self.bindings.join.extend(join.get_bindings());
        self.joins.push(join);
        self
    }

    fn join_on(
        self,
        kind: &str,
        table: Ident,
        first: Ident,
        operator: &str,
        second: Ident,
    ) -> Self {
        let join = JoinClause::new(&self, kind, table).on(first, operator, second);
        self.push_join(join)
    }

    /// Add an inner join to the query.
    pub fn join(
        self,
        table: impl Into<Ident>,
        first: impl Into<Ident>,
        operator: &str,
        second: impl Into<Ident>,
    ) -> Self {
        self.join_on("inner", table.into(), first.into(), operator, second.into())
    }

    /// Add an inner join with complex conditions built by a closure.
    pub fn join_with(self, table: impl Into<Ident>, callback: impl FnOnce(JoinClause) -> JoinClause) -> Self {
        let join = callback(JoinClause::new(&self, "inner", table));
        self.push_join(join)
    }

    /// Add an inner join comparing a column with a value.
    pub fn join_where(
        self,
        table: impl Into<Ident>,
        first: impl Into<Ident>,
        operator: &str,
        second: impl Into<Operand>,
    ) -> Self {
        let join = JoinClause::new(&self, "inner", table).where_op(first, operator, second);
        self.push_join(join)
    }

    /// Add a left join to the query.
    pub fn left_join(
        self,
        table: impl Into<Ident>,
        first: impl Into<Ident>,
        operator: &str,
        second: impl Into<Ident>,
    ) -> Self {
        self.join_on("left", table.into(), first.into(), operator, second.into())
    }

    /// Add a left join with complex conditions built by a closure.
    pub fn left_join_with(self, table: impl Into<Ident>, callback: impl FnOnce(JoinClause) -> JoinClause) -> Self {
        let join = callback(JoinClause::new(&self, "left", table));
        self.push_join(join)
    }

    /// Add a left join comparing a column with a value.
    pub fn left_join_where(
        self,
        table: impl Into<Ident>,
        first: impl Into<Ident>,
        operator: &str,
        second: impl Into<Operand>,
    ) -> Self {
        let join = JoinClause::new(&self, "left", table).where_op(first, operator, second);
        self.push_join(join)
    }

    /// Add a right join to the query.
    pub fn right_join(
        self,
        table: impl Into<Ident>,
        first: impl Into<Ident>,
        operator: &str,
        second: impl Into<Ident>,
    ) -> Self {
        self.join_on("right", table.into(), first.into(), operator, second.into())
    }

    /// Add a right join with complex conditions built by a closure.
    pub fn right_join_with(self, table: impl Into<Ident>, callback: impl FnOnce(JoinClause) -> JoinClause) -> Self {
        let join = callback(JoinClause::new(&self, "right", table));
        self.push_join(join)
    }

    /// Add a right join comparing a column with a value.
    pub fn right_join_where(
        self,
        table: impl Into<Ident>,
        first: impl Into<Ident>,
        operator: &str,
        second: impl Into<Operand>,
    ) -> Self {
        let join = JoinClause::new(&self, "right", table).where_op(first, operator, second);
        self.push_join(join)
    }

    /// Add a cross join to the query.
    pub fn cross_join(self, table: impl Into<Ident>) -> Self {
        let join = JoinClause::new(&self, "cross", table);
        self.push_join(join)
    }

    fn sub_join_table(&mut self, query: impl IntoSubQuery, alias: &str) -> Ident {
        let (sql, bindings) = self.create_sub(query);
        self.bindings.join.extend(bindings);
        let wrapped = self.get_grammar().wrap_table_str(alias);
        Ident::Raw(Expression::new(format!("({sql}) as {wrapped}")))
    }

    /// Add a sub-query inner join to the query.
    pub fn join_sub(
        mut self,
        query: impl IntoSubQuery,
        alias: &str,
        first: impl Into<Ident>,
        operator: &str,
        second: impl Into<Ident>,
    ) -> Self {
        let table = self.sub_join_table(query, alias);
        self.join_on("inner", table, first.into(), operator, second.into())
    }

    /// Add a sub-query inner join with conditions built by a closure.
    pub fn join_sub_with(
        mut self,
        query: impl IntoSubQuery,
        alias: &str,
        callback: impl FnOnce(JoinClause) -> JoinClause,
    ) -> Self {
        let table = self.sub_join_table(query, alias);
        self.join_with(table, callback)
    }

    /// Add a sub-query left join to the query.
    pub fn left_join_sub(
        mut self,
        query: impl IntoSubQuery,
        alias: &str,
        first: impl Into<Ident>,
        operator: &str,
        second: impl Into<Ident>,
    ) -> Self {
        let table = self.sub_join_table(query, alias);
        self.join_on("left", table, first.into(), operator, second.into())
    }

    /// Add a sub-query right join to the query.
    pub fn right_join_sub(
        mut self,
        query: impl IntoSubQuery,
        alias: &str,
        first: impl Into<Ident>,
        operator: &str,
        second: impl Into<Ident>,
    ) -> Self {
        let table = self.sub_join_table(query, alias);
        self.join_on("right", table, first.into(), operator, second.into())
    }

    /// Add a sub-query cross join to the query.
    pub fn cross_join_sub(mut self, query: impl IntoSubQuery, alias: &str) -> Self {
        let table = self.sub_join_table(query, alias);
        self.cross_join(table)
    }

    // ------------------------------------------------------------------
    // Where clauses
    // ------------------------------------------------------------------

    fn push_where(mut self, boolean: &str, kind: WhereKind) -> Self {
        self.wheres.push(Where {
            boolean: boolean.to_string(),
            kind,
        });
        self
    }

    fn bind_where(&mut self, value: &Operand) {
        if let Operand::Value(value) = value {
            self.bindings.where_.push(flatten_value(value));
        }
    }

    /// Determine if the given operator is supported.
    pub fn is_valid_operator(&self, operator: &str) -> bool {
        let operator = operator.to_lowercase();
        OPERATORS.contains(&operator.as_str())
            || match self.connection.driver() {
                crate::Driver::Postgres => POSTGRES_OPERATORS.contains(&operator.as_str()),
                crate::Driver::MySql | crate::Driver::MariaDb => operator == "sounds like",
                crate::Driver::Sqlite => false,
            }
    }

    fn is_bitwise_operator(&self, operator: &str) -> bool {
        let operator = operator.to_lowercase();
        BITWISE_OPERATORS.contains(&operator.as_str())
            || (self.connection.driver() == crate::Driver::Postgres
                && POSTGRES_BITWISE_OPERATORS.contains(&operator.as_str()))
    }

    /// The general "where" used by every basic comparison.
    pub fn add_where(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>, boolean: &str) -> Self {
        let column = column.into();
        let mut operator = operator.to_string();
        let mut value = value.into();

        // An unknown operator is treated as the value, compared with "=".
        if !self.is_valid_operator(&operator) {
            value = Operand::Value(Value::String(operator));
            operator = "=".to_string();
        }

        if matches!(value, Operand::Value(Value::Null)) {
            if !["=", "<=>", "<>", "!=", "is", "is not"].contains(&operator.as_str()) {
                return self.fail("Illegal operator and value combination.");
            }
            let not = !["=", "<=>", "is"].contains(&operator.as_str());
            return self.add_where_null(column, boolean, not);
        }

        let mut this = self;

        if let (Ident::Name(name), Operand::Value(Value::Bool(b))) = (&column, &value) {
            if name.contains("->") {
                let kind = WhereKind::JsonBoolean {
                    column: name.clone(),
                    operator,
                    value: *b,
                };
                return this.push_where(boolean, kind);
            }
        }

        let kind = if operator == "<=>" {
            WhereKind::NullSafeEquals {
                column,
                value: value.clone(),
            }
        } else if this.is_bitwise_operator(&operator) {
            WhereKind::Bitwise {
                column,
                operator,
                value: value.clone(),
            }
        } else {
            WhereKind::Basic {
                column,
                operator,
                value: value.clone(),
            }
        };
        this.bind_where(&value);
        this.push_where(boolean, kind)
    }

    /// Add a basic equality where clause: `where "column" = ?`.
    ///
    /// Comparing against `null` produces `where "column" is null`.
    pub fn where_(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_where(column, "=", value, "and")
    }

    /// Add a where clause with an operator: `where "votes" > ?`.
    pub fn where_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_where(column, operator, value, "and")
    }

    /// Add an "or where" equality clause.
    pub fn or_where(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_where(column, "=", value, "or")
    }

    /// Add an "or where" clause with an operator.
    pub fn or_where_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_where(column, operator, value, "or")
    }

    /// Add equality clauses for every key / value pair, grouped in parentheses.
    ///
    /// ```
    /// # use illuminate_database::Connection;
    /// # use illuminate_support::json;
    /// # let db = Connection::new("sqlite", json!({"driver": "sqlite"}));
    /// let sql = db.table("users").where_map(json!({"status": 1, "subscribed": 1})).to_sql();
    /// assert_eq!(sql, "select * from \"users\" where (\"status\" = ? and \"subscribed\" = ?)");
    /// ```
    pub fn where_map(self, values: impl IntoRecord) -> Self {
        let record = values.into_record();
        self.add_nested_where(
            |mut q| {
                for (column, value) in record {
                    q = q.where_(column, value);
                }
                q
            },
            "and",
        )
    }

    /// Add an "or where" group of equality clauses.
    pub fn or_where_map(self, values: impl IntoRecord) -> Self {
        let record = values.into_record();
        self.add_nested_where(
            |mut q| {
                for (column, value) in record {
                    q = q.where_(column, value);
                }
                q
            },
            "or",
        )
    }

    /// Add a nested where group: `where (... )`.
    pub fn where_group(self, callback: impl FnOnce(Builder) -> Builder) -> Self {
        self.add_nested_where(callback, "and")
    }

    /// Add an "or where" nested group: `or (...)`.
    pub fn or_where_group(self, callback: impl FnOnce(Builder) -> Builder) -> Self {
        self.add_nested_where(callback, "or")
    }

    /// Add a negated nested group: `where not (...)`.
    pub fn where_not(self, callback: impl FnOnce(Builder) -> Builder) -> Self {
        self.add_nested_where(callback, "and not")
    }

    /// Add an "or not" nested group: `or not (...)`.
    pub fn or_where_not(self, callback: impl FnOnce(Builder) -> Builder) -> Self {
        self.add_nested_where(callback, "or not")
    }

    /// Add a nested where statement to the query.
    pub fn add_nested_where(self, callback: impl FnOnce(Builder) -> Builder, boolean: &str) -> Self {
        let nested = callback(self.for_nested_where());
        self.add_nested_where_query(nested, boolean)
    }

    /// Add another query builder as a nested where to the query.
    pub fn add_nested_where_query(mut self, query: Builder, boolean: &str) -> Self {
        if query.wheres.is_empty() {
            return self;
        }
        if let Some(error) = &query.error {
            self.error.get_or_insert(error.clone());
        }
        self.bindings.where_.extend(query.bindings.where_.iter().cloned());
        self.push_where(
            boolean,
            WhereKind::Nested {
                query: Box::new(query),
            },
        )
    }

    pub(crate) fn add_where_column(self, first: Ident, operator: &str, second: Ident, boolean: &str) -> Self {
        let (operator, second) = if self.is_valid_operator(operator) {
            (operator.to_string(), second)
        } else {
            ("=".to_string(), Ident::Name(operator.to_string()))
        };
        self.push_where(
            boolean,
            WhereKind::Column {
                first,
                operator,
                second,
            },
        )
    }

    /// Add a "where" clause comparing two columns for equality.
    pub fn where_column(self, first: impl Into<Ident>, second: impl Into<Ident>) -> Self {
        self.add_where_column(first.into(), "=", second.into(), "and")
    }

    /// Add a "where" clause comparing two columns with an operator.
    pub fn where_column_op(self, first: impl Into<Ident>, operator: &str, second: impl Into<Ident>) -> Self {
        self.add_where_column(first.into(), operator, second.into(), "and")
    }

    /// Add an "or where" clause comparing two columns for equality.
    pub fn or_where_column(self, first: impl Into<Ident>, second: impl Into<Ident>) -> Self {
        self.add_where_column(first.into(), "=", second.into(), "or")
    }

    /// Add an "or where" clause comparing two columns with an operator.
    pub fn or_where_column_op(self, first: impl Into<Ident>, operator: &str, second: impl Into<Ident>) -> Self {
        self.add_where_column(first.into(), operator, second.into(), "or")
    }

    /// Add a raw where clause to the query.
    pub fn where_raw(mut self, sql: &str, bindings: impl IntoBindings) -> Self {
        self.bindings.where_.extend(bindings.into_bindings());
        self.push_where("and", WhereKind::Raw { sql: sql.to_string() })
    }

    /// Add a raw "or where" clause to the query.
    pub fn or_where_raw(mut self, sql: &str, bindings: impl IntoBindings) -> Self {
        self.bindings.where_.extend(bindings.into_bindings());
        self.push_where("or", WhereKind::Raw { sql: sql.to_string() })
    }

    /// Add a "where in" clause with full control over its boolean / negation.
    pub fn add_where_in(mut self, column: impl Into<Ident>, values: impl WhereInValues, boolean: &str, not: bool) -> Self {
        let column = column.into();
        let values = match values.into_in_values(&self) {
            InValues::List(values) => values,
            InValues::Query(sub) => {
                let (sql, bindings) = match sub {
                    SubQuery::Raw(sql, bindings) => (sql, bindings),
                    SubQuery::Query(query) => match query.try_to_sql() {
                        Ok(sql) => (sql, query.get_bindings()),
                        Err(error) => return self.fail(error.to_string()),
                    },
                };
                self.bindings.where_.extend(bindings);
                vec![Operand::Raw(Expression::new(sql))]
            }
        };
        for value in &values {
            if let Operand::Value(value) = value {
                if value.is_array() || value.is_object() {
                    return self.fail("Nested arrays may not be passed to whereIn method.");
                }
                self.bindings.where_.push(value.clone());
            }
        }
        self.push_where(boolean, WhereKind::In { column, values, not })
    }

    /// Add a "where in" clause: values, a query, or a closure building a sub-query.
    pub fn where_in(self, column: impl Into<Ident>, values: impl WhereInValues) -> Self {
        self.add_where_in(column, values, "and", false)
    }

    /// Add an "or where in" clause.
    pub fn or_where_in(self, column: impl Into<Ident>, values: impl WhereInValues) -> Self {
        self.add_where_in(column, values, "or", false)
    }

    /// Add a "where not in" clause.
    pub fn where_not_in(self, column: impl Into<Ident>, values: impl WhereInValues) -> Self {
        self.add_where_in(column, values, "and", true)
    }

    /// Add an "or where not in" clause.
    pub fn or_where_not_in(self, column: impl Into<Ident>, values: impl WhereInValues) -> Self {
        self.add_where_in(column, values, "or", true)
    }

    fn add_where_integer_in_raw(
        self,
        column: impl Into<Ident>,
        values: impl IntoIterator<Item = impl Into<Value>>,
        boolean: &str,
        not: bool,
    ) -> Self {
        let values = values
            .into_iter()
            .map(|v| v.into().to_i64_lossy().unwrap_or(0))
            .collect();
        self.push_where(
            boolean,
            WhereKind::InRaw {
                column: column.into(),
                values,
                not,
            },
        )
    }

    /// Add a "where in raw" clause for integer values (inlined into the SQL).
    pub fn where_integer_in_raw(self, column: impl Into<Ident>, values: impl IntoIterator<Item = impl Into<Value>>) -> Self {
        self.add_where_integer_in_raw(column, values, "and", false)
    }

    /// Add an "or where in raw" clause for integer values.
    pub fn or_where_integer_in_raw(
        self,
        column: impl Into<Ident>,
        values: impl IntoIterator<Item = impl Into<Value>>,
    ) -> Self {
        self.add_where_integer_in_raw(column, values, "or", false)
    }

    /// Add a "where not in raw" clause for integer values.
    pub fn where_integer_not_in_raw(
        self,
        column: impl Into<Ident>,
        values: impl IntoIterator<Item = impl Into<Value>>,
    ) -> Self {
        self.add_where_integer_in_raw(column, values, "and", true)
    }

    /// Add an "or where not in raw" clause for integer values.
    pub fn or_where_integer_not_in_raw(
        self,
        column: impl Into<Ident>,
        values: impl IntoIterator<Item = impl Into<Value>>,
    ) -> Self {
        self.add_where_integer_in_raw(column, values, "or", true)
    }

    fn add_where_null(self, column: Ident, boolean: &str, not: bool) -> Self {
        self.push_where(boolean, WhereKind::Null { column, not })
    }

    fn add_where_nulls(mut self, columns: impl IntoColumns, boolean: &str, not: bool) -> Self {
        for column in columns.into_columns() {
            self = self.add_where_null(column, boolean, not);
        }
        self
    }

    /// Add a "where null" clause (for one column or many).
    pub fn where_null(self, columns: impl IntoColumns) -> Self {
        self.add_where_nulls(columns, "and", false)
    }

    /// Add an "or where null" clause.
    pub fn or_where_null(self, columns: impl IntoColumns) -> Self {
        self.add_where_nulls(columns, "or", false)
    }

    /// Add a "where not null" clause.
    pub fn where_not_null(self, columns: impl IntoColumns) -> Self {
        self.add_where_nulls(columns, "and", true)
    }

    /// Add an "or where not null" clause.
    pub fn or_where_not_null(self, columns: impl IntoColumns) -> Self {
        self.add_where_nulls(columns, "or", true)
    }

    fn add_where_between(mut self, column: Ident, values: impl BetweenValues<Operand>, boolean: &str, not: bool) -> Self {
        let (min, max) = values.bounds();
        self.bind_where(&min);
        self.bind_where(&max);
        self.push_where(
            boolean,
            WhereKind::Between {
                column,
                min,
                max,
                not,
            },
        )
    }

    /// Add a "where between" clause: `where_between("votes", [1, 100])`.
    pub fn where_between(self, column: impl Into<Ident>, values: impl BetweenValues<Operand>) -> Self {
        self.add_where_between(column.into(), values, "and", false)
    }

    /// Add an "or where between" clause.
    pub fn or_where_between(self, column: impl Into<Ident>, values: impl BetweenValues<Operand>) -> Self {
        self.add_where_between(column.into(), values, "or", false)
    }

    /// Add a "where not between" clause.
    pub fn where_not_between(self, column: impl Into<Ident>, values: impl BetweenValues<Operand>) -> Self {
        self.add_where_between(column.into(), values, "and", true)
    }

    /// Add an "or where not between" clause.
    pub fn or_where_not_between(self, column: impl Into<Ident>, values: impl BetweenValues<Operand>) -> Self {
        self.add_where_between(column.into(), values, "or", true)
    }

    fn add_where_between_columns(self, column: Ident, values: impl BetweenValues<Ident>, boolean: &str, not: bool) -> Self {
        let (min, max) = values.bounds();
        self.push_where(
            boolean,
            WhereKind::BetweenColumns {
                column,
                min,
                max,
                not,
            },
        )
    }

    /// Add a "where between columns" clause: `where_between_columns("weight", ["min", "max"])`.
    pub fn where_between_columns(self, column: impl Into<Ident>, values: impl BetweenValues<Ident>) -> Self {
        self.add_where_between_columns(column.into(), values, "and", false)
    }

    /// Add an "or where between columns" clause.
    pub fn or_where_between_columns(self, column: impl Into<Ident>, values: impl BetweenValues<Ident>) -> Self {
        self.add_where_between_columns(column.into(), values, "or", false)
    }

    /// Add a "where not between columns" clause.
    pub fn where_not_between_columns(self, column: impl Into<Ident>, values: impl BetweenValues<Ident>) -> Self {
        self.add_where_between_columns(column.into(), values, "and", true)
    }

    /// Add an "or where not between columns" clause.
    pub fn or_where_not_between_columns(self, column: impl Into<Ident>, values: impl BetweenValues<Ident>) -> Self {
        self.add_where_between_columns(column.into(), values, "or", true)
    }

    fn add_date_where(
        mut self,
        part: DatePart,
        column: impl Into<Ident>,
        operator: &str,
        value: impl Into<Operand>,
        boolean: &str,
    ) -> Self {
        let mut value = value.into();
        let mut operator = operator.to_string();
        if !self.is_valid_operator(&operator) {
            value = Operand::Value(Value::String(operator));
            operator = "=".to_string();
        }
        if let Operand::Value(v) = &value {
            let v = flatten_value(v);
            let v = match part {
                DatePart::Day | DatePart::Month => match v.to_i64_lossy() {
                    Some(n) => Value::String(format!("{n:02}")),
                    None => v,
                },
                _ => v,
            };
            value = Operand::Value(v);
        }
        self.bind_where(&value);
        self.push_where(
            boolean,
            WhereKind::Date {
                part,
                column: column.into(),
                operator,
                value,
            },
        )
    }

    /// Add a "where date" clause: `where_date("created_at", "2016-12-31")`.
    pub fn where_date(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Date, column, "=", value, "and")
    }

    /// Add a "where date" clause with an operator.
    pub fn where_date_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Date, column, operator, value, "and")
    }

    /// Add an "or where date" clause.
    pub fn or_where_date(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Date, column, "=", value, "or")
    }

    /// Add an "or where date" clause with an operator.
    pub fn or_where_date_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Date, column, operator, value, "or")
    }

    /// Add a "where time" clause: `where_time("created_at", "11:20:45")`.
    pub fn where_time(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Time, column, "=", value, "and")
    }

    /// Add a "where time" clause with an operator.
    pub fn where_time_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Time, column, operator, value, "and")
    }

    /// Add an "or where time" clause.
    pub fn or_where_time(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Time, column, "=", value, "or")
    }

    /// Add an "or where time" clause with an operator.
    pub fn or_where_time_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Time, column, operator, value, "or")
    }

    /// Add a "where day" clause: `where_day("created_at", 31)`.
    pub fn where_day(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Day, column, "=", value, "and")
    }

    /// Add a "where day" clause with an operator.
    pub fn where_day_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Day, column, operator, value, "and")
    }

    /// Add an "or where day" clause.
    pub fn or_where_day(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Day, column, "=", value, "or")
    }

    /// Add an "or where day" clause with an operator.
    pub fn or_where_day_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Day, column, operator, value, "or")
    }

    /// Add a "where month" clause: `where_month("created_at", 12)`.
    pub fn where_month(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Month, column, "=", value, "and")
    }

    /// Add a "where month" clause with an operator.
    pub fn where_month_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Month, column, operator, value, "and")
    }

    /// Add an "or where month" clause.
    pub fn or_where_month(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Month, column, "=", value, "or")
    }

    /// Add an "or where month" clause with an operator.
    pub fn or_where_month_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Month, column, operator, value, "or")
    }

    /// Add a "where year" clause: `where_year("created_at", 2016)`.
    pub fn where_year(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Year, column, "=", value, "and")
    }

    /// Add a "where year" clause with an operator.
    pub fn where_year_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Year, column, operator, value, "and")
    }

    /// Add an "or where year" clause.
    pub fn or_where_year(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Year, column, "=", value, "or")
    }

    /// Add an "or where year" clause with an operator.
    pub fn or_where_year_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_date_where(DatePart::Year, column, operator, value, "or")
    }

    /// Add an exists clause with full control over its boolean / negation.
    pub fn add_where_exists(mut self, query: impl IntoQuery, boolean: &str, not: bool) -> Self {
        let query = query.into_query(&self);
        if let Some(error) = &query.error {
            self.error.get_or_insert(error.clone());
        }
        self.bindings.where_.extend(query.get_bindings());
        self.push_where(
            boolean,
            WhereKind::Exists {
                query: Box::new(query),
                not,
            },
        )
    }

    /// Add a "where exists" clause: `where exists (select ...)`.
    pub fn where_exists(self, query: impl IntoQuery) -> Self {
        self.add_where_exists(query, "and", false)
    }

    /// Add an "or where exists" clause.
    pub fn or_where_exists(self, query: impl IntoQuery) -> Self {
        self.add_where_exists(query, "or", false)
    }

    /// Add a "where not exists" clause.
    pub fn where_not_exists(self, query: impl IntoQuery) -> Self {
        self.add_where_exists(query, "and", true)
    }

    /// Add an "or where not exists" clause.
    pub fn or_where_not_exists(self, query: impl IntoQuery) -> Self {
        self.add_where_exists(query, "or", true)
    }

    fn add_where_sub(mut self, column: impl Into<Ident>, operator: &str, query: impl IntoQuery, boolean: &str) -> Self {
        let query = query.into_query(&self);
        if let Some(error) = &query.error {
            self.error.get_or_insert(error.clone());
        }
        self.bindings.where_.extend(query.get_bindings());
        self.push_where(
            boolean,
            WhereKind::Sub {
                column: column.into(),
                operator: operator.to_string(),
                query: Box::new(query),
            },
        )
    }

    /// Compare a column against the result of a sub-query:
    /// `where "price" > (select avg("price") from ...)`.
    pub fn where_sub(self, column: impl Into<Ident>, operator: &str, query: impl IntoQuery) -> Self {
        self.add_where_sub(column, operator, query, "and")
    }

    /// Compare a column against the result of a sub-query with "or".
    pub fn or_where_sub(self, column: impl Into<Ident>, operator: &str, query: impl IntoQuery) -> Self {
        self.add_where_sub(column, operator, query, "or")
    }

    /// Compare the result of a sub-query against a value:
    /// `where (select ...) = ?`.
    pub fn where_sub_value(mut self, query: impl IntoQuery, operator: &str, value: impl Into<Operand>) -> Self {
        let query = query.into_query(&self);
        let sql = match query.try_to_sql() {
            Ok(sql) => sql,
            Err(error) => return self.fail(error.to_string()),
        };
        self.bindings.where_.extend(query.get_bindings());
        self.where_op(Expression::new(format!("({sql})")), operator, value)
    }

    /// Add a "where like" clause with full control over its options.
    pub fn add_where_like(
        mut self,
        column: impl Into<Ident>,
        value: impl Into<Operand>,
        case_sensitive: bool,
        boolean: &str,
        not: bool,
    ) -> Self {
        let value = value.into();
        if let Operand::Value(v) = &value {
            let prepared = self.get_grammar().prepare_where_like_binding(v, case_sensitive);
            self.bindings.where_.push(prepared);
        }
        self.push_where(
            boolean,
            WhereKind::Like {
                column: column.into(),
                value,
                case_sensitive,
                not,
            },
        )
    }

    /// Add a case-insensitive "where like" clause.
    pub fn where_like(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_where_like(column, value, false, "and", false)
    }

    /// Add a case-sensitive "where like" clause.
    pub fn where_like_case_sensitive(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_where_like(column, value, true, "and", false)
    }

    /// Add an "or where like" clause.
    pub fn or_where_like(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_where_like(column, value, false, "or", false)
    }

    /// Add a "where not like" clause.
    pub fn where_not_like(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_where_like(column, value, false, "and", true)
    }

    /// Add an "or where not like" clause.
    pub fn or_where_not_like(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_where_like(column, value, false, "or", true)
    }

    fn add_where_any(
        self,
        columns: impl IntoColumns,
        operator: &str,
        value: impl Into<Operand>,
        inner: &str,
        boolean: &str,
    ) -> Self {
        let columns = columns.into_columns();
        let value = value.into();
        let operator = operator.to_string();
        self.add_nested_where(
            |mut q| {
                for column in columns {
                    q = q.add_where(column, &operator, value.clone(), inner);
                }
                q
            },
            boolean,
        )
    }

    /// Add a "where" clause matching if *any* of the columns match.
    pub fn where_any(self, columns: impl IntoColumns, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_where_any(columns, operator, value, "or", "and")
    }

    /// Add an "or where" clause matching if any of the columns match.
    pub fn or_where_any(self, columns: impl IntoColumns, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_where_any(columns, operator, value, "or", "or")
    }

    /// Add a "where" clause matching if *all* of the columns match.
    pub fn where_all(self, columns: impl IntoColumns, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_where_any(columns, operator, value, "and", "and")
    }

    /// Add an "or where" clause matching if all of the columns match.
    pub fn or_where_all(self, columns: impl IntoColumns, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_where_any(columns, operator, value, "and", "or")
    }

    /// Add a "where" clause matching if *none* of the columns match.
    pub fn where_none(self, columns: impl IntoColumns, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_where_any(columns, operator, value, "or", "and not")
    }

    /// Add an "or where" clause matching if none of the columns match.
    pub fn or_where_none(self, columns: impl IntoColumns, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_where_any(columns, operator, value, "or", "or not")
    }

    fn add_where_json_contains(mut self, column: &str, value: impl Into<Operand>, boolean: &str, not: bool) -> Self {
        let value = value.into();
        if let Operand::Value(v) = &value {
            let prepared = self.get_grammar().prepare_binding_for_json_contains(v);
            self.bindings.where_.push(prepared);
        }
        self.push_where(
            boolean,
            WhereKind::JsonContains {
                column: column.to_string(),
                value,
                not,
            },
        )
    }

    /// Add a "where JSON contains" clause: `where_json_contains("options->languages", "en")`.
    pub fn where_json_contains(self, column: &str, value: impl Into<Operand>) -> Self {
        self.add_where_json_contains(column, value, "and", false)
    }

    /// Add an "or where JSON contains" clause.
    pub fn or_where_json_contains(self, column: &str, value: impl Into<Operand>) -> Self {
        self.add_where_json_contains(column, value, "or", false)
    }

    /// Add a "where JSON not contains" clause.
    pub fn where_json_doesnt_contain(self, column: &str, value: impl Into<Operand>) -> Self {
        self.add_where_json_contains(column, value, "and", true)
    }

    /// Add an "or where JSON not contains" clause.
    pub fn or_where_json_doesnt_contain(self, column: &str, value: impl Into<Operand>) -> Self {
        self.add_where_json_contains(column, value, "or", true)
    }

    /// Add a clause that determines if a JSON path exists.
    pub fn where_json_contains_key(self, column: &str) -> Self {
        self.push_where(
            "and",
            WhereKind::JsonContainsKey {
                column: column.to_string(),
                not: false,
            },
        )
    }

    /// Add a clause that determines if a JSON path does not exist.
    pub fn where_json_doesnt_contain_key(self, column: &str) -> Self {
        self.push_where(
            "and",
            WhereKind::JsonContainsKey {
                column: column.to_string(),
                not: true,
            },
        )
    }

    /// Add a "where JSON length" equality clause.
    pub fn where_json_length(self, column: &str, value: impl Into<Operand>) -> Self {
        self.where_json_length_op(column, "=", value)
    }

    /// Add a "where JSON length" clause with an operator.
    pub fn where_json_length_op(mut self, column: &str, operator: &str, value: impl Into<Operand>) -> Self {
        let value = value.into();
        self.bind_where(&value);
        self.push_where(
            "and",
            WhereKind::JsonLength {
                column: column.to_string(),
                operator: operator.to_string(),
                value,
            },
        )
    }

    /// Add a full text "where" clause (MySQL / PostgreSQL).
    pub fn where_fulltext(self, columns: impl IntoColumns, value: impl Into<Operand>) -> Self {
        self.where_fulltext_with(columns, value, FullTextOptions::default())
    }

    /// Add a full text "where" clause with options.
    pub fn where_fulltext_with(mut self, columns: impl IntoColumns, value: impl Into<Operand>, options: FullTextOptions) -> Self {
        let value = value.into();
        self.bind_where(&value);
        let columns = columns.into_columns().iter().map(|c| c.value().to_string()).collect();
        self.push_where(
            "and",
            WhereKind::FullText {
                columns,
                value,
                options,
            },
        )
    }

    /// Add an "or where" full text clause.
    pub fn or_where_fulltext(mut self, columns: impl IntoColumns, value: impl Into<Operand>) -> Self {
        let value = value.into();
        self.bind_where(&value);
        let columns = columns.into_columns().iter().map(|c| c.value().to_string()).collect();
        self.push_where(
            "or",
            WhereKind::FullText {
                columns,
                value,
                options: FullTextOptions::default(),
            },
        )
    }

    /// Merge an array of where clauses and bindings.
    pub fn merge_wheres(mut self, wheres: Vec<Where>, bindings: Vec<Value>) -> Self {
        self.wheres.extend(wheres);
        self.bindings.where_.extend(bindings);
        self
    }

    // ------------------------------------------------------------------
    // Grouping & having
    // ------------------------------------------------------------------

    /// Add a "group by" clause to the query.
    pub fn group_by(mut self, groups: impl IntoColumns) -> Self {
        self.groups.extend(groups.into_columns());
        self
    }

    /// Add a raw "group by" clause to the query.
    pub fn group_by_raw(mut self, sql: &str, bindings: impl IntoBindings) -> Self {
        self.groups.push(Ident::Raw(Expression::new(sql)));
        self.bindings.group_by.extend(bindings.into_bindings());
        self
    }

    fn push_having(mut self, boolean: &str, kind: HavingKind) -> Self {
        self.havings.push(Having {
            boolean: boolean.to_string(),
            kind,
        });
        self
    }

    /// Add a "having" clause with full control over its boolean.
    pub fn add_having(mut self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>, boolean: &str) -> Self {
        let mut operator = operator.to_string();
        let mut value = value.into();
        if !self.is_valid_operator(&operator) {
            value = Operand::Value(Value::String(operator));
            operator = "=".to_string();
        }
        if let Operand::Value(v) = &value {
            self.bindings.having.push(flatten_value(v));
        }
        let column = column.into();
        let kind = if self.is_bitwise_operator(&operator) {
            HavingKind::Bitwise {
                column,
                operator,
                value,
            }
        } else {
            HavingKind::Basic {
                column,
                operator,
                value,
            }
        };
        self.push_having(boolean, kind)
    }

    /// Add an equality "having" clause.
    pub fn having(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_having(column, "=", value, "and")
    }

    /// Add a "having" clause with an operator: `having_op("account_id", ">", 100)`.
    pub fn having_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_having(column, operator, value, "and")
    }

    /// Add an "or having" equality clause.
    pub fn or_having(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        self.add_having(column, "=", value, "or")
    }

    /// Add an "or having" clause with an operator.
    pub fn or_having_op(self, column: impl Into<Ident>, operator: &str, value: impl Into<Operand>) -> Self {
        self.add_having(column, operator, value, "or")
    }

    /// Add a nested "having" group.
    pub fn having_group(mut self, callback: impl FnOnce(Builder) -> Builder) -> Self {
        let nested = callback(self.for_nested_where());
        if nested.havings.is_empty() {
            return self;
        }
        self.bindings.having.extend(nested.bindings.having.iter().cloned());
        self.push_having(
            "and",
            HavingKind::Nested {
                query: Box::new(nested),
            },
        )
    }

    /// Add a "having null" clause.
    pub fn having_null(mut self, columns: impl IntoColumns) -> Self {
        for column in columns.into_columns() {
            self = self.push_having("and", HavingKind::Null { column, not: false });
        }
        self
    }

    /// Add a "having not null" clause.
    pub fn having_not_null(mut self, columns: impl IntoColumns) -> Self {
        for column in columns.into_columns() {
            self = self.push_having("and", HavingKind::Null { column, not: true });
        }
        self
    }

    fn add_having_between(mut self, column: Ident, values: impl BetweenValues<Operand>, boolean: &str, not: bool) -> Self {
        let (min, max) = values.bounds();
        for value in [&min, &max] {
            if let Operand::Value(v) = value {
                self.bindings.having.push(v.clone());
            }
        }
        self.push_having(
            boolean,
            HavingKind::Between {
                column,
                min,
                max,
                not,
            },
        )
    }

    /// Add a "having between" clause.
    pub fn having_between(self, column: impl Into<Ident>, values: impl BetweenValues<Operand>) -> Self {
        self.add_having_between(column.into(), values, "and", false)
    }

    /// Add a "having not between" clause.
    pub fn having_not_between(self, column: impl Into<Ident>, values: impl BetweenValues<Operand>) -> Self {
        self.add_having_between(column.into(), values, "and", true)
    }

    /// Add an "or having between" clause.
    pub fn or_having_between(self, column: impl Into<Ident>, values: impl BetweenValues<Operand>) -> Self {
        self.add_having_between(column.into(), values, "or", false)
    }

    /// Add a raw "having" clause.
    pub fn having_raw(mut self, sql: &str, bindings: impl IntoBindings) -> Self {
        self.bindings.having.extend(bindings.into_bindings());
        self.push_having("and", HavingKind::Raw { sql: sql.to_string() })
    }

    /// Add a raw "or having" clause.
    pub fn or_having_raw(mut self, sql: &str, bindings: impl IntoBindings) -> Self {
        self.bindings.having.extend(bindings.into_bindings());
        self.push_having("or", HavingKind::Raw { sql: sql.to_string() })
    }

    // ------------------------------------------------------------------
    // Ordering, limits & offsets
    // ------------------------------------------------------------------

    fn push_order(mut self, order: Order) -> Self {
        if self.unions.is_empty() {
            self.orders.push(order);
        } else {
            self.union_orders.push(order);
        }
        self
    }

    /// Add an "order by" clause: `order_by("name", "desc")`.
    pub fn order_by(self, column: impl Into<Ident>, direction: &str) -> Self {
        match Direction::parse(direction) {
            Some(direction) => self.push_order(Order::Column {
                column: column.into(),
                direction,
            }),
            None => self.fail("Order direction must be \"asc\" or \"desc\"."),
        }
    }

    /// Add a descending "order by" clause.
    pub fn order_by_desc(self, column: impl Into<Ident>) -> Self {
        self.order_by(column, "desc")
    }

    /// Order by a sub-query's result.
    pub fn order_by_sub(mut self, query: impl IntoSubQuery, direction: &str) -> Self {
        let (sql, bindings) = self.create_sub(query);
        if self.unions.is_empty() {
            self.bindings.order.extend(bindings);
        } else {
            self.bindings.union_order.extend(bindings);
        }
        self.order_by(Expression::new(format!("({sql})")), direction)
    }

    /// Order by `created_at`, newest first.
    pub fn latest(self) -> Self {
        self.order_by("created_at", "desc")
    }

    /// Order by the given column, newest first.
    pub fn latest_by(self, column: impl Into<Ident>) -> Self {
        self.order_by(column, "desc")
    }

    /// Order by `created_at`, oldest first.
    pub fn oldest(self) -> Self {
        self.order_by("created_at", "asc")
    }

    /// Order by the given column, oldest first.
    pub fn oldest_by(self, column: impl Into<Ident>) -> Self {
        self.order_by(column, "asc")
    }

    /// Put the query's results in random order.
    pub fn in_random_order(self) -> Self {
        let sql = self.get_grammar().compile_random("");
        self.order_by_raw(&sql, ())
    }

    /// Put the query's results in a seeded random order.
    pub fn in_random_order_seed(self, seed: impl ToString) -> Self {
        let sql = self.get_grammar().compile_random(&seed.to_string());
        self.order_by_raw(&sql, ())
    }

    /// Add a raw "order by" clause.
    pub fn order_by_raw(mut self, sql: &str, bindings: impl IntoBindings) -> Self {
        if self.unions.is_empty() {
            self.bindings.order.extend(bindings.into_bindings());
        } else {
            self.bindings.union_order.extend(bindings.into_bindings());
        }
        self.push_order(Order::Raw { sql: sql.to_string() })
    }

    /// Remove all existing orders.
    pub fn reorder(mut self) -> Self {
        self.orders.clear();
        self.union_orders.clear();
        self.bindings.order.clear();
        self.bindings.union_order.clear();
        self
    }

    /// Remove all existing orders and order by the given column.
    pub fn reorder_by(self, column: impl Into<Ident>, direction: &str) -> Self {
        self.reorder().order_by(column, direction)
    }

    /// Set the "offset" value of the query.
    pub fn offset(mut self, value: i64) -> Self {
        let value = value.max(0);
        if self.unions.is_empty() {
            self.offset = Some(value);
        } else {
            self.union_offset = Some(value);
        }
        self
    }

    /// Alias to set the "offset" value of the query.
    pub fn skip(self, value: i64) -> Self {
        self.offset(value)
    }

    /// Set the "limit" value of the query (negative values are ignored).
    pub fn limit(mut self, value: i64) -> Self {
        if value >= 0 {
            if self.unions.is_empty() {
                self.limit = Some(value);
            } else {
                self.union_limit = Some(value);
            }
        }
        self
    }

    /// Alias to set the "limit" value of the query.
    pub fn take(self, value: i64) -> Self {
        self.limit(value)
    }

    /// Set the limit and offset for a given page.
    pub fn for_page(self, page: i64, per_page: i64) -> Self {
        self.offset((page.max(1) - 1) * per_page).limit(per_page)
    }

    fn remove_existing_orders_for(mut self, column: &str) -> Self {
        self.orders.retain(|order| order.column_name() != Some(column));
        self
    }

    /// Constrain the query to the next "page" of results after a given ID.
    pub fn for_page_after_id(self, per_page: i64, last_id: impl Into<Value>, column: &str) -> Self {
        let last_id = last_id.into();
        let query = self.remove_existing_orders_for(column);
        let query = if last_id.is_null() {
            query.where_not_null(column)
        } else {
            query.where_op(column, ">", last_id)
        };
        query.order_by(column, "asc").limit(per_page)
    }

    /// Constrain the query to the previous "page" of results before a given ID.
    pub fn for_page_before_id(self, per_page: i64, last_id: impl Into<Value>, column: &str) -> Self {
        let last_id = last_id.into();
        let query = self.remove_existing_orders_for(column);
        let query = if last_id.is_null() {
            query.where_not_null(column)
        } else {
            query.where_op(column, "<", last_id)
        };
        query.order_by(column, "desc").limit(per_page)
    }

    /// Get the "limit" value from the query (or the union limit).
    pub fn get_limit(&self) -> Option<i64> {
        if self.unions.is_empty() { self.limit } else { self.union_limit }
    }

    /// Get the "offset" value from the query (or the union offset).
    pub fn get_offset(&self) -> Option<i64> {
        if self.unions.is_empty() { self.offset } else { self.union_offset }
    }

    // ------------------------------------------------------------------
    // Unions & locks
    // ------------------------------------------------------------------

    /// Add a union statement to the query.
    pub fn union(self, query: impl IntoQuery) -> Self {
        self.add_union(query, false)
    }

    /// Add a union all statement to the query.
    pub fn union_all(self, query: impl IntoQuery) -> Self {
        self.add_union(query, true)
    }

    fn add_union(mut self, query: impl IntoQuery, all: bool) -> Self {
        let query = query.into_query(&self.new_query());
        self.bindings.union.extend(query.get_bindings());
        self.unions.push(Union {
            query: Box::new(query),
            all,
        });
        self
    }

    /// Lock the selected rows in the table for updating.
    pub fn lock_for_update(mut self) -> Self {
        self.lock = Some(Lock::Update);
        self.use_write_connection = true;
        self
    }

    /// Share lock the selected rows in the table.
    pub fn shared_lock(mut self) -> Self {
        self.lock = Some(Lock::Shared);
        self.use_write_connection = true;
        self
    }

    /// Lock the selected rows with a custom lock clause.
    pub fn lock(mut self, lock: Lock) -> Self {
        self.lock = Some(lock);
        self.use_write_connection = true;
        self
    }

    /// Use the write connection for the query.
    pub fn use_write_pdo(mut self) -> Self {
        self.use_write_connection = true;
        self
    }

    // ------------------------------------------------------------------
    // Bindings & introspection
    // ------------------------------------------------------------------

    /// Get the current query value bindings, flattened in SQL order.
    pub fn get_bindings(&self) -> Vec<Value> {
        self.bindings.flatten()
    }

    /// Get the raw binding buckets.
    pub fn get_raw_bindings(&self) -> &Bindings {
        &self.bindings
    }

    /// Set the bindings of a bucket.
    pub fn set_bindings(mut self, bindings: Vec<Value>, kind: BindingType) -> Self {
        *self.bindings.get_mut(kind) = bindings;
        self
    }

    /// Add a binding to a bucket.
    pub fn add_binding(mut self, value: impl Into<Value>, kind: BindingType) -> Self {
        self.bindings.get_mut(kind).push(to_binding(value));
        self
    }

    /// Merge the bindings of another query into this one.
    pub fn merge_bindings(mut self, query: &Builder) -> Self {
        self.bindings.merge(&query.bindings);
        self
    }

    /// Get the selected columns as strings.
    pub fn get_columns(&self) -> Vec<String> {
        self.columns
            .as_ref()
            .map(|columns| columns.iter().map(|c| c.value().to_string()).collect())
            .unwrap_or_default()
    }

    /// Clone the query without the given properties (`columns`, `orders`,
    /// `limit`, `offset`, `wheres`, `joins`, `groups`, `havings`, `unions`,
    /// `union_orders`, `union_limit`, `union_offset`, `lock`, `aggregate`,
    /// `distinct`).
    pub fn clone_without(&self, properties: &[&str]) -> Builder {
        let mut clone = self.clone();
        for property in properties {
            match *property {
                "columns" => clone.columns = None,
                "orders" => clone.orders.clear(),
                "limit" => clone.limit = None,
                "offset" => clone.offset = None,
                "wheres" => clone.wheres.clear(),
                "joins" => clone.joins.clear(),
                "groups" => clone.groups.clear(),
                "havings" => clone.havings.clear(),
                "unions" => clone.unions.clear(),
                "union_orders" | "unionOrders" => clone.union_orders.clear(),
                "union_limit" | "unionLimit" => clone.union_limit = None,
                "union_offset" | "unionOffset" => clone.union_offset = None,
                "lock" => clone.lock = None,
                "aggregate" => clone.aggregate = None,
                "distinct" => clone.distinct = Distinct::No,
                "from" => clone.from = None,
                _ => {}
            }
        }
        clone
    }

    /// Clone the query without the given binding buckets.
    pub fn clone_without_bindings(&self, kinds: &[BindingType]) -> Builder {
        let mut clone = self.clone();
        for kind in kinds {
            clone.bindings.get_mut(*kind).clear();
        }
        clone
    }

    /// Get the SQL representation of the query.
    ///
    /// # Panics
    ///
    /// Panics when the query can't be compiled (an invalid operator / value
    /// combination, or a feature the driver doesn't support). Use
    /// [`Builder::try_to_sql`] to handle those cases.
    pub fn to_sql(&self) -> String {
        match self.try_to_sql() {
            Ok(sql) => sql,
            Err(error) => panic!("{error}"),
        }
    }

    /// Get the SQL representation of the query, or the reason it can't be compiled.
    pub fn try_to_sql(&self) -> Result<String> {
        self.ensure_valid()?;
        self.get_grammar().compile_select(self)
    }

    fn ensure_valid(&self) -> Result<()> {
        if let Some(error) = &self.error {
            bail!(illuminate_support::error::InvalidArgumentException::new(error.clone()));
        }
        Ok(())
    }

    /// Get the raw SQL representation of the query with the bindings embedded.
    pub fn to_raw_sql(&self) -> String {
        self.get_grammar()
            .substitute_bindings_into_raw_sql(&self.to_sql(), &self.get_bindings())
    }

    /// Dump the current SQL and bindings (to stderr).
    pub fn dump(self) -> Self {
        eprintln!("{}", self.try_to_sql().unwrap_or_else(|e| e.to_string()));
        eprintln!("{:?}", self.get_bindings());
        self
    }

    /// Dump the raw SQL with the bindings embedded (to stderr).
    pub fn dump_raw_sql(self) -> Self {
        eprintln!("{}", self.to_raw_sql());
        self
    }

    // ------------------------------------------------------------------
    // Retrieving results
    // ------------------------------------------------------------------

    /// Execute the query as a "select" statement.
    pub async fn get(&self) -> Result<Collection<Value>> {
        let sql = self.try_to_sql()?;
        let rows = self.connection.select(&sql, self.get_bindings()).await?;
        Ok(Collection::from(rows))
    }

    /// Execute the query and deserialize every row into `T` (leniently:
    /// integers become booleans, numeric strings become numbers, JSON text
    /// becomes structs, and so on).
    pub async fn get_as<T: DeserializeOwned>(&self) -> Result<Collection<T>> {
        self.get()
            .await?
            .into_iter()
            .map(from_value::<T>)
            .collect::<Result<Vec<T>>>()
            .map(Collection::from)
    }

    /// Execute the query and get the first result.
    pub async fn first(&self) -> Result<Option<Value>> {
        Ok(self.clone().limit(1).get().await?.into_iter().next())
    }

    /// Execute the query and deserialize the first result into `T`.
    pub async fn first_as<T: DeserializeOwned>(&self) -> Result<Option<T>> {
        self.first().await?.map(from_value::<T>).transpose()
    }

    /// Execute the query and get the first result, or fail with a
    /// [`RecordNotFoundException`].
    pub async fn first_or_fail(&self) -> Result<Value> {
        match self.first().await? {
            Some(row) => Ok(row),
            None => Err(RecordNotFoundException::default().into()),
        }
    }

    /// Get the only record matching the query, failing when there are none
    /// ([`RecordsNotFoundException`]) or more than one
    /// ([`MultipleRecordsFoundException`]).
    pub async fn sole(&self) -> Result<Value> {
        let mut results = self.clone().limit(2).get().await?.into_vec();
        match results.len() {
            0 => Err(RecordsNotFoundException.into()),
            1 => Ok(results.remove(0)),
            count => Err(MultipleRecordsFoundException::new(count).into()),
        }
    }

    /// Get the sole record matching the query, deserialized into `T`.
    pub async fn sole_as<T: DeserializeOwned>(&self) -> Result<T> {
        from_value(self.sole().await?)
    }

    /// Execute a query for a single record by ID.
    pub async fn find(&self, id: impl Into<Value>) -> Result<Option<Value>> {
        self.clone().where_op("id", "=", Operand::Value(to_binding(id))).first().await
    }

    /// Execute a query for a single record by ID, deserialized into `T`.
    pub async fn find_as<T: DeserializeOwned>(&self, id: impl Into<Value>) -> Result<Option<T>> {
        self.find(id).await?.map(from_value::<T>).transpose()
    }

    fn first_column(row: Value) -> Value {
        match row {
            Value::Object(map) => map.into_iter().next().map(|(_, v)| v).unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }

    /// Get a single column's value from the first result of the query.
    pub async fn value(&self, column: impl Into<Ident>) -> Result<Option<Value>> {
        let mut query = self.clone();
        if query.columns.is_none() {
            query = query.select(vec![column.into()]);
        }
        Ok(query.first().await?.map(Self::first_column))
    }

    /// Get a single column's value from the first result, deserialized into `T`.
    pub async fn value_as<T: DeserializeOwned>(&self, column: impl Into<Ident>) -> Result<Option<T>> {
        match self.value(column).await? {
            None | Some(Value::Null) => Ok(None),
            Some(value) => from_value(value).map(Some),
        }
    }

    /// Get a single column's value from the sole result of the query.
    pub async fn sole_value(&self, column: impl Into<Ident>) -> Result<Value> {
        let mut query = self.clone();
        if query.columns.is_none() {
            query = query.select(vec![column.into()]);
        }
        Ok(Self::first_column(query.sole().await?))
    }

    /// Strip off the table name or alias from a column identifier.
    fn strip_table_for_pluck(column: &str) -> String {
        let lower = column.to_lowercase();
        if let Some(index) = lower.rfind(" as ") {
            return column[index + 4..].trim().to_string();
        }
        column.rsplit('.').next().unwrap_or(column).to_string()
    }

    /// Get a collection with the values of a given column.
    pub async fn pluck(&self, column: impl Into<Ident>) -> Result<Collection<Value>> {
        let column = column.into();
        let mut query = self.clone();
        if query.columns.is_none() {
            query = query.select(vec![column.clone()]);
        }
        let name = Self::strip_table_for_pluck(column.value());
        let rows = query.get().await?;
        Ok(rows
            .into_iter()
            .map(|row| row.get(&name).cloned().unwrap_or(Value::Null))
            .collect())
    }

    /// Get the values of a given column, keyed by another column.
    pub async fn pluck_with_key(&self, column: impl Into<Ident>, key: impl Into<Ident>) -> Result<IndexMap<String, Value>> {
        let (column, key) = (column.into(), key.into());
        let mut query = self.clone();
        if query.columns.is_none() {
            query = query.select(vec![column.clone(), key.clone()]);
        }
        let name = Self::strip_table_for_pluck(column.value());
        let key_name = Self::strip_table_for_pluck(key.value());
        let rows = query.get().await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let key = row.get(&key_name).map(|k| k.to_string_lossy()).unwrap_or_default();
                (key, row.get(&name).cloned().unwrap_or(Value::Null))
            })
            .collect())
    }

    /// Concatenate the values of a given column as a string.
    pub async fn implode(&self, column: impl Into<Ident>, glue: &str) -> Result<String> {
        Ok(self
            .pluck(column)
            .await?
            .iter()
            .map(|v| v.to_string_lossy())
            .collect::<Vec<_>>()
            .join(glue))
    }

    /// Determine if any rows exist for the current query.
    pub async fn exists(&self) -> Result<bool> {
        self.ensure_valid()?;
        let sql = self.get_grammar().compile_exists(self)?;
        let rows = self.connection.select(&sql, self.get_bindings()).await?;
        Ok(rows
            .first()
            .and_then(|row| row.get("exists"))
            .map(|v| v.truthy() || v.as_str() == Some("t"))
            .unwrap_or(false))
    }

    /// Determine if no rows exist for the current query.
    pub async fn doesnt_exist(&self) -> Result<bool> {
        Ok(!self.exists().await?)
    }

    /// Retrieve the "count" result of the query.
    pub async fn count(&self) -> Result<i64> {
        Ok(self.aggregate("count", vec![Ident::from("*")]).await?.to_i64_lossy().unwrap_or(0))
    }

    /// Retrieve the "count" of a specific column.
    pub async fn count_column(&self, column: impl Into<Ident>) -> Result<i64> {
        Ok(self.aggregate("count", vec![column.into()]).await?.to_i64_lossy().unwrap_or(0))
    }

    /// Retrieve the minimum value of a given column.
    pub async fn min(&self, column: impl Into<Ident>) -> Result<Value> {
        self.aggregate("min", vec![column.into()]).await
    }

    /// Retrieve the maximum value of a given column.
    pub async fn max(&self, column: impl Into<Ident>) -> Result<Value> {
        self.aggregate("max", vec![column.into()]).await
    }

    /// Retrieve the sum of the values of a given column (`0` when empty).
    pub async fn sum(&self, column: impl Into<Ident>) -> Result<Value> {
        let result = self.aggregate("sum", vec![column.into()]).await?;
        Ok(if result.is_null() { Value::from(0) } else { numeric(result) })
    }

    /// Retrieve the average of the values of a given column.
    pub async fn avg(&self, column: impl Into<Ident>) -> Result<Value> {
        Ok(numeric(self.aggregate("avg", vec![column.into()]).await?))
    }

    /// Alias for the "avg" method.
    pub async fn average(&self, column: impl Into<Ident>) -> Result<Value> {
        self.avg(column).await
    }

    fn with_aggregate(&self, function: &str, columns: Vec<Ident>) -> Builder {
        let mut query = if self.unions.is_empty() && self.havings.is_empty() {
            self.clone_without(&["columns"])
                .clone_without_bindings(&[BindingType::Select])
        } else {
            self.clone()
        };
        query.aggregate = Some(Aggregate {
            function: function.to_string(),
            columns,
        });
        if query.groups.is_empty() {
            query.orders.clear();
            query.bindings.order.clear();
        }
        query
    }

    /// Execute an aggregate function on the database.
    pub async fn aggregate(&self, function: &str, columns: Vec<Ident>) -> Result<Value> {
        let rows = self.with_aggregate(function, columns).get().await?;
        Ok(rows.first().map(aggregate_value).unwrap_or(Value::Null))
    }

    /// Get the count of the total records for the paginator.
    pub async fn get_count_for_pagination(&self) -> Result<i64> {
        let query = if !self.groups.is_empty() || !self.havings.is_empty() {
            let mut clone = self
                .clone_without(&["orders", "limit", "offset"])
                .clone_without_bindings(&[BindingType::Order]);
            if clone.columns.is_none() && !self.joins.is_empty() {
                let table = self.table_name().unwrap_or_default().to_string();
                clone = clone.select(format!("{table}.*"));
            }
            let sql = clone.try_to_sql()?;
            let wrapped = self.get_grammar().wrap_str("aggregate_table");
            self.new_query()
                .from_raw(&format!("({sql}) as {wrapped}"), clone.get_bindings())
                .with_aggregate("count", vec![Ident::from("*")])
        } else {
            let without: &[&str] = if self.unions.is_empty() {
                &["columns", "orders", "limit", "offset"]
            } else {
                &["union_orders", "union_limit", "union_offset"]
            };
            let without_bindings: &[BindingType] = if self.unions.is_empty() {
                &[BindingType::Select, BindingType::Order]
            } else {
                &[BindingType::UnionOrder]
            };
            let mut query = self.clone_without(without).clone_without_bindings(without_bindings);
            query.aggregate = Some(Aggregate {
                function: "count".into(),
                columns: vec![Ident::from("*")],
            });
            if query.groups.is_empty() {
                query.orders.clear();
            }
            query
        };
        let rows = query.get().await?;
        Ok(rows.first().map(aggregate_value).and_then(|v| v.to_i64_lossy()).unwrap_or(0))
    }

    // ------------------------------------------------------------------
    // Chunking
    // ------------------------------------------------------------------

    fn enforce_order_by(&self) -> Result<()> {
        if self.orders.is_empty() && self.union_orders.is_empty() {
            bail!(illuminate_support::error::RuntimeException::new(
                "You must specify an orderBy clause when using this function."
            ));
        }
        Ok(())
    }

    /// Chunk the results of the query: the callback receives each chunk and
    /// its page number, and returns `Ok(false)` to stop.
    ///
    /// ```no_run
    /// # async fn example() -> illuminate_support::Result<()> {
    /// use illuminate_database::DB;
    ///
    /// DB::table("users").order_by("id", "asc").chunk(100, |users, _page| async move {
    ///     for user in users {
    ///         // ...
    ///     }
    ///     Ok(true)
    /// }).await?;
    /// # Ok(()) }
    /// ```
    pub async fn chunk<F, Fut>(&self, count: i64, mut callback: F) -> Result<bool>
    where
        F: FnMut(Collection<Value>, i64) -> Fut,
        Fut: Future<Output = Result<bool>>,
    {
        self.enforce_order_by()?;
        let count = count.max(1);
        let skip = self.get_offset().unwrap_or(0);
        let mut remaining = self.get_limit();
        let mut page = 1;
        loop {
            let offset = (page - 1) * count + skip;
            let limit = remaining.map(|r| r.min(count)).unwrap_or(count);
            if limit == 0 {
                break;
            }
            let results = self.clone().offset(offset).limit(limit).get().await?;
            let found = results.count() as i64;
            if found == 0 {
                break;
            }
            if let Some(r) = remaining.as_mut() {
                *r = (*r - found).max(0);
            }
            if !callback(results, page).await? {
                return Ok(false);
            }
            page += 1;
            if found != count {
                break;
            }
        }
        Ok(true)
    }

    /// Execute a callback over each item while chunking; return `Ok(false)` to stop.
    pub async fn each<F, Fut>(&self, count: i64, mut callback: F) -> Result<bool>
    where
        F: FnMut(Value) -> Fut,
        Fut: Future<Output = Result<bool>>,
    {
        self.enforce_order_by()?;
        let count = count.max(1);
        let skip = self.get_offset().unwrap_or(0);
        let mut remaining = self.get_limit();
        let mut page = 1;
        loop {
            let offset = (page - 1) * count + skip;
            let limit = remaining.map(|r| r.min(count)).unwrap_or(count);
            if limit == 0 {
                break;
            }
            let results = self.clone().offset(offset).limit(limit).get().await?;
            let found = results.count() as i64;
            if found == 0 {
                break;
            }
            if let Some(r) = remaining.as_mut() {
                *r = (*r - found).max(0);
            }
            for item in results {
                if !callback(item).await? {
                    return Ok(false);
                }
            }
            page += 1;
            if found != count {
                break;
            }
        }
        Ok(true)
    }

    /// Chunk the results of the query by comparing IDs (safe to use while
    /// updating the rows being chunked).
    pub async fn chunk_by_id<F, Fut>(&self, count: i64, callback: F) -> Result<bool>
    where
        F: FnMut(Collection<Value>, i64) -> Fut,
        Fut: Future<Output = Result<bool>>,
    {
        self.chunk_by_id_column(count, "id", None, false, callback).await
    }

    /// Chunk the results by comparing IDs in descending order.
    pub async fn chunk_by_id_desc<F, Fut>(&self, count: i64, callback: F) -> Result<bool>
    where
        F: FnMut(Collection<Value>, i64) -> Fut,
        Fut: Future<Output = Result<bool>>,
    {
        self.chunk_by_id_column(count, "id", None, true, callback).await
    }

    /// Chunk the results by comparing a given column (read from the results
    /// under `alias`, defaulting to the column).
    pub async fn chunk_by_id_column<F, Fut>(
        &self,
        count: i64,
        column: &str,
        alias: Option<&str>,
        descending: bool,
        mut callback: F,
    ) -> Result<bool>
    where
        F: FnMut(Collection<Value>, i64) -> Fut,
        Fut: Future<Output = Result<bool>>,
    {
        let count = count.max(1);
        let alias = alias.unwrap_or(column).to_string();
        let skip = self.get_offset().unwrap_or(0);
        let mut remaining = self.get_limit();
        let mut last_id = Value::Null;
        let mut page = 1;
        loop {
            let mut clone = self.clone();
            if skip > 0 && page > 1 {
                clone = clone.offset(0);
            }
            let limit = remaining.map(|r| r.min(count)).unwrap_or(count);
            if limit == 0 {
                break;
            }
            let clone = if descending {
                clone.for_page_before_id(limit, last_id.clone(), column)
            } else {
                clone.for_page_after_id(limit, last_id.clone(), column)
            };
            let results = clone.get().await?;
            let found = results.count() as i64;
            if found == 0 {
                break;
            }
            if let Some(r) = remaining.as_mut() {
                *r = (*r - found).max(0);
            }
            let next_id = results
                .last()
                .and_then(|row| illuminate_support::data_get(row, &alias).into())
                .filter(|v: &Value| !v.is_null());
            if !callback(results, page).await? {
                return Ok(false);
            }
            match next_id {
                Some(id) => last_id = id,
                None => bail!(illuminate_support::error::RuntimeException::new(format!(
                    "The chunkById operation was aborted because the [{alias}] column is not present in the query result."
                ))),
            }
            page += 1;
            if found != count {
                break;
            }
        }
        Ok(true)
    }

    // ------------------------------------------------------------------
    // Inserts, updates & deletes
    // ------------------------------------------------------------------

    fn prepare_records(values: impl IntoRecords) -> Vec<Record> {
        let is_list = values.is_list();
        let mut records = values.into_records();
        if is_list {
            for record in records.iter_mut() {
                record.sort_by(|a, b| a.0.cmp(&b.0));
            }
        }
        records
    }

    fn record_bindings(records: &[Record]) -> Vec<Value> {
        records
            .iter()
            .flat_map(|record| record.iter().filter_map(|(_, v)| v.as_value().cloned()))
            .collect()
    }

    /// Insert new records into the database: a JSON object inserts one
    /// record, an array of objects inserts many.
    pub async fn insert(&self, values: impl IntoRecords) -> Result<bool> {
        let records = Self::prepare_records(values);
        if records.is_empty() || (records.len() == 1 && records[0].is_empty()) {
            return Ok(true);
        }
        let sql = self.get_grammar().compile_insert(self, &records);
        self.connection.insert(&sql, Self::record_bindings(&records)).await
    }

    /// Insert new records into the database while ignoring errors (duplicates).
    pub async fn insert_or_ignore(&self, values: impl IntoRecords) -> Result<u64> {
        let records = Self::prepare_records(values);
        if records.is_empty() {
            return Ok(0);
        }
        let sql = self.get_grammar().compile_insert_or_ignore(self, &records);
        self.connection.affecting_statement(&sql, Self::record_bindings(&records)).await
    }

    /// Insert a new record and get the value of its auto-incrementing `id`.
    pub async fn insert_get_id(&self, values: impl IntoRecord) -> Result<i64> {
        self.insert_get_id_with_sequence(values, "id").await
    }

    /// Insert a new record and get the value of the given sequence / key column.
    pub async fn insert_get_id_with_sequence(&self, values: impl IntoRecord, sequence: &str) -> Result<i64> {
        let records = vec![values.into_record()];
        let sql = self.get_grammar().compile_insert_get_id(self, &records, sequence);
        self.connection
            .insert_get_id(&sql, Self::record_bindings(&records), sequence)
            .await
    }

    /// Insert new records into the table using a sub-query.
    pub async fn insert_using(&self, columns: &[&str], query: impl IntoSubQuery) -> Result<u64> {
        let mut this = self.clone();
        let (sql, bindings) = this.create_sub(query);
        this.ensure_valid()?;
        let columns: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
        let sql = self.get_grammar().compile_insert_using(self, &columns, &sql);
        self.connection.affecting_statement(&sql, bindings).await
    }

    /// Insert new records into the table using a sub-query, ignoring errors.
    pub async fn insert_or_ignore_using(&self, columns: &[&str], query: impl IntoSubQuery) -> Result<u64> {
        let mut this = self.clone();
        let (sql, bindings) = this.create_sub(query);
        this.ensure_valid()?;
        let columns: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
        let sql = self.get_grammar().compile_insert_or_ignore_using(self, &columns, &sql);
        self.connection.affecting_statement(&sql, bindings).await
    }

    /// Insert new records or update the existing ones.
    ///
    /// `update` lists the columns to update when a record already exists;
    /// `None` updates every inserted column.
    pub async fn upsert(&self, values: impl IntoRecords, unique_by: &[&str], update: Option<&[&str]>) -> Result<u64> {
        let update = update.map(|columns| columns.iter().map(|c| UpsertColumn::Column(c.to_string())).collect());
        self.upsert_with(values, unique_by, update).await
    }

    /// Insert new records or update the existing ones, with full control
    /// over the update clause (columns or explicit values).
    pub async fn upsert_with(
        &self,
        values: impl IntoRecords,
        unique_by: &[&str],
        update: Option<Vec<UpsertColumn>>,
    ) -> Result<u64> {
        if unique_by.is_empty() {
            bail!(illuminate_support::error::InvalidArgumentException::new(
                "The unique columns must not be empty."
            ));
        }
        let records = Self::prepare_records(values);
        if records.is_empty() {
            return Ok(0);
        }
        if update.as_ref().is_some_and(|u| u.is_empty()) {
            return Ok(self.insert(records).await? as u64);
        }
        let update = update.unwrap_or_else(|| {
            records[0]
                .iter()
                .map(|(column, _)| UpsertColumn::Column(column.clone()))
                .collect()
        });
        let mut bindings = Self::record_bindings(&records);
        for column in &update {
            if let UpsertColumn::Value(_, Operand::Value(value)) = column {
                bindings.push(value.clone());
            }
        }
        let unique_by: Vec<String> = unique_by.iter().map(|c| c.to_string()).collect();
        let sql = self.get_grammar().compile_upsert(self, &records, &unique_by, &update);
        self.connection.affecting_statement(&sql, bindings).await
    }

    /// Update records in the database, returning the number of affected rows.
    pub async fn update(&self, values: impl IntoRecord) -> Result<u64> {
        self.ensure_valid()?;
        let values = values.into_record();
        let grammar = self.get_grammar();
        let sql = grammar.compile_update(self, &values)?;
        let bindings = grammar.prepare_bindings_for_update(&self.bindings, &values);
        self.connection.update(&sql, bindings).await
    }

    /// Insert or update a record matching the attributes, and fill it with values.
    pub async fn update_or_insert(&self, attributes: impl IntoRecord, values: impl IntoRecord) -> Result<bool> {
        let attributes = attributes.into_record();
        let values = values.into_record();
        let query = self.clone().where_map(attributes.clone());
        if !query.exists().await? {
            let mut record = attributes;
            for (column, value) in values {
                match record.iter_mut().find(|(c, _)| *c == column) {
                    Some(existing) => existing.1 = value,
                    None => record.push((column, value)),
                }
            }
            return self.insert(vec![record]).await;
        }
        if values.is_empty() {
            return Ok(true);
        }
        Ok(query.limit(1).update(values).await? > 0)
    }

    fn increments(&self, columns: Vec<(String, Value)>, extra: Record, sign: &str, method: &str) -> Result<Record> {
        let grammar = self.get_grammar();
        let mut record = Record::new();
        for (column, amount) in columns {
            let amount = match &amount {
                Value::Number(n) => n.to_string(),
                Value::String(s) if s.trim().parse::<f64>().is_ok() => s.trim().to_string(),
                _ => bail!(illuminate_support::error::InvalidArgumentException::new(format!(
                    "Non-numeric value passed to {method} method."
                ))),
            };
            let expression = format!("{} {sign} {amount}", grammar.wrap_str(&column));
            record.push((column, Operand::Raw(Expression::new(expression))));
        }
        record.extend(extra);
        Ok(record)
    }

    /// Increment a column's value by a given amount.
    pub async fn increment(&self, column: &str, amount: impl Into<Value>) -> Result<u64> {
        self.increment_each_with(vec![(column.to_string(), amount.into())], Vec::<(String, Operand)>::new())
            .await
    }

    /// Increment a column's value, updating extra columns at the same time.
    pub async fn increment_with(&self, column: &str, amount: impl Into<Value>, extra: impl IntoRecord) -> Result<u64> {
        self.increment_each_with(vec![(column.to_string(), amount.into())], extra).await
    }

    /// Increment the given columns by the given amounts.
    pub async fn increment_each(&self, columns: impl IntoIterator<Item = (impl Into<String>, impl Into<Value>)>) -> Result<u64> {
        self.increment_each_with(columns, Vec::<(String, Operand)>::new()).await
    }

    /// Increment the given columns, updating extra columns at the same time.
    pub async fn increment_each_with(
        &self,
        columns: impl IntoIterator<Item = (impl Into<String>, impl Into<Value>)>,
        extra: impl IntoRecord,
    ) -> Result<u64> {
        let columns = columns.into_iter().map(|(c, a)| (c.into(), a.into())).collect();
        let record = self.increments(columns, extra.into_record(), "+", "increment")?;
        self.update(record).await
    }

    /// Decrement a column's value by a given amount.
    pub async fn decrement(&self, column: &str, amount: impl Into<Value>) -> Result<u64> {
        self.decrement_each_with(vec![(column.to_string(), amount.into())], Vec::<(String, Operand)>::new())
            .await
    }

    /// Decrement a column's value, updating extra columns at the same time.
    pub async fn decrement_with(&self, column: &str, amount: impl Into<Value>, extra: impl IntoRecord) -> Result<u64> {
        self.decrement_each_with(vec![(column.to_string(), amount.into())], extra).await
    }

    /// Decrement the given columns by the given amounts.
    pub async fn decrement_each(&self, columns: impl IntoIterator<Item = (impl Into<String>, impl Into<Value>)>) -> Result<u64> {
        self.decrement_each_with(columns, Vec::<(String, Operand)>::new()).await
    }

    /// Decrement the given columns, updating extra columns at the same time.
    pub async fn decrement_each_with(
        &self,
        columns: impl IntoIterator<Item = (impl Into<String>, impl Into<Value>)>,
        extra: impl IntoRecord,
    ) -> Result<u64> {
        let columns = columns.into_iter().map(|(c, a)| (c.into(), a.into())).collect();
        let record = self.increments(columns, extra.into_record(), "-", "decrement")?;
        self.update(record).await
    }

    /// Delete records from the database, returning the number of deleted rows.
    pub async fn delete(&self) -> Result<u64> {
        self.ensure_valid()?;
        let grammar = self.get_grammar();
        let sql = grammar.compile_delete(self)?;
        let bindings = grammar.prepare_bindings_for_delete(&self.bindings);
        self.connection.delete(&sql, bindings).await
    }

    /// Delete the record with the given ID.
    pub async fn delete_by_id(&self, id: impl Into<Value>) -> Result<u64> {
        let column = format!("{}.id", self.table_name().unwrap_or_default());
        self.clone().where_op(column, "=", Operand::Value(to_binding(id))).delete().await
    }

    /// Run a truncate statement on the table.
    pub async fn truncate(&self) -> Result<()> {
        for (sql, bindings) in self.get_grammar().compile_truncate(self) {
            if let Err(error) = self.connection.statement(&sql, bindings).await {
                // SQLite only has a sequence table once AUTOINCREMENT was used.
                if sql.contains("sqlite_sequence") && error.to_string().contains("no such table") {
                    continue;
                }
                return Err(error);
            }
        }
        Ok(())
    }
}

/// Laravel's `flattenValue`: an array value contributes its first element.
fn flatten_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => items.first().map(flatten_value).unwrap_or(Value::Null),
        other => other.clone(),
    }
}

/// Read the `aggregate` column of an aggregate result (case-insensitively).
fn aggregate_value(row: &Value) -> Value {
    match row {
        Value::Object(map) => map
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("aggregate"))
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// Numeric strings (from `numeric` / `decimal` columns) become numbers.
fn numeric(value: Value) -> Value {
    match &value {
        Value::String(s) => {
            if let Ok(i) = s.parse::<i64>() {
                Value::from(i)
            } else if let Ok(f) = s.parse::<f64>() {
                illuminate_support::JsonNumber::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or(value)
            } else {
                value
            }
        }
        _ => value,
    }
}
