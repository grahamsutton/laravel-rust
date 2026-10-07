//! Turning schema types into JSON Schema documents.
//!
//! Keywords come out in the order Laravel's serializer writes them: the
//! shared keywords first, then the type's own, then `type` (and, for
//! objects, `required`).

use illuminate_support::Value;

use crate::types::{Attributes, Common, Type};

/// Serialize a schema type.
pub(crate) fn serialize(schema: &Type) -> Attributes {
    let mut attributes = common(schema.common());

    match schema {
        Type::StringType(string) => {
            insert(&mut attributes, "minLength", string.min_length);
            insert(&mut attributes, "maxLength", string.max_length);
            insert(&mut attributes, "pattern", string.pattern.clone());
            insert(&mut attributes, "format", string.format.clone());
        }
        Type::IntegerType(integer) => {
            insert(&mut attributes, "minimum", integer.minimum);
            insert(&mut attributes, "maximum", integer.maximum);
            insert(&mut attributes, "multipleOf", integer.multiple_of);
        }
        Type::NumberType(number) => {
            insert(&mut attributes, "minimum", number.minimum.clone());
            insert(&mut attributes, "maximum", number.maximum.clone());
            insert(&mut attributes, "multipleOf", number.multiple_of.clone());
        }
        Type::BooleanType(_) | Type::UnionType(_) => {}
        Type::ArrayType(array) => {
            insert(&mut attributes, "minItems", array.min_items);
            insert(&mut attributes, "maxItems", array.max_items);
            if let Some(items) = &array.items {
                attributes.insert("items".into(), Value::Object(serialize(items)));
            }
            insert(&mut attributes, "uniqueItems", array.unique_items);
        }
        Type::ObjectType(object) => {
            insert(&mut attributes, "additionalProperties", object.additional_properties);
            if !object.properties.is_empty() {
                let properties = object
                    .properties
                    .iter()
                    .map(|(name, property)| (name.clone(), Value::Object(serialize(property))))
                    .collect();
                attributes.insert("properties".into(), Value::Object(properties));
            }
        }
        Type::AnyOfType(any_of) => {
            let mut schemas: Vec<Value> = any_of
                .schemas
                .iter()
                .map(|schema| Value::Object(serialize(schema)))
                .collect();
            if any_of.common.nullable {
                schemas.push(serde_json::json!({"type": "null"}));
            }
            attributes.insert("anyOf".into(), Value::Array(schemas));
            return attributes;
        }
    }

    let mut types: Vec<Value> = match schema {
        Type::UnionType(union) => union.types.iter().cloned().map(Value::String).collect(),
        _ => vec![Value::from(schema.type_name().unwrap_or_default())],
    };
    let type_ = if schema.is_nullable() {
        types.push(Value::from("null"));
        Value::Array(types)
    } else if let Type::UnionType(_) = schema {
        Value::Array(types)
    } else {
        types.remove(0)
    };
    attributes.insert("type".into(), type_);

    if let Type::ObjectType(object) = schema {
        let required: Vec<Value> = object
            .properties
            .iter()
            .filter(|(_, property)| property.is_required())
            .map(|(name, _)| Value::String(name.clone()))
            .collect();
        if !required.is_empty() {
            attributes.insert("required".into(), Value::Array(required));
        }
    }

    attributes
}

fn common(common: &Common) -> Attributes {
    let mut attributes = Attributes::new();
    insert(&mut attributes, "title", common.title.clone());
    insert(&mut attributes, "description", common.description.clone());
    insert(&mut attributes, "default", common.default.clone());
    insert(&mut attributes, "enum", common.enum_.clone());
    attributes
}

fn insert(attributes: &mut Attributes, key: &str, value: Option<impl Into<Value>>) {
    match value.map(Into::into) {
        Some(Value::Null) | None => {}
        Some(value) => {
            attributes.insert(key.into(), value);
        }
    }
}

/// Pretty-print a document the way PHP's `JSON_PRETTY_PRINT` does, with
/// four spaces of indentation.
pub(crate) fn pretty(value: &Value) -> String {
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    serde::Serialize::serialize(value, &mut serializer).expect("JSON values always serialize");
    String::from_utf8(out).expect("serde_json writes UTF-8")
}
