//! Fluent JSON testing — Laravel's `AssertableJson`.
//!
//! Hand [`TestResponse::assert_json_fluent`](super::TestResponse::assert_json_fluent)
//! a closure and make assertions against the JSON your application returned:
//!
//! ```ignore
//! app.get_json("/users/1").await.assert_json_fluent(|json| {
//!     json.where_("id", 1)
//!         .where_("name", "Victoria Faith")
//!         .where_fn("email", |email| email.as_str().is_some_and(|email| email.ends_with("@gmail.com")))
//!         .where_not("status", "pending")
//!         .missing("password")
//!         .etc()
//! });
//! ```
//!
//! Every property you don't make an assertion against fails the test —
//! unless you call [`etc`](AssertableJson::etc), which allows additional
//! properties at that level. That protects you from accidentally exposing
//! sensitive data in your JSON responses.

use illuminate_support::{Arr, Value, ValueExt, data_get};

/// Fluent assertions against a JSON value (Laravel's
/// `Illuminate\Testing\Fluent\AssertableJson`).
///
/// Every method takes and returns the assertable JSON, so assertions chain;
/// closures receive the scoped JSON and return it when they're done:
///
/// ```
/// use illuminate_foundation::testing::AssertableJson;
/// use illuminate_support::json;
///
/// let data = json!({
///     "meta": {"total": 2},
///     "users": [
///         {"id": 1, "name": "Victoria Faith", "email": "victoria@gmail.com"},
///         {"id": 2, "name": "Taylor Otwell", "email": "taylor@laravel.com"},
///     ],
/// });
///
/// AssertableJson::from_array(data)
///     .has("meta")
///     .has_with("users", 2, |user| {
///         user.where_("id", 1)
///             .where_("name", "Victoria Faith")
///             .where_type("email", "string")
///             .missing("password")
///             .etc()
///     })
///     .interacted();
/// ```
#[derive(Clone, Debug)]
pub struct AssertableJson {
    props: Value,
    path: Option<String>,
    interacted: Vec<String>,
}

/// One or more JSON type names for [`AssertableJson::where_type`]:
/// `"string"`, `"string|null"`, or `["string", "integer"]`.
///
/// The recognized types are `string`, `integer`, `double`, `boolean`,
/// `array` (JSON arrays and objects alike) and `null`.
pub trait JsonTypes {
    /// The type names.
    fn types(self) -> Vec<String>;
}

impl JsonTypes for &str {
    fn types(self) -> Vec<String> {
        self.split('|').map(|t| t.trim().to_string()).collect()
    }
}

impl JsonTypes for String {
    fn types(self) -> Vec<String> {
        self.as_str().types()
    }
}

impl JsonTypes for &[&str] {
    fn types(self) -> Vec<String> {
        self.iter().map(|t| t.to_string()).collect()
    }
}

impl<const N: usize> JsonTypes for [&str; N] {
    fn types(self) -> Vec<String> {
        self.iter().map(|t| t.to_string()).collect()
    }
}

impl JsonTypes for Vec<&str> {
    fn types(self) -> Vec<String> {
        self.into_iter().map(str::to_string).collect()
    }
}

impl AssertableJson {
    fn new(props: Value, path: Option<String>) -> Self {
        Self {
            props,
            path,
            interacted: Vec::new(),
        }
    }

    /// Create an assertable JSON instance from decoded JSON.
    pub fn from_array(data: impl Into<Value>) -> Self {
        Self::new(data.into(), None)
    }

    /// The JSON being asserted against.
    pub fn to_array(&self) -> Value {
        self.props.clone()
    }

    /// Determine if the JSON at this level is an object (Laravel's `Arr::isAssoc`).
    pub(crate) fn is_assoc(&self) -> bool {
        self.props.is_object()
    }

    /// The "dot" path of the current scope (`None` at the root level).
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// Get a property of the current scope using "dot" notation.
    pub fn get(&self, key: &str) -> Value {
        self.prop(Some(key))
    }

    fn dot_path(&self, key: &str) -> String {
        match &self.path {
            None => key.to_string(),
            Some(path) => format!("{path}.{key}").trim_end_matches('.').to_string(),
        }
    }

    fn prop(&self, key: Option<&str>) -> Value {
        match key {
            None => self.props.clone(),
            Some(key) => Arr::get(&self.props, key),
        }
    }

    fn exists(&self, key: &str) -> bool {
        Arr::has(&self.props, key)
    }

    // ------------------------------------------------------------------
    // Interaction
    // ------------------------------------------------------------------

    fn interacts_with(&mut self, key: &str) {
        let prop = key.split('.').next().unwrap_or(key).to_string();
        if !self.interacted.contains(&prop) {
            self.interacted.push(prop);
        }
    }

