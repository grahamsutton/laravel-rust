//! Raw expressions, identifiers, operands and bindings: the small vocabulary
//! the query builder speaks.

use std::fmt;

use illuminate_support::{Carbon, Map, Value};

/// A raw SQL expression that is injected into a query verbatim.
///
/// Raw expressions are never escaped or bound, so be careful to never pass
/// user input into one.
///
/// ```
/// use illuminate_database::Expression;
///
/// let expression = Expression::new("count(*) as user_count");
/// assert_eq!(expression.value(), "count(*) as user_count");
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Expression(String);

impl Expression {
    /// Create a new raw expression.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Get the value of the expression.
    pub fn value(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Create a raw expression (the `DB::raw` helper as a function).
pub fn raw(value: impl Into<String>) -> Expression {
    Expression::new(value)
}

/// A column (or table) reference: either a name that will be wrapped by the
/// grammar, or a raw expression that is used as-is.
#[derive(Clone, Debug, PartialEq)]
pub enum Ident {
    /// A column name such as `users.email` or `name as n`.
    Name(String),
    /// A raw expression.
    Raw(Expression),
}

impl Ident {
    /// The name of the identifier, if it is not a raw expression.
    pub fn as_name(&self) -> Option<&str> {
        match self {
            Ident::Name(name) => Some(name),
            Ident::Raw(_) => None,
        }
    }

    /// The textual value: the name, or the raw SQL of an expression.
    pub fn value(&self) -> &str {
        match self {
            Ident::Name(name) => name,
            Ident::Raw(expression) => expression.value(),
        }
    }

    /// Determine if the identifier is a raw expression.
    pub fn is_raw(&self) -> bool {
        matches!(self, Ident::Raw(_))
    }
}

impl From<&str> for Ident {
    fn from(value: &str) -> Self {
        Ident::Name(value.to_string())
    }
}

impl From<String> for Ident {
    fn from(value: String) -> Self {
        Ident::Name(value)
    }
}

impl From<&String> for Ident {
    fn from(value: &String) -> Self {
        Ident::Name(value.clone())
    }
}

impl From<Expression> for Ident {
    fn from(value: Expression) -> Self {
        Ident::Raw(value)
    }
}

impl From<&Expression> for Ident {
    fn from(value: &Expression) -> Self {
        Ident::Raw(value.clone())
    }
}

impl From<&Ident> for Ident {
    fn from(value: &Ident) -> Self {
        value.clone()
    }
}

/// A value in a query: either a binding or a raw expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Operand {
    /// A value bound to a `?` placeholder.
    Value(Value),
    /// A raw expression injected into the SQL.
    Raw(Expression),
}

impl Operand {
    /// Determine if the operand is a raw expression.
    pub fn is_raw(&self) -> bool {
        matches!(self, Operand::Raw(_))
    }

    /// The bound value, if this operand is not a raw expression.
    pub fn as_value(&self) -> Option<&Value> {
        match self {
            Operand::Value(value) => Some(value),
            Operand::Raw(_) => None,
        }
    }
}

impl<T: Into<Value>> From<T> for Operand {
    fn from(value: T) -> Self {
        // Dates are bound using the database's storage format.
        Operand::Value(Carbon::with_storage_format(|| value.into()))
    }
}

impl From<Expression> for Operand {
    fn from(value: Expression) -> Self {
        Operand::Raw(value)
    }
}

impl From<&Expression> for Operand {
    fn from(value: &Expression) -> Self {
        Operand::Raw(value.clone())
    }
}

/// Convert a value into a binding, formatting dates in storage format.
pub(crate) fn to_binding(value: impl Into<Value>) -> Value {
    Carbon::with_storage_format(|| value.into())
}

/// Types that can be used as a list of query bindings.
///
/// Implemented for tuples (so bindings may mix types), vectors, arrays, and
/// `()` for "no bindings".
///
/// ```
/// use illuminate_database::IntoBindings;
/// use illuminate_support::json;
///
/// assert_eq!((1, "taylor").into_bindings(), vec![json!(1), json!("taylor")]);
/// assert!(().into_bindings().is_empty());
/// ```
pub trait IntoBindings {
    /// Convert into a list of bindings.
    fn into_bindings(self) -> Vec<Value>;
}

impl IntoBindings for () {
    fn into_bindings(self) -> Vec<Value> {
        Vec::new()
    }
}

impl<T: Into<Value>> IntoBindings for Vec<T> {
    fn into_bindings(self) -> Vec<Value> {
        self.into_iter().map(to_binding).collect()
    }
}

impl<T: Into<Value>, const N: usize> IntoBindings for [T; N] {
    fn into_bindings(self) -> Vec<Value> {
        self.into_iter().map(to_binding).collect()
    }
}

impl<T: Into<Value> + Clone> IntoBindings for &[T] {
    fn into_bindings(self) -> Vec<Value> {
        self.iter().cloned().map(to_binding).collect()
    }
}

impl IntoBindings for Value {
    fn into_bindings(self) -> Vec<Value> {
        match self {
            Value::Array(items) => items,
            Value::Null => Vec::new(),
            other => vec![other],
        }
    }
}

macro_rules! tuple_bindings {
    ($($name:ident),+) => {
        impl<$($name: Into<Value>),+> IntoBindings for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_bindings(self) -> Vec<Value> {
                let ($($name,)+) = self;
                vec![$(to_binding($name)),+]
            }
        }
    };
}

tuple_bindings!(A);
tuple_bindings!(A, B);
tuple_bindings!(A, B, C);
tuple_bindings!(A, B, C, D);
tuple_bindings!(A, B, C, D, E);
tuple_bindings!(A, B, C, D, E, F);
tuple_bindings!(A, B, C, D, E, F, G);
tuple_bindings!(A, B, C, D, E, F, G, H);

