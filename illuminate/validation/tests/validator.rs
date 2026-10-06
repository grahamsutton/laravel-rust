//! The validator's behavior: wildcards, bail, nullable, sometimes, custom
//! messages and attributes, validated data, exclusion and hooks.

mod common;

use common::{container, errors, passes};
use illuminate_support::{Value, json};
use illuminate_validation::{Rule, Rules, ValidationException, Validator, rules};

#[tokio::test]
async fn it_matches_laravels_documented_error_response() {
    let _c = container();
    let mut validator = Validator::make(
        json!({
            "team_name": [],
            "authorization": {"role": "god"},
            "users": [{}, {"email": "taylor@laravel.com"}, {"email": "nope"}],
        }),
        rules! {
            "team_name" => "string|min:1",
            "authorization.role" => [Rule::in_(["admin", "member"])],
            "users.*.email" => "required|email",
        },
    );
    let exception = validator.validate().await.unwrap_err();
    assert_eq!(exception.status, 422);
    assert_eq!(
        exception.message(),
        "The team name field must be a string. (and 4 more errors)"
    );
    assert_eq!(
        exception.to_json(),
        json!({
            "message": "The team name field must be a string. (and 4 more errors)",
            "errors": {
                "team_name": [
                    "The team name field must be a string.",
                    "The team name field must be at least 1 characters."
                ],
                "authorization.role": ["The selected authorization.role is invalid."],
                "users.0.email": ["The users.0.email field is required."],
                "users.2.email": ["The users.2.email field must be a valid email address."]
            }
        })
    );
}

#[tokio::test]
async fn wildcards_expand_over_arrays() {
    let _c = container();
    let bag = errors(
        json!({"items": [{"name": "a"}, {"name": ""}, {"price": 1}]}),
        [("items.*.name", "required|string")],
    )
    .await;
    assert!(!bag.has("items.0.name"));
    assert_eq!(
        bag.first("items.1.name"),
        Some("The items.1.name field is required.")
    );
    assert_eq!(
        bag.first("items.2.name"),
        Some("The items.2.name field is required.")
    );
    assert_eq!(bag.get("items.*.name").len(), 2);

    // Nested wildcards.
    let bag = errors(
        json!({"users": [{"emails": ["a@b.com", "nope"]}, {"emails": ["also-nope"]}]}),
        [("users.*.emails.*", "email")],
    )
    .await;
    assert_eq!(bag.keys(), vec!["users.0.emails.1", "users.1.emails.0"]);

    // A missing parent means there's nothing to validate.
    assert!(passes(json!({}), [("items.*.name", "required")]).await);
    assert!(
        !passes(
            json!({}),
            [("items", "required|array"), ("items.*.name", "required")]
        )
        .await
    );
}

#[tokio::test]
async fn explicit_rules_come_before_wildcard_rules() {
    let _c = container();
    let bag = errors(
        json!({"items": [{}]}),
        rules! { "items.*.name" => "required", "title" => "required" },
    )
    .await;
    assert_eq!(bag.keys(), vec!["title", "items.0.name"]);
}

#[tokio::test]
async fn dependent_rules_resolve_wildcards_to_the_same_index() {
    let _c = container();
    let bag = errors(
        json!({"users": [{"first_name": "Taylor", "last_name": "Otwell"}, {"last_name": "Doe"}]}),
        [("users.*.first_name", "required_with:users.*.last_name")],
    )
    .await;
    assert!(!bag.has("users.0.first_name"));
    assert_eq!(
        bag.first("users.1.first_name"),
        // Like Laravel, only the attribute under validation keeps its raw array name.
        Some("The users.1.first_name field is required when users.1.last name is present.")
    );
}

#[tokio::test]
async fn bail_stops_after_the_first_failure() {
    let _c = container();
    let bag = errors(json!({"field": "abcdef"}), [("field", "integer|max:3")]).await;
    assert_eq!(bag.get("field").len(), 2);
    let bag = errors(
        json!({"field": "abcdef"}),
        [("field", "bail|integer|max:3")],
    )
    .await;
    assert_eq!(
        bag.get("field"),
        vec!["The field field must be an integer."]
    );
}

