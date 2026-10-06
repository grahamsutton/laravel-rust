//! Forgiving deserialization of request data.
//!
//! Query strings, form submissions and route parameters are all strings, so
//! deserializing them into typed structs needs PHP's flexibility: `"42"`
//! should become `42`, `"on"` should become `true`, and an empty string
//! should be `None`. [`from_value`] does exactly that, at every level of
//! nesting (unlike a strict `serde_json::from_value`).
//!
//! ```
//! use illuminate_routing::de::from_value;
//! use illuminate_support::json;
//!
//! #[derive(serde::Deserialize, Debug, PartialEq)]
//! struct Filters {
//!     page: u32,
//!     active: bool,
//!     tags: Vec<String>,
//!     search: Option<String>,
//! }
//!
//! let filters: Filters = from_value(json!({
//!     "page": "2",
//!     "active": "on",
//!     "tags": "laravel",
//!     "search": "",
//! })).unwrap();
//!
//! assert_eq!(filters, Filters { page: 2, active: true, tags: vec!["laravel".into()], search: None });
//! ```

use std::fmt;

use serde::de::{
    self, DeserializeOwned, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};

use illuminate_support::{Map, Value};

/// A deserialization error, distinguishing malformed *data* from a target
/// type whose *shape* can't be produced from the source at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeError {
    message: String,
    shape: bool,
}

impl DeError {
    pub(crate) fn shape(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            shape: true,
        }
    }

    /// Determine if the error means the target type's shape doesn't fit the
    /// source (a programming error) rather than the data being invalid.
    pub fn is_shape_error(&self) -> bool {
        self.shape
    }

    /// The error message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for DeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DeError {}

impl de::Error for DeError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self {
            message: msg.to_string(),
            shape: false,
        }
    }
}

/// Deserialize a value leniently, coercing strings into numbers, booleans,
/// and lists as the target type requires.
pub fn from_value<T: DeserializeOwned>(value: Value) -> Result<T, DeError> {
    T::deserialize(Lenient(value))
}

/// Deserialize route parameters (in order) into `T`: a single value, a
/// tuple (positionally), or a struct / map (by name).
pub fn from_parameters<T: DeserializeOwned>(
    parameters: Vec<(String, String)>,
) -> Result<T, DeError> {
    T::deserialize(Parameters(parameters))
}

/// A lenient deserializer over a [`Value`].
pub struct Lenient(pub Value);

fn kind(value: &Value) -> de::Unexpected<'_> {
    match value {
        Value::Null => de::Unexpected::Unit,
        Value::Bool(b) => de::Unexpected::Bool(*b),
        Value::Number(n) => match n.as_f64() {
            Some(f) => de::Unexpected::Float(f),
            None => de::Unexpected::Other("number"),
        },
        Value::String(s) => de::Unexpected::Str(s),
        Value::Array(_) => de::Unexpected::Seq,
        Value::Object(_) => de::Unexpected::Map,
    }
}

fn parse_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|f| f != 0.0),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" => Some(true),
            "0" | "false" | "off" | "no" | "" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn parse_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn parse_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(n) => n.as_u64().or_else(|| {
            n.as_f64()
                .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                .map(|f| f as u64)
        }),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(u64::from(*b)),
        _ => None,
    }
}

fn parse_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

macro_rules! lenient_signed {
    ($($method:ident => $ty:ty, $visit:ident;)*) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
                match parse_i64(&self.0).and_then(|n| <$ty>::try_from(n).ok()) {
                    Some(n) => visitor.$visit(n),
                    None => Err(de::Error::invalid_type(kind(&self.0), &visitor)),
                }
            }
        )*
    };
}

macro_rules! lenient_unsigned {
    ($($method:ident => $ty:ty, $visit:ident;)*) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
                match parse_u64(&self.0).and_then(|n| <$ty>::try_from(n).ok()) {
                    Some(n) => visitor.$visit(n),
                    None => Err(de::Error::invalid_type(kind(&self.0), &visitor)),
                }
            }
        )*
    };
}

