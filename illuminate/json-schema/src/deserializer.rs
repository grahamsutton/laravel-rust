//! Turning JSON Schema documents back into schema types.

use std::collections::HashMap;

use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Result, Value, ValueExt};
use serde_json::Map;

use crate::types::{
    AnyOfType, ArrayType, BooleanType, IntegerType, NumberType, ObjectType, StringType, Type,
    UnionType,
};

/// The most schema fragments a document may expand into (through `$ref`s).
const MAX_NODES: usize = 20_000;

type Schema = Map<String, Value>;

fn invalid(message: impl Into<String>) -> illuminate_support::Error {
    InvalidArgumentException::new(message).into()
}

pub(crate) struct Deserializer<'a> {
    root: &'a Schema,
    nodes: usize,
    ref_cache: HashMap<String, Schema>,
}

impl<'a> Deserializer<'a> {
    pub(crate) fn deserialize(schema: &'a Value) -> Result<Type> {
        let Value::Object(root) = schema else {
            return Err(invalid("The JSON Schema must be an object."));
        };
        Deserializer {
            root,
            nodes: 0,
            ref_cache: HashMap::new(),
        }
        .build(root.clone(), Vec::new())
    }

    fn build(&mut self, schema: Schema, refs: Vec<String>) -> Result<Type> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(invalid(format!(
                "The JSON Schema is too large to deserialize; it expands beyond [{MAX_NODES}] fragments."
            )));
        }

        let (schema, refs) = self.resolve_ref(schema, refs)?;

        if let Some(any_of) = self.build_any_of(&schema, &refs)? {
            return apply_common(any_of.into(), &schema);
        }

        let (schema, nullable_from_union, refs) = self.normalize_unions(schema, refs)?;
        let (name, nullable_from_type) = resolve_type(&schema)?;

        let schema_type: Type = match name {
            TypeName::Union(names) => {
                ensure_union_constraints_are_supported(&schema)?;
                UnionType::try_new(names)?.into()
            }
            TypeName::Single(name) => match name.as_str() {
                "object" => self.build_object(&schema, &refs)?.into(),
                "array" => self.build_array(&schema, &refs)?.into(),
                "string" => build_string(&schema).into(),
                "integer" => build_integer(&schema)?.into(),
                "number" => build_number(&schema)?.into(),
                "boolean" => <BooleanType as Default>::default().into(),
                _ => return Err(invalid(format!("Unsupported JSON Schema type [{name}]."))),
            },
        };

        let mut schema_type = apply_common(schema_type, &schema)?;
        if nullable_from_union || nullable_from_type {
            schema_type = schema_type.nullable();
        }
        Ok(schema_type)
    }

    fn build_any_of(&mut self, schema: &Schema, refs: &[String]) -> Result<Option<AnyOfType>> {
        let Some(Value::Array(any_of)) = schema.get("anyOf") else {
            return Ok(None);
        };

        let mut nullable = false;
        let mut branches = Vec::new();
        for branch in any_of {
            let Value::Object(branch) = branch else {
                return Err(invalid(
                    "Unable to represent the schema for an anyOf branch; boolean schemas are not supported.",
                ));
            };
            let (branch, branch_refs) = self.resolve_ref(branch.clone(), refs.to_vec())?;
            if is_null_branch(&branch) {
                nullable = true;
            } else {
                branches.push((branch, branch_refs));
            }
        }

        if nullable && branches.len() == 1 {
            // A nullable schema, spelled as a union: built as that schema.
            return Ok(None);
        }

        let mut any_of = <AnyOfType as Default>::default();
        for (branch, branch_refs) in branches {
            any_of = any_of.schema(self.build(branch, branch_refs)?);
        }
        if nullable {
            any_of = any_of.nullable();
        }
        Ok(Some(any_of))
    }

    fn build_object(&mut self, schema: &Schema, refs: &[String]) -> Result<ObjectType> {
        let mut object = <ObjectType as Default>::default();

        if let Some(Value::Object(properties)) = schema.get("properties") {
            let required: Vec<String> = match schema.get("required") {
                Some(Value::Array(required)) => required.iter().map(ValueExt::to_string_lossy).collect(),
                _ => Vec::new(),
            };
            for (key, definition) in properties {
                let Value::Object(definition) = definition else {
                    return Err(invalid(format!(
                        "Unable to represent the schema for property [{key}]; boolean schemas are not supported."
                    )));
                };
                let mut property = self.build(definition.clone(), refs.to_vec())?;
                if required.contains(key) {
                    property = property.required();
                }
                object = object.property(key.clone(), property);
            }
        }

        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            object = object.without_additional_properties();
        }
        Ok(object)
    }

    fn build_array(&mut self, schema: &Schema, refs: &[String]) -> Result<ArrayType> {
        let mut array = <ArrayType as Default>::default();

        match schema.get("items") {
            None | Some(Value::Null) => {}
            Some(Value::Array(items)) if items.is_empty() => {}
            Some(Value::Object(items)) if items.is_empty() => {}
            Some(Value::Object(items)) => array = array.items(self.build(items.clone(), refs.to_vec())?),
            Some(_) => return Err(invalid("Tuple and boolean JSON Schema \"items\" are not supported.")),
        }
        if let Some(min) = set(schema, "minItems") {
            array = array.min(to_u64(min));
        }
        if let Some(max) = set(schema, "maxItems") {
            array = array.max(to_u64(max));
        }
        if let Some(unique) = set(schema, "uniqueItems") {
            array.unique_items = unique.truthy().then_some(true);
        }
        Ok(array)
    }

    /// Follow a local `$ref`, merging the keywords beside it over the
    /// schema it points to.
    fn resolve_ref(&mut self, schema: Schema, mut refs: Vec<String>) -> Result<(Schema, Vec<String>)> {
        let Some(Value::String(reference)) = schema.get("$ref") else {
            return Ok((schema, refs));
        };
        let reference = reference.clone();
        if refs.contains(&reference) {
            return Err(invalid(format!("Circular JSON Schema $ref [{reference}] detected.")));
        }
        refs.push(reference.clone());

        let mut resolved = self.lookup_ref(&reference)?;
        for (key, value) in schema {
            if key != "$ref" {
                resolved.insert(key, value);
            }
        }
        self.resolve_ref(resolved, refs)
    }

    fn lookup_ref(&mut self, reference: &str) -> Result<Schema> {
        if let Some(schema) = self.ref_cache.get(reference) {
            return Ok(schema.clone());
        }
        if reference == "#" {
            self.ref_cache.insert(reference.into(), self.root.clone());
            return Ok(self.root.clone());
        }
        let Some(pointer) = reference.strip_prefix("#/") else {
            return Err(invalid(format!("Unable to resolve non-local JSON Schema $ref [{reference}].")));
        };

        let mut target = Value::Object(self.root.clone());
        for segment in pointer.split('/') {
            let segment = percent_decode(segment).replace("~1", "/").replace("~0", "~");
            let next = match &target {
                Value::Object(map) => map.get(&segment).cloned(),
                Value::Array(items) => segment.parse::<usize>().ok().and_then(|index| items.get(index).cloned()),
                _ => None,
            };
            match next {
                Some(next) => target = next,
                None => return Err(invalid(format!("Unable to resolve JSON Schema $ref [{reference}]."))),
            }
        }

        let Value::Object(schema) = target else {
            return Err(invalid(format!("The JSON Schema $ref [{reference}] does not point to a schema.")));
        };
        self.ref_cache.insert(reference.into(), schema.clone());
        Ok(schema)
    }

    /// Fold a nullable `anyOf` / `oneOf` (one schema plus a `null` branch)
    /// into that schema.
    fn normalize_unions(&mut self, schema: Schema, refs: Vec<String>) -> Result<(Schema, bool, Vec<String>)> {
        for key in ["anyOf", "oneOf"] {
            let Some(Value::Array(branches_value)) = schema.get(key) else {
                continue;
            };

            let mut nullable = false;
            let mut branches = Vec::new();
            for branch in branches_value {
                let Value::Object(branch) = branch else {
                    continue;
                };
                let (branch, branch_refs) = self.resolve_ref(branch.clone(), refs.clone())?;
                if is_null_branch(&branch) {
                    nullable = true;
                } else {
                    branches.push((branch, branch_refs));
                }
            }

            if !nullable || branches.len() != 1 {
                return Err(invalid(format!(
                    "Only a nullable \"{key}\" (a single schema plus a \"null\" branch) is supported."
                )));
            }

            let (branch, branch_refs) = branches.remove(0);
            let mut merged = schema.clone();
            merged.remove(key);
            for (sibling_key, value) in &merged {
                if let Some(branch_value) = branch.get(sibling_key)
                    && branch_value != value
                {
                    return Err(invalid(format!(
                        "Conflicting [{sibling_key}] between a \"{key}\" branch and its sibling keys."
                    )));
                }
            }
            for (branch_key, value) in branch {
                merged.insert(branch_key, value);
            }
            return Ok((merged, true, branch_refs));
        }
        Ok((schema, false, refs))
    }
}

