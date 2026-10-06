//! Join clauses.

use illuminate_support::Value;

use super::Builder;
use super::clauses::{Where, WhereKind};
use crate::expression::{Ident, IntoBindings, IntoColumns, Operand};

/// A join clause: `inner join "contacts" on "users"."id" = "contacts"."user_id"`.
///
/// Advanced joins receive a `JoinClause` in a closure:
///
/// ```
/// use illuminate_database::{Connection, JoinClause};
/// use illuminate_support::json;
///
/// let db = Connection::new("sqlite", json!({"driver": "sqlite", "database": ":memory:"}));
/// let sql = db
///     .table("users")
///     .join_with("contacts", |join: JoinClause| {
///         join.on("users.id", "=", "contacts.user_id").where_op("contacts.user_id", ">", 5)
///     })
///     .to_sql();
///
/// assert_eq!(
///     sql,
///     "select * from \"users\" inner join \"contacts\" on \"users\".\"id\" = \"contacts\".\"user_id\" and \"contacts\".\"user_id\" > ?"
/// );
/// ```
#[derive(Clone, Debug)]
pub struct JoinClause {
    /// The type of join (`inner`, `left`, `right`, `cross`).
    pub kind: String,
    /// The table the join clause is joining to.
    pub table: Ident,
    /// The join's conditions (its wheres and bindings).
    pub query: Builder,
}

impl JoinClause {
    /// Create a new join clause for the given parent query.
    pub fn new(parent: &Builder, kind: impl Into<String>, table: impl Into<Ident>) -> Self {
        let mut query = parent.new_query();
        query.is_join_clause = true;
        Self {
            kind: kind.into(),
            table: table.into(),
            query,
        }
    }

    fn map(mut self, f: impl FnOnce(Builder) -> Builder) -> Self {
        self.query = f(self.query);
        self
    }

    /// Add an "on" clause to the join.
    pub fn on(self, first: impl Into<Ident>, operator: &str, second: impl Into<Ident>) -> Self {
        let (first, second) = (first.into(), second.into());
        self.map(|q| q.add_where_column(first, operator, second, "and"))
    }

    /// Add an "or on" clause to the join.
    pub fn or_on(self, first: impl Into<Ident>, operator: &str, second: impl Into<Ident>) -> Self {
        let (first, second) = (first.into(), second.into());
        self.map(|q| q.add_where_column(first, operator, second, "or"))
    }

    /// Add a parenthesized group of "on" clauses.
    pub fn on_group(self, callback: impl FnOnce(JoinClause) -> JoinClause) -> Self {
        self.nested_on(callback, "and")
    }

    /// Add a parenthesized group of "on" clauses, joined with "or".
    pub fn or_on_group(self, callback: impl FnOnce(JoinClause) -> JoinClause) -> Self {
        self.nested_on(callback, "or")
    }

    fn nested_on(mut self, callback: impl FnOnce(JoinClause) -> JoinClause, boolean: &str) -> Self {
        let nested = callback(JoinClause::new(
            &self.query,
            self.kind.clone(),
            self.table.clone(),
        ));
        if !nested.query.wheres.is_empty() {
            let bindings = nested.query.bindings.where_.clone();
            self.query.wheres.push(Where {
                boolean: boolean.to_string(),
                kind: WhereKind::Nested {
                    query: Box::new(nested.query),
                },
            });
            self.query.bindings.where_.extend(bindings);
        }
        self
    }

    /// Add an equality "where" clause to the join.
    pub fn where_(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        let (column, value) = (column.into(), value.into());
        self.map(|q| q.where_(column, value))
    }

    /// Add a "where" clause with an operator to the join.
    pub fn where_op(
        self,
        column: impl Into<Ident>,
        operator: &str,
        value: impl Into<Operand>,
    ) -> Self {
        let (column, value) = (column.into(), value.into());
        self.map(|q| q.where_op(column, operator, value))
    }

    /// Add an "or where" clause to the join.
    pub fn or_where(self, column: impl Into<Ident>, value: impl Into<Operand>) -> Self {
        let (column, value) = (column.into(), value.into());
        self.map(|q| q.or_where(column, value))
    }

    /// Add an "or where" clause with an operator to the join.
    pub fn or_where_op(
        self,
        column: impl Into<Ident>,
        operator: &str,
        value: impl Into<Operand>,
    ) -> Self {
        let (column, value) = (column.into(), value.into());
        self.map(|q| q.or_where_op(column, operator, value))
    }

    /// Add a "where null" clause to the join.
    pub fn where_null(self, columns: impl IntoColumns) -> Self {
        self.map(|q| q.where_null(columns))
    }

    /// Add a "where not null" clause to the join.
    pub fn where_not_null(self, columns: impl IntoColumns) -> Self {
        self.map(|q| q.where_not_null(columns))
    }

    /// Add an "or where null" clause to the join.
    pub fn or_where_null(self, columns: impl IntoColumns) -> Self {
        self.map(|q| q.or_where_null(columns))
    }

    /// Add a "where in" clause to the join.
    pub fn where_in(self, column: impl Into<Ident>, values: impl super::WhereInValues) -> Self {
        let column = column.into();
        self.map(|q| q.where_in(column, values))
    }

    /// Add a "where not in" clause to the join.
    pub fn where_not_in(self, column: impl Into<Ident>, values: impl super::WhereInValues) -> Self {
        let column = column.into();
        self.map(|q| q.where_not_in(column, values))
    }

    /// Add a "where column" clause to the join.
    pub fn where_column(
        self,
        first: impl Into<Ident>,
        operator: &str,
        second: impl Into<Ident>,
    ) -> Self {
        let (first, second) = (first.into(), second.into());
        self.map(|q| q.where_column_op(first, operator, second))
    }

    /// Add a raw "where" clause to the join.
    pub fn where_raw(self, sql: &str, bindings: impl IntoBindings) -> Self {
        self.map(|q| q.where_raw(sql, bindings))
    }

    /// Get the join's bindings.
    pub fn get_bindings(&self) -> Vec<Value> {
        self.query.get_bindings()
    }
}