/// Types that can be used as a list of columns (`select`, `group_by`, ...).
///
/// ```
/// use illuminate_database::{IntoColumns, Ident};
///
/// assert_eq!("name".into_columns(), vec![Ident::from("name")]);
/// assert_eq!(["id", "name"].into_columns().len(), 2);
/// ```
pub trait IntoColumns {
    /// Convert into a list of column identifiers.
    fn into_columns(self) -> Vec<Ident>;
}

impl IntoColumns for &str {
    fn into_columns(self) -> Vec<Ident> {
        vec![Ident::from(self)]
    }
}

impl IntoColumns for String {
    fn into_columns(self) -> Vec<Ident> {
        vec![Ident::from(self)]
    }
}

impl IntoColumns for &String {
    fn into_columns(self) -> Vec<Ident> {
        vec![Ident::from(self)]
    }
}

impl IntoColumns for Expression {
    fn into_columns(self) -> Vec<Ident> {
        vec![Ident::Raw(self)]
    }
}

impl IntoColumns for Ident {
    fn into_columns(self) -> Vec<Ident> {
        vec![self]
    }
}

impl<T: Into<Ident>> IntoColumns for Vec<T> {
    fn into_columns(self) -> Vec<Ident> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T: Into<Ident>, const N: usize> IntoColumns for [T; N] {
    fn into_columns(self) -> Vec<Ident> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T: Into<Ident> + Clone> IntoColumns for &[T] {
    fn into_columns(self) -> Vec<Ident> {
        self.iter().cloned().map(Into::into).collect()
    }
}

/// A single record of column / value pairs, in insertion order.
pub type Record = Vec<(String, Operand)>;

/// Types that can be turned into a single record (`update`, `insert`, ...).
///
/// Implemented for JSON objects (`json!({...})`), maps, and lists of
/// `(column, value)` pairs, which may contain raw expressions.
pub trait IntoRecord {
    /// Convert into a record.
    fn into_record(self) -> Record;
}

impl IntoRecord for Value {
    fn into_record(self) -> Record {
        match self {
            Value::Object(map) => map.into_record(),
            _ => Vec::new(),
        }
    }
}

impl IntoRecord for Map<String, Value> {
    fn into_record(self) -> Record {
        self.into_iter()
            .map(|(key, value)| (key, Operand::Value(value)))
            .collect()
    }
}

impl<K: Into<String>, V: Into<Operand>> IntoRecord for Vec<(K, V)> {
    fn into_record(self) -> Record {
        self.into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect()
    }
}

impl<K: Into<String>, V: Into<Operand>, const N: usize> IntoRecord for [(K, V); N] {
    fn into_record(self) -> Record {
        self.into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect()
    }
}

impl<V: Into<Operand>> IntoRecord for indexmap::IndexMap<String, V> {
    fn into_record(self) -> Record {
        self.into_iter().map(|(k, v)| (k, v.into())).collect()
    }
}

/// Types that can be turned into one or more records for `insert`.
///
/// A JSON object is a single record; a JSON array of objects is many.
pub trait IntoRecords {
    /// Convert into a list of records.
    fn into_records(self) -> Vec<Record>;

    /// Whether the value was given as a list of records (Laravel sorts the
    /// keys of each record in that case).
    fn is_list(&self) -> bool {
        false
    }
}

impl IntoRecords for Value {
    fn into_records(self) -> Vec<Record> {
        match self {
            Value::Array(items) => items.into_iter().map(IntoRecord::into_record).collect(),
            Value::Object(map) => vec![map.into_record()],
            _ => Vec::new(),
        }
    }

    fn is_list(&self) -> bool {
        matches!(self, Value::Array(_))
    }
}

impl IntoRecords for Map<String, Value> {
    fn into_records(self) -> Vec<Record> {
        vec![self.into_record()]
    }
}

impl IntoRecords for Vec<Value> {
    fn into_records(self) -> Vec<Record> {
        self.into_iter().map(IntoRecord::into_record).collect()
    }

    fn is_list(&self) -> bool {
        true
    }
}

impl IntoRecords for Vec<Record> {
    fn into_records(self) -> Vec<Record> {
        self
    }

    fn is_list(&self) -> bool {
        true
    }
}

impl IntoRecords for Record {
    fn into_records(self) -> Vec<Record> {
        vec![self]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn operands_convert_from_values_and_expressions() {
        assert_eq!(Operand::from(1), Operand::Value(json!(1)));
        assert_eq!(Operand::from("a"), Operand::Value(json!("a")));
        assert!(Operand::from(Expression::new("now()")).is_raw());
        assert_eq!(Operand::from(None::<i64>), Operand::Value(Value::Null));
    }

    #[test]
    fn dates_are_bound_in_storage_format() {
        let date = Carbon::parse("2024-01-02 03:04:05").unwrap();
        assert_eq!(
            Operand::from(date),
            Operand::Value(json!("2024-01-02 03:04:05"))
        );
    }

    #[test]
    fn records_preserve_order() {
        let record = json!({"name": "Taylor", "email": "t@laravel.com"}).into_record();
        assert_eq!(record[0].0, "name");
        assert_eq!(record[1].0, "email");
        let records = json!([{"a": 1}, {"a": 2}]).into_records();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn tuples_mix_binding_types() {
        assert_eq!(
            (1, "a", true, 2.5).into_bindings(),
            vec![json!(1), json!("a"), json!(true), json!(2.5)]
        );
        assert_eq!(vec![1, 2].into_bindings(), vec![json!(1), json!(2)]);
        assert_eq!(json!([1, "x"]).into_bindings(), vec![json!(1), json!("x")]);
    }
}
