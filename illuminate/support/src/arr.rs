//! Array helpers operating on dynamic [`Value`]s with "dot" notation.

use crate::collection::{compare_for_sort, loose_eq};
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
            Value::Object(map) => map.values().next_back().cloned().unwrap_or(Value::Null),
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

    /// Convert the array into a query string, like PHP's
    /// `http_build_query($array, '', '&', PHP_QUERY_RFC3986)`.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// let query = Arr::query(&json!({"name": "Taylor", "order": {"column": "created_at", "direction": "desc"}}));
    /// assert_eq!(query, "name=Taylor&order%5Bcolumn%5D=created_at&order%5Bdirection%5D=desc");
    /// ```
    pub fn query(array: &Value) -> String {
        fn build(prefix: Option<&str>, value: &Value, out: &mut Vec<String>) {
            match (prefix, value) {
                (_, Value::Null) => {}
                (_, Value::Array(items)) => {
                    for (i, item) in items.iter().enumerate() {
                        let key = match prefix {
                            Some(p) => format!("{p}[{i}]"),
                            None => i.to_string(),
                        };
                        build(Some(&key), item, out);
                    }
                }
                (_, Value::Object(map)) => {
                    for (k, item) in map {
                        let key = match prefix {
                            Some(p) => format!("{p}[{k}]"),
                            None => k.clone(),
                        };
                        build(Some(&key), item, out);
                    }
                }
                (None, _) => {}
                (Some(key), Value::Bool(b)) => {
                    out.push(format!("{}={}", url_encode(key), if *b { "1" } else { "0" }));
                }
                (Some(key), other) => {
                    out.push(format!("{}={}", url_encode(key), url_encode(&other.to_string_lossy())));
                }
            }
        }
        let mut out = Vec::new();
        if matches!(array, Value::Array(_) | Value::Object(_)) {
            build(None, array, &mut out);
        }
        out.join("&")
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

/// The rest of Laravel's array helpers. Callbacks receive `(value, key)`,
/// like Laravel's (keys of lists are their stringified indexes).
impl Arr {
    /// Collapse an array of arrays into a single list.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// assert_eq!(Arr::collapse(&json!([[1, 2, 3], [4, 5, 6], [7, 8, 9]])), json!([1, 2, 3, 4, 5, 6, 7, 8, 9]));
    /// ```
    pub fn collapse(array: &Value) -> Value {
        let mut out = Vec::new();
        for item in entries(array).into_iter().map(|(_, v)| v) {
            match item {
                Value::Array(items) => out.extend(items.iter().cloned()),
                Value::Object(map) => out.extend(map.values().cloned()),
                _ => {}
            }
        }
        Value::Array(out)
    }

    /// Cross join the given arrays, returning all possible permutations.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// let matrix = Arr::cross_join(&[json!([1, 2]), json!(["a", "b"])]);
    /// assert_eq!(matrix, json!([[1, "a"], [1, "b"], [2, "a"], [2, "b"]]));
    /// ```
    pub fn cross_join(arrays: &[Value]) -> Value {
        let mut results: Vec<Vec<Value>> = vec![Vec::new()];
        for array in arrays {
            let values: Vec<Value> = entries(array).into_iter().map(|(_, v)| v.clone()).collect();
            let mut appended = Vec::with_capacity(results.len() * values.len());
            for product in &results {
                for value in &values {
                    let mut next = product.clone();
                    next.push(value.clone());
                    appended.push(next);
                }
            }
            results = appended;
        }
        Value::Array(results.into_iter().map(Value::Array).collect())
    }

    /// Divide an array into two lists: one with keys and one with values.
    pub fn divide(array: &Value) -> (Vec<String>, Vec<Value>) {
        entries(array)
            .into_iter()
            .map(|(k, v)| (k, v.clone()))
            .unzip()
    }

    /// Determine if the given key exists in the array (no "dot" notation).
    pub fn exists(array: &Value, key: &str) -> bool {
        match array {
            Value::Object(map) => map.contains_key(key),
            Value::Array(items) => key.parse::<usize>().is_ok_and(|i| i < items.len()),
            _ => false,
        }
    }

    /// Return the first element passing the truth test (null when none do).
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// assert_eq!(Arr::first_fn(&json!([100, 200, 300]), |value, _| value.as_i64() >= Some(150)), json!(200));
    /// ```
    pub fn first_fn(array: &Value, mut callback: impl FnMut(&Value, &str) -> bool) -> Value {
        entries(array)
            .into_iter()
            .find(|(k, v)| callback(v, k))
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Null)
    }

    /// Return the first element of the array, or the default.
    pub fn first_or(array: &Value, default: impl Into<Value>) -> Value {
        match Self::first(array) {
            Value::Null => default.into(),
            other => other,
        }
    }

    /// Return the last element passing the truth test (null when none do).
    pub fn last_fn(array: &Value, mut callback: impl FnMut(&Value, &str) -> bool) -> Value {
        entries(array)
            .into_iter()
            .rev()
            .find(|(k, v)| callback(v, k))
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Null)
    }

    /// Remove several items using "dot" notation.
    pub fn forget_many(array: &mut Value, keys: &[&str]) {
        for key in keys {
            Self::forget(array, key);
        }
    }

    /// Get an integer item, failing if it is not an integer.
    pub fn integer(array: &Value, key: &str) -> crate::Result<i64> {
        let value = Self::get(array, key);
        value
            .as_i64()
            .filter(|_| value.is_i64() || value.is_u64())
            .ok_or_else(|| typed_error(key, "an integer", &value))
    }

    /// Get a float item, failing if it is not a float.
    pub fn float(array: &Value, key: &str) -> crate::Result<f64> {
        let value = Self::get(array, key);
        value
            .as_f64()
            .filter(|_| value.is_f64())
            .ok_or_else(|| typed_error(key, "a float", &value))
    }

    /// Get a string item, failing if it is not a string.
    pub fn string(array: &Value, key: &str) -> crate::Result<String> {
        let value = Self::get(array, key);
        value
            .as_str()
            .map(String::from)
            .ok_or_else(|| typed_error(key, "a string", &value))
    }

    /// Get a boolean item, failing if it is not a boolean.
    pub fn boolean(array: &Value, key: &str) -> crate::Result<bool> {
        let value = Self::get(array, key);
        value.as_bool().ok_or_else(|| typed_error(key, "a boolean", &value))
    }

    /// Get an array item, failing if it is not an array.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// let array = json!({"name": "Joe", "languages": ["PHP", "Ruby"]});
    /// assert_eq!(Arr::array(&array, "languages").unwrap(), json!(["PHP", "Ruby"]));
    /// assert!(Arr::array(&array, "name").is_err());
    /// ```
    pub fn array(array: &Value, key: &str) -> crate::Result<Value> {
        let value = Self::get(array, key);
        if Self::accessible(&value) {
            Ok(value)
        } else {
            Err(typed_error(key, "an array", &value))
        }
    }

    /// Join all items using a string, with a different glue for the last item.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// let array = json!(["Tailwind", "Alpine", "Laravel", "Livewire"]);
    /// assert_eq!(Arr::join(&array, ", ", ", "), "Tailwind, Alpine, Laravel, Livewire");
    /// assert_eq!(Arr::join(&array, ", ", ", and "), "Tailwind, Alpine, Laravel, and Livewire");
    /// ```
    pub fn join(array: &Value, glue: &str, final_glue: &str) -> String {
        let strings: Vec<String> = entries(array)
            .into_iter()
            .map(|(_, v)| v.to_string_lossy())
            .collect();
        match strings.len() {
            0 => String::new(),
            1 => strings[0].clone(),
            n => format!("{}{}{}", strings[..n - 1].join(glue), final_glue, strings[n - 1]),
        }
    }

    /// Key an array of items by the value of the given ("dot" notation) key.
    pub fn key_by(array: &Value, key: &str) -> Value {
        Self::key_by_fn(array, |item, _| item.dot_or_null(key).to_string_lossy())
    }

    /// Key an array of items by the value returned from the callback.
    pub fn key_by_fn(array: &Value, mut callback: impl FnMut(&Value, &str) -> String) -> Value {
        let mut out = Map::new();
        for (k, v) in entries(array) {
            out.insert(callback(v, &k), v.clone());
        }
        Value::Object(out)
    }

    /// Run a map over each of the items, preserving keys.
    ///
    /// ```
    /// use illuminate_support::{Arr, Str, json};
    ///
    /// let mapped = Arr::map(&json!({"first": "james", "last": "kirk"}), |value, _| {
    ///     Str::ucfirst(value.as_str().unwrap_or_default()).into()
    /// });
    /// assert_eq!(mapped, json!({"first": "James", "last": "Kirk"}));
    /// ```
    pub fn map(array: &Value, mut callback: impl FnMut(&Value, &str) -> Value) -> Value {
        match array {
            Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), callback(v, k))).collect()),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, v)| callback(v, &i.to_string()))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// Run a map over nested arrays, spreading each one into the callback.
    pub fn map_spread(array: &Value, mut callback: impl FnMut(&[Value]) -> Value) -> Value {
        Value::Array(
            entries(array)
                .into_iter()
                .map(|(_, v)| match v {
                    Value::Array(items) => callback(items),
                    Value::Object(map) => callback(&map.values().cloned().collect::<Vec<_>>()),
                    other => callback(std::slice::from_ref(other)),
                })
                .collect(),
        )
    }

    /// Run an associative map over each of the items.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// let people = json!([{"name": "John", "email": "john@example.com"}, {"name": "Jane", "email": "jane@example.com"}]);
    /// let mapped = Arr::map_with_keys(&people, |item, _| {
    ///     (item["email"].as_str().unwrap().to_string(), item["name"].clone())
    /// });
    /// assert_eq!(mapped, json!({"john@example.com": "John", "jane@example.com": "Jane"}));
    /// ```
    pub fn map_with_keys(
        array: &Value,
        mut callback: impl FnMut(&Value, &str) -> (String, Value),
    ) -> Value {
        let mut out = Map::new();
        for (k, v) in entries(array) {
            let (key, value) = callback(v, &k);
            out.insert(key, value);
        }
        Value::Object(out)
    }

    /// Get only the items whose values loosely equal one of the given values.
    pub fn only_values(array: &Value, values: &[Value]) -> Value {
        Self::where_values(array, values, false, true)
    }

    /// Get only the items whose values strictly equal one of the given values.
    pub fn only_values_strict(array: &Value, values: &[Value]) -> Value {
        Self::where_values(array, values, true, true)
    }

    /// Get all items except those whose values loosely equal one of the given values.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// assert_eq!(Arr::except_values(&json!(["foo", "bar", "baz", "qux"]), &[json!("foo"), json!("baz")]), json!(["bar", "qux"]));
    /// assert_eq!(Arr::except_values_strict(&json!([1, "1", 2, "2"]), &[json!(1), json!(2)]), json!(["1", "2"]));
    /// ```
    pub fn except_values(array: &Value, values: &[Value]) -> Value {
        Self::where_values(array, values, false, false)
    }

    /// Get all items except those whose values strictly equal one of the given values.
    pub fn except_values_strict(array: &Value, values: &[Value]) -> Value {
        Self::where_values(array, values, true, false)
    }

    fn where_values(array: &Value, values: &[Value], strict: bool, keep: bool) -> Value {
        Self::filter_entries(array, |v, _| {
            let found = values
                .iter()
                .any(|candidate| if strict { candidate == v } else { loose_eq(candidate, v) });
            found == keep
        })
    }

    /// Partition the array into two arrays using the given callback.
    pub fn partition(array: &Value, mut callback: impl FnMut(&Value, &str) -> bool) -> (Value, Value) {
        let passed = Self::filter_entries(array, &mut callback);
        let failed = Self::filter_entries(array, |v, k| !callback(v, k));
        (passed, failed)
    }

    /// Pluck an array of values keyed by another value of each item.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// let array = json!([
    ///     {"developer": {"id": 1, "name": "Taylor"}},
    ///     {"developer": {"id": 2, "name": "Abigail"}},
    /// ]);
    /// assert_eq!(Arr::pluck_with_key(&array, "developer.name", "developer.id"), json!({"1": "Taylor", "2": "Abigail"}));
    /// ```
    pub fn pluck_with_key(array: &Value, value: &str, key: &str) -> Value {
        let mut out = Map::new();
        for (_, item) in entries(array) {
            out.insert(item.dot_or_null(key).to_string_lossy(), item.dot_or_null(value));
        }
        Value::Object(out)
    }

    /// Push an item onto the beginning of a list.
    pub fn prepend(array: &Value, value: impl Into<Value>) -> Value {
        let mut items = vec![value.into()];
        items.extend(entries(array).into_iter().map(|(_, v)| v.clone()));
        Value::Array(items)
    }

    /// Push an item onto the beginning of an object, under the given key.
    pub fn prepend_with_key(array: &Value, value: impl Into<Value>, key: &str) -> Value {
        let mut out = Map::new();
        out.insert(key.to_string(), value.into());
        for (k, v) in entries(array) {
            if k != key {
                out.insert(k, v.clone());
            }
        }
        Value::Object(out)
    }

    /// Prefix every key of the array with the given string.
    pub fn prepend_keys_with(array: &Value, prefix: &str) -> Value {
        Value::Object(
            entries(array)
                .into_iter()
                .map(|(k, v)| (format!("{prefix}{k}"), v.clone()))
                .collect(),
        )
    }

    /// Get a value from the array and remove it, falling back to the default.
    pub fn pull_or(array: &mut Value, key: &str, default: impl Into<Value>) -> Value {
        if Self::has(array, key) {
            Self::pull(array, key)
        } else {
            default.into()
        }
    }

    /// Push a value onto the list found at the given ("dot" notation) key.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// let mut array = json!({});
    /// Arr::push(&mut array, "office.furniture", "Desk");
    /// assert_eq!(array, json!({"office": {"furniture": ["Desk"]}}));
    /// ```
    pub fn push(array: &mut Value, key: &str, value: impl Into<Value>) {
        let mut list = match Self::get(array, key) {
            Value::Array(items) => items,
            Value::Null => Vec::new(),
            other => vec![other],
        };
        list.push(value.into());
        Self::set(array, key, Value::Array(list));
    }

    /// Get a random value from the array.
    pub fn random(array: &Value) -> Option<Value> {
        use rand::seq::IndexedRandom;
        let values: Vec<&Value> = entries(array).into_iter().map(|(_, v)| v).collect();
        values.choose(&mut rand::rng()).map(|v| (*v).clone())
    }

    /// Get the given number of random values from the array.
    pub fn random_many(array: &Value, number: usize) -> crate::Result<Value> {
        use rand::seq::IndexedRandom;
        let values: Vec<&Value> = entries(array).into_iter().map(|(_, v)| v).collect();
        if number > values.len() {
            return Err(crate::error::InvalidArgumentException::new(format!(
                "You requested {number} items, but there are only {} items available.",
                values.len()
            ))
            .into());
        }
        Ok(Value::Array(
            values
                .choose_multiple(&mut rand::rng(), number)
                .map(|v| (*v).clone())
                .collect(),
        ))
    }

    /// Filter the array using the negation of the callback.
    pub fn reject(array: &Value, mut callback: impl FnMut(&Value, &str) -> bool) -> Value {
        Self::filter_entries(array, |v, k| !callback(v, k))
    }

    /// Select only the given keys from each item of the array.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// let array = json!([{"id": 1, "name": "Desk", "price": 200}, {"id": 2, "name": "Table", "price": 150}]);
    /// assert_eq!(Arr::select(&array, &["name", "price"]), json!([{"name": "Desk", "price": 200}, {"name": "Table", "price": 150}]));
    /// ```
    pub fn select(array: &Value, keys: &[&str]) -> Value {
        Value::Array(
            entries(array)
                .into_iter()
                .map(|(_, item)| {
                    let mut out = Map::new();
                    for key in keys {
                        if let Some(value) = item.get(*key) {
                            out.insert((*key).to_string(), value.clone());
                        }
                    }
                    Value::Object(out)
                })
                .collect(),
        )
    }

    /// Shuffle the values of the array into a list.
    pub fn shuffle(array: &Value) -> Value {
        use rand::seq::SliceRandom;
        let mut values: Vec<Value> = entries(array).into_iter().map(|(_, v)| v.clone()).collect();
        values.shuffle(&mut rand::rng());
        Value::Array(values)
    }

    /// Get the one and only item passing the truth test.
    pub fn sole(array: &Value, mut callback: impl FnMut(&Value, &str) -> bool) -> crate::Result<Value> {
        let matches: Vec<Value> = entries(array)
            .into_iter()
            .filter(|(k, v)| callback(v, k))
            .map(|(_, v)| v.clone())
            .collect();
        match matches.len() {
            0 => Err(crate::collection::ItemNotFoundException.into()),
            1 => Ok(matches.into_iter().next().unwrap_or(Value::Null)),
            count => Err(crate::collection::MultipleItemsFoundException { count }.into()),
        }
    }

    /// Determine if any item passes the truth test.
    pub fn some(array: &Value, mut callback: impl FnMut(&Value, &str) -> bool) -> bool {
        entries(array).into_iter().any(|(k, v)| callback(v, &k))
    }

    /// Determine if every item passes the truth test.
    pub fn every(array: &Value, mut callback: impl FnMut(&Value, &str) -> bool) -> bool {
        entries(array).into_iter().all(|(k, v)| callback(v, &k))
    }

    /// Sort the array by its values (objects keep their keys).
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// assert_eq!(Arr::sort(&json!(["Desk", "Table", "Chair"])), json!(["Chair", "Desk", "Table"]));
    /// ```
    pub fn sort(array: &Value) -> Value {
        Self::sort_by(array, |v| v.clone())
    }

    /// Sort the array by the values returned from the callback.
    pub fn sort_by(array: &Value, callback: impl FnMut(&Value) -> Value) -> Value {
        sort_entries(array, callback, false)
    }

    /// Sort the array by its values, descending.
    pub fn sort_desc(array: &Value) -> Value {
        Self::sort_desc_by(array, |v| v.clone())
    }

    /// Sort the array by the values returned from the callback, descending.
    pub fn sort_desc_by(array: &Value, callback: impl FnMut(&Value) -> Value) -> Value {
        sort_entries(array, callback, true)
    }

    /// Recursively sort an array: objects by key, lists by value.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// let sorted = Arr::sort_recursive(&json!({"b": [3, 1, 2], "a": {"y": 1, "x": 2}}));
    /// assert_eq!(serde_json::to_string(&sorted).unwrap(), r#"{"a":{"x":2,"y":1},"b":[1,2,3]}"#);
    /// ```
    pub fn sort_recursive(array: &Value) -> Value {
        sort_recursive(array, false)
    }

    /// Recursively sort an array in descending order.
    pub fn sort_recursive_desc(array: &Value) -> Value {
        sort_recursive(array, true)
    }

    /// Take the first (or, when negative, last) `limit` items.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// assert_eq!(Arr::take(&json!([0, 1, 2, 3, 4, 5]), 3), json!([0, 1, 2]));
    /// assert_eq!(Arr::take(&json!([0, 1, 2, 3, 4, 5]), -2), json!([4, 5]));
    /// ```
    pub fn take(array: &Value, limit: isize) -> Value {
        let all = entries(array);
        let len = all.len() as isize;
        let (skip, take) = if limit >= 0 {
            (0, limit.min(len) as usize)
        } else {
            ((len + limit).max(0) as usize, (-limit).min(len) as usize)
        };
        let selected = all.into_iter().skip(skip).take(take);
        match array {
            Value::Object(_) => Value::Object(selected.map(|(k, v)| (k, v.clone())).collect()),
            _ => Value::Array(selected.map(|(_, v)| v.clone()).collect()),
        }
    }

    /// Conditionally compile styles from an array into a style list.
    ///
    /// ```
    /// use illuminate_support::{Arr, json};
    ///
    /// let styles = Arr::to_css_styles(&json!(["background-color: blue", {"color: blue": true, "font-weight: bold": false}]));
    /// assert_eq!(styles, "background-color: blue; color: blue;");
    /// ```
    pub fn to_css_styles(array: &Value) -> String {
        let mut styles = Vec::new();
        let mut visit = |key: &str, value: &Value| {
            if !key.is_empty() && value.truthy() {
                styles.push(format!("{};", key.trim_end_matches(';')));
            }
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
        styles.join(" ")
    }

    /// Convert any serializable value into a dynamic array value.
    pub fn from<T: serde::Serialize + ?Sized>(value: &T) -> Value {
        crate::value::to_value(value)
    }

    fn filter_entries(array: &Value, mut keep: impl FnMut(&Value, &str) -> bool) -> Value {
        match array {
            Value::Object(map) => Value::Object(
                map.iter()
                    .filter(|(k, v)| keep(v, k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .enumerate()
                    .filter(|(i, v)| keep(v, &i.to_string()))
                    .map(|(_, v)| v.clone())
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

/// The `(key, value)` entries of an array value; lists use their indexes as keys.
fn entries(array: &Value) -> Vec<(String, &Value)> {
    match array {
        Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v)).collect(),
        Value::Array(items) => items.iter().enumerate().map(|(i, v)| (i.to_string(), v)).collect(),
        _ => Vec::new(),
    }
}

fn php_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "NULL",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_f64() => "double",
        Value::Number(_) => "integer",
        Value::String(_) => "string",
        Value::Array(_) | Value::Object(_) => "array",
    }
}

fn typed_error(key: &str, expected: &str, value: &Value) -> crate::Error {
    crate::error::InvalidArgumentException::new(format!(
        "Array value for key [{key}] must be {expected}, {} found.",
        php_type(value)
    ))
    .into()
}

fn sort_entries(array: &Value, mut callback: impl FnMut(&Value) -> Value, descending: bool) -> Value {
    let mut keyed: Vec<(String, Value, Value)> = entries(array)
        .into_iter()
        .map(|(k, v)| (k, callback(v), v.clone()))
        .collect();
    keyed.sort_by(|a, b| {
        let ordering = compare_for_sort(&a.1, &b.1);
        if descending { ordering.reverse() } else { ordering }
    });
    match array {
        Value::Object(_) => Value::Object(keyed.into_iter().map(|(k, _, v)| (k, v)).collect()),
        _ => Value::Array(keyed.into_iter().map(|(_, _, v)| v).collect()),
    }
}

fn sort_recursive(array: &Value, descending: bool) -> Value {
    match array {
        Value::Object(map) => {
            let mut sorted: Vec<(String, Value)> = map
                .iter()
                .map(|(k, v)| (k.clone(), sort_recursive(v, descending)))
                .collect();
            sorted.sort_by(|a, b| {
                let ordering = compare_for_sort(&Value::String(a.0.clone()), &Value::String(b.0.clone()));
                if descending { ordering.reverse() } else { ordering }
            });
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => {
            let mut sorted: Vec<Value> = items.iter().map(|v| sort_recursive(v, descending)).collect();
            sorted.sort_by(|a, b| {
                let ordering = compare_for_sort(a, b);
                if descending { ordering.reverse() } else { ordering }
            });
            Value::Array(sorted)
        }
        other => other.clone(),
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

/// Percent-encode a string like PHP's `rawurlencode`.
pub(crate) fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
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
    fn it_handles_laravel_doc_examples() {
        assert!(Arr::accessible(&json!({"a": 1, "b": 2})));
        assert!(!Arr::accessible(&json!("abc")));
        let mut array = json!({"name": "Desk", "price": null});
        Arr::add(&mut array, "price", 100);
        assert_eq!(array, json!({"name": "Desk", "price": 100}));
        assert_eq!(
            Arr::cross_join(&[json!([1, 2]), json!(["a", "b"]), json!(["I", "II"])]),
            json!([[1, "a", "I"], [1, "a", "II"], [1, "b", "I"], [1, "b", "II"], [2, "a", "I"], [2, "a", "II"], [2, "b", "I"], [2, "b", "II"]])
        );
        let (keys, values) = Arr::divide(&json!({"name": "Desk"}));
        assert_eq!(keys, vec!["name"]);
        assert_eq!(values, vec![json!("Desk")]);
        assert!(Arr::every(&json!([1, 2, 3]), |v, _| v.as_i64() > Some(0)));
        assert!(!Arr::every(&json!([1, 2, 3]), |v, _| v.as_i64() > Some(2)));
        assert!(Arr::some(&json!([1, 2, 3]), |v, _| v.as_i64() > Some(2)));
        assert!(Arr::exists(&json!({"name": "John Doe", "age": 17}), "name"));
        assert!(!Arr::exists(&json!({"name": "John Doe", "age": 17}), "salary"));
        assert_eq!(Arr::flatten(&json!({"name": "Joe", "languages": ["PHP", "Ruby"]}), None), vec![json!("Joe"), json!("PHP"), json!("Ruby")]);
        let mut array = json!({"products": {"desk": {"price": 100}}});
        Arr::forget(&mut array, "products.desk");
        assert_eq!(array, json!({"products": {}}));
        assert_eq!(Arr::get_or(&json!({"products": {"desk": {"price": 100}}}), "products.desk.discount", 0), json!(0));
        assert!(!Arr::has_all(&json!({"product": {"name": "Desk", "price": 100}}), &["product.price", "product.discount"]));
        assert!(Arr::has_any(&json!({"product": {"name": "Desk"}}), &["product.name", "product.discount"]));
        assert!(Arr::is_assoc(&json!({"product": {"name": "Desk"}})));
        assert!(Arr::is_list(&json!(["foo", "bar"])));
        assert_eq!(
            Arr::key_by(&json!([{"product_id": "prod-100", "name": "Desk"}, {"product_id": "prod-200", "name": "Chair"}]), "product_id"),
            json!({"prod-100": {"product_id": "prod-100", "name": "Desk"}, "prod-200": {"product_id": "prod-200", "name": "Chair"}})
        );
        assert_eq!(Arr::last_fn(&json!([100, 200, 300, 110]), |v, _| v.as_i64() >= Some(150)), json!(300));
        assert_eq!(Arr::map_spread(&json!([[0, 1], [2, 3], [4, 5]]), |items| json!(items[0].as_i64().unwrap() + items[1].as_i64().unwrap())), json!([1, 5, 9]));
        assert_eq!(Arr::only_values(&json!(["foo", "bar", "baz", "qux"]), &[json!("foo"), json!("baz")]), json!(["foo", "baz"]));
        assert_eq!(Arr::only_values_strict(&json!([1, "1", 2, "2"]), &[json!(1), json!(2)]), json!([1, 2]));
        let (under, over) = Arr::partition(&json!([1, 2, 3, 4, 5, 6]), |v, _| v.as_i64() < Some(3));
        assert_eq!((under, over), (json!([1, 2]), json!([3, 4, 5, 6])));
        assert_eq!(Arr::pluck(&json!([{"developer": {"name": "Taylor"}}, {"developer": {"name": "Abigail"}}]), "developer.name"), vec![json!("Taylor"), json!("Abigail")]);
        assert_eq!(Arr::prepend(&json!(["one", "two"]), "zero"), json!(["zero", "one", "two"]));
        assert_eq!(Arr::prepend_with_key(&json!({"price": 100}), "Desk", "name"), json!({"name": "Desk", "price": 100}));
        assert_eq!(Arr::prepend_keys_with(&json!({"name": "Desk", "price": 100}), "product."), json!({"product.name": "Desk", "product.price": 100}));
        let mut array = json!({"name": "Desk", "price": 100});
        assert_eq!(Arr::pull(&mut array, "name"), json!("Desk"));
        assert_eq!(array, json!({"price": 100}));
        assert_eq!(Arr::pull_or(&mut array, "missing", "default"), json!("default"));
        assert_eq!(Arr::reject(&json!([100, "200", 300, "400", 500]), |v, _| v.is_string()), json!([100, 300, 500]));
        assert_eq!(Arr::where_(&json!([100, "200", 300, "400", 500]), |_, v| v.is_string()), json!(["200", "400"]));
        assert_eq!(Arr::where_not_null(&json!([0, null])), json!([0]));
        assert_eq!(Arr::sole(&json!(["Desk", "Table", "Chair"]), |v, _| v == "Desk").unwrap(), json!("Desk"));
        assert!(Arr::sole(&json!(["Desk", "Desk"]), |v, _| v == "Desk").is_err());
        assert_eq!(Arr::sort_by(&json!([{"name": "Desk"}, {"name": "Table"}, {"name": "Chair"}]), |v| v["name"].clone()), json!([{"name": "Chair"}, {"name": "Desk"}, {"name": "Table"}]));
        assert_eq!(Arr::sort_desc(&json!(["Desk", "Table", "Chair"])), json!(["Table", "Desk", "Chair"]));
        assert_eq!(Arr::undot(&json!({"user.name": "Kevin Malone", "user.occupation": "Accountant"})), json!({"user": {"name": "Kevin Malone", "occupation": "Accountant"}}));
        assert_eq!(Arr::wrap("Laravel"), json!(["Laravel"]));
        assert_eq!(Arr::wrap(Value::Null), json!([]));
        assert_eq!(Arr::to_css_classes(&json!(["p-4", {"font-bold": false, "bg-red": true}])), "p-4 bg-red");
        assert_eq!(Arr::join(&json!(["a"]), ", ", " and "), "a");
        assert_eq!(Arr::join(&json!([]), ", ", " and "), "");
        assert_eq!(Arr::collapse(&json!([[1], {"a": 2}])), json!([1, 2]));
    }

    #[test]
    fn it_reads_typed_values() {
        let array = json!({"name": "Joe", "age": 42, "balance": 123.45, "available": true, "languages": ["PHP"]});
        assert_eq!(Arr::integer(&array, "age").unwrap(), 42);
        assert_eq!(Arr::float(&array, "balance").unwrap(), 123.45);
        assert_eq!(Arr::string(&array, "name").unwrap(), "Joe");
        assert!(Arr::boolean(&array, "available").unwrap());
        assert_eq!(
            Arr::integer(&array, "name").unwrap_err().to_string(),
            "Array value for key [name] must be an integer, string found."
        );
        assert!(Arr::float(&array, "age").is_err());
        assert!(Arr::string(&array, "languages").is_err());
        assert!(Arr::boolean(&array, "missing").is_err());
    }

    #[test]
    fn it_picks_random_values() {
        let array = json!([1, 2, 3, 4, 5]);
        assert!(Arr::random(&array).is_some());
        assert_eq!(Arr::random_many(&array, 2).unwrap().as_array().unwrap().len(), 2);
        assert_eq!(
            Arr::random_many(&array, 6).unwrap_err().to_string(),
            "You requested 6 items, but there are only 5 items available."
        );
        assert_eq!(Arr::shuffle(&array).as_array().unwrap().len(), 5);
        assert_eq!(Arr::random(&json!([])), None);
    }

    #[test]
    fn it_builds_query_strings() {
        assert_eq!(Arr::query(&json!({"name": "Taylor Otwell", "page": 2})), "name=Taylor%20Otwell&page=2");
        assert_eq!(Arr::query(&json!({"a": [1, 2], "b": null, "c": true, "d": false, "e": []})), "a%5B0%5D=1&a%5B1%5D=2&c=1&d=0");
    }
}