enum TypeName {
    Single(String),
    Union(Vec<String>),
}

fn resolve_type(schema: &Schema) -> Result<(TypeName, bool)> {
    let mut nullable = false;
    let mut name = match schema.get("type") {
        Some(Value::Array(types)) => {
            nullable = types.iter().any(|value| value == "null");
            let mut names: Vec<String> = Vec::new();
            for value in types.iter().filter(|value| *value != "null") {
                let value = value.to_string_lossy();
                if !names.contains(&value) {
                    names.push(value);
                }
            }
            if names.len() > 1 {
                return Ok((TypeName::Union(names), nullable));
            }
            names.into_iter().next().map(Value::String)
        }
        Some(Value::Null) | None => None,
        Some(other) => Some(other.clone()),
    };
    if name.is_none() {
        name = infer_type(schema).map(Value::from);
    }
    match name {
        Some(Value::String(name)) => Ok((TypeName::Single(name), nullable)),
        _ => Err(invalid("Unable to determine the JSON Schema type for the given schema.")),
    }
}

fn infer_type(schema: &Schema) -> Option<&'static str> {
    let has = |keys: &[&str]| keys.iter().any(|key| set(schema, key).is_some());

    if has(&["properties", "additionalProperties", "required"]) {
        Some("object")
    } else if has(&["items", "minItems", "maxItems", "uniqueItems"]) {
        Some("array")
    } else if let Some(Value::Array(values)) = schema.get("enum") {
        infer_enum_type(values)
    } else if has(&["minLength", "maxLength", "pattern", "format"]) {
        Some("string")
    } else if has(&["minimum", "maximum", "multipleOf"]) {
        Some("number")
    } else {
        None
    }
}

