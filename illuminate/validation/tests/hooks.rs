//! Conditional callbacks, appended rules, and per-validator extensions.

mod common;

use common::container;
use illuminate_support::json;
use illuminate_validation::{Validator, rules};

#[tokio::test]
async fn when_passes_and_when_fails_run_the_matching_callback() {
    let _c = container();

    let mut passing = Validator::make(json!({"name": "Taylor"}), rules! { "name" => "required" });
    assert_eq!(passing.when_passes(|_| "passed").await, Some("passed"));
    assert_eq!(passing.when_fails(|_| "failed").await, None);
    assert_eq!(passing.when_passes_or(|_| 1, |_| 2).await, 1);
    assert_eq!(passing.when_fails_or(|_| 1, |_| 2).await, 2);

    let mut failing = Validator::make(json!({}), rules! { "name" => "required" });
    let errors = failing
        .when_fails(|validator| {
            validator
                .errors()
                .all()
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
        })
        .await;
    assert_eq!(
        errors,
        Some(vec!["The name field is required.".to_string()])
    );
    assert_eq!(failing.when_passes(|_| "passed").await, None);
    assert_eq!(failing.when_passes_or(|_| 1, |_| 2).await, 2);

    // Callbacks may change the validator, like adding a failure.
    let mut validator = Validator::make(json!({"name": "Taylor"}), rules! { "name" => "required" });
    validator
        .when_passes(|validator| {
            validator.errors_mut().add("name", "Taken.");
        })
        .await;
    assert_eq!(validator.errors().first("name"), Some("Taken."));
}

#[tokio::test]
async fn appended_rules_run_after_the_existing_ones() {
    let _c = container();

    let mut validator = Validator::make(
        json!({"name": "", "email": "taylor"}),
        rules! { "name" => "required" },
    )
    .append_rules(rules! { "name" => "string", "email" => "email" });

    assert!(validator.fails().await);
    assert_eq!(
        validator.errors().get("name"),
        vec!["The name field is required."]
    );
    assert_eq!(
        validator.errors().first("email"),
        Some("The email field must be a valid email address.")
    );
    assert!(validator.has_rule("name", &["Required"]));
    assert!(validator.has_rule("name", &["String"]));
}

#[tokio::test]
async fn extensions_can_be_added_to_a_single_validator() {
    let _c = container();

    let mut validator = Validator::make(json!({"color": "purple"}), [("color", "primary_color")]);
    validator
        .add_extension("primaryColor", |_, value, _, _| {
            matches!(value.as_str(), Some("red" | "yellow" | "blue"))
        })
        .set_fallback_messages([("primary_color", "The :attribute must be a primary color.")]);
    assert!(validator.fails().await);
    assert_eq!(
        validator.errors().first("color"),
        Some("The color must be a primary color.")
    );

    // Other validators don't know the rule.
    let mut other = Validator::make(json!({"color": "red"}), [("color", "primary_color")]);
    assert!(other.try_passes().await.is_err());
}

#[tokio::test]
async fn implicit_extensions_run_for_missing_values() {
    let _c = container();

    let mut validator = Validator::make(json!({}), [("terms", "accepted_terms")]);
    validator
        .add_implicit_extension("accepted_terms", |_, value, _, _| {
            value.as_bool() == Some(true)
        })
        .set_fallback_messages([("accepted_terms", "You must accept the :attribute.")]);
    assert!(validator.fails().await);
    assert_eq!(
        validator.errors().first("terms"),
        Some("You must accept the terms.")
    );

    // A regular extension skips missing values.
    let mut validator = Validator::make(json!({}), [("terms", "accepted_terms")]);
    validator.add_extension("accepted_terms", |_, _, _, _| false);
    assert!(validator.passes().await);
}

#[tokio::test]
async fn dependent_extensions_receive_resolved_parameters() {
    let _c = container();

    let mut validator = Validator::make(
        json!({"items": [{"min": 5, "max": 3}, {"min": 1, "max": 9}]}),
        [("items.*.max", "greater_than_field:items.*.min")],
    );
    validator
        .add_dependent_extension("greater_than_field", |_, value, parameters, context| {
            let other = context.input(&parameters[0]);
            value.as_i64().unwrap_or(0) > other.as_i64().unwrap_or(0)
        })
        .add_replacer("greater_than_field", |message, _, _, parameters| {
            message.replace(":other", &parameters[0])
        })
        .set_fallback_messages([(
            "greater_than_field",
            "The :attribute must be greater than :other.",
        )]);

    assert!(validator.fails().await);
    assert_eq!(validator.errors().keys(), vec!["items.0.max"]);
    assert_eq!(
        validator.errors().first("items.0.max"),
        Some("The items.0.max must be greater than items.0.min.")
    );
}
