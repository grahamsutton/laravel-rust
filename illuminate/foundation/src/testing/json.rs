//! JSON assertion helpers.

use illuminate_support::{Arr, Value, ValueExt, data_get};

/// Determine if `expected` is a subset of `actual` (objects by key, lists by index).
pub fn is_subset(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => expected.iter().all(|(key, value)| {
            actual
                .get(key)
                .is_some_and(|actual_value| is_subset(value, actual_value))
        }),
        (Value::Array(expected), Value::Array(actual)) => {
            expected.len() <= actual.len()
                && expected
                    .iter()
                    .zip(actual.iter())
                    .all(|(e, a)| is_subset(e, a))
        }
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (expected, actual) => expected == actual,
    }
}

/// Determine if `fragment` appears anywhere inside `haystack`.
pub fn contains_fragment(fragment: &Value, haystack: &Value) -> bool {
    match haystack {
        Value::Object(map) => {
            if let Value::Object(expected) = fragment
                && expected.iter().all(|(k, v)| map.get(k).is_some_and(|a| is_subset(v, a))) {
                    return true;
                }
            map.values().any(|value| contains_fragment(fragment, value))
        }
        Value::Array(items) => items.iter().any(|value| contains_fragment(fragment, value)),
        _ => false,
    }
}

/// Check the JSON matches a structure like `{"data": {"*": ["id", "name"]}}`.
pub fn matches_structure(structure: &Value, actual: &Value) -> Result<(), String> {
    match structure {
        Value::Array(keys) => {
            for key in keys {
                match key {
                    Value::String(key) => {
                        if key == "*" {
                            continue;
                        }
                        if actual.get(key.as_str()).is_none() {
                            return Err(format!("Missing key [{key}]."));
                        }
                    }
                    Value::Object(_) => matches_structure(key, actual)?,
                    _ => {}
                }
            }
            Ok(())
        }
        Value::Object(map) => {
            for (key, nested) in map {
                if key == "*" {
                    let items: Vec<&Value> = match actual {
                        Value::Array(items) => items.iter().collect(),
                        Value::Object(map) => map.values().collect(),
                        _ => return Err("Expected an array for [*].".into()),
                    };
                    for item in items {
                        matches_structure(nested, item)?;
                    }
                } else {
                    let Some(value) = actual.get(key.as_str()) else {
                        return Err(format!("Missing key [{key}]."));
                    };
                    matches_structure(nested, value)?;
                }
            }
            Ok(())
        }
        Value::Null => Ok(()),
        Value::String(key) => {
            if actual.get(key.as_str()).is_some() {
                Ok(())
            } else {
                Err(format!("Missing key [{key}]."))
            }
        }
        _ => Ok(()),
    }
}

/// Get a value from JSON using Laravel's dot (and wildcard) path syntax.
pub fn path(json: &Value, path: &str) -> Value {
    data_get(json, path)
}

/// Count the items at a path (or the root).
pub fn count(json: &Value, key: Option<&str>) -> usize {
    let target = match key {
        Some(key) => path(json, key),
        None => json.clone(),
    };
    target.count()
}

