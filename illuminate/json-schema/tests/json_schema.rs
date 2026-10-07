use illuminate_json_schema::{JsonSchema, Type, UnionType};
use illuminate_support::{Value, json};

fn from_array(schema: Value) -> Type {
    JsonSchema::from_array(&schema).unwrap()
}

fn error(schema: Value) -> String {
    JsonSchema::from_array(&schema).unwrap_err().to_string()
}

#[test]
fn strings_serialize_with_their_constraints() {
    let schema = JsonSchema::string()
        .title("Name")
        .description("The user's name.")
        .min(1)
        .max(255)
        .pattern("^[a-z]+$")
        .format("hostname")
        .default("taylor");

    assert_eq!(
        serde_json::to_string(&schema).unwrap(),
        r#"{"title":"Name","description":"The user's name.","default":"taylor","minLength":1,"maxLength":255,"pattern":"^[a-z]+$","format":"hostname","type":"string"}"#
    );
}

#[test]
fn numbers_serialize_with_their_bounds() {
    assert_eq!(
        JsonSchema::integer().min(1).max(10).multiple_of(2).default(4).to_array(),
        json!({"default": 4, "minimum": 1, "maximum": 10, "multipleOf": 2, "type": "integer"})
    );
    assert_eq!(
        JsonSchema::number().min(0.5).max(10).to_array(),
        json!({"minimum": 0.5, "maximum": 10, "type": "number"})
    );
    assert_eq!(
        JsonSchema::boolean().default(true).to_array(),
        json!({"default": true, "type": "boolean"})
    );
}

#[test]
fn arrays_describe_their_items() {
    let tags = JsonSchema::array()
        .items(JsonSchema::string().max(20))
        .min(1)
        .max(5)
        .unique()
        .default(["laravel"]);

    assert_eq!(
        serde_json::to_string(&tags).unwrap(),
        r#"{"default":["laravel"],"minItems":1,"maxItems":5,"items":{"maxLength":20,"type":"string"},"uniqueItems":true,"type":"array"}"#
    );
}

#[test]
fn objects_list_their_required_properties_last() {
    let user = JsonSchema::object()
        .description("A user.")
        .property("name", JsonSchema::string().required())
        .property("email", JsonSchema::string().format("email").required())
        .property("age", JsonSchema::integer().min(0))
        .without_additional_properties();

    assert_eq!(
        serde_json::to_string(&user).unwrap(),
        r#"{"description":"A user.","additionalProperties":false,"properties":{"name":{"type":"string"},"email":{"format":"email","type":"string"},"age":{"minimum":0,"type":"integer"}},"type":"object","required":["name","email"]}"#
    );
    assert_eq!(JsonSchema::object().to_array(), json!({"type": "object"}));
}

#[test]
fn properties_can_be_replaced() {
    let schema = JsonSchema::object()
        .property("name", JsonSchema::string())
        .property("name", JsonSchema::integer());

    assert_eq!(schema.get_properties().len(), 1);
    assert_eq!(schema.get_property("name").unwrap().type_name(), Some("integer"));
}

#[test]
fn nullable_types_allow_null() {
    assert_eq!(
        JsonSchema::string().nullable().to_array(),
        json!({"type": ["string", "null"]})
    );
    assert_eq!(
        JsonSchema::union(["string", "integer"]).nullable().to_array(),
        json!({"type": ["string", "integer", "null"]})
    );
    assert_eq!(
        JsonSchema::any_of([JsonSchema::string().into(), JsonSchema::integer().into()])
            .nullable()
            .to_array(),
        json!({"anyOf": [{"type": "string"}, {"type": "integer"}, {"type": "null"}]})
    );
}

#[test]
fn enums_keep_their_order() {
    assert_eq!(
        JsonSchema::string().enum_(["draft", "published", "draft"]).to_array(),
        json!({"enum": ["draft", "published", "draft"], "type": "string"})
    );
}

#[test]
fn unions_only_accept_json_schema_types() {
    let union = UnionType::try_new(["string", "string", "null"]).unwrap();
    assert_eq!(union.types(), ["string"]);
    assert!(union.is_nullable());

    let error = UnionType::try_new(["string", "date"]).unwrap_err();
    assert_eq!(error.to_string(), "Unsupported JSON Schema type [date] in a multi-type union.");
}

