//! Array helpers operating on dynamic [`Value`]s with "dot" notation.

use crate::value::{Map, Value, ValueExt};

/// Static helpers for working with arrays (objects and lists of values).
pub struct Arr;

impl Arr {
    /// Determine whether the given value is array accessible.
    pub fn accessible(value: &Value) -> bool {
        matches!(value, Value::Array(_) | Value::Object(_))
    }

    /// Determine if the value is a list (sequential array).
    pub fn is_list(value: &Value) -> bool {
        matches!(value, Value::Array(_))
    }

    /// Determine if the value is an associative array (object).
    pub fn is_assoc(value: &Value) -> bool {
        matches!(value, Value::Object(_))
    }

    /// Get an item from an array using "dot" notation (null when missing).
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// let data = json!({"products": {"desk": {"price": 100}}});
    /// assert_eq!(Arr::get(&data, "products.desk.price"), json!(100));
    /// ```
    pub fn get(array: &Value, key: &str) -> Value {
        array.dot_or_null(key)
    }

    /// Get an item using "dot" notation, or the given default.
    pub fn get_or(array: &Value, key: &str, default: impl Into<Value>) -> Value {
        array.dot(key).cloned().unwrap_or_else(|| default.into())
    }

    /// Determine if an item exists in an array using "dot" notation.
    pub fn has(array: &Value, key: &str) -> bool {
        array.dot(key).is_some()
    }

    /// Determine if all of the given keys exist.
    pub fn has_all(array: &Value, keys: &[&str]) -> bool {
        !keys.is_empty() && keys.iter().all(|k| Self::has(array, k))
    }

    /// Determine if any of the given keys exist.
    pub fn has_any(array: &Value, keys: &[&str]) -> bool {
        keys.iter().any(|k| Self::has(array, k))
    }