/// Check the JSON matches a structure exactly: every level has precisely
/// the given keys (Laravel's `assertExactJsonStructure`).
pub fn matches_exact_structure(structure: &Value, actual: &Value) -> Result<(), String> {
    if !actual.is_array() && !actual.is_object() {
        return Err(format!("Expected an array or object, found [{actual}]."));
    }

    let mut expected_keys: Vec<String> = match structure {
        Value::Array(keys) => keys
            .iter()
            .flat_map(|key| match key {
                Value::String(key) => vec![key.clone()],
                Value::Object(map) => map.keys().cloned().collect(),
                _ => Vec::new(),
            })
            .collect(),
        Value::Object(map) => map.keys().cloned().collect(),
        Value::String(key) => vec![key.clone()],
        _ => Vec::new(),
    };
    if expected_keys != ["*"] {
        let mut actual_keys: Vec<String> = match actual {
            Value::Object(map) => map.keys().cloned().collect(),
            Value::Array(items) => (0..items.len()).map(|i| i.to_string()).collect(),
            _ => Vec::new(),
        };
        expected_keys.sort();
        actual_keys.sort();
        if expected_keys != actual_keys {
            return Err(format!(
                "Expected exactly the keys [{}] but found [{}].",
                expected_keys.join(", "),
                actual_keys.join(", ")
            ));
        }
    }

    let nested = |key: &str, nested: &Value| -> Result<(), String> {
        if key == "*" {
            let items: Vec<&Value> = match actual {
                Value::Array(items) => items.iter().collect(),
                Value::Object(map) => map.values().collect(),
                _ => return Err("Expected an array for [*].".into()),
            };
            for item in items {
                matches_exact_structure(nested, item)?;
            }
            Ok(())
        } else {
            match actual.get(key) {
                Some(value) => matches_exact_structure(nested, value),
                None => Err(format!("Missing key [{key}].")),
            }
        }
    };

    match structure {
        Value::Array(keys) => {
            for key in keys {
                if let Value::Object(map) = key {
                    for (key, value) in map {
                        nested(key, value)?;
                    }
                }
            }
            Ok(())
        }
        Value::Object(map) => {
            for (key, value) in map {
                if value.is_array() || value.is_object() {
                    nested(key, value)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The strings Laravel searches the encoded JSON for to find a `key: value`
/// pair: `"key":value` followed by `]`, `}` or `,`.
fn search_strings(key: Option<&str>, value: &Value) -> [String; 3] {
    let value = encode(value);
    let needle = match key {
        Some(key) => format!("{}:{value}", encode(&Value::String(key.to_string()))),
        None => value,
    };
    [format!("{needle}]"), format!("{needle}}}"), format!("{needle},")]
}

/// The encoded JSON, recursively sorted, as Laravel's fragment assertions
/// search it.
pub fn sorted_encoding(json: &Value) -> String {
    encode(&Arr::sort_recursive(json))
}

/// Encode a value as compact JSON.
pub fn encode(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

/// The `key: value` pairs of the (sorted) data that appear anywhere in the
/// JSON, each paired with whether it was found.
pub fn find_pairs(data: &Value, actual: &Value) -> Vec<(Value, bool)> {
    let haystack = sorted_encoding(actual);
    let data = Arr::sort_recursive(data);
    let found = |key: Option<&str>, value: &Value| {
        search_strings(key, value)
            .iter()
            .any(|needle| haystack.contains(needle.as_str()))
    };
    match &data {
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| {
                let mut pair = illuminate_support::Map::new();
                pair.insert(key.clone(), value.clone());
                (Value::Object(pair), found(Some(key), value))
            })
            .collect(),
        Value::Array(items) => items
            .iter()
            .map(|value| (Value::Array(vec![value.clone()]), found(None, value)))
            .collect(),
        other => vec![(other.clone(), found(None, other))],
    }
}

/// Determine if the path (which may contain `*` wildcards) exists in the JSON.
pub fn has_path(json: &Value, path: &str) -> bool {
    if !path.contains('*') {
        return json.dot(path).is_some();
    }
    let segments: Vec<&str> = path.split('.').collect();
    fn walk(value: &Value, segments: &[&str]) -> bool {
        let Some((segment, rest)) = segments.split_first() else {
            return true;
        };
        let children: Vec<&Value> = match (value, *segment) {
            (Value::Object(map), "*") => map.values().collect(),
            (Value::Array(items), "*") => items.iter().collect(),
            (Value::Object(map), key) => map.get(key).into_iter().collect(),
            (Value::Array(items), key) => key
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get(index))
                .into_iter()
                .collect(),
            _ => Vec::new(),
        };
        children.into_iter().any(|child| walk(child, rest))
    }
    walk(json, &segments)
}

/// PHPUnit's `assertEquals`: numbers compare by value, objects regardless
/// of key order, and numeric strings equal their numbers.
pub fn loosely_equal(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Number(n), Value::String(s)) | (Value::String(s), Value::Number(n)) => {
            s.trim().parse::<f64>().ok() == n.as_f64()
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| loosely_equal(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter().all(|(key, value)| b.get(key).is_some_and(|other| loosely_equal(value, other)))
        }
        (a, b) => a == b,
    }
}

/// PHPUnit's `assertEqualsCanonicalizing`: compare after sorting.
pub fn canonically_equal(expected: &Value, actual: &Value) -> bool {
    loosely_equal(&Arr::sort_recursive(expected), &Arr::sort_recursive(actual))
}
