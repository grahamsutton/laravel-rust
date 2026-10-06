//! Lenient deserialization of database rows into Rust types.
//!
//! Databases rarely return exactly the type a struct field asks for: SQLite
//! stores booleans as integers, `decimal` columns come back as strings and
//! JSON columns as text. The lenient deserializer coerces each field the way
//! PHP would, so rows deserialize into ordinary structs.

use serde::de::value::{MapDeserializer, SeqDeserializer, StrDeserializer};
use serde::de::{self, DeserializeOwned, IntoDeserializer, Visitor};
use serde::forward_to_deserialize_any;

use illuminate_support::{Result, Value, ValueExt};

/// Deserialize a row (or any value) into `T`, leniently.
///
/// ```
/// use illuminate_database::from_row;
/// use illuminate_support::json;
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct User {
///     id: i64,
///     active: bool,
///     balance: f64,
///     name: String,
///     settings: Option<serde_json::Value>,
/// }
///
/// let user: User = from_row(json!({
///     "id": "7", "active": 1, "balance": "10.50", "name": "Taylor", "settings": null
/// })).unwrap();
///
/// assert_eq!(user.id, 7);
/// assert!(user.active);
/// assert_eq!(user.balance, 10.5);
/// ```
pub fn from_row<T: DeserializeOwned>(row: Value) -> Result<T> {
    from_value(row)
}

/// Deserialize a value into `T`, leniently.
pub fn from_value<T: DeserializeOwned>(value: Value) -> Result<T> {
    Ok(T::deserialize(Lenient(value))?)
}

type Error = serde_json::Error;

/// A deserializer that coerces values the way PHP would.
pub struct Lenient(pub Value);

impl<'de> IntoDeserializer<'de, Error> for Lenient {
    type Deserializer = Lenient;

    fn into_deserializer(self) -> Lenient {
        self
    }
}

fn invalid(value: &Value, expected: &str) -> Error {
    de::Error::custom(format!("invalid type: {value}, expected {expected}"))
}

fn as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|f| f != 0.0),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "t" | "on" | "yes" | "y" => Some(true),
            "0" | "false" | "f" | "off" | "no" | "n" | "" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)),
        Value::Bool(b) => Some(*b as i64),
        Value::String(s) => {
            let s = s.trim();
            s.parse::<i64>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().filter(|f| f.fract() == 0.0).map(|f| f as i64))
        }
        _ => None,
    }
}

fn as_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

/// Parse a string holding a JSON array / object.
fn parse_json(value: &Value) -> Option<Value> {
    match value {
        Value::String(s) => {
            let trimmed = s.trim();
            if (trimmed.starts_with('{') && trimmed.ends_with('}'))
                || (trimmed.starts_with('[') && trimmed.ends_with(']'))
            {
                serde_json::from_str(trimmed).ok()
            } else {
                None
            }
        }
        _ => None,
    }
}

impl Lenient {
    fn visit_seq_of<'de, V: Visitor<'de>>(items: Vec<Value>, visitor: V) -> Result<V::Value, Error> {
        let mut seq = SeqDeserializer::new(items.into_iter().map(Lenient));
        let value = visitor.visit_seq(&mut seq)?;
        seq.end()?;
        Ok(value)
    }

    fn visit_map_of<'de, V: Visitor<'de>>(
        map: illuminate_support::Map<String, Value>,
        visitor: V,
    ) -> Result<V::Value, Error> {
        let mut access = MapDeserializer::new(map.into_iter().map(|(k, v)| (k, Lenient(v))));
        let value = visitor.visit_map(&mut access)?;
        access.end()?;
        Ok(value)
    }
}

macro_rules! deserialize_signed {
    ($($method:ident),*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            match as_i64(&self.0) {
                Some(i) => visitor.visit_i64(i),
                None => match as_u64(&self.0) {
                    Some(u) => visitor.visit_u64(u),
                    None => Err(invalid(&self.0, "an integer")),
                },
            }
        }
    )*};
}

macro_rules! deserialize_unsigned {
    ($($method:ident),*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            match as_u64(&self.0) {
                Some(u) => visitor.visit_u64(u),
                None => match as_i64(&self.0) {
                    Some(i) => visitor.visit_i64(i),
                    None => Err(invalid(&self.0, "an unsigned integer")),
                },
            }
        }
    )*};
}