    /// Set an item in an array using "dot" notation, creating nested
    /// objects as needed.
    pub fn set(array: &mut Value, key: &str, value: impl Into<Value>) {
        let value = value.into();
        if key.is_empty() {
            *array = value;
            return;
        }
        let segments: Vec<&str> = key.split('.').collect();
        let mut current = array;
        for (i, segment) in segments.iter().enumerate() {
            let last = i == segments.len() - 1;
            if !current.is_object() && !current.is_array() {
                *current = Value::Object(Map::new());
            }
            if let Value::Array(items) = current {
                if segment.parse::<usize>().is_err() {
                    // Convert the list into an object keyed by index.
                    let map: Map<String, Value> = std::mem::take(items)
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| (i.to_string(), v))
                        .collect();
                    *current = Value::Object(map);
                }
            }
            match current {
                Value::Array(items) => {
                    let index = segment.parse::<usize>().unwrap_or_default();
                    while items.len() <= index {
                        items.push(Value::Null);
                    }
                    if last {
                        items[index] = value;
                        return;
                    }
                    current = &mut items[index];
                }
                Value::Object(map) => {
                    if last {
                        map.insert(segment.to_string(), value);
                        return;
                    }
                    current = map
                        .entry(segment.to_string())
                        .or_insert_with(|| Value::Object(Map::new()));
                }
                _ => unreachable!(),
            }
        }
    }

    /// Add an element using "dot" notation if it doesn't already exist.
    pub fn add(array: &mut Value, key: &str, value: impl Into<Value>) {
        if !Self::has(array, key) || Self::get(array, key).is_null() {
            Self::set(array, key, value);
        }
    }

    /// Remove an item using "dot" notation.
    pub fn forget(array: &mut Value, key: &str) {
        if let Value::Object(map) = array {
            if map.shift_remove(key).is_some() {
                return;
            }
        }
        let (parent_key, last) = match key.rsplit_once('.') {
            Some((parent, last)) => (Some(parent), last),
            None => (None, key),
        };
        let parent = match parent_key {
            Some(p) => match dot_mut(array, p) {
                Some(parent) => parent,
                None => return,
            },
            None => array,
        };
        match parent {
            Value::Object(map) => {
                map.shift_remove(last);
            }
            Value::Array(items) => {
                if let Ok(index) = last.parse::<usize>() {
                    if index < items.len() {
                        items.remove(index);
                    }
                }
            }
            _ => {}
        }
    }

    /// Get a subset of the items from the given object.
    pub fn only(array: &Value, keys: &[&str]) -> Value {
        let mut out = Value::Object(Map::new());
        for key in keys {
            if let Some(value) = array.dot(key) {
                Self::set(&mut out, key, value.clone());
            }
        }
        out
    }

    /// Get all of the given object except for the specified keys.
    pub fn except(array: &Value, keys: &[&str]) -> Value {
        let mut out = array.clone();
        for key in keys {
            Self::forget(&mut out, key);
        }
        out
    }

    /// Pull (get and remove) a value from the array.
    pub fn pull(array: &mut Value, key: &str) -> Value {
        let value = Self::get(array, key);
        Self::forget(array, key);
        value
    }

    /// Wrap the given value in an array if it isn't one already.
    pub fn wrap(value: impl Into<Value>) -> Value {
        match value.into() {
            Value::Null => Value::Array(Vec::new()),
            Value::Array(items) => Value::Array(items),
            other => Value::Array(vec![other]),
        }
    }

    /// Flatten a multi-dimensional associative array with dots.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// let flat = Arr::dot(&json!({"user": {"name": "Taylor"}}));
    /// assert_eq!(flat, json!({"user.name": "Taylor"}));
    /// ```
    pub fn dot(array: &Value) -> Value {
        fn walk(value: &Value, prefix: &str, out: &mut Map<String, Value>) {
            match value {
                Value::Object(map) if !map.is_empty() => {
                    for (k, v) in map {
                        walk(v, &format!("{prefix}{k}."), out);
                    }
                }
                Value::Array(items) if !items.is_empty() => {
                    for (i, v) in items.iter().enumerate() {
                        walk(v, &format!("{prefix}{i}."), out);
                    }
                }
                other => {
                    out.insert(prefix.trim_end_matches('.').to_string(), other.clone());
                }
            }
        }
        let mut out = Map::new();
        if let Value::Object(map) = array {
            for (k, v) in map {
                walk(v, &format!("{k}."), &mut out);
            }
        } else if let Value::Array(items) = array {
            for (i, v) in items.iter().enumerate() {
                walk(v, &format!("{i}."), &mut out);
            }
        }
        Value::Object(out)
    }

    /// Convert a flattened "dot" notation array into an expanded array.
    pub fn undot(array: &Value) -> Value {
        let mut out = Value::Object(Map::new());
        if let Value::Object(map) = array {
            for (k, v) in map {
                Self::set(&mut out, k, v.clone());
            }
        }
        out
    }

    /// Flatten a multi-dimensional array into a single level list.
    pub fn flatten(array: &Value, depth: Option<usize>) -> Vec<Value> {
        fn walk(value: &Value, depth: Option<usize>, out: &mut Vec<Value>) {
            let children: Vec<&Value> = match value {
                Value::Array(items) => items.iter().collect(),
                Value::Object(map) => map.values().collect(),
                _ => return,
            };
            for child in children {
                let nested = matches!(child, Value::Array(_) | Value::Object(_));
                match depth {
                    _ if !nested => out.push(child.clone()),
                    Some(1) => out.extend(match child {
                        Value::Array(items) => items.clone(),
                        Value::Object(map) => map.values().cloned().collect(),
                        _ => vec![],
                    }),
                    Some(d) => walk(child, Some(d - 1), out),
                    None => walk(child, None, out),
                }
            }
        }
        let mut out = Vec::new();
        walk(array, depth, &mut out);
        out
    }

    /// Get the first element of a list (or object value).
    pub fn first(array: &Value) -> Value {
        match array {
            Value::Array(items) => items.first().cloned().unwrap_or(Value::Null),
            Value::Object(map) => map.values().next().cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }

    /// Get the last element of a list (or object value).
    pub fn last(array: &Value) -> Value {
        match array {
            Value::Array(items) => items.last().cloned().unwrap_or(Value::Null),
            Value::Object(map) => map.values().last().cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }

    /// Pluck an array of values from a list of objects.
    pub fn pluck(array: &Value, key: &str) -> Vec<Value> {
        match array {
            Value::Array(items) => items.iter().map(|item| item.dot_or_null(key)).collect(),
            Value::Object(map) => map.values().map(|item| item.dot_or_null(key)).collect(),
            _ => Vec::new(),
        }
    }

    /// Get the keys of an object.
    pub fn keys(array: &Value) -> Vec<String> {
        match array {
            Value::Object(map) => map.keys().cloned().collect(),
            Value::Array(items) => (0..items.len()).map(|i| i.to_string()).collect(),
            _ => Vec::new(),
        }
    }

    /// Filter the array using the given callback (key, value).
    pub fn where_(array: &Value, mut callback: impl FnMut(&str, &Value) -> bool) -> Value {
        match array {
            Value::Object(map) => Value::Object(
                map.iter()
                    .filter(|(k, v)| callback(k, v))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .enumerate()
                    .filter(|(i, v)| callback(&i.to_string(), v))
                    .map(|(_, v)| v.clone())
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// Filter out null values.
    pub fn where_not_null(array: &Value) -> Value {
        Self::where_(array, |_, v| !v.is_null())
    }

    /// Recursively merge `other` into `array` (objects merge, everything else replaces).
    pub fn merge_recursive(array: &mut Value, other: Value) {
        match (array, other) {
            (Value::Object(base), Value::Object(incoming)) => {
                for (k, v) in incoming {
                    match base.get_mut(&k) {
                        Some(existing) if existing.is_object() && v.is_object() => {
                            Self::merge_recursive(existing, v)
                        }
                        _ => {
                            base.insert(k, v);
                        }
                    }
                }
            }
            (slot, other) => *slot = other,
        }
    }

    /// Convert the array into a query string.
    pub fn query(array: &Value) -> String {
        let flat = Self::dot(array);
        let mut serializer = Vec::new();
        if let Value::Object(map) = flat {
            for (key, value) in map {
                let key = match key.split_once('.') {
                    Some((head, rest)) => format!(
                        "{head}{}",
                        rest.split('.').map(|s| format!("[{s}]")).collect::<String>()
                    ),
                    None => key,
                };
                serializer.push(format!(
                    "{}={}",
                    url_encode(&key),
                    url_encode(&value.to_string_lossy())
                ));
            }
        }
        serializer.join("&")
    }

    /// Conditionally compile classes from an array into a CSS class list.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// let classes = Arr::to_css_classes(&json!(["p-4", {"font-bold": true, "hidden": false}]));
    /// assert_eq!(classes, "p-4 font-bold");
    /// ```
    pub fn to_css_classes(array: &Value) -> String {
        let mut classes = Vec::new();
        let mut visit = |key: &str, value: &Value| {
            classes.push((key.to_string(), value.clone()));
        };
        match array {
            Value::Array(items) => {
                for item in items {
                    match item {
                        Value::Object(map) => map.iter().for_each(|(k, v)| visit(k, v)),
                        other => visit(&other.to_string_lossy(), &Value::Bool(true)),
                    }
                }
            }
            Value::Object(map) => map.iter().for_each(|(k, v)| visit(k, v)),
            Value::String(s) => visit(s, &Value::Bool(true)),
            _ => {}
        }
        classes
            .into_iter()
            .filter(|(k, v)| !k.is_empty() && v.truthy())
            .map(|(k, _)| k)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn dot_mut<'a>(value: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    let mut current = value;
    for segment in key.split('.') {
        current = match current {
            Value::Object(map) => map.get_mut(segment)?,
            Value::Array(items) => items.get_mut(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn it_sets_nested_values() {
        let mut data = json!({});
        Arr::set(&mut data, "products.desk.price", 100);
        assert_eq!(data, json!({"products": {"desk": {"price": 100}}}));
        Arr::set(&mut data, "list.0", "a");
        assert_eq!(data["list"], json!({"0": "a"}));
    }

    #[test]
    fn it_forgets_nested_values() {
        let mut data = json!({"products": {"desk": {"price": 100, "name": "Desk"}}});
        Arr::forget(&mut data, "products.desk.price");
        assert_eq!(data, json!({"products": {"desk": {"name": "Desk"}}}));
    }

    #[test]
    fn it_gets_only_and_except() {
        let data = json!({"name": "Desk", "price": 100, "orders": 10});
        assert_eq!(Arr::only(&data, &["name", "price"]), json!({"name": "Desk", "price": 100}));
        assert_eq!(Arr::except(&data, &["price"]), json!({"name": "Desk", "orders": 10}));
    }

    #[test]
    fn it_dots_and_undots() {
        let data = json!({"user": {"name": "Taylor", "tags": ["a", "b"]}});
        let flat = Arr::dot(&data);
        assert_eq!(flat, json!({"user.name": "Taylor", "user.tags.0": "a", "user.tags.1": "b"}));
    }

    #[test]
    fn it_builds_query_strings() {
        assert_eq!(Arr::query(&json!({"name": "Taylor Otwell", "page": 2})), "name=Taylor+Otwell&page=2");
    }
}
