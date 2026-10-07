//! The schema types: one builder per JSON type, and [`Type`] to hold any
//! of them.

use std::fmt;

use illuminate_support::Value;
use illuminate_support::error::InvalidArgumentException;
use serde::{Serialize, Serializer};
use serde_json::Map;

/// The keywords every type shares.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Common {
    pub(crate) required: bool,
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) default: Option<Value>,
    pub(crate) enum_: Option<Vec<Value>>,
    pub(crate) nullable: bool,
}

/// The methods every schema type has: `required`, `nullable`, `title`,
/// `description`, and `enum_`.
macro_rules! common_methods {
    ($($type:ident),* $(,)?) => {
        $(
            impl $type {
                /// Mark the property as required by its parent object.
                pub fn required(mut self) -> Self {
                    self.common.required = true;
                    self
                }

                /// Mark the property as optional (the default).
                pub fn optional(mut self) -> Self {
                    self.common.required = false;
                    self
                }

                /// Allow `null` as well.
                pub fn nullable(mut self) -> Self {
                    self.common.nullable = true;
                    self
                }

                /// Set the schema's title.
                pub fn title(mut self, title: impl Into<String>) -> Self {
                    self.common.title = Some(title.into());
                    self
                }

                /// Set the schema's description.
                pub fn description(mut self, description: impl Into<String>) -> Self {
                    self.common.description = Some(description.into());
                    self
                }

                /// Restrict the value to the given values, in order.
                pub fn enum_<V: Into<Value>>(mut self, values: impl IntoIterator<Item = V>) -> Self {
                    self.common.enum_ = Some(values.into_iter().map(Into::into).collect());
                    self
                }

                /// Whether the property is required by its parent object.
                pub fn is_required(&self) -> bool {
                    self.common.required
                }

                /// Whether `null` is allowed.
                pub fn is_nullable(&self) -> bool {
                    self.common.nullable
                }

                /// The schema as a JSON Schema document.
                pub fn to_array(&self) -> Value {
                    Type::from(self.clone()).to_array()
                }
            }

            impl From<$type> for Type {
                fn from(value: $type) -> Self {
                    Type::$type(value)
                }
            }

            impl Serialize for $type {
                fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                    self.to_array().serialize(serializer)
                }
            }

            impl fmt::Display for $type {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str(&crate::serializer::pretty(&self.to_array()))
                }
            }
        )*
    };
}

/// A `string`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StringType {
    pub(crate) common: Common,
    pub(crate) min_length: Option<u64>,
    pub(crate) max_length: Option<u64>,
    pub(crate) pattern: Option<String>,
    pub(crate) format: Option<String>,
}

impl StringType {
    /// The minimum length (`minLength`).
    pub fn min(mut self, length: u64) -> Self {
        self.min_length = Some(length);
        self
    }

    /// The maximum length (`maxLength`).
    pub fn max(mut self, length: u64) -> Self {
        self.max_length = Some(length);
        self
    }

    /// A regular expression the value must match.
    pub fn pattern(mut self, pattern: impl Into<String>) -> Self {
        self.pattern = Some(pattern.into());
        self
    }

    /// The value's format: `email`, `date-time`, `uri`, ...
    pub fn format(mut self, format: impl Into<String>) -> Self {
        self.format = Some(format.into());
        self
    }

    /// The default value.
    pub fn default(mut self, value: impl Into<String>) -> Self {
        self.common.default = Some(Value::String(value.into()));
        self
    }
}

/// An `integer`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IntegerType {
    pub(crate) common: Common,
    pub(crate) minimum: Option<i64>,
    pub(crate) maximum: Option<i64>,
    pub(crate) multiple_of: Option<i64>,
}

impl IntegerType {
    /// The minimum value (`minimum`).
    pub fn min(mut self, value: i64) -> Self {
        self.minimum = Some(value);
        self
    }

    /// The maximum value (`maximum`).
    pub fn max(mut self, value: i64) -> Self {
        self.maximum = Some(value);
        self
    }

    /// The value must be a multiple of this one.
    pub fn multiple_of(mut self, value: i64) -> Self {
        self.multiple_of = Some(value);
        self
    }

    /// The default value.
    pub fn default(mut self, value: i64) -> Self {
        self.common.default = Some(Value::from(value));
        self
    }
}

/// A `number`: an integer or a float.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NumberType {
    pub(crate) common: Common,
    pub(crate) minimum: Option<Value>,
    pub(crate) maximum: Option<Value>,
    pub(crate) multiple_of: Option<Value>,
}

impl NumberType {
    /// The minimum value (`minimum`).
    pub fn min(mut self, value: impl Into<Number>) -> Self {
        self.minimum = Some(value.into().0);
        self
    }