    /// Assert that every property at this level was interacted with. This
    /// runs automatically when a scope closes (and for the root object);
    /// call [`etc`](Self::etc) to allow properties you didn't assert against.
    pub fn interacted(&self) {
        let unexpected: Vec<String> = keys_of(&self.props)
            .into_iter()
            .filter(|key| !self.interacted.contains(key))
            .collect();
        if !unexpected.is_empty() {
            let message = match &self.path {
                Some(path) => format!("Unexpected properties were found in scope [{path}]."),
                None => "Unexpected properties were found on the root level.".to_string(),
            };
            fail(format!(
                "{message}\n\nUnexpected properties: [{}]",
                unexpected.join(", ")
            ));
        }
    }

    /// Allow properties at this level that weren't asserted against.
    pub fn etc(mut self) -> Self {
        self.interacted = keys_of(&self.props);
        self
    }

    // ------------------------------------------------------------------
    // Scoping
    // ------------------------------------------------------------------

    fn scope(self, key: &str, callback: impl FnOnce(AssertableJson) -> AssertableJson) -> Self {
        let props = self.prop(Some(key));
        let path = self.dot_path(key);
        if !props.is_array() && !props.is_object() {
            fail(format!("Property [{path}] is not scopeable."));
        }
        callback(AssertableJson::new(props, Some(path))).interacted();
        self
    }

    /// Scope a chain of assertions onto the first element of this level.
    pub fn first(mut self, callback: impl FnOnce(AssertableJson) -> AssertableJson) -> Self {
        let keys = keys_of(&self.props);
        let Some(key) = keys.first() else {
            let path = self.dot_path("");
            fail(if path.is_empty() {
                "Cannot scope directly onto the first element of the root level because it is empty."
                    .to_string()
            } else {
                format!(
                    "Cannot scope directly onto the first element of property [{path}] because it is empty."
                )
            });
        };
        self.interacts_with(key);
        self.scope(key, callback)
    }

    /// Make the same assertions against every element of this level.
    pub fn each(mut self, mut callback: impl FnMut(AssertableJson) -> AssertableJson) -> Self {
        let keys = keys_of(&self.props);
        if keys.is_empty() {
            let path = self.dot_path("");
            fail(if path.is_empty() {
                "Cannot scope directly onto each element of the root level because it is empty."
                    .to_string()
            } else {
                format!(
                    "Cannot scope directly onto each element of property [{path}] because it is empty."
                )
            });
        }
        for key in keys {
            self.interacts_with(&key);
            self = self.scope(&key, &mut callback);
        }
        self
    }

    // ------------------------------------------------------------------
    // Presence
    // ------------------------------------------------------------------

    /// Assert the number of elements at this level (`$json->has(3)`).
    pub fn count(self, length: usize) -> Self {
        let path = self.dot_path("");
        let message = if path.is_empty() {
            "Root level does not have the expected size.".to_string()
        } else {
            format!("Property [{path}] does not have the expected size.")
        };
        assert_count(&self.props, length, &message);
        self
    }

    /// Assert the size of this level is between `min` and `max` (inclusive).
    pub fn count_between(self, min: usize, max: usize) -> Self {
        let path = self.dot_path("");
        let size = self.props.count();
        if size < min {
            fail(if path.is_empty() {
                format!("Root level size is not greater than or equal to [{min}].")
            } else {
                format!("Property [{path}] size is not greater than or equal to [{min}].")
            });
        }
        if size > max {
            fail(if path.is_empty() {
                format!("Root level size is not less than or equal to [{max}].")
            } else {
                format!("Property [{path}] size is not less than or equal to [{max}].")
            });
        }
        self
    }

    /// Assert the property exists.
    pub fn has(mut self, key: &str) -> Self {
        if !self.exists(key) {
            fail(format!("Property [{}] does not exist.", self.dot_path(key)));
        }
        self.interacts_with(key);
        self
    }

    /// Assert the property exists and has the given number of elements
    /// (`$json->has('users', 3)`).
    pub fn has_count(self, key: &str, length: usize) -> Self {
        let this = self.has(key);
        let message = format!(
            "Property [{}] does not have the expected size.",
            this.dot_path(key)
        );
        assert_count(&this.prop(Some(key)), length, &message);
        this
    }

    /// Assert the property exists, optionally that it has the given number
    /// of elements, and scope a chain of assertions onto its *first*
    /// element (`$json->has('users', 3, fn ($user) => ...)`).
    pub fn has_with(
        self,
        key: &str,
        length: impl Into<Option<usize>>,
        callback: impl FnOnce(AssertableJson) -> AssertableJson,
    ) -> Self {
        let length = length.into();
        self.has(key).scope(key, move |scope| {
            let scope = match length {
                Some(length) => scope.count(length),
                None => scope,
            };
            scope.first(callback).etc()
        })
    }

