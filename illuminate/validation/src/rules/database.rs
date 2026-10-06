//! The `unique` and `exists` rules.

use illuminate_support::{Conditionable, Value, ValueExt};

use super::{IntoRuleItems, RuleItem};

/// An extra condition applied to a presence query.
#[derive(Clone, Debug, PartialEq)]
pub enum Condition {
    /// `column = value`
    Where(String, Value),
    /// `column != value`
    WhereNot(String, Value),
    /// `column IS NULL`
    WhereNull(String),
    /// `column IS NOT NULL`
    WhereNotNull(String),
    /// `column IN (values)`
    WhereIn(String, Vec<Value>),
    /// `column NOT IN (values)`
    WhereNotIn(String, Vec<Value>),
}

impl Condition {
    /// Parse a string-rule condition pair (`account_id`, `"1"`), following
    /// Laravel's conventions: `NULL`, `NOT_NULL`, and a `!` prefix for "not".
    pub(crate) fn from_pair(column: &str, value: &str) -> Condition {
        match value {
            "NULL" => Condition::WhereNull(column.to_string()),
            "NOT_NULL" => Condition::WhereNotNull(column.to_string()),
            v if v.starts_with('!') => {
                Condition::WhereNot(column.to_string(), Value::String(v[1..].to_string()))
            }
            v => Condition::Where(column.to_string(), Value::String(v.to_string())),
        }
    }

    /// The column the condition applies to.
    pub fn column(&self) -> &str {
        match self {
            Condition::Where(c, _)
            | Condition::WhereNot(c, _)
            | Condition::WhereNull(c)
            | Condition::WhereNotNull(c)
            | Condition::WhereIn(c, _)
            | Condition::WhereNotIn(c, _) => c,
        }
    }

    fn to_params(&self) -> Option<(String, String)> {
        let quote = |v: &Value| format!("\"{}\"", v.to_string_lossy().replace('"', "\"\""));
        match self {
            Condition::Where(c, v) => Some((c.clone(), quote(v))),
            Condition::WhereNot(c, v) => Some((c.clone(), format!("\"!{}\"", v.to_string_lossy()))),
            Condition::WhereNull(c) => Some((c.clone(), "\"NULL\"".into())),
            Condition::WhereNotNull(c) => Some((c.clone(), "\"NOT_NULL\"".into())),
            _ => None,
        }
    }
}

/// Which presence check a [`DatabaseRule`] performs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatabaseRuleKind {
    /// The value must not exist in the table.
    Unique,
    /// The value must exist in the table.
    Exists,
}

/// A `unique` or `exists` rule, built fluently with `Rule::unique` /
/// `Rule::exists`.
///
/// ```
/// use illuminate_validation::Rule;
///
/// let rule = Rule::unique("users", "email").ignore(5).where_("account_id", 1);
/// assert_eq!(rule.to_string(), r#"unique:users,email,"5",id,account_id,"1""#);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct DatabaseRule {
    pub(crate) kind: DatabaseRuleKind,
    pub(crate) table: String,
    pub(crate) column: Option<String>,
    pub(crate) ignore: Option<Value>,
    pub(crate) id_column: Option<String>,
    pub(crate) wheres: Vec<Condition>,
}

/// A `unique` rule.
pub type Unique = DatabaseRule;

/// An `exists` rule.
pub type Exists = DatabaseRule;

impl DatabaseRule {
    /// Create a rule of the given kind. A column of `""` or `"NULL"` means
    /// "use the attribute's name".
    pub fn new(kind: DatabaseRuleKind, table: &str, column: &str) -> Self {
        let column = match column {
            "" | "NULL" => None,
            c => Some(c.to_string()),
        };
        Self {
            kind,
            table: table.to_string(),
            column,
            ignore: None,
            id_column: None,
            wheres: Vec::new(),
        }
    }

    /// Ignore the row with the given ID (for "update" forms).
    ///
    /// Never pass user-controlled input here; use an ID you trust.
    pub fn ignore(mut self, id: impl Into<Value>) -> Self {
        let id = id.into();
        self.ignore = if id.is_null() { None } else { Some(id) };
        self
    }

    /// The column holding the ignored ID (defaults to `id`).
    pub fn id_column(mut self, column: &str) -> Self {
        self.id_column = Some(column.to_string());
        self
    }

    /// Set the column to check (instead of the attribute's name).
    pub fn column(mut self, column: &str) -> Self {
        self.column = Some(column.to_string());
        self
    }

    /// Add a `where column = value` condition. Arrays become `where in`
    /// and `null` becomes `where null`.
    pub fn where_(mut self, column: &str, value: impl Into<Value>) -> Self {
        let value = value.into();
        let condition = match value {
            Value::Null => Condition::WhereNull(column.to_string()),
            Value::Array(values) => Condition::WhereIn(column.to_string(), values),
            Value::Bool(false) => Condition::Where(column.to_string(), Value::from(0)),
            other => Condition::Where(column.to_string(), other),
        };
        self.wheres.push(condition);
        self
    }

