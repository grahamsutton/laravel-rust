//! The building blocks of a query: where / having clauses, orders, unions,
//! locks and bindings. They are public so other components (Eloquent) can
//! inspect and rewrite queries.

use illuminate_support::Value;

use super::Builder;
use crate::expression::{Ident, Operand};

/// The kind of date comparison performed by `where_date` and friends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatePart {
    /// `where_date`
    Date,
    /// `where_time`
    Time,
    /// `where_day`
    Day,
    /// `where_month`
    Month,
    /// `where_year`
    Year,
}

/// Options for a full text where clause.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FullTextOptions {
    /// The search mode (`boolean` on MySQL; `phrase`, `websearch` or `raw` on PostgreSQL).
    pub mode: Option<String>,
    /// Whether to use query expansion (MySQL).
    pub expanded: bool,
    /// The text search language (PostgreSQL, defaults to `english`).
    pub language: Option<String>,
}

/// A where clause: its boolean (`and`, `or`, `and not`, `or not`) and kind.
#[derive(Clone, Debug)]
pub struct Where {
    /// How the clause is joined to the previous one.
    pub boolean: String,
    /// What the clause does.
    pub kind: WhereKind,
}

/// The different kinds of where clauses.
#[derive(Clone, Debug)]
pub enum WhereKind {
    /// `"column" operator ?`
    Basic {
        column: Ident,
        operator: String,
        value: Operand,
    },
    /// A JSON path compared to a boolean.
    JsonBoolean {
        column: String,
        operator: String,
        value: bool,
    },
    /// A bitwise comparison.
    Bitwise {
        column: Ident,
        operator: String,
        value: Operand,
    },
    /// A null-safe equality comparison.
    NullSafeEquals { column: Ident, value: Operand },
    /// Raw SQL.
    Raw { sql: String },
    /// `"column" [not] in (...)`; a sub-query is stored as a single raw operand.
    In {
        column: Ident,
        values: Vec<Operand>,
        not: bool,
    },
    /// `"column" [not] in (1, 2, 3)` with the integers inlined.
    InRaw {
        column: Ident,
        values: Vec<i64>,
        not: bool,
    },
    /// `"column" is [not] null`
    Null { column: Ident, not: bool },
    /// `"column" [not] between ? and ?`
    Between {
        column: Ident,
        min: Operand,
        max: Operand,
        not: bool,
    },
    /// `"column" [not] between "a" and "b"`
    BetweenColumns {
        column: Ident,
        min: Ident,
        max: Ident,
        not: bool,
    },
    /// Date based comparisons.
    Date {
        part: DatePart,
        column: Ident,
        operator: String,
        value: Operand,
    },
    /// `"first" operator "second"`
    Column {
        first: Ident,
        operator: String,
        second: Ident,
    },
    /// A parenthesized group of clauses.
    Nested { query: Box<Builder> },
    /// `"column" operator (select ...)`
    Sub {
        column: Ident,
        operator: String,
        query: Box<Builder>,
    },
    /// `[not] exists (select ...)`
    Exists { query: Box<Builder>, not: bool },
    /// `"column" [not] like ?`
    Like {
        column: Ident,
        value: Operand,
        case_sensitive: bool,
        not: bool,
    },
    /// JSON containment.
    JsonContains {
        column: String,
        value: Operand,
        not: bool,
    },
    /// JSON key existence.
    JsonContainsKey { column: String, not: bool },
    /// JSON array length.
    JsonLength {
        column: String,
        operator: String,
        value: Operand,
    },
    /// Full text search.
    FullText {
        columns: Vec<String>,
        value: Operand,
        options: FullTextOptions,
    },
}

/// A having clause.
#[derive(Clone, Debug)]
pub struct Having {
    /// How the clause is joined to the previous one.
    pub boolean: String,
    /// What the clause does.
    pub kind: HavingKind,
}

/// The different kinds of having clauses.
#[derive(Clone, Debug)]
pub enum HavingKind {
    /// `"column" operator ?`
    Basic {
        column: Ident,
        operator: String,
        value: Operand,
    },
    /// A bitwise comparison.
    Bitwise {
        column: Ident,
        operator: String,
        value: Operand,
    },
    /// Raw SQL.
    Raw { sql: String },
    /// `"column" [not] between ? and ?`
    Between {
        column: Ident,
        min: Operand,
        max: Operand,
        not: bool,
    },
    /// `"column" is [not] null`
    Null { column: Ident, not: bool },
    /// A parenthesized group of having clauses.
    Nested { query: Box<Builder> },
}

/// The direction of an ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Ascending.
    Asc,
    /// Descending.
    Desc,
}

