//! Laravel is built on PHP's wonderfully flexible arrays. In Rust, the
//! dynamic `Value` (a JSON value with insertion-ordered objects) plays that
//! role: request input, configuration, view data, and database rows are all
//! expressed as values.

use serde::Serialize;
use serde::de::DeserializeOwned;

pub use serde_json::{Map, Number, Value, json};

/// Convert any serializable type into a [`Value`].
///
/// Serialization failures (which are exceedingly rare for well-formed
/// types) collapse into `Value::Null`.
pub fn to_value<T: Serialize + ?Sized>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Leniently cast a [`Value`] into a concrete type.
///
/// Databases and HTTP input rarely hand you exactly the type you want: SQLite
/// stores booleans as integers, query strings are always strings, and so on.
/// `cast` first tries a strict conversion and then falls back to the same
/// loose coercions PHP would perform.
///
/// ```
/// use illuminate_support::{cast, json};
///
/// assert_eq!(cast::<bool>(json!(1)).unwrap(), true);
/// assert_eq!(cast::<i64>(json!("42")).unwrap(), 42);
/// assert_eq!(cast::<String>(json!(42)).unwrap(), "42");
/// assert_eq!(cast::<Option<i64>>(json!(null)).unwrap(), None);
/// ```
pub fn cast<T: DeserializeOwned>(value: Value) -> crate::Result<T> {
    match serde_json::from_value::<T>(value.clone()) {
        Ok(v) => Ok(v),
        Err(original) => {
            for candidate in coercions(&value) {
                if let Ok(v) = serde_json::from_value::<T>(candidate) {
                    return Ok(v);
                }
            }
            Err(original.into())
        }
    }
}

/// The loose alternatives a value may be coerced into, in order of preference.
fn coercions(value: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    match value {
        Value::Null => {
            out.push(Value::Bool(false));
            out.push(Value::String(String::new()));
            out.push(json!(0));
            out.push(json!([]));
            out.push(json!({}));
        }
        Value::Bool(b) => {
            out.push(json!(if *b { 1 } else { 0 }));
            out.push(Value::String(if *b { "1".into() } else { String::new() }));
        }
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push(Value::Bool(i != 0));
            } else if let Some(f) = n.as_f64() {
                out.push(Value::Bool(f != 0.0));
                // Allow 3.0 to become an integer.
                if f.fract() == 0.0 && f.abs() < i64::MAX as f64 {
                    out.push(json!(f as i64));
                }
            }
            out.push(Value::String(n.to_string()));
        }
        Value::String(s) => {
            let trimmed = s.trim();
            if let Ok(i) = trimmed.parse::<i64>() {
                out.push(json!(i));
            } else if let Ok(u) = trimmed.parse::<u64>() {
                out.push(json!(u));
            } else if let Ok(f) = trimmed.parse::<f64>() {
                if f.is_finite() {
                    out.push(json!(f));
                }
            }
            match trimmed.to_ascii_lowercase().as_str() {
                "1" | "true" | "on" | "yes" => out.push(Value::Bool(true)),
                "0" | "false" | "off" | "no" | "" => out.push(Value::Bool(false)),
                _ => {}
            }
            if s.is_empty() {
                out.push(Value::Null);
            }
            if (trimmed.starts_with('{') && trimmed.ends_with('}'))
                || (trimmed.starts_with('[') && trimmed.ends_with(']'))
            {
                if let Ok(decoded) = serde_json::from_str::<Value>(trimmed) {
                    out.push(decoded);
                }
            }
        }
        Value::Array(items) => {
            out.push(Value::String(value.to_string()));
            if items.is_empty() {
                out.push(json!({}));
            }
        }
        Value::Object(map) => {
            out.push(Value::String(value.to_string()));
            // A PHP "list" that was serialized as an object with numeric keys.
            if map.keys().enumerate().all(|(i, k)| k == &i.to_string()) {
                out.push(Value::Array(map.values().cloned().collect()));
            }
        }
    }
    out
}

/// PHP-flavoured conveniences for working with dynamic values.
pub trait ValueExt {
    /// Determine if the value is "blank": null, an empty or whitespace-only
    /// string, or an empty array / object. Mirrors Laravel's `blank()`.
    fn is_blank(&self) -> bool;

    /// The inverse of [`ValueExt::is_blank`]. Mirrors Laravel's `filled()`.
    fn is_filled(&self) -> bool {
        !self.is_blank()
    }