impl<'de> de::Deserializer<'de> for Lenient {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
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
            Value::Array(items) => visitor.visit_seq(LenientSeq(items.into_iter())),
            Value::Object(map) => visitor.visit_map(LenientMap::new(map)),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match parse_bool(&self.0) {
            Some(b) => visitor.visit_bool(b),
            None => Err(de::Error::invalid_type(kind(&self.0), &visitor)),
        }
    }

    lenient_signed! {
        deserialize_i8 => i8, visit_i8;
        deserialize_i16 => i16, visit_i16;
        deserialize_i32 => i32, visit_i32;
        deserialize_i64 => i64, visit_i64;
    }

    lenient_unsigned! {
        deserialize_u8 => u8, visit_u8;
        deserialize_u16 => u16, visit_u16;
        deserialize_u32 => u32, visit_u32;
        deserialize_u64 => u64, visit_u64;
    }

    fn deserialize_i128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match parse_i64(&self.0) {
            Some(n) => visitor.visit_i128(i128::from(n)),
            None => Err(de::Error::invalid_type(kind(&self.0), &visitor)),
        }
    }

    fn deserialize_u128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match parse_u64(&self.0) {
            Some(n) => visitor.visit_u128(u128::from(n)),
            None => Err(de::Error::invalid_type(kind(&self.0), &visitor)),
        }
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match parse_f64(&self.0) {
            Some(f) => visitor.visit_f32(f as f32),
            None => Err(de::Error::invalid_type(kind(&self.0), &visitor)),
        }
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match parse_f64(&self.0) {
            Some(f) => visitor.visit_f64(f),
            None => Err(de::Error::invalid_type(kind(&self.0), &visitor)),
        }
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match &self.0 {
            Value::String(s) if s.chars().count() == 1 => {
                visitor.visit_char(s.chars().next().unwrap_or_default())
            }
            other => Err(de::Error::invalid_type(kind(other), &visitor)),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_string(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match self.0 {
            Value::String(s) => visitor.visit_string(s),
            Value::Number(n) => visitor.visit_string(n.to_string()),
            Value::Bool(b) => visitor.visit_string(if b { "1".into() } else { String::new() }),
            other => Err(de::Error::invalid_type(kind(&other), &visitor)),
        }
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_byte_buf(visitor)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match self.0 {
            Value::String(s) => visitor.visit_byte_buf(s.into_bytes()),
            other => Lenient(other).deserialize_any(visitor),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match &self.0 {
            Value::Null => visitor.visit_none(),
            Value::String(s) if s.is_empty() => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match self.0 {
            Value::Array(items) => visitor.visit_seq(LenientSeq(items.into_iter())),
            Value::Object(map) => {
                let items: Vec<Value> = map.into_iter().map(|(_, v)| v).collect();
                visitor.visit_seq(LenientSeq(items.into_iter()))
            }
            Value::Null => visitor.visit_seq(LenientSeq(Vec::new().into_iter())),
            Value::String(s) if s.trim_start().starts_with('[') => {
                match serde_json::from_str::<Value>(&s) {
                    Ok(Value::Array(items)) => visitor.visit_seq(LenientSeq(items.into_iter())),
                    _ => visitor.visit_seq(LenientSeq(vec![Value::String(s)].into_iter())),
                }
            }
            scalar => visitor.visit_seq(LenientSeq(vec![scalar].into_iter())),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match self.0 {
            Value::Object(map) => visitor.visit_map(LenientMap::new(map)),
            Value::Array(items) => {
                let map: Map<String, Value> = items
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| (i.to_string(), v))
                    .collect();
                visitor.visit_map(LenientMap::new(map))
            }
            Value::String(s) if s.trim_start().starts_with('{') => {
                match serde_json::from_str::<Value>(&s) {
                    Ok(Value::Object(map)) => visitor.visit_map(LenientMap::new(map)),
                    _ => Err(de::Error::invalid_type(de::Unexpected::Str(&s), &visitor)),
                }
            }
            other => Err(de::Error::invalid_type(kind(&other), &visitor)),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        match self.0 {
            Value::String(s) => visitor.visit_enum(LenientEnum {
                variant: s,
                value: None,
            }),
            Value::Number(n) => visitor.visit_enum(LenientEnum {
                variant: n.to_string(),
                value: None,
            }),
            Value::Object(map) if map.len() == 1 => {
                let (variant, value) = map.into_iter().next().expect("one entry");
                visitor.visit_enum(LenientEnum {
                    variant,
                    value: Some(value),
                })
            }
            other => Err(de::Error::invalid_type(kind(&other), &visitor)),
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_string(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_unit()
    }
}

struct LenientSeq(std::vec::IntoIter<Value>);

impl<'de> SeqAccess<'de> for LenientSeq {
    type Error = DeError;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, DeError> {
        match self.0.next() {
            Some(value) => seed.deserialize(Lenient(value)).map(Some),
            None => Ok(None),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.0.len())
    }
}

struct LenientMap {
    entries: std::vec::IntoIter<(String, Value)>,
    value: Option<Value>,
}

impl LenientMap {
    fn new(map: Map<String, Value>) -> Self {
        Self {
            entries: map.into_iter().collect::<Vec<_>>().into_iter(),
            value: None,
        }
    }
}

impl<'de> MapAccess<'de> for LenientMap {
    type Error = DeError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, DeError> {
        match self.entries.next() {
            Some((key, value)) => {
                self.value = Some(value);
                seed.deserialize(Lenient(Value::String(key))).map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, DeError> {
        let value = self.value.take().unwrap_or(Value::Null);
        seed.deserialize(Lenient(value))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.entries.len())
    }
}

struct LenientEnum {
    variant: String,
    value: Option<Value>,
}

impl<'de> EnumAccess<'de> for LenientEnum {
    type Error = DeError;
    type Variant = LenientVariant;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, LenientVariant), DeError> {
        let variant = seed.deserialize(self.variant.into_deserializer())?;
        Ok((variant, LenientVariant(self.value)))
    }
}

struct LenientVariant(Option<Value>);

impl<'de> VariantAccess<'de> for LenientVariant {
    type Error = DeError;

    fn unit_variant(self) -> Result<(), DeError> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, DeError> {
        seed.deserialize(Lenient(self.0.unwrap_or(Value::Null)))
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, DeError> {
        de::Deserializer::deserialize_seq(Lenient(self.0.unwrap_or(Value::Null)), visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        de::Deserializer::deserialize_map(Lenient(self.0.unwrap_or(Value::Null)), visitor)
    }
}

/// Deserializes the ordered route parameters into the requested shape.
struct Parameters(Vec<(String, String)>);

impl Parameters {
    fn single(self) -> Result<Lenient, DeError> {
        match self.0.len() {
            1 => Ok(Lenient(Value::String(
                self.0.into_iter().next().expect("one").1,
            ))),
            0 => Err(DeError::shape("The route has no parameters to extract.")),
            count => Err(DeError::shape(format!(
                "Expected 1 route parameter but the route has {count}. Extract a tuple or a struct instead."
            ))),
        }
    }

    fn object(self) -> Lenient {
        Lenient(Value::Object(
            self.0
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect(),
        ))
    }

    fn array(self) -> Lenient {
        Lenient(Value::Array(
            self.0.into_iter().map(|(_, v)| Value::String(v)).collect(),
        ))
    }
}

macro_rules! single_parameter {
    ($($method:ident),*) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
                self.single()?.$method(visitor)
            }
        )*
    };
}

impl<'de> de::Deserializer<'de> for Parameters {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        if self.0.len() == 1 {
            self.single()?.deserialize_any(visitor)
        } else {
            self.object().deserialize_any(visitor)
        }
    }

    single_parameter!(
        deserialize_bool,
        deserialize_i8,
        deserialize_i16,
        deserialize_i32,
        deserialize_i64,
        deserialize_i128,
        deserialize_u8,
        deserialize_u16,
        deserialize_u32,
        deserialize_u64,
        deserialize_u128,
        deserialize_f32,
        deserialize_f64,
        deserialize_char,
        deserialize_str,
        deserialize_string,
        deserialize_bytes,
        deserialize_byte_buf,
        deserialize_identifier
    );

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        if self.0.is_empty() {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.array().deserialize_seq(visitor)
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        if len != self.0.len() {
            return Err(DeError::shape(format!(
                "Expected {len} route parameters but the route has {}.",
                self.0.len()
            )));
        }
        self.array().deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.object().deserialize_map(visitor)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.object().deserialize_map(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.single()?.deserialize_enum(name, variants, visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_unit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;
    use serde::Deserialize;
    use std::collections::HashMap;

    #[derive(Deserialize, Debug, PartialEq)]
    struct Params {
        user: u64,
        slug: String,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    #[serde(rename_all = "lowercase")]
    enum Category {
        Fruits,
        People,
    }

    fn params(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn strings_become_numbers_and_booleans() {
        assert_eq!(from_value::<u32>(json!("42")).unwrap(), 42);
        assert_eq!(from_value::<i64>(json!(" -7 ")).unwrap(), -7);
        assert_eq!(from_value::<f64>(json!("1.5")).unwrap(), 1.5);
        assert!(from_value::<bool>(json!("yes")).unwrap());
        assert!(!from_value::<bool>(json!("0")).unwrap());
        assert_eq!(from_value::<String>(json!(5)).unwrap(), "5");
        assert!(from_value::<u8>(json!("300")).is_err());
        assert!(from_value::<u32>(json!("taylor")).is_err());
    }

    #[test]
    fn nested_values_are_coerced() {
        #[derive(Deserialize, Debug, PartialEq)]
        struct Filters {
            ids: Vec<u32>,
            range: HashMap<String, f64>,
            page: Option<u32>,
            missing: Option<u32>,
        }
        let filters: Filters = from_value(json!({
            "ids": ["1", "2"],
            "range": {"min": "0.5"},
            "page": "",
        }))
        .unwrap();
        assert_eq!(filters.ids, vec![1, 2]);
        assert_eq!(filters.range["min"], 0.5);
        assert_eq!(filters.page, None);
        assert_eq!(filters.missing, None);
    }

    #[test]
    fn enums_deserialize_from_strings() {
        assert_eq!(
            from_value::<Category>(json!("fruits")).unwrap(),
            Category::Fruits
        );
        assert!(from_value::<Category>(json!("cars")).is_err());
    }

    #[test]
    fn single_parameters_extract_values() {
        assert_eq!(from_parameters::<u64>(params(&[("id", "5")])).unwrap(), 5);
        assert_eq!(
            from_parameters::<String>(params(&[("name", "taylor")])).unwrap(),
            "taylor"
        );
        assert_eq!(
            from_parameters::<Category>(params(&[("category", "people")])).unwrap(),
            Category::People
        );
        let error = from_parameters::<u64>(params(&[("id", "abc")])).unwrap_err();
        assert!(!error.is_shape_error());
    }

    #[test]
    fn several_parameters_need_a_tuple_or_struct() {
        let list = params(&[("user", "1"), ("slug", "hello")]);
        let error = from_parameters::<u64>(list.clone()).unwrap_err();
        assert!(error.is_shape_error());
        assert_eq!(
            from_parameters::<(u64, String)>(list.clone()).unwrap(),
            (1, "hello".to_string())
        );
        assert_eq!(
            from_parameters::<Params>(list.clone()).unwrap(),
            Params {
                user: 1,
                slug: "hello".into()
            }
        );
        let map = from_parameters::<HashMap<String, String>>(list.clone()).unwrap();
        assert_eq!(map["slug"], "hello");
        assert!(
            from_parameters::<(u64, String, u8)>(list)
                .unwrap_err()
                .is_shape_error()
        );
    }

    #[test]
    fn optional_parameters_extract_options() {
        assert_eq!(from_parameters::<Option<String>>(Vec::new()).unwrap(), None);
        assert_eq!(
            from_parameters::<Option<String>>(params(&[("name", "x")])).unwrap(),
            Some("x".to_string())
        );
    }
}