    /// Assert the property exists and scope a chain of assertions onto it
    /// (`$json->has('users.0', fn ($user) => ...)`).
    pub fn has_scoped(
        self,
        key: &str,
        callback: impl FnOnce(AssertableJson) -> AssertableJson,
    ) -> Self {
        self.has(key).scope(key, callback)
    }

    /// Assert all of the properties exist.
    pub fn has_all<K: AsRef<str>>(mut self, keys: impl IntoIterator<Item = K>) -> Self {
        for key in keys {
            self = self.has(key.as_ref());
        }
        self
    }

    /// Assert at least one of the properties exists.
    pub fn has_any<K: AsRef<str>>(mut self, keys: impl IntoIterator<Item = K>) -> Self {
        let keys: Vec<String> = keys
            .into_iter()
            .map(|key| key.as_ref().to_string())
            .collect();
        if !keys.iter().any(|key| self.exists(key)) {
            fail(format!("None of properties [{}] exist.", keys.join(", ")));
        }
        for key in &keys {
            self.interacts_with(key);
        }
        self
    }

    /// Assert the property does not exist.
    pub fn missing(self, key: &str) -> Self {
        if self.exists(key) {
            fail(format!(
                "Property [{}] was found while it was expected to be missing.",
                self.dot_path(key)
            ));
        }
        self
    }

    /// Assert none of the properties exist.
    pub fn missing_all<K: AsRef<str>>(mut self, keys: impl IntoIterator<Item = K>) -> Self {
        for key in keys {
            self = self.missing(key.as_ref());
        }
        self
    }

    // ------------------------------------------------------------------
    // Matching
    // ------------------------------------------------------------------

    /// Assert the property has exactly the given value. Objects match
    /// regardless of key order; types must match (`1` is not `1.0` or `"1"`).
    pub fn where_(self, key: &str, expected: impl Into<Value>) -> Self {
        let this = self.has(key);
        let expected = expected.into();
        let actual = this.prop(Some(key));
        if actual != expected {
            fail(format!(
                "Property [{}] does not match the expected value.\nFailed asserting that {} is identical to {}.",
                this.dot_path(key),
                export(&actual),
                export(&expected)
            ));
        }
        this
    }

    /// Assert the property passes the given truth test.
    pub fn where_fn(self, key: &str, callback: impl FnOnce(&Value) -> bool) -> Self {
        let this = self.has(key);
        if !callback(&this.prop(Some(key))) {
            fail(format!(
                "Property [{}] was marked as invalid using a closure.",
                this.dot_path(key)
            ));
        }
        this
    }

    /// Assert the property does not have the given value.
    pub fn where_not(self, key: &str, expected: impl Into<Value>) -> Self {
        let this = self.has(key);
        let expected = expected.into();
        if this.prop(Some(key)) == expected {
            fail(format!(
                "Property [{}] contains a value that should be missing: [{key}, {}]",
                this.dot_path(key),
                expected.to_string_lossy_json()
            ));
        }
        this
    }

    /// Assert the property fails the given truth test.
    pub fn where_not_fn(self, key: &str, callback: impl FnOnce(&Value) -> bool) -> Self {
        let this = self.has(key);
        if callback(&this.prop(Some(key))) {
            fail(format!(
                "Property [{}] was marked as invalid using a closure.",
                this.dot_path(key)
            ));
        }
        this
    }

    /// Assert the property is `null`.
    pub fn where_null(self, key: &str) -> Self {
        let this = self.has(key);
        if !this.prop(Some(key)).is_null() {
            fail(format!("Property [{}] should be null.", this.dot_path(key)));
        }
        this
    }

    /// Assert the property is not `null`.
    pub fn where_not_null(self, key: &str) -> Self {
        let this = self.has(key);
        if this.prop(Some(key)).is_null() {
            fail(format!(
                "Property [{}] should not be null.",
                this.dot_path(key)
            ));
        }
        this
    }

    /// Assert every property of the given object has the given value:
    /// `where_all(json!({"id": 1, "name": "Taylor"}))`.
    pub fn where_all(mut self, bindings: Value) -> Self {
        for (key, value) in entries(bindings) {
            self = self.where_(&key, value);
        }
        self
    }

    /// Assert the property is of the given type (`"string"`,
    /// `"string|null"`, `["string", "integer"]`).
    pub fn where_type(self, key: &str, expected: impl JsonTypes) -> Self {
        let this = self.has(key);
        let expected = expected.types();
        let actual = type_name(&this.prop(Some(key)));
        if !expected.iter().any(|t| t == actual) {
            fail(format!(
                "Property [{}] is not of expected type [{}].",
                this.dot_path(key),
                expected.join("|")
            ));
        }
        this
    }

