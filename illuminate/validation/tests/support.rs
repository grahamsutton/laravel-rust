//! The service provider, factory, rule maps and validated input helpers.

mod common;

use std::sync::Arc;

use common::container;
use illuminate_container::ServiceProvider;
use illuminate_support::json;
use illuminate_validation::{
    Factory, RuleSet, Rules, ValidatedInput, ValidationServiceProvider, Validator, rules,
};

#[tokio::test]
async fn the_provider_registers_a_shared_factory() {
    let (app, _guard) = container();
    ValidationServiceProvider.register(&app);
    let factory = app.make::<Factory>();
    assert!(Arc::ptr_eq(&factory, &Factory::current()));

    factory.include_unvalidated_array_keys();
    let mut validator = Validator::make(
        json!({"items": [{"name": "a", "price": 1}]}),
        rules! { "items" => "array", "items.*.name" => "required" },
    );
    assert_eq!(
        validator.validate().await.unwrap(),
        json!({"items": [{"name": "a", "price": 1}]})
    );
}

#[test]
fn rule_maps_can_be_composed() {
    let mut rules = Rules::from([("name", "required")]);
    rules.append("name", "string|max:255");
    assert_eq!(rules.get("name").unwrap().len(), 3);

    let merged = rules.clone().merge([("email", "required")]);
    assert_eq!(merged.keys().collect::<Vec<_>>(), vec!["name", "email"]);

    let mut rules = merged;
    assert!(rules.remove("email").is_some());
    assert!(!rules.contains_key("email"));
    assert!(!rules.is_empty());

    let set = RuleSet::new()
        .push("required")
        .push(vec!["email", "max:255"]);
    assert_eq!(set.len(), 3);
    assert_eq!(RuleSet::from("required|email").len(), 2);
    assert_eq!(RuleSet::from(["regex:/^a|b$/"]).len(), 1);
}

#[test]
fn validated_input_helpers() {
    let input = ValidatedInput::new(json!({
        "name": "Taylor",
        "age": "37",
        "admin": "1",
        "address": {"city": "Little Rock"},
        "bio": "",
    }));
    assert_eq!(input.input("address.city"), json!("Little Rock"));
    assert_eq!(input.input_or("missing", "default"), json!("default"));
    assert_eq!(input.string("name"), "Taylor");
    assert_eq!(input.integer("age"), 37);
    assert!(input.boolean("admin"));
    assert!(input.filled("name"));
    assert!(!input.filled("bio"));
    assert!(input.missing("email"));
    assert!(input.has_any(&["email", "name"]));
    assert_eq!(input.keys(), vec!["name", "age", "admin", "address", "bio"]);
    assert_eq!(
        input.only(&["address.city"]),
        json!({"address": {"city": "Little Rock"}})
    );

    #[derive(serde::Deserialize)]
    struct Person {
        name: String,
    }
    let person: Person = input.deserialize().unwrap();
    assert_eq!(person.name, "Taylor");

    let pairs: Vec<(String, illuminate_support::Value)> = input.clone().into_iter().collect();
    assert_eq!(pairs.len(), 5);
    let value: illuminate_support::Value = input.into();
    assert_eq!(value["name"], json!("Taylor"));
}