impl Direction {
    /// The SQL keyword.
    pub fn as_str(&self) -> &'static str {
        match self {
            Direction::Asc => "asc",
            Direction::Desc => "desc",
        }
    }

    /// Parse `asc` / `desc` (case-insensitively).
    pub fn parse(direction: &str) -> Option<Direction> {
        match direction.to_ascii_lowercase().as_str() {
            "asc" => Some(Direction::Asc),
            "desc" => Some(Direction::Desc),
            _ => None,
        }
    }
}

/// An `order by` clause.
#[derive(Clone, Debug, PartialEq)]
pub enum Order {
    /// Order by a column (or expression) in a direction.
    Column { column: Ident, direction: Direction },
    /// Raw SQL.
    Raw { sql: String },
}

impl Order {
    /// The column being ordered by, if it is a plain column.
    pub fn column_name(&self) -> Option<&str> {
        match self {
            Order::Column {
                column: Ident::Name(name),
                ..
            } => Some(name),
            _ => None,
        }
    }
}

/// A union with another query.
#[derive(Clone, Debug)]
pub struct Union {
    /// The query being unioned.
    pub query: Box<Builder>,
    /// Whether this is a `union all`.
    pub all: bool,
}

/// A pessimistic lock.
#[derive(Clone, Debug, PartialEq)]
pub enum Lock {
    /// `for update`
    Update,
    /// `for share` / `lock in share mode`
    Shared,
    /// A custom lock clause.
    Raw(String),
}

/// An aggregate function call.
#[derive(Clone, Debug, PartialEq)]
pub struct Aggregate {
    /// The function (`count`, `max`, ...).
    pub function: String,
    /// The columns being aggregated.
    pub columns: Vec<Ident>,
}

/// Whether the query is distinct, optionally on specific columns.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Distinct {
    /// Not distinct.
    #[default]
    No,
    /// `select distinct`
    Yes,
    /// `select distinct on (...)` on PostgreSQL.
    Columns(Vec<Ident>),
}

impl Distinct {
    /// Whether any kind of distinct is applied.
    pub fn is_distinct(&self) -> bool {
        !matches!(self, Distinct::No)
    }
}

/// The binding buckets of a query, in the order they appear in the SQL.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bindings {
    pub select: Vec<Value>,
    pub from: Vec<Value>,
    pub join: Vec<Value>,
    pub where_: Vec<Value>,
    pub group_by: Vec<Value>,
    pub having: Vec<Value>,
    pub order: Vec<Value>,
    pub union: Vec<Value>,
    pub union_order: Vec<Value>,
}

/// The binding buckets of a query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingType {
    Select,
    From,
    Join,
    Where,
    GroupBy,
    Having,
    Order,
    Union,
    UnionOrder,
}

impl Bindings {
    /// Get a bucket.
    pub fn get(&self, kind: BindingType) -> &Vec<Value> {
        match kind {
            BindingType::Select => &self.select,
            BindingType::From => &self.from,
            BindingType::Join => &self.join,
            BindingType::Where => &self.where_,
            BindingType::GroupBy => &self.group_by,
            BindingType::Having => &self.having,
            BindingType::Order => &self.order,
            BindingType::Union => &self.union,
            BindingType::UnionOrder => &self.union_order,
        }
    }

    /// Get a bucket mutably.
    pub fn get_mut(&mut self, kind: BindingType) -> &mut Vec<Value> {
        match kind {
            BindingType::Select => &mut self.select,
            BindingType::From => &mut self.from,
            BindingType::Join => &mut self.join,
            BindingType::Where => &mut self.where_,
            BindingType::GroupBy => &mut self.group_by,
            BindingType::Having => &mut self.having,
            BindingType::Order => &mut self.order,
            BindingType::Union => &mut self.union,
            BindingType::UnionOrder => &mut self.union_order,
        }
    }

    /// All the buckets, in SQL order.
    pub const ORDER: [BindingType; 9] = [
        BindingType::Select,
        BindingType::From,
        BindingType::Join,
        BindingType::Where,
        BindingType::GroupBy,
        BindingType::Having,
        BindingType::Order,
        BindingType::Union,
        BindingType::UnionOrder,
    ];

    /// Flatten every bucket into a single list.
    pub fn flatten(&self) -> Vec<Value> {
        self.flatten_except(&[])
    }

    /// Flatten every bucket except the given ones.
    pub fn flatten_except(&self, except: &[BindingType]) -> Vec<Value> {
        Self::ORDER
            .iter()
            .filter(|kind| !except.contains(kind))
            .flat_map(|kind| self.get(*kind).iter().cloned())
            .collect()
    }

    /// Merge another set of bindings into this one, bucket by bucket.
    pub fn merge(&mut self, other: &Bindings) {
        for kind in Self::ORDER {
            self.get_mut(kind).extend(other.get(kind).iter().cloned());
        }
    }
}