    /// Assert the properties are of the given types:
    /// `where_all_type(json!({"id": "integer", "users.0.name": "string"}))`.
    pub fn where_all_type(mut self, bindings: Value) -> Self {
        for (key, types) in entries(bindings) {
            let types: Vec<String> = match types {
                Value::Array(items) => items.iter().map(ValueExt::to_string_lossy).collect(),
                other => other.to_string_lossy().types(),
            };
            let types: Vec<&str> = types.iter().map(String::as_str).collect();
            self = self.where_type(&key, types);
        }
        self
    }

    /// Assert the property (a list) contains the given value, or every one
    /// of the given values.
    pub fn where_contains(self, key: &str, expected: impl Into<Value>) -> Self {
        let actual = match self.prop(Some(key)) {
            Value::Null => self.props.clone(),
            value => value,
        };
        let items = items_of(actual);
        let missing: Vec<Value> = items_of(expected.into())
            .into_iter()
            .filter(|search| {
                let by_key = items.iter().any(|item| &data_get(item, key) == search);
                !(by_key || items.contains(search))
            })
            .collect();
        if !missing.is_empty() {
            fail(format!(
                "Property [{key}] does not contain [{}].",
                missing
                    .iter()
                    .map(|value| value.to_string_lossy_json())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        self
    }

    // ------------------------------------------------------------------
    // Conditionable & debugging
    // ------------------------------------------------------------------

    /// Apply the callback when the condition is true.
    pub fn when(self, condition: bool, callback: impl FnOnce(Self) -> Self) -> Self {
        if condition { callback(self) } else { self }
    }

    /// Apply the callback unless the condition is true.
    pub fn unless(self, condition: bool, callback: impl FnOnce(Self) -> Self) -> Self {
        self.when(!condition, callback)
    }

    /// Dump the current scope (or one of its properties).
    pub fn dump(self, key: Option<&str>) -> Self {
        let value = self.prop(key);
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
        );
        self
    }
}

fn fail(message: String) -> ! {
    panic!("{message}");
}

fn assert_count(value: &Value, length: usize, message: &str) {
    match value {
        Value::Array(_) | Value::Object(_) => {
            let actual = value.count();
            if actual != length {
                fail(format!(
                    "{message}\nFailed asserting that actual size {actual} matches expected size {length}."
                ));
            }
        }
        other => fail(format!(
            "{message}\nFailed asserting that {} is countable.",
            export(other)
        )),
    }
}

/// The keys of an object, or the indexes of a list.
fn keys_of(value: &Value) -> Vec<String> {
    match value {
        Value::Object(map) => map.keys().cloned().collect(),
        Value::Array(items) => (0..items.len()).map(|i| i.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// The items of a collection: a list's elements, an object's values, or
/// the value itself.
fn items_of(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items,
        Value::Object(map) => map.into_iter().map(|(_, value)| value).collect(),
        Value::Null => Vec::new(),
        other => vec![other],
    }
}

fn entries(bindings: Value) -> Vec<(String, Value)> {
    match bindings {
        Value::Object(map) => map.into_iter().collect(),
        other => fail(format!(
            "Expected a JSON object of bindings, got {}.",
            export(&other)
        )),
    }
}

/// PHP's `gettype()` names, as Laravel's `whereType` sees decoded JSON.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_f64() => "double",
        Value::Number(_) => "integer",
        Value::String(_) => "string",
        Value::Array(_) | Value::Object(_) => "array",
    }
}

fn export(value: &Value) -> String {
    match value {
        Value::String(s) => format!("'{s}'"),
        other => other.to_string(),
    }
}

trait ToStringLossyJson {
    fn to_string_lossy_json(&self) -> String;
}

impl ToStringLossyJson for Value {
    /// Strings as-is, everything else as JSON.
    fn to_string_lossy_json(&self) -> String {
        match self {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn json_types_are_named_like_php() {
        assert_eq!(type_name(&json!(1)), "integer");
        assert_eq!(type_name(&json!(1.5)), "double");
        assert_eq!(type_name(&json!("a")), "string");
        assert_eq!(type_name(&json!(true)), "boolean");
        assert_eq!(type_name(&json!([1])), "array");
        assert_eq!(type_name(&json!({"a": 1})), "array");
        assert_eq!(type_name(&json!(null)), "null");
    }

    #[test]
    fn keys_of_lists_are_indexes() {
        assert_eq!(keys_of(&json!(["a", "b"])), vec!["0", "1"]);
        assert_eq!(keys_of(&json!({"b": 1, "a": 2})), vec!["b", "a"]);
    }
}