#[tokio::test]
async fn failed_implicit_rules_stop_validation() {
    let _c = container();
    let bag = errors(json!({"field": ""}), [("field", "required|email|min:3")]).await;
    assert_eq!(bag.get("field"), vec!["The field field is required."]);
}

#[tokio::test]
async fn nullable_allows_null() {
    let _c = container();
    assert!(passes(json!({"field": null}), [("field", "nullable|email")]).await);
    assert!(passes(json!({"field": ""}), [("field", "nullable|email")]).await);
    assert!(!passes(json!({"field": "x"}), [("field", "nullable|email")]).await);
    assert!(!passes(json!({"field": null}), [("field", "email")]).await);
    assert!(!passes(json!({"field": null}), [("field", "nullable|required")]).await);
}

#[tokio::test]
async fn sometimes_only_validates_present_fields() {
    let _c = container();
    assert!(passes(json!({}), [("email", "sometimes|required|email")]).await);
    let bag = errors(
        json!({"email": ""}),
        [("email", "sometimes|required|email")],
    )
    .await;
    assert_eq!(bag.first("email"), Some("The email field is required."));
    assert!(!passes(json!({"email": "x"}), [("email", "sometimes|email")]).await);
}

#[tokio::test]
async fn stop_on_first_failure_stops_everything() {
    let _c = container();
    let mut validator = Validator::make(json!({}), rules! { "a" => "required", "b" => "required" })
        .stop_on_first_failure();
    assert!(validator.fails().await);
    assert_eq!(validator.errors().keys(), vec!["a"]);
}

#[tokio::test]
async fn custom_messages() {
    let _c = container();
    let mut validator = Validator::make(
        json!({"items": [{"name": ""}], "title": "abcdef", "body": 5}),
        rules! {
            "email" => "required|email",
            "name" => "required",
            "items.*.name" => "required",
            "title" => "max:3",
            "body" => "string",
        },
    )
    .messages([
        (
            "email.required",
            json!("We need to know your email address!"),
        ),
        ("required", json!("The :attribute is missing.")),
        (
            "items.*.name.required",
            json!("Item #:position needs a name."),
        ),
        (
            "max",
            json!({"string": "Keep :attribute under :max characters."}),
        ),
        ("body", json!({"string": "Write some text!"})),
    ]);
    assert!(validator.fails().await);
    let bag = validator.errors();
    assert_eq!(
        bag.first("email"),
        Some("We need to know your email address!")
    );
    assert_eq!(bag.first("name"), Some("The name is missing."));
    assert_eq!(bag.first("items.0.name"), Some("Item #1 needs a name."));
    assert_eq!(bag.first("title"), Some("Keep title under 3 characters."));
    assert_eq!(bag.first("body"), Some("Write some text!"));
}

#[tokio::test]
async fn custom_attributes() {
    let _c = container();
    let mut validator = Validator::make(
        json!({"items": [{}]}),
        rules! { "email" => "required", "items.*.name" => "required", "first_name" => "required_with:email_address" },
    )
    .attributes([("email", "email address"), ("items.*.name", "item name")]);
    assert!(validator.fails().await);
    assert_eq!(
        validator.errors().first("email"),
        Some("The email address field is required.")
    );
    assert_eq!(
        validator.errors().first("items.0.name"),
        Some("The item name field is required.")
    );
}

#[tokio::test]
async fn placeholders_for_positions_and_input() {
    let _c = container();
    let mut validator = Validator::make(
        json!({
            "photos": [
                {"name": "BeachVacation.jpg", "description": "A photo of my beach vacation!"},
                {"name": "GrandCanyon.jpg", "description": ""},
            ],
            "matrix": [[1, "x"]],
            "age": 150,
        }),
        rules! {
            "photos.*.description" => "required",
            "matrix.*.*" => "integer",
            "age" => "integer|between:1,120",
        },
    )
    .messages([
        (
            "photos.*.description.required",
            "Please describe photo #:position (:ordinal-position, index :index).",
        ),
        (
            "matrix.*.*.integer",
            "Row :first-position, column :second-position must be an integer.",
        ),
        (
            "between",
            "The :attribute value :input is not between :min - :max.",
        ),
    ]);
    assert!(validator.fails().await);
    let bag = validator.errors();
    assert_eq!(
        bag.first("photos.1.description"),
        Some("Please describe photo #2 (2nd, index 1).")
    );
    assert_eq!(
        bag.first("matrix.0.1"),
        Some("Row 1, column 2 must be an integer.")
    );
    assert_eq!(
        bag.first("age"),
        Some("The age value 150 is not between 1 - 120.")
    );
}