    /// Add a `where column != value` condition.
    pub fn where_not(mut self, column: &str, value: impl Into<Value>) -> Self {
        let value = value.into();
        let condition = match value {
            Value::Array(values) => Condition::WhereNotIn(column.to_string(), values),
            Value::Bool(false) => Condition::WhereNot(column.to_string(), Value::from(0)),
            other => Condition::WhereNot(column.to_string(), other),
        };
        self.wheres.push(condition);
        self
    }

    /// Add a `where column is null` condition.
    pub fn where_null(mut self, column: &str) -> Self {
        self.wheres.push(Condition::WhereNull(column.to_string()));
        self
    }

    /// Add a `where column is not null` condition.
    pub fn where_not_null(mut self, column: &str) -> Self {
        self.wheres
            .push(Condition::WhereNotNull(column.to_string()));
        self
    }

    /// Add a `where column in (...)` condition.
    pub fn where_in<V: Into<Value>>(
        mut self,
        column: &str,
        values: impl IntoIterator<Item = V>,
    ) -> Self {
        self.wheres.push(Condition::WhereIn(
            column.to_string(),
            values.into_iter().map(Into::into).collect(),
        ));
        self
    }

    /// Add a `where column not in (...)` condition.
    pub fn where_not_in<V: Into<Value>>(
        mut self,
        column: &str,
        values: impl IntoIterator<Item = V>,
    ) -> Self {
        self.wheres.push(Condition::WhereNotIn(
            column.to_string(),
            values.into_iter().map(Into::into).collect(),
        ));
        self
    }

    /// Ignore soft deleted rows (`deleted_at` is null).
    pub fn without_trashed(self) -> Self {
        self.where_null("deleted_at")
    }

    /// Ignore soft deleted rows using a custom column.
    pub fn without_trashed_column(self, column: &str) -> Self {
        self.where_null(column)
    }

    /// Only consider soft deleted rows.
    pub fn only_trashed(self) -> Self {
        self.where_not_null("deleted_at")
    }

    /// The table (possibly prefixed with a connection name: `mysql.users`).
    pub fn table(&self) -> &str {
        &self.table
    }

    /// The extra conditions.
    pub fn wheres(&self) -> &[Condition] {
        &self.wheres
    }

    /// Build the rule from string parameters (`unique:users,email,5,id,...`).
    pub(crate) fn from_params(kind: DatabaseRuleKind, params: &[String]) -> DatabaseRule {
        let table = params.first().cloned().unwrap_or_default();
        let column = params.get(1).map(String::as_str).unwrap_or("NULL");
        let mut rule = DatabaseRule::new(kind, &table, column);
        let extra_start = match kind {
            DatabaseRuleKind::Unique => {
                if let Some(id) = params.get(2) {
                    rule.ignore = Some(Value::String(id.clone()));
                    rule.id_column = params.get(3).cloned();
                }
                4
            }
            DatabaseRuleKind::Exists => 2,
        };
        let extra = params.get(extra_start..).unwrap_or_default();
        for pair in extra.chunks(2) {
            let column = &pair[0];
            let value = pair.get(1).map(String::as_str).unwrap_or("");
            rule.wheres.push(Condition::from_pair(column, value));
        }
        rule
    }

    /// The rule's string parameters (for `failed()`).
    pub(crate) fn params(&self) -> Vec<String> {
        let mut params = vec![
            self.table.clone(),
            self.column.clone().unwrap_or_else(|| "NULL".into()),
        ];
        if self.kind == DatabaseRuleKind::Unique {
            params.push(match &self.ignore {
                Some(id) => id.to_string_lossy(),
                None => "NULL".into(),
            });
            params.push(self.id_column.clone().unwrap_or_else(|| "id".into()));
        }
        for condition in &self.wheres {
            if let Some((column, value)) = condition.to_params() {
                params.push(column);
                params.push(value.trim_matches('"').to_string());
            }
        }
        params
    }
}

impl Conditionable for DatabaseRule {}

impl std::fmt::Display for DatabaseRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let column = self.column.clone().unwrap_or_else(|| "NULL".into());
        let wheres: Vec<String> = self
            .wheres
            .iter()
            .filter_map(Condition::to_params)
            .map(|(c, v)| format!("{c},{v}"))
            .collect();
        let rendered = match self.kind {
            DatabaseRuleKind::Unique => {
                let ignore = match &self.ignore {
                    Some(id) => format!("\"{}\"", id.to_string_lossy().replace('"', "\\\"")),
                    None => "NULL".into(),
                };
                format!(
                    "unique:{},{},{},{},{}",
                    self.table,
                    column,
                    ignore,
                    self.id_column.as_deref().unwrap_or("id"),
                    wheres.join(",")
                )
            }
            DatabaseRuleKind::Exists => {
                format!("exists:{},{},{}", self.table, column, wheres.join(","))
            }
        };
        f.write_str(rendered.trim_end_matches(','))
    }
}

impl IntoRuleItems for DatabaseRule {
    fn into_rule_items(self) -> Vec<RuleItem> {
        vec![RuleItem::Database(self)]
    }
}