    /// The maximum value (`maximum`).
    pub fn max(mut self, value: impl Into<Number>) -> Self {
        self.maximum = Some(value.into().0);
        self
    }

    /// The value must be a multiple of this one.
    pub fn multiple_of(mut self, value: impl Into<Number>) -> Self {
        self.multiple_of = Some(value.into().0);
        self
    }

    /// The default value.
    pub fn default(mut self, value: impl Into<Number>) -> Self {
        self.common.default = Some(value.into().0);
        self
    }
}

/// An integer or a float, for [`NumberType`]'s constraints.
#[derive(Clone, Debug, PartialEq)]
pub struct Number(pub(crate) Value);

macro_rules! number_from {
    ($($type:ty),*) => {
        $(
            impl From<$type> for Number {
                fn from(value: $type) -> Self {
                    Number(Value::from(value))
                }
            }
        )*
    };
}

number_from!(i8, i16, i32, i64, u8, u16, u32, u64, f32, f64);

/// A `boolean`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BooleanType {
    pub(crate) common: Common,
}

impl BooleanType {
    /// The default value.
    pub fn default(mut self, value: bool) -> Self {
        self.common.default = Some(Value::Bool(value));
        self
    }
}

/// An `array`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArrayType {
    pub(crate) common: Common,
    pub(crate) min_items: Option<u64>,
    pub(crate) max_items: Option<u64>,
    pub(crate) items: Option<Box<Type>>,
    pub(crate) unique_items: Option<bool>,
}

impl ArrayType {
    /// The minimum number of items (`minItems`).
    pub fn min(mut self, count: u64) -> Self {
        self.min_items = Some(count);
        self
    }

    /// The maximum number of items (`maxItems`).
    pub fn max(mut self, count: u64) -> Self {
        self.max_items = Some(count);
        self
    }

    /// The schema every item must match.
    pub fn items(mut self, schema: impl Into<Type>) -> Self {
        self.items = Some(Box::new(schema.into()));
        self
    }

    /// The items must be unique (`uniqueItems`).
    pub fn unique(mut self) -> Self {
        self.unique_items = Some(true);
        self
    }

    /// The default value.
    pub fn default<V: Into<Value>>(mut self, value: impl IntoIterator<Item = V>) -> Self {
        self.common.default = Some(Value::Array(value.into_iter().map(Into::into).collect()));
        self
    }

    /// The schema every item must match.
    pub fn item_schema(&self) -> Option<&Type> {
        self.items.as_deref()
    }
}

/// An `object`, with its properties in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectType {
    pub(crate) common: Common,
    pub(crate) additional_properties: Option<bool>,
    pub(crate) properties: Vec<(String, Type)>,
}

impl ObjectType {
    /// Add a property. [Required](StringType::required) properties are
    /// listed in the object's `required` keyword.
    pub fn property(mut self, name: impl Into<String>, schema: impl Into<Type>) -> Self {
        let name = name.into();
        let schema = schema.into();
        match self.properties.iter_mut().find(|(existing, _)| *existing == name) {
            Some((_, existing)) => *existing = schema,
            None => self.properties.push((name, schema)),
        }
        self
    }

    /// Add several properties.
    pub fn properties<K: Into<String>>(mut self, properties: impl IntoIterator<Item = (K, Type)>) -> Self {
        for (name, schema) in properties {
            self = self.property(name, schema);
        }
        self
    }

    /// Disallow properties that aren't defined (`additionalProperties: false`).
    pub fn without_additional_properties(mut self) -> Self {
        self.additional_properties = Some(false);
        self
    }

    /// The default value.
    pub fn default(mut self, value: impl Into<Value>) -> Self {
        self.common.default = Some(value.into());
        self
    }

    /// The object's properties, in order.
    pub fn get_properties(&self) -> &[(String, Type)] {
        &self.properties
    }

    /// The property with the given name.
    pub fn get_property(&self, name: &str) -> Option<&Type> {
        self.properties
            .iter()
            .find(|(existing, _)| existing == name)
            .map(|(_, schema)| schema)
    }
}

/// A value of one of several primitive types: `"type": ["string", "integer"]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnionType {
    pub(crate) common: Common,
    pub(crate) types: Vec<String>,
}