#[tokio::test]
async fn uppercase_placeholders_keep_their_case() {
    let _c = container();
    let mut validator = Validator::make(json!({}), [("first_name", "required")])
        .messages([("required", ":Attribute / :ATTRIBUTE / :attribute")]);
    validator.passes().await;
    assert_eq!(
        validator.errors().first("first_name"),
        Some("First name / FIRST NAME / first name")
    );
}

#[tokio::test]
async fn custom_values_are_displayed() {
    let _c = container();
    let mut validator = Validator::make(
        json!({"payment_type": "cc"}),
        [("credit_card_number", "required_if:payment_type,cc")],
    )
    .values([("payment_type", [("cc", "credit card")])]);
    assert!(validator.fails().await);
    assert_eq!(
        validator.errors().first("credit_card_number"),
        Some("The credit card number field is required when payment type is credit card.")
    );
}

#[tokio::test]
async fn validated_returns_only_validated_keys() {
    let _c = container();
    let mut validator = Validator::make(
        json!({
            "name": "Taylor",
            "role": "admin",
            "author": {"name": "Abigail", "secret": "x"},
            "items": [{"name": "a", "price": 1}, {"name": "b", "price": 2}],
            "settings": {"theme": "dark", "beta": true},
        }),
        rules! {
            "name" => "required",
            "author.name" => "required",
            "items" => "required|array",
            "items.*.name" => "required",
            "settings" => "array",
            "missing" => "nullable",
        },
    );
    let validated = validator.validate().await.unwrap();
    assert_eq!(
        validated,
        json!({
            "name": "Taylor",
            "author": {"name": "Abigail"},
            "items": [{"name": "a"}, {"name": "b"}],
            "settings": {"theme": "dark", "beta": true},
        })
    );
    assert_eq!(validator.validated().unwrap(), validated);

    let safe = validator.safe().unwrap();
    assert_eq!(safe.only(&["name"]), json!({"name": "Taylor"}));
    assert_eq!(
        safe.except(&["items", "settings", "author"]),
        json!({"name": "Taylor"})
    );
    assert_eq!(safe.input("items.1.name"), json!("b"));
    assert_eq!(safe["author.name"], json!("Abigail"));
    assert!(safe.has("settings.theme"));
    assert_eq!(safe.collect().count(), 4);
}

#[tokio::test]
async fn unvalidated_array_keys_can_be_included() {
    let _c = container();
    let mut validator = Validator::make(
        json!({"items": [{"name": "a", "price": 1}]}),
        rules! { "items" => "array", "items.*.name" => "required" },
    )
    .exclude_unvalidated_array_keys(false);
    assert_eq!(
        validator.validate().await.unwrap(),
        json!({"items": [{"name": "a", "price": 1}]})
    );
}

#[tokio::test]
async fn validated_fails_when_validation_failed() {
    let _c = container();
    let mut validator = Validator::make(json!({}), [("name", "required")]);
    assert!(validator.fails().await);
    let error = validator.validated().unwrap_err();
    assert_eq!(error.message(), "The name field is required.");
    assert!(validator.safe().is_err());
}

#[tokio::test]
#[should_panic(expected = "hasn't run yet")]
async fn validated_requires_a_run() {
    let _c = container();
    let validator = Validator::make(json!({}), [("name", "required")]);
    let _ = validator.validated();
}