#[test]
#[should_panic(expected = "Unsupported JSON Schema type [date] in a multi-type union.")]
fn invalid_union_literals_panic() {
    JsonSchema::union(["date"]);
}

#[test]
fn schemas_display_as_pretty_json() {
    assert_eq!(
        JsonSchema::object().property("name", JsonSchema::string().required()).to_string(),
        "{\n    \"properties\": {\n        \"name\": {\n            \"type\": \"string\"\n        }\n    },\n    \"type\": \"object\",\n    \"required\": [\n        \"name\"\n    ]\n}"
    );
}

#[test]
fn documents_round_trip() {
    let schema: Type = JsonSchema::object()
        .title("Order")
        .property("id", JsonSchema::integer().min(1).required())
        .property("total", JsonSchema::number().min(0).multiple_of(0.01))
        .property("status", JsonSchema::string().enum_(["pending", "paid"]).default("pending"))
        .property("tags", JsonSchema::array().items(JsonSchema::string()).unique())
        .property("note", JsonSchema::string().nullable())
        .property("reference", JsonSchema::union(["string", "integer"]))
        .property(
            "contact",
            JsonSchema::any_of([JsonSchema::string().format("email").into(), JsonSchema::string().format("uri").into()]),
        )
        .property("paid", JsonSchema::boolean())
        .without_additional_properties()
        .into();

    assert_eq!(from_array(schema.to_array()), schema);
}

#[test]
fn types_are_inferred_from_keywords() {
    let inferred = |schema: Value| from_array(schema).type_name();

    assert_eq!(inferred(json!({"properties": {}})), Some("object"));
    assert_eq!(inferred(json!({"required": ["id"]})), Some("object"));
    assert_eq!(inferred(json!({"items": {"type": "string"}})), Some("array"));
    assert_eq!(inferred(json!({"maxLength": 3})), Some("string"));
    assert_eq!(inferred(json!({"minimum": 3})), Some("number"));
    assert_eq!(inferred(json!({"enum": ["a", "b"]})), Some("string"));
    assert_eq!(inferred(json!({"enum": [1, 2]})), Some("integer"));
    assert_eq!(inferred(json!({"enum": [1, 2.5]})), Some("number"));
    assert_eq!(inferred(json!({"enum": [true, false]})), Some("boolean"));
    assert_eq!(
        error(json!({"enum": ["a", 1]})),
        "Unable to determine the JSON Schema type for the given schema."
    );
    assert_eq!(
        error(json!({"description": "Anything"})),
        "Unable to determine the JSON Schema type for the given schema."
    );
}

#[test]
fn local_references_are_followed() {
    let schema = from_array(json!({
        "type": "object",
        "$defs": {"address": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}},
        "properties": {
            "billing": {"$ref": "#/$defs/address", "description": "Where invoices go."},
            "shipping": {"$ref": "#/%24defs/address"},
        },
    }));

    let Type::ObjectType(object) = &schema else { panic!("not an object") };
    let billing = object.get_property("billing").unwrap().to_array();
    assert_eq!(
        billing,
        json!({"description": "Where invoices go.", "properties": {"city": {"type": "string"}}, "type": "object", "required": ["city"]})
    );
    assert_eq!(object.get_property("shipping").unwrap().type_name(), Some("object"));
}

#[test]
fn bad_references_are_rejected() {
    assert_eq!(
        error(json!({"$ref": "#/$defs/node", "$defs": {"node": {"$ref": "#/$defs/node"}}})),
        "Circular JSON Schema $ref [#/$defs/node] detected."
    );
    assert_eq!(
        error(json!({"$ref": "https://example.com/schema.json"})),
        "Unable to resolve non-local JSON Schema $ref [https://example.com/schema.json]."
    );
    assert_eq!(
        error(json!({"$ref": "#/$defs/missing"})),
        "Unable to resolve JSON Schema $ref [#/$defs/missing]."
    );
    assert_eq!(
        error(json!({"$ref": "#/$defs/name", "$defs": {"name": "string"}})),
        "The JSON Schema $ref [#/$defs/name] does not point to a schema."
    );
}

#[test]
fn recursive_schemas_through_the_root_are_circular() {
    assert_eq!(
        error(json!({"type": "object", "properties": {"child": {"$ref": "#"}}})),
        "Circular JSON Schema $ref [#] detected."
    );
}