    /// PHP truthiness: `null`, `false`, `0`, `0.0`, `""`, `"0"`, and empty
    /// arrays are falsy; everything else is truthy.
    fn truthy(&self) -> bool;

    /// Convert the value to a string the way PHP would when casting
    /// (`null` becomes `""`, `true` becomes `"1"`, arrays become JSON).
    fn to_string_lossy(&self) -> String;

    /// Loosely interpret the value as an integer.
    fn to_i64_lossy(&self) -> Option<i64>;

    /// Loosely interpret the value as a float.
    fn to_f64_lossy(&self) -> Option<f64>;

    /// Retrieve a nested value using "dot" notation (`user.address.city`).
    fn dot(&self, key: &str) -> Option<&Value>;

    /// Retrieve a nested value using "dot" notation, cloning the result and
    /// falling back to `Value::Null`.
    fn dot_or_null(&self, key: &str) -> Value {
        self.dot(key).cloned().unwrap_or(Value::Null)
    }

    /// The "count" of a value: array length, object size, or string length.
    fn count(&self) -> usize;
}

impl ValueExt for Value {
    fn is_blank(&self) -> bool {
        match self {
            Value::Null => true,
            Value::Bool(_) | Value::Number(_) => false,
            Value::String(s) => s.trim().is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
            Value::String(s) => !(s.is_empty() || s == "0"),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
        }
    }

    fn to_string_lossy(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Bool(true) => "1".to_string(),
            Value::Bool(false) => String::new(),
            Value::Number(n) => {
                if let Some(f) = n.as_f64().filter(|_| n.is_f64()) {
                    format_float(f)
                } else {
                    n.to_string()
                }
            }
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    fn to_i64_lossy(&self) -> Option<i64> {
        match self {
            Value::Null => None,
            Value::Bool(b) => Some(*b as i64),
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
            Value::String(s) => {
                let s = s.trim();
                s.parse::<i64>()
                    .ok()
                    .or_else(|| s.parse::<f64>().ok().map(|f| f as i64))
            }
            _ => None,
        }
    }

    fn to_f64_lossy(&self) -> Option<f64> {
        match self {
            Value::Null => None,
            Value::Bool(b) => Some(*b as i64 as f64),
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        }
    }

    fn dot(&self, key: &str) -> Option<&Value> {
        if key.is_empty() {
            return Some(self);
        }
        if let Value::Object(map) = self {
            if let Some(v) = map.get(key) {
                return Some(v);
            }
        }
        let mut current = self;
        for segment in key.split('.') {
            current = match current {
                Value::Object(map) => map.get(segment)?,
                Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(current)
    }

    fn count(&self) -> usize {
        match self {
            Value::Null => 0,
            Value::Array(a) => a.len(),
            Value::Object(o) => o.len(),
            Value::String(s) => s.chars().count(),
            _ => 1,
        }
    }
}

/// Format a float the way PHP prints it: no trailing `.0` for whole numbers.
pub fn format_float(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        let s = format!("{}", f);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn casting_is_forgiving() {
        assert!(cast::<bool>(json!("true")).unwrap());
        assert!(!cast::<bool>(json!(0)).unwrap());
        assert_eq!(cast::<f64>(json!("1.5")).unwrap(), 1.5);
        assert_eq!(cast::<Vec<i64>>(json!("[1,2]")).unwrap(), vec![1, 2]);
        assert_eq!(cast::<i64>(json!(3.0)).unwrap(), 3);
        assert!(cast::<i64>(json!("taylor")).is_err());
    }

    #[test]
    fn values_have_php_truthiness() {
        assert!(!json!("0").truthy());
        assert!(!json!([]).truthy());
        assert!(json!("taylor").truthy());
        assert!(json!("   ").is_blank());
        assert!(json!(0).is_filled());
    }

    #[test]
    fn dot_notation_digs_into_values() {
        let value = json!({"user": {"name": "Taylor", "roles": ["admin"]}});
        assert_eq!(value.dot("user.name"), Some(&json!("Taylor")));
        assert_eq!(value.dot("user.roles.0"), Some(&json!("admin")));
        assert_eq!(value.dot("user.email"), None);
    }

    #[test]
    fn values_convert_to_php_strings() {
        assert_eq!(json!(null).to_string_lossy(), "");
        assert_eq!(json!(true).to_string_lossy(), "1");
        assert_eq!(json!(2.0).to_string_lossy(), "2");
        assert_eq!(json!(2.5).to_string_lossy(), "2.5");
        assert_eq!(json!("hi").to_string_lossy(), "hi");
    }
}