#[tokio::test]
async fn exclude_rules_remove_attributes() {
    let _c = container();
    let rules = rules! {
        "has_appointment" => "required|boolean",
        "appointment_date" => "exclude_if:has_appointment,false|required|date",
        "doctor_name" => "exclude_unless:has_appointment,true|required|string",
    };
    let mut validator = Validator::make(json!({"has_appointment": false}), rules.clone());
    assert_eq!(
        validator.validate().await.unwrap(),
        json!({"has_appointment": false})
    );

    let mut validator = Validator::make(json!({"has_appointment": true}), rules);
    assert!(validator.fails().await);
    assert_eq!(
        validator.errors().keys(),
        vec!["appointment_date", "doctor_name"]
    );

    let mut validator = Validator::make(
        json!({"a": 1, "b": 2, "c": 3, "d": 4, "e": 5}),
        rules! {
            "a" => "exclude",
            "b" => "exclude_with:a|integer",
            "c" => "exclude_without:missing|integer",
            "d" => [Rule::exclude_if(true), "integer"],
            "e" => [Rule::exclude_if(|| false), "integer"],
        },
    );
    assert_eq!(validator.validate().await.unwrap(), json!({"e": 5}));
}

#[tokio::test]
async fn excluding_a_parent_excludes_its_children() {
    let _c = container();
    let mut validator = Validator::make(
        json!({"type": "none", "address": {"street": ""}}),
        rules! {
            "type" => "required",
            "address" => "exclude_if:type,none|array",
            "address.street" => "required",
        },
    );
    assert_eq!(validator.validate().await.unwrap(), json!({"type": "none"}));
}

#[tokio::test]
async fn after_hooks_can_add_errors() {
    let _c = container();
    let mut validator = Validator::make(
        json!({"a": 1, "b": 2}),
        rules! { "a" => "required", "b" => "required" },
    )
    .after(|validator| {
        let sum = validator.data()["a"].as_i64().unwrap() + validator.data()["b"].as_i64().unwrap();
        if sum != 10 {
            validator
                .errors_mut()
                .add("sum", "The numbers must add up to 10.");
        }
    });
    assert!(validator.fails().await);
    assert_eq!(
        validator.errors().first("sum"),
        Some("The numbers must add up to 10.")
    );
}

#[tokio::test]
async fn sometimes_adds_rules_conditionally() {
    let _c = container();
    let mut validator = Validator::make(
        json!({"games": 120}),
        rules! { "games" => "required|integer|min:0" },
    )
    .sometimes(["reason", "cost"], "required", |input, _| {
        input["games"].as_i64() >= Some(100)
    });
    assert!(validator.fails().await);
    assert_eq!(validator.errors().keys(), vec!["reason", "cost"]);

    let mut validator = Validator::make(
        json!({"games": 5}),
        rules! { "games" => "required|integer|min:0" },
    )
    .sometimes("reason", "required", |input, _| {
        input["games"].as_i64() >= Some(100)
    });
    assert!(validator.passes().await);

    let mut validator = Validator::make(
        json!({"channels": [
            {"type": "email", "address": "abigail@example.com"},
            {"type": "url", "address": "https://example.com"},
            {"type": "email", "address": "nope"},
        ]}),
        Rules::new(),
    )
    .sometimes("channels.*.address", "email", |_, item: &Value| {
        item["type"] == "email"
    })
    .sometimes("channels.*.address", "url", |_, item: &Value| {
        item["type"] != "email"
    });
    assert!(validator.fails().await);
    assert_eq!(validator.errors().keys(), vec!["channels.2.address"]);
}

#[tokio::test]
async fn failed_rules_are_reported() {
    let _c = container();
    let mut validator = Validator::make(
        json!({"name": "abcdef"}),
        rules! { "name" => "string|max:3", "email" => "required" },
    );
    assert!(validator.fails().await);
    let failed = validator.failed();
    assert_eq!(failed["name"]["Max"], vec!["3".to_string()]);
    assert!(failed["email"].contains_key("Required"));
}

