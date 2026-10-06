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
    pub fn new() -> Self {
        Self::default()
    }

    /// Get an attribute using "dot" notation (null when missing).
    pub fn get(&self, key: &str) -> Value {
        Value::Object(self.attributes.clone()).dot_or_null(key)
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

    pub fn has(&self, key: &str) -> bool {
        Value::Object(self.attributes.clone()).dot(key).is_some()
    }

    pub fn attributes(&self) -> &Map<String, Value> {
        &self.attributes
    }

    pub fn to_array(&self) -> Value {
        Value::Object(self.attributes.clone())
    }

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
