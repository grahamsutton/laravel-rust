//! `ValidatedInput`: a convenient wrapper around validated data.

use indexmap::IndexMap;
use serde::de::DeserializeOwned;

use illuminate_http::UploadedFile;
use illuminate_support::{Collection, Map, Value, ValueExt, cast};

use crate::data;

/// The data that passed validation, with helpers to pick it apart.
///
/// ```
/// use illuminate_validation::ValidatedInput;
/// use illuminate_support::json;
///
/// let input = ValidatedInput::new(json!({"name": "Taylor", "email": "taylor@laravel.com", "role": "admin"}));
///
/// assert_eq!(input.only(&["name", "email"]), json!({"name": "Taylor", "email": "taylor@laravel.com"}));
/// assert_eq!(input.except(&["role"]), json!({"name": "Taylor", "email": "taylor@laravel.com"}));
/// assert_eq!(input.merge(json!({"role": "owner"})).input("role"), json!("owner"));
/// assert!(input.has("email"));
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ValidatedInput {
    input: Value,
    files: IndexMap<String, UploadedFile>,
}

impl ValidatedInput {
    /// Wrap validated data.
    pub fn new(input: Value) -> Self {
        let input = if input.is_object() { input } else { Value::Object(Map::new()) };
        Self {
            input,
            files: IndexMap::new(),
        }
    }

    /// Wrap validated data and the validated uploaded files.
    pub fn with_files(input: Value, files: IndexMap<String, UploadedFile>) -> Self {
        let mut validated = Self::new(input);
        validated.files = files;
        validated
    }

    /// All of the validated data.
    pub fn all(&self) -> &Value {
        &self.input
    }

    /// Convert into the underlying value.
    pub fn into_value(self) -> Value {
        self.input
    }

    /// Get a subset of the validated data (dot notation supported).
    pub fn only(&self, keys: &[&str]) -> Value {
        let mut result = Value::Object(Map::new());
        for key in keys {
            if let Some(value) = data::get(&self.input, key) {
                data::set(&mut result, key, value.clone());
            }
        }
        result
    }

    /// Get all of the validated data except the given keys.
    pub fn except(&self, keys: &[&str]) -> Value {
        let mut result = self.input.clone();
        for key in keys {
            data::forget(&mut result, key);
        }
        result
    }

    /// Merge additional data into the validated data.
    pub fn merge(&self, items: Value) -> Self {
        let mut input = self.input.clone();
        if let (Value::Object(target), Value::Object(items)) = (&mut input, items) {
            for (key, value) in items {
                target.insert(key, value);
            }
        }
        Self {
            input,
            files: self.files.clone(),
        }
    }

    /// Get a validated value using dot notation (`null` when missing).
    pub fn input(&self, key: &str) -> Value {
        data::get(&self.input, key).cloned().unwrap_or(Value::Null)
    }

    /// Get a validated value, or a default.
    pub fn input_or(&self, key: &str, default: impl Into<Value>) -> Value {
        match data::get(&self.input, key) {
            Some(Value::Null) | None => default.into(),
            Some(value) => value.clone(),
        }
    }

    /// Get a validated value as a string.
    pub fn string(&self, key: &str) -> String {
        self.input(key).to_string_lossy()
    }

    /// Get a validated value as an integer.
    pub fn integer(&self, key: &str) -> i64 {
        self.input(key).to_i64_lossy().unwrap_or_default()
    }

    /// Get a validated value as a boolean.
    pub fn boolean(&self, key: &str) -> bool {
        cast::<bool>(self.input(key)).unwrap_or(false)
    }

    /// Deserialize the validated data into a concrete type.
    pub fn deserialize<T: DeserializeOwned>(&self) -> illuminate_support::Result<T> {
        cast(self.input.clone())
    }

    /// Get a validated uploaded file.
    pub fn file(&self, key: &str) -> Option<&UploadedFile> {
        self.files.get(key)
    }

    /// The validated uploaded files, keyed by attribute.
    pub fn files(&self) -> &IndexMap<String, UploadedFile> {
        &self.files
    }

    /// The top-level keys of the validated data.
    pub fn keys(&self) -> Vec<String> {
        self.input.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default()
    }

    /// Determine if the validated data contains the key.
    pub fn has(&self, key: &str) -> bool {
        data::has(&self.input, key) || self.files.contains_key(key)
    }

    /// Determine if the validated data contains any of the keys.
    pub fn has_any(&self, keys: &[&str]) -> bool {
        keys.iter().any(|k| self.has(k))
    }

    /// Determine if the validated data is missing the key.
    pub fn missing(&self, key: &str) -> bool {
        !self.has(key)
    }

    /// Determine if the key is present and not blank.
    pub fn filled(&self, key: &str) -> bool {
        data::get(&self.input, key).is_some_and(|v| !v.is_blank())
    }

    /// The validated data as a collection of key / value pairs.
    pub fn collect(&self) -> Collection<(String, Value)> {
        self.input
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }
}

impl std::ops::Index<&str> for ValidatedInput {
    type Output = Value;

    fn index(&self, key: &str) -> &Value {
        data::get(&self.input, key).unwrap_or(&Value::Null)
    }
}

impl IntoIterator for ValidatedInput {
    type Item = (String, Value);
    type IntoIter = serde_json::map::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        match self.input {
            Value::Object(map) => map.into_iter(),
            _ => Map::new().into_iter(),
        }
    }
}

impl From<ValidatedInput> for Value {
    fn from(input: ValidatedInput) -> Self {
        input.input
    }
}