#[tokio::test]
async fn escaped_dots_address_literal_keys() {
    let _c = container();
    assert!(passes(json!({"v1.0": "x"}), [("v1\\.0", "required")]).await);
    let bag = errors(json!({"v1": {"0": "x"}}), [("v1\\.0", "required")]).await;
    assert_eq!(bag.first("v1.0"), Some("The v1.0 field is required."));
    let mut validator = Validator::make(json!({"v1.0": "x", "other": 1}), [("v1\\.0", "required")]);
    assert_eq!(validator.validate().await.unwrap(), json!({"v1.0": "x"}));
}

#[tokio::test]
async fn distinct_finds_duplicates() {
    let _c = container();
    let bag = errors(
        json!({"items": [{"id": 1}, {"id": 2}, {"id": 1}]}),
        [("items.*.id", "distinct")],
    )
    .await;
    assert_eq!(bag.keys(), vec!["items.0.id", "items.2.id"]);
    assert_eq!(
        bag.first("items.0.id"),
        Some("The items.0.id field has a duplicate value.")
    );

    assert!(
        !passes(
            json!({"tags": ["a", "A"]}),
            [("tags.*", "distinct:ignore_case")]
        )
        .await
    );
    assert!(passes(json!({"tags": ["a", "A"]}), [("tags.*", "distinct")]).await);
    assert!(!passes(json!({"tags": [1, "1"]}), [("tags.*", "distinct")]).await);
    assert!(passes(json!({"tags": [1, "1"]}), [("tags.*", "distinct:strict")]).await);
}

#[tokio::test]
async fn validate_with_bag_names_the_bag() {
    let _c = container();
    let mut validator = Validator::make(json!({}), [("email", "required")]);
    let exception: ValidationException = validator.validate_with_bag("login").await.unwrap_err();
    assert_eq!(exception.error_bag, "login");
}

#[tokio::test]
async fn try_validate_returns_any_error() {
    let _c = container();
    let mut validator = Validator::make(json!({}), [("email", "required")]);
    let error = validator.try_validate().await.unwrap_err();
    assert!(error.downcast_ref::<ValidationException>().is_some());

    let mut validator = Validator::make(json!({"email": "x"}), [("email", "unique:users")]);
    let error = validator.try_validate().await.unwrap_err();
    assert_eq!(error.to_string(), "Presence verifier has not been set.");
}

#[tokio::test]
async fn valid_and_invalid_split_the_data() {
    let _c = container();
    let mut validator = Validator::make(
        json!({"name": "Taylor", "email": "nope"}),
        rules! { "name" => "required", "email" => "email" },
    );
    validator.passes().await;
    assert_eq!(validator.valid(), json!({"name": "Taylor"}));
    assert_eq!(validator.invalid(), json!({"email": "nope"}));
}

#[tokio::test]
async fn implicit_attribute_names_can_be_formatted() {
    let _c = container();
    let mut validator = Validator::make(json!({"items": [{}]}), [("items.*.name", "required")])
        .implicit_attributes_formatter(|attribute| attribute.replace('.', " "));
    validator.passes().await;
    assert_eq!(
        validator.errors().first("items.0.name"),
        Some("The items 0 name field is required.")
    );
}

#[tokio::test]
async fn validators_can_be_rerun() {
    let _c = container();
    let mut validator = Validator::make(json!({"name": ""}), [("name", "required")]);
    assert!(validator.fails().await);
    assert!(validator.fails().await);
    assert_eq!(validator.errors().count(), 1);
}

#[tokio::test]
async fn rules_can_be_built_many_ways() {
    let _c = container();
    let data = json!({"email": "taylor@laravel.com"});
    assert!(passes(data.clone(), [("email", "required|email")]).await);
    assert!(passes(data.clone(), vec![("email", vec!["required", "email"])]).await);
    assert!(
        passes(
            data.clone(),
            Rules::new().rule("email", ["required", "email"])
        )
        .await
    );
    assert!(
        passes(
            data.clone(),
            rules! { "email" => ["required", Rule::email()] }
        )
        .await
    );
    let more = Validator::make(data, [("email", "required")]).add_rules([("email", "max:3")]);
    let mut more = more;
    assert!(more.fails().await);
}