#[test]
fn documents_may_not_expand_without_bound() {
    // Every level references the next one ten times: 10^5 fragments.
    let mut defs = serde_json::Map::new();
    for level in 0..5 {
        let properties: serde_json::Map<String, Value> = (0..10)
            .map(|i| (format!("p{i}"), json!({"$ref": format!("#/$defs/level{}", level + 1)})))
            .collect();
        defs.insert(format!("level{level}"), json!({"type": "object", "properties": properties}));
    }
    defs.insert("level5".into(), json!({"type": "string"}));

    assert_eq!(
        error(json!({"$ref": "#/$defs/level0", "$defs": defs})),
        "The JSON Schema is too large to deserialize; it expands beyond [20000] fragments."
    );
}

#[test]
fn nullable_unions_become_nullable_types() {
    let schema = from_array(json!({"oneOf": [{"type": "string", "maxLength": 5}, {"type": "null"}], "description": "A code."}));
    assert_eq!(
        schema.to_array(),
        json!({"description": "A code.", "maxLength": 5, "type": ["string", "null"]})
    );

    let schema = from_array(json!({"anyOf": [{"type": "integer"}, {"type": ["null"]}]}));
    assert_eq!(schema.to_array(), json!({"type": ["integer", "null"]}));

    assert_eq!(
        error(json!({"oneOf": [{"type": "string"}, {"type": "integer"}]})),
        "Only a nullable \"oneOf\" (a single schema plus a \"null\" branch) is supported."
    );
    assert_eq!(
        error(json!({"description": "A", "oneOf": [{"type": "string", "description": "B"}, {"type": "null"}]})),
        "Conflicting [description] between a \"oneOf\" branch and its sibling keys."
    );
}

#[test]
fn any_of_compositions_are_kept() {
    let schema = from_array(json!({"anyOf": [{"type": "string"}, {"type": "integer"}, {"type": "null"}], "title": "Id"}));

    let Type::AnyOfType(any_of) = &schema else { panic!("not anyOf") };
    assert_eq!(any_of.schemas().len(), 2);
    assert!(schema.is_nullable());
    assert_eq!(
        schema.to_array(),
        json!({"title": "Id", "anyOf": [{"type": "string"}, {"type": "integer"}, {"type": "null"}]})
    );
    assert_eq!(
        error(json!({"anyOf": [true]})),
        "Unable to represent the schema for an anyOf branch; boolean schemas are not supported."
    );
}

#[test]
fn multi_type_unions_are_deserialized() {
    let schema = from_array(json!({"type": ["string", "integer", "null"], "description": "An id."}));

    let Type::UnionType(union) = &schema else { panic!("not a union") };
    assert_eq!(union.types(), ["string", "integer"]);
    assert!(schema.is_nullable());
    assert_eq!(
        error(json!({"type": ["string", "integer"], "maxLength": 5})),
        "Type-specific keywords [maxLength] are not supported on a multi-type JSON Schema union."
    );
    assert_eq!(
        error(json!({"type": ["string", "date"]})),
        "Unsupported JSON Schema type [date] in a multi-type union."
    );
}

#[test]
fn unsupported_documents_are_rejected() {
    assert_eq!(error(json!({"type": "null"})), "Unsupported JSON Schema type [null].");
    assert_eq!(
        error(json!({"type": "array", "items": [{"type": "string"}]})),
        "Tuple and boolean JSON Schema \"items\" are not supported."
    );
    assert_eq!(
        error(json!({"type": "object", "properties": {"name": true}})),
        "Unable to represent the schema for property [name]; boolean schemas are not supported."
    );
    assert_eq!(
        error(json!({"type": "integer", "minimum": 1.5})),
        "The JSON Schema integer constraint [1.5] must be an integer."
    );
    assert_eq!(
        error(json!({"type": "number", "maximum": "lots"})),
        "The JSON Schema [maximum] constraint must be a number."
    );
    assert_eq!(
        error(json!({"type": "string", "default": null})),
        "A null JSON Schema [default] is not supported."
    );
}

#[test]
fn numeric_strings_and_whole_floats_are_accepted() {
    assert_eq!(
        from_array(json!({"type": "integer", "minimum": 2.0, "maximum": "10"})).to_array(),
        json!({"minimum": 2, "maximum": 10, "type": "integer"})
    );
    assert_eq!(
        from_array(json!({"type": "array", "items": {}})).to_array(),
        json!({"type": "array"})
    );
}
