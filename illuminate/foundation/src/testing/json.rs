//! JSON assertion helpers.

use illuminate_support::{Value, ValueExt, data_get};

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