impl UnionType {
    /// The types JSON Schema allows in a multi-type union.
    pub const SUPPORTED: [&'static str; 6] = ["string", "integer", "number", "boolean", "object", "array"];

    /// Create a union of the given type names. Including `"null"` makes
    /// the union [nullable](UnionType::nullable).
    pub fn try_new<S: AsRef<str>>(types: impl IntoIterator<Item = S>) -> illuminate_support::Result<Self> {
        let mut union = <Self as Default>::default();
        for name in types {
            let name = name.as_ref();
            if name == "null" {
                union.common.nullable = true;
                continue;
            }
            if !Self::SUPPORTED.contains(&name) {
                return Err(InvalidArgumentException::new(format!(
                    "Unsupported JSON Schema type [{name}] in a multi-type union."
                ))
                .into());
            }
            if !union.types.iter().any(|existing| existing == name) {
                union.types.push(name.to_string());
            }
        }
        Ok(union)
    }

    /// The union's types, without `null`.
    pub fn types(&self) -> &[String] {
        &self.types
    }

    /// The default value.
    pub fn default(mut self, value: impl Into<Value>) -> Self {
        self.common.default = Some(value.into());
        self
    }
}

/// A value matching any of several schemas (`anyOf`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnyOfType {
    pub(crate) common: Common,
    pub(crate) schemas: Vec<Type>,
}

impl AnyOfType {
    /// Add a schema the value may match.
    pub fn schema(mut self, schema: impl Into<Type>) -> Self {
        self.schemas.push(schema.into());
        self
    }

    /// The schemas the value may match.
    pub fn schemas(&self) -> &[Type] {
        &self.schemas
    }

    /// The default value.
    pub fn default(mut self, value: impl Into<Value>) -> Self {
        self.common.default = Some(value.into());
        self
    }
}

common_methods!(
    StringType,
    IntegerType,
    NumberType,
    BooleanType,
    ArrayType,
    ObjectType,
    UnionType,
    AnyOfType,
);

/// Any schema type.
#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    /// A `string`.
    StringType(StringType),
    /// An `integer`.
    IntegerType(IntegerType),
    /// A `number`.
    NumberType(NumberType),
    /// A `boolean`.
    BooleanType(BooleanType),
    /// An `array`.
    ArrayType(ArrayType),
    /// An `object`.
    ObjectType(ObjectType),
    /// A multi-type union.
    UnionType(UnionType),
    /// An `anyOf` composition.
    AnyOfType(AnyOfType),
}

impl Type {
    pub(crate) fn common(&self) -> &Common {
        match self {
            Type::StringType(t) => &t.common,
            Type::IntegerType(t) => &t.common,
            Type::NumberType(t) => &t.common,
            Type::BooleanType(t) => &t.common,
            Type::ArrayType(t) => &t.common,
            Type::ObjectType(t) => &t.common,
            Type::UnionType(t) => &t.common,
            Type::AnyOfType(t) => &t.common,
        }
    }

    pub(crate) fn common_mut(&mut self) -> &mut Common {
        match self {
            Type::StringType(t) => &mut t.common,
            Type::IntegerType(t) => &mut t.common,
            Type::NumberType(t) => &mut t.common,
            Type::BooleanType(t) => &mut t.common,
            Type::ArrayType(t) => &mut t.common,
            Type::ObjectType(t) => &mut t.common,
            Type::UnionType(t) => &mut t.common,
            Type::AnyOfType(t) => &mut t.common,
        }
    }

    /// Mark the property as required by its parent object.
    pub fn required(mut self) -> Self {
        self.common_mut().required = true;
        self
    }

    /// Allow `null` as well.
    pub fn nullable(mut self) -> Self {
        self.common_mut().nullable = true;
        self
    }

    /// Set the schema's title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.common_mut().title = Some(title.into());
        self
    }

    /// Set the schema's description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.common_mut().description = Some(description.into());
        self
    }

    /// Whether the property is required by its parent object.
    pub fn is_required(&self) -> bool {
        self.common().required
    }

    /// Whether `null` is allowed.
    pub fn is_nullable(&self) -> bool {
        self.common().nullable
    }

    /// The JSON Schema `type` name: `string`, `object`, ... (`None` for
    /// unions and `anyOf`).
    pub fn type_name(&self) -> Option<&'static str> {
        match self {
            Type::StringType(_) => Some("string"),
            Type::IntegerType(_) => Some("integer"),
            Type::NumberType(_) => Some("number"),
            Type::BooleanType(_) => Some("boolean"),
            Type::ArrayType(_) => Some("array"),
            Type::ObjectType(_) => Some("object"),
            Type::UnionType(_) | Type::AnyOfType(_) => None,
        }
    }

    /// The schema as a JSON Schema document.
    pub fn to_array(&self) -> Value {
        Value::Object(crate::serializer::serialize(self))
    }
}

impl Serialize for Type {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_array().serialize(serializer)
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::serializer::pretty(&self.to_array()))
    }
}

impl From<Type> for Value {
    fn from(schema: Type) -> Self {
        schema.to_array()
    }
}

pub(crate) type Attributes = Map<String, Value>;
