//! A fluent, dynamic attribute container.

use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;

use crate::value::{Map, Value, ValueExt};

/// A dynamic bag of attributes with "dot" notation access.
///
/// ```
/// use illuminate_support::{Fluent, json};
///
/// let fluent = Fluent::from(json!({"name": "Taylor", "address": {"city": "Little Rock"}}));
/// assert_eq!(fluent.get("address.city"), json!("Little Rock"));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Fluent {
    attributes: Map<String, Value>,
}

impl Fluent {
    /// Create an empty fluent instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get an attribute using "dot" notation (null when missing).
    pub fn get(&self, key: &str) -> Value {
        self.lookup(key).cloned().unwrap_or(Value::Null)
    }

    fn lookup(&self, key: &str) -> Option<&Value> {
        if let Some(value) = self.attributes.get(key) {
            return Some(value);
        }
        let (head, rest) = key.split_once('.')?;
        self.attributes.get(head)?.dot(rest)
    }

    /// Get an attribute using "dot" notation, or the given default.
    pub fn get_or(&self, key: &str, default: impl Into<Value>) -> Value {
        self.lookup(key).cloned().unwrap_or_else(|| default.into())
    }

    /// Get an attribute as a string (empty when missing).
    pub fn string(&self, key: &str) -> String {
        self.lookup(key).map(ValueExt::to_string_lossy).unwrap_or_default()
    }

    /// Get an attribute as an integer (0 when missing or invalid).
    pub fn integer(&self, key: &str) -> i64 {
        self.lookup(key).and_then(ValueExt::to_i64_lossy).unwrap_or(0)
    }

    /// Get an attribute as a float (0.0 when missing or invalid).
    pub fn float(&self, key: &str) -> f64 {
        self.lookup(key).and_then(ValueExt::to_f64_lossy).unwrap_or(0.0)
    }

    /// Get an attribute as a boolean ("1", "true", "on" and "yes" are true).
    pub fn boolean(&self, key: &str) -> bool {
        match self.lookup(key) {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => crate::stringable::Stringable::new(s.clone()).to_boolean(),
            Some(other) => other.truthy(),
            None => false,
        }
    }

    /// Get an attribute as a date.
    pub fn date(&self, key: &str) -> Option<crate::carbon::Carbon> {
        let value = self.lookup(key)?;
        if value.is_blank() {
            return None;
        }
        crate::carbon::Carbon::parse(&value.to_string_lossy()).ok()
    }

    /// Get an attribute as a collection.
    pub fn collect(&self, key: &str) -> crate::collection::Collection<Value> {
        crate::collection::Collection::wrap(self.get(key))
    }

    /// Determine if the attribute is present and not blank.
    pub fn filled(&self, key: &str) -> bool {
        self.lookup(key).is_some_and(ValueExt::is_filled)
    }

    /// Determine if the attribute is missing.
    pub fn missing(&self, key: &str) -> bool {
        !self.has(key)
    }

    /// Determine if any of the attributes are present.
    pub fn has_any(&self, keys: &[&str]) -> bool {
        keys.iter().any(|k| self.has(k))
    }

    /// Get only the given attributes.
    pub fn only(&self, keys: &[&str]) -> Value {
        crate::arr::Arr::only(&self.to_array(), keys)
    }

    /// Get all of the attributes except the given ones.
    pub fn except(&self, keys: &[&str]) -> Value {
        crate::arr::Arr::except(&self.to_array(), keys)
    }

    /// Fill the fluent instance with the given attributes.
    pub fn fill(&mut self, attributes: Value) -> &mut Self {
        if let Value::Object(map) = attributes {
            for (key, value) in map {
                self.attributes.insert(key, value);
            }
        }
        self
    }

    /// Remove an attribute using "dot" notation.
    pub fn forget(&mut self, key: &str) -> &mut Self {
        let mut root = Value::Object(std::mem::take(&mut self.attributes));
        crate::arr::Arr::forget(&mut root, key);
        if let Value::Object(map) = root {
            self.attributes = map;
        }
        self
    }

    /// Determine if there are no attributes.
    pub fn is_empty(&self) -> bool {
        self.attributes.is_empty()
    }

    /// Determine if there are attributes.
    pub fn is_not_empty(&self) -> bool {
        !self.attributes.is_empty()
    }

    /// Get an attribute deserialized into a concrete type.
    pub fn get_as<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        crate::value::cast(self.get(key)).ok()
    }

    /// Set an attribute using "dot" notation.
    pub fn set(&mut self, key: &str, value: impl Into<Value>) -> &mut Self {
        let mut root = Value::Object(std::mem::take(&mut self.attributes));
        crate::arr::Arr::set(&mut root, key, value.into());
        if let Value::Object(map) = root {
            self.attributes = map;
        }
        self
    }

    /// Determine if an attribute exists using "dot" notation.
    pub fn has(&self, key: &str) -> bool {
        self.lookup(key).is_some()
    }

    /// Get the raw attributes.
    pub fn attributes(&self) -> &Map<String, Value> {
        &self.attributes
    }

    /// Get the attributes as a value.
    pub fn to_array(&self) -> Value {
        Value::Object(self.attributes.clone())
    }

    /// Convert the attributes to JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.attributes).unwrap_or_default()
    }
}

impl From<Value> for Fluent {
    fn from(value: Value) -> Self {
        match value {
            Value::Object(attributes) => Self { attributes },
            _ => Self::default(),
        }
    }
}

impl From<Map<String, Value>> for Fluent {
    fn from(attributes: Map<String, Value>) -> Self {
        Self { attributes }
    }
}

impl std::ops::Index<&str> for Fluent {
    type Output = Value;

    /// Read an attribute using "dot" notation (null when missing).
    fn index(&self, key: &str) -> &Value {
        static NULL: Value = Value::Null;
        self.lookup(key).unwrap_or(&NULL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn it_reads_and_writes_attributes() {
        let mut fluent = Fluent::from(json!({"name": "Taylor", "age": "38", "admin": "yes", "address": {"city": "Little Rock"}}));
        assert_eq!(fluent.get("address.city"), json!("Little Rock"));
        assert_eq!(fluent["address.city"], json!("Little Rock"));
        assert_eq!(fluent["missing"], Value::Null);
        assert_eq!(fluent.get_or("missing", "default"), json!("default"));
        assert_eq!(fluent.integer("age"), 38);
        assert_eq!(fluent.float("age"), 38.0);
        assert!(fluent.boolean("admin"));
        assert_eq!(fluent.string("name"), "Taylor");
        assert!(fluent.filled("name") && !fluent.filled("missing"));
        assert!(fluent.has_any(&["missing", "name"]));
        assert_eq!(fluent.only(&["name"]), json!({"name": "Taylor"}));
        fluent.set("address.zip", "72201").forget("admin").fill(json!({"framework": "Laravel"}));
        assert_eq!(fluent.get("address.zip"), json!("72201"));
        assert!(fluent.missing("admin"));
        assert_eq!(fluent.except(&["address", "age"]), json!({"name": "Taylor", "framework": "Laravel"}));
        assert_eq!(fluent.collect("name").count(), 1);
        assert!(fluent.is_not_empty());
        let dated = Fluent::from(json!({"born": "1986-06-27"}));
        assert_eq!(dated.date("born").unwrap().year(), 1986);
    }
}
