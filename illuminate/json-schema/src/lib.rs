//! Build JSON Schema documents fluently — the schemas tools, structured
//! output, and APIs describe their data with.
//!
//! ```
//! use illuminate_json_schema::JsonSchema;
//! use illuminate_support::json;
//!
//! let schema = JsonSchema::object()
//!     .property("location", JsonSchema::string().description("The location to get the weather for.").required())
//!     .property("units", JsonSchema::string().enum_(["celsius", "fahrenheit"]).default("celsius"));
//!
//! assert_eq!(schema.to_array(), json!({
//!     "properties": {
//!         "location": {"description": "The location to get the weather for.", "type": "string"},
//!         "units": {"default": "celsius", "enum": ["celsius", "fahrenheit"], "type": "string"},
//!     },
//!     "type": "object",
//!     "required": ["location"],
//! }));
//! ```
//!
//! Every builder is a value: each method returns the updated schema. Turn
//! a schema into a document with `to_array()`, serialize it with serde, or
//! `to_string()` it for pretty-printed JSON. [`JsonSchema::from_array`]
//! goes the other way.

mod deserializer;
mod serializer;
pub mod types;

use illuminate_support::{Result, Value};

pub use types::{
    AnyOfType, ArrayType, BooleanType, IntegerType, Number, NumberType, ObjectType, StringType,
    Type, UnionType,
};

/// Start a schema (Laravel's `JsonSchema` factory).
pub struct JsonSchema;

impl JsonSchema {
    /// An `object`. Add its properties with [`ObjectType::property`].
    pub fn object() -> ObjectType {
        <ObjectType as Default>::default()
    }

    /// An `array`. Describe its items with [`ArrayType::items`].
    pub fn array() -> ArrayType {
        <ArrayType as Default>::default()
    }

    /// A `string`.
    pub fn string() -> StringType {
        <StringType as Default>::default()
    }

    /// An `integer`.
    pub fn integer() -> IntegerType {
        <IntegerType as Default>::default()
    }

    /// A `number`.
    pub fn number() -> NumberType {
        <NumberType as Default>::default()
    }

    /// A `boolean`.
    pub fn boolean() -> BooleanType {
        <BooleanType as Default>::default()
    }

    /// A value of one of several primitive types. Including `"null"` makes
    /// it nullable.
    ///
    /// ```
    /// use illuminate_json_schema::JsonSchema;
    /// use illuminate_support::json;
    ///
    /// let id = JsonSchema::union(["string", "integer", "null"]);
    ///
    /// assert_eq!(id.to_array(), json!({"type": ["string", "integer", "null"]}));
    /// ```
    ///
    /// # Panics
    ///
    /// When a type isn't one JSON Schema allows in a union, like Laravel's
    /// `InvalidArgumentException`. Use [`UnionType::try_new`] for types
    /// that come from elsewhere.
    pub fn union<S: AsRef<str>>(types: impl IntoIterator<Item = S>) -> UnionType {
        UnionType::try_new(types).unwrap_or_else(|error| panic!("{error}"))
    }

    /// A value matching any of the given schemas (`anyOf`).
    ///
    /// ```
    /// use illuminate_json_schema::JsonSchema;
    /// use illuminate_support::json;
    ///
    /// let contact = JsonSchema::any_of([
    ///     JsonSchema::string().format("email").into(),
    ///     JsonSchema::string().format("uri").into(),
    /// ]);
    ///
    /// assert_eq!(contact.to_array(), json!({"anyOf": [
    ///     {"format": "email", "type": "string"},
    ///     {"format": "uri", "type": "string"},
    /// ]}));
    /// ```
    pub fn any_of(schemas: impl IntoIterator<Item = Type>) -> AnyOfType {
        schemas.into_iter().fold(<AnyOfType as Default>::default(), AnyOfType::schema)
    }

    /// Build a schema from a JSON Schema document.
    ///
    /// Local `$ref`s (`#/$defs/address`) are followed, `["string", "null"]`
    /// types and nullable `anyOf`/`oneOf` unions become nullable schemas,
    /// and a missing `type` is inferred from the other keywords.
    ///
    /// ```
    /// use illuminate_json_schema::{JsonSchema, Type};
    /// use illuminate_support::json;
    ///
    /// let schema = JsonSchema::from_array(&json!({
    ///     "type": "object",
    ///     "properties": {"name": {"type": ["string", "null"], "maxLength": 255}},
    ///     "required": ["name"],
    /// }))?;
    ///
    /// let Type::ObjectType(object) = &schema else { unreachable!() };
    /// let name = object.get_property("name").unwrap();
    /// assert!(name.is_required() && name.is_nullable());
    /// # Ok::<(), illuminate_support::Error>(())
    /// ```
    pub fn from_array(schema: &Value) -> Result<Type> {
        deserializer::Deserializer::deserialize(schema)
    }
}