fn infer_enum_type(values: &[Value]) -> Option<&'static str> {
    let mut resolved: Option<&'static str> = None;
    for value in values {
        let current = match value {
            Value::Bool(_) => "boolean",
            Value::Number(number) if number.is_f64() => "number",
            Value::Number(_) => "integer",
            Value::String(_) => "string",
            _ => return None,
        };
        match resolved {
            None => resolved = Some(current),
            Some(previous) if previous == current => {}
            // A mix of integers and floats is still numeric.
            Some("integer" | "number") if matches!(current, "integer" | "number") => resolved = Some("number"),
            Some(_) => return None,
        }
    }
    resolved
}

fn ensure_union_constraints_are_supported(schema: &Schema) -> Result<()> {
    const KEYWORDS: [&str; 14] = [
        "minLength",
        "maxLength",
        "pattern",
        "format",
        "minimum",
        "maximum",
        "multipleOf",
        "items",
        "minItems",
        "maxItems",
        "uniqueItems",
        "properties",
        "required",
        "additionalProperties",
    ];
    let unsupported: Vec<&str> = KEYWORDS.into_iter().filter(|key| schema.contains_key(*key)).collect();
    if unsupported.is_empty() {
        return Ok(());
    }
    Err(invalid(format!(
        "Type-specific keywords [{}] are not supported on a multi-type JSON Schema union.",
        unsupported.join(", ")
    )))
}