impl<'de> de::Deserializer<'de> for Lenient {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Value::Null => visitor.visit_unit(),
            Value::Bool(b) => visitor.visit_bool(b),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    visitor.visit_i64(i)
                } else if let Some(u) = n.as_u64() {
                    visitor.visit_u64(u)
                } else {
                    visitor.visit_f64(n.as_f64().unwrap_or_default())
                }
            }
            Value::String(s) => visitor.visit_string(s),
            Value::Array(items) => Self::visit_seq_of(items, visitor),
            Value::Object(map) => Self::visit_map_of(map, visitor),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match as_bool(&self.0) {
            Some(b) => visitor.visit_bool(b),
            None => Err(invalid(&self.0, "a boolean")),
        }
    }

    deserialize_signed!(deserialize_i8, deserialize_i16, deserialize_i32, deserialize_i64);
    deserialize_unsigned!(deserialize_u8, deserialize_u16, deserialize_u32, deserialize_u64);

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_f64(visitor)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0.to_f64_lossy() {
            Some(f) if !self.0.is_null() => visitor.visit_f64(f),
            _ => Err(invalid(&self.0, "a number")),
        }
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0.to_string_lossy().chars().next() {
            Some(c) => visitor.visit_char(c),
            None => Err(invalid(&self.0, "a character")),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_string(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Value::String(s) => visitor.visit_string(s),
            Value::Bool(b) => visitor.visit_string(if b { "1".into() } else { "0".into() }),
            Value::Number(n) => visitor.visit_string(Value::Number(n).to_string_lossy()),
            Value::Null => Err(invalid(&Value::Null, "a string")),
            other => visitor.visit_string(other.to_string()),
        }
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_byte_buf(visitor)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Value::String(s) => visitor.visit_byte_buf(s.into_bytes()),
            Value::Array(items) => {
                let bytes: Option<Vec<u8>> = items
                    .iter()
                    .map(|i| i.as_u64().and_then(|b| u8::try_from(b).ok()))
                    .collect();
                match bytes {
                    Some(bytes) => visitor.visit_byte_buf(bytes),
                    None => Err(invalid(&Value::Array(items), "bytes")),
                }
            }
            other => Err(invalid(&other, "bytes")),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Value::Null => visitor.visit_none(),
            other => visitor.visit_some(Lenient(other)),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Value::Array(items) => Self::visit_seq_of(items, visitor),
            Value::Null => Self::visit_seq_of(Vec::new(), visitor),
            ref other => match parse_json(other) {
                Some(Value::Array(items)) => Self::visit_seq_of(items, visitor),
                _ => Err(invalid(other, "a sequence")),
            },
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Value::Object(map) => Self::visit_map_of(map, visitor),
            ref other => match parse_json(other) {
                Some(Value::Object(map)) => Self::visit_map_of(map, visitor),
                _ => Err(invalid(other, "a map")),
            },
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self.0 {
            Value::Object(map) => Self::visit_map_of(map, visitor),
            Value::Array(items) => Self::visit_seq_of(items, visitor),
            ref other => match parse_json(other) {
                Some(Value::Object(map)) => Self::visit_map_of(map, visitor),
                Some(Value::Array(items)) => Self::visit_seq_of(items, visitor),
                _ => Err(invalid(other, "a struct")),
            },
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self.0 {
            Value::String(s) => {
                let deserializer: StrDeserializer<'_, Error> = s.as_str().into_deserializer();
                visitor.visit_enum(deserializer)
            }
            Value::Number(n) => {
                let s = n.to_string();
                let deserializer: StrDeserializer<'_, Error> = s.as_str().into_deserializer();
                visitor.visit_enum(deserializer)
            }
            other => de::Deserializer::deserialize_enum(other, name, variants, visitor),
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_string(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    forward_to_deserialize_any! { i128 u128 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;
    use serde::Deserialize;
    use std::collections::HashMap;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Settings {
        theme: String,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    enum Status {
        #[serde(rename = "active")]
        Active,
        #[serde(rename = "banned")]
        Banned,
    }

    #[derive(Debug, Deserialize)]
    struct Row {
        id: u32,
        admin: bool,
        score: f32,
        name: String,
        tags: Vec<String>,
        settings: Settings,
        status: Status,
        nickname: Option<String>,
        code: String,
        extra: HashMap<String, i64>,
    }

    #[test]
    fn rows_deserialize_leniently() {
        let row: Row = from_value(json!({
            "id": "3",
            "admin": "1",
            "score": "4.5",
            "name": "Taylor",
            "tags": "[\"a\",\"b\"]",
            "settings": "{\"theme\":\"dark\"}",
            "status": "active",
            "nickname": null,
            "code": 42,
            "extra": {"a": "1"},
            "unknown": true,
        }))
        .unwrap();
        assert_eq!(row.id, 3);
        assert!(row.admin);
        assert_eq!(row.score, 4.5);
        assert_eq!(row.tags, vec!["a", "b"]);
        assert_eq!(row.settings, Settings { theme: "dark".into() });
        assert_eq!(row.status, Status::Active);
        assert_eq!(row.nickname, None);
        assert_eq!(row.code, "42");
        assert_eq!(row.extra["a"], 1);
        let _ = Status::Banned;
    }

    #[test]
    fn invalid_values_report_errors() {
        assert!(from_value::<i64>(json!("taylor")).is_err());
        assert!(from_value::<bool>(json!("maybe")).is_err());
        assert_eq!(from_value::<i64>(json!(3.0)).unwrap(), 3);
        assert_eq!(from_value::<Option<i64>>(json!(null)).unwrap(), None);
        assert_eq!(from_value::<serde_json::Value>(json!({"a": 1})).unwrap(), json!({"a": 1}));
    }
}