fn build_string(schema: &Schema) -> StringType {
    let mut string = <StringType as Default>::default();
    if let Some(min) = set(schema, "minLength") {
        string = string.min(to_u64(min));
    }
    if let Some(max) = set(schema, "maxLength") {
        string = string.max(to_u64(max));
    }
    if let Some(pattern) = set(schema, "pattern") {
        string = string.pattern(pattern.to_string_lossy());
    }
    if let Some(format) = set(schema, "format") {
        string = string.format(format.to_string_lossy());
    }
    string
}

fn build_integer(schema: &Schema) -> Result<IntegerType> {
    let mut integer = <IntegerType as Default>::default();
    if let Some(value) = numeric_bound(schema, "minimum")? {
        integer = integer.min(to_integer(&value)?);
    }
    if let Some(value) = numeric_bound(schema, "maximum")? {
        integer = integer.max(to_integer(&value)?);
    }
    if let Some(value) = numeric_bound(schema, "multipleOf")? {
        integer = integer.multiple_of(to_integer(&value)?);
    }
    Ok(integer)
}

fn build_number(schema: &Schema) -> Result<NumberType> {
    Ok(NumberType {
        minimum: numeric_bound(schema, "minimum")?.map(Value::Number),
        maximum: numeric_bound(schema, "maximum")?.map(Value::Number),
        multiple_of: numeric_bound(schema, "multipleOf")?.map(Value::Number),
        ..Default::default()
    })
}

fn numeric_bound(schema: &Schema, keyword: &str) -> Result<Option<serde_json::Number>> {
    let Some(value) = set(schema, keyword) else {
        return Ok(None);
    };
    to_number(value)
        .map(Some)
        .ok_or_else(|| invalid(format!("The JSON Schema [{keyword}] constraint must be a number.")))
}

fn to_number(value: &Value) -> Option<serde_json::Number> {
    match value {
        Value::Number(number) => Some(number.clone()),
        Value::String(string) => {
            let string = string.trim();
            string
                .parse::<i64>()
                .ok()
                .map(serde_json::Number::from)
                .or_else(|| string.parse::<f64>().ok().and_then(serde_json::Number::from_f64))
        }
        _ => None,
    }
}

fn to_integer(number: &serde_json::Number) -> Result<i64> {
    if let Some(integer) = number.as_i64() {
        return Ok(integer);
    }
    let float = number.as_f64().unwrap_or_default();
    if float.fract() != 0.0 || !float.is_finite() {
        return Err(invalid(format!("The JSON Schema integer constraint [{float}] must be an integer.")));
    }
    Ok(float as i64)
}

fn to_u64(value: &Value) -> u64 {
    value.to_i64_lossy().unwrap_or_default().max(0) as u64
}

/// The keyword's value, when it's present and not `null` (PHP's `isset`).
fn set<'s>(schema: &'s Schema, key: &str) -> Option<&'s Value> {
    schema.get(key).filter(|value| !value.is_null())
}

fn is_null_branch(branch: &Schema) -> bool {
    match branch.get("type") {
        Some(Value::String(name)) => name == "null",
        Some(Value::Array(names)) => names.len() == 1 && names[0] == "null",
        _ => false,
    }
}

fn apply_common(mut schema_type: Type, schema: &Schema) -> Result<Type> {
    if let Some(title) = set(schema, "title") {
        schema_type = schema_type.title(title.to_string_lossy());
    }
    if let Some(description) = set(schema, "description") {
        schema_type = schema_type.description(description.to_string_lossy());
    }
    if let Some(Value::Array(values)) = schema.get("enum") {
        schema_type.common_mut().enum_ = Some(values.clone());
    }
    if let Some(default) = schema.get("default") {
        if default.is_null() {
            return Err(invalid("A null JSON Schema [default] is not supported."));
        }
        schema_type.common_mut().default = Some(default.clone());
    }
    Ok(schema_type)
}

/// Decode `%XX` escapes (PHP's `rawurldecode`).
fn percent_decode(segment: &str) -> String {
    let hex = |byte: u8| (byte as char).to_digit(16);
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            decoded.push((high * 16 + low) as u8);
            i += 3;
            continue;
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}
