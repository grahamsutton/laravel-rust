//! Rule objects, closures, extensions, fluent rule builders, passwords,
//! enums and message resolvers.

mod common;

use std::sync::Arc;

use common::{container, errors, passes};
use illuminate_support::{Value, json};
use illuminate_validation::{
    BackedEnum, FailCallback, MessageResolver, Password, Rule, RuleSet, UncompromisedVerifier,
    ValidationContext, ValidationRule, Validator, async_trait, rules,
};

struct Uppercase;

#[async_trait]
impl ValidationRule for Uppercase {
    async fn validate(&self, _attribute: &str, value: &Value, fail: &mut FailCallback<'_>) {
        if value.as_str().is_some_and(|v| v.to_uppercase() != v) {
            fail("The :attribute must be uppercase.");
        }
    }
}

struct AlwaysPresent;

#[async_trait]
impl ValidationRule for AlwaysPresent {
    async fn validate(&self, _attribute: &str, value: &Value, fail: &mut FailCallback<'_>) {
        if value.is_null() {
            fail("The :attribute must be here.");
        }
    }

    fn implicit(&self) -> bool {
        true
    }
}

/// A data-aware rule: the value must match another field.
struct MatchesField(&'static str);

#[async_trait]
impl ValidationRule for MatchesField {
    async fn validate_with(
        &self,
        _attribute: &str,
        value: &Value,
        context: &ValidationContext<'_>,
        fail: &mut FailCallback<'_>,
    ) {
        tokio::task::yield_now().await;
        if context.input(self.0) != *value {
            fail(&format!("The :attribute must match {}.", self.0));
        }
    }
}

struct Explodes;

#[async_trait]
impl ValidationRule for Explodes {
    async fn validate_with(
        &self,
        _: &str,
        _: &Value,
        context: &ValidationContext<'_>,
        _: &mut FailCallback<'_>,
    ) {
        context.abort(illuminate_support::error::RuntimeException::new("Boom."));
    }
}

#[tokio::test]
async fn rule_objects_validate_values() {
    let _c = container();
    assert!(
        passes(
            json!({"name": "TAYLOR"}),
            rules! { "name" => ["required", Uppercase] }
        )
        .await
    );

    let mut validator = Validator::make(
        json!({"name": "taylor"}),
        rules! { "name" => ["required", "string", Uppercase] },
    );
    assert!(validator.fails().await);
    assert_eq!(
        validator.errors().first("name"),
        Some("The name must be uppercase.")
    );
    assert!(validator.failed()["name"].contains_key("Uppercase"));

    // Non-implicit rule objects skip missing and empty values...
    assert!(passes(json!({}), rules! { "name" => [Uppercase] }).await);
    assert!(passes(json!({"name": ""}), rules! { "name" => [Uppercase] }).await);
    // ...implicit ones don't.
    let bag = errors(json!({}), rules! { "name" => [AlwaysPresent] }).await;
    assert_eq!(bag.first("name"), Some("The name must be here."));
}

#[tokio::test]
async fn rule_object_messages_can_be_customized() {
    let _c = container();
    let mut validator =
        Validator::make(json!({"name": "taylor"}), rules! { "name" => [Uppercase] })
            .messages([("name.uppercase", "Shout your :attribute!")]);
    validator.passes().await;
    assert_eq!(validator.errors().first("name"), Some("Shout your name!"));
}

#[tokio::test]
async fn data_aware_rules_see_all_of_the_data() {
    let _c = container();
    assert!(
        passes(
            json!({"a": "x", "b": "x"}),
            rules! { "b" => [MatchesField("a")] }
        )
        .await
    );
    let bag = errors(
        json!({"a": "x", "b": "y"}),
        rules! { "b" => [MatchesField("a")] },
    )
    .await;
    assert_eq!(bag.first("b"), Some("The b must match a."));
}

#[tokio::test]
async fn rules_can_abort_validation() {
    let _c = container();
    let mut validator = Validator::make(json!({"a": "x"}), rules! { "a" => [Explodes] });
    let error = validator.try_passes().await.unwrap_err();
    assert_eq!(error.to_string(), "Boom.");
}

#[tokio::test]
async fn closures_are_rules() {
    let _c = container();
    let rules = rules! {
        "title" => ["required", "max:255", Rule::closure(|attribute, value, fail| {
            if value == "foo" {
                fail(&format!("The {attribute} is invalid."));
            }
        })],
    };
    assert!(passes(json!({"title": "bar"}), rules.clone()).await);
    let mut validator = Validator::make(json!({"title": "foo"}), rules);
    assert!(validator.fails().await);
    assert_eq!(
        validator.errors().first("title"),
        Some("The title is invalid.")
    );
    assert!(validator.failed()["title"].contains_key("ClosureValidationRule"));

    let implicit = Rule::closure(|_, value, fail| {
        if value.is_null() {
            fail("Needed!");
        }
    })
    .implicit();
    assert!(!passes(json!({}), rules! { "x" => [implicit] }).await);
}

#[tokio::test]
async fn extensions_add_string_rules() {
    let _c = container();
    Validator::extend("foo", |_, value, _, _| value == "foo")
        .message("The :attribute must be foo.");
    assert!(passes(json!({"name": "foo"}), [("name", "foo")]).await);
    let bag = errors(json!({"name": "bar"}), [("name", "foo")]).await;
    assert_eq!(bag.first("name"), Some("The name must be foo."));

    // Non-implicit extensions skip missing values.
    assert!(passes(json!({}), [("name", "foo")]).await);

    Validator::extend("divisible_by", |_, value, parameters, _| {
        let divisor: i64 = parameters[0].parse().unwrap();
        value.as_i64().is_some_and(|v| v % divisor == 0)
    });
    Validator::replacer("divisible_by", |message, _, _, parameters| {
        message.replace(":divisor", &parameters[0])
    });
    let mut validator = Validator::make(json!({"count": 7}), [("count", "divisible_by:3")])
        .messages([(
            "divisible_by",
            "The :attribute must be divisible by :divisor.",
        )]);
    validator.passes().await;
    assert_eq!(
        validator.errors().first("count"),
        Some("The count must be divisible by 3.")
    );

    // Without any message the language key is returned, like Laravel.
    Validator::extend("nameless", |_, _, _, _| false);
    let bag = errors(json!({"x": 1}), [("x", "nameless")]).await;
    assert_eq!(bag.first("x"), Some("validation.nameless"));
}

#[tokio::test]
async fn implicit_and_dependent_extensions() {
    let _c = container();
    Validator::extend_implicit("must_exist", |_, value, _, _| !value.is_null())
        .message("The :attribute must exist.");
    let bag = errors(json!({}), [("thing", "must_exist")]).await;
    assert_eq!(bag.first("thing"), Some("The thing must exist."));

    Validator::extend_dependent(
        "same_as",
        |_, value, parameters, context: &ValidationContext<'_>| {
            context.input(&parameters[0]) == *value
        },
    );
    assert!(
        passes(
            json!({"items": [{"a": 1, "b": 1}, {"a": 2, "b": 2}]}),
            [("items.*.a", "same_as:items.*.b")]
        )
        .await
    );
    assert!(
        !passes(
            json!({"items": [{"a": 1, "b": 2}]}),
            [("items.*.a", "same_as:items.*.b")]
        )
        .await
    );
}

#[tokio::test]
async fn date_numeric_and_string_builders() {
    let _c = container();
    let bag = errors(
        json!({"starts_at": "2000-01-01"}),
        rules! { "starts_at" => [Rule::date().after_today()] },
    )
    .await;
    assert_eq!(
        bag.first("starts_at"),
        Some("The starts at field must be a date after today.")
    );
    assert!(
        passes(
            json!({"starts_at": "2000-01-01"}),
            rules! { "starts_at" => [Rule::date().before_today()] }
        )
        .await
    );
    assert!(
        passes(
            json!({"at": "2024-01-01 10:00:00"}),
            rules! { "at" => [Rule::date_time()] }
        )
        .await
    );
    assert!(
        !passes(
            json!({"at": "2024-01-01"}),
            rules! { "at" => [Rule::date_time()] }
        )
        .await
    );

    let numeric = || rules! { "count" => [Rule::numeric().integer().min(1.0).max(10.0)] };
    assert!(passes(json!({"count": 5}), numeric()).await);
    let bag = errors(json!({"count": 11}), numeric()).await;
    assert_eq!(
        bag.first("count"),
        Some("The count field must not be greater than 10.")
    );

    let string = || rules! { "name" => [Rule::string().min(3).max(5).alpha_dash(true)] };
    assert!(passes(json!({"name": "abc_d"}), string()).await);
    let bag = errors(json!({"name": "ab"}), string()).await;
    assert_eq!(
        bag.first("name"),
        Some("The name field must be at least 3 characters.")
    );
}

#[tokio::test]
async fn conditional_rules() {
    let _c = container();
    assert!(!passes(json!({}), rules! { "role_id" => [Rule::required_if(true)] }).await);
    assert!(
        passes(
            json!({}),
            rules! { "role_id" => [Rule::required_if(false)] }
        )
        .await
    );
    assert!(
        !passes(
            json!({}),
            rules! { "role_id" => [Rule::required_if(|| true)] }
        )
        .await
    );
    assert!(
        !passes(
            json!({}),
            rules! { "role_id" => [Rule::required_unless(false)] }
        )
        .await
    );
    assert!(
        !passes(
            json!({"role_id": 1}),
            rules! { "role_id" => [Rule::prohibited_if(true)] }
        )
        .await
    );
    assert!(
        passes(
            json!({"role_id": 1}),
            rules! { "role_id" => [Rule::prohibited_unless(true)] }
        )
        .await
    );

    let when = || {
        rules! {
            "code" => [Rule::when(|data: &Value| data["type"] == "coupon", "required|alpha_num", "nullable")],
        }
    };
    assert!(!passes(json!({"type": "coupon"}), when()).await);
    assert!(passes(json!({"type": "other", "code": null}), when()).await);
    assert!(
        !passes(
            json!({"type": "coupon"}),
            rules! { "code" => [Rule::unless(false, "required", "")] }
        )
        .await
    );
}

#[tokio::test]
async fn for_each_builds_rules_per_item() {
    let _c = container();
    let rules = rules! {
        "companies.*.id" => [Rule::for_each(|_value, attribute| {
            if attribute == "companies.0.id" { RuleSet::from("integer") } else { RuleSet::from("string") }
        })],
    };
    assert!(
        passes(
            json!({"companies": [{"id": 1}, {"id": "x"}]}),
            rules.clone()
        )
        .await
    );
    let bag = errors(json!({"companies": [{"id": "x"}, {"id": 2}]}), rules).await;
    assert_eq!(bag.keys(), vec!["companies.0.id", "companies.1.id"]);
}

#[derive(Clone, Debug, PartialEq)]
enum ServerStatus {
    Active,
    Inactive,
    Pending,
}

impl BackedEnum for ServerStatus {
    fn cases() -> Vec<Self> {
        vec![
            ServerStatus::Active,
            ServerStatus::Inactive,
            ServerStatus::Pending,
        ]
    }

    fn value(&self) -> Value {
        match self {
            ServerStatus::Active => json!("active"),
            ServerStatus::Inactive => json!("inactive"),
            ServerStatus::Pending => json!("pending"),
        }
    }
}

#[tokio::test]
async fn enums() {
    let _c = container();
    assert!(
        passes(
            json!({"status": "active"}),
            rules! { "status" => [Rule::enum_::<ServerStatus>()] }
        )
        .await
    );
    let bag = errors(
        json!({"status": "nope"}),
        rules! { "status" => [Rule::enum_::<ServerStatus>()] },
    )
    .await;
    assert_eq!(bag.first("status"), Some("The selected status is invalid."));
    assert!(
        !passes(
            json!({"status": "pending"}),
            rules! { "status" => [Rule::enum_::<ServerStatus>().only([ServerStatus::Active])] }
        )
        .await
    );
    assert!(
        !passes(
            json!({"status": "pending"}),
            rules! { "status" => [Rule::enum_::<ServerStatus>().except([ServerStatus::Pending])] }
        )
        .await
    );
    assert!(
        passes(
            json!({"size": "m"}),
            rules! { "size" => [Rule::enum_values(["s", "m", "l"])] }
        )
        .await
    );
    assert!(
        !passes(
            json!({"size": "xl"}),
            rules! { "size" => [Rule::enum_values(["s", "m", "l"])] }
        )
        .await
    );
}

#[tokio::test]
async fn any_of_passes_when_one_set_passes() {
    let _c = container();
    let rules = || {
        rules! {
            "username" => ["required", Rule::any_of(["string|email", "string|alpha_dash|min:6"])],
        }
    };
    assert!(passes(json!({"username": "taylor@laravel.com"}), rules()).await);
    assert!(passes(json!({"username": "taylor_otwell"}), rules()).await);
    let bag = errors(json!({"username": "tay"}), rules()).await;
    assert_eq!(
        bag.first("username"),
        Some("The username field is invalid.")
    );
}

#[tokio::test]
async fn in_and_not_in_builders() {
    let _c = container();
    let rules =
        || rules! { "airports" => "required|array", "airports.*" => [Rule::in_(["NYC", "LIT"])] };
    assert!(passes(json!({"airports": ["NYC", "LIT"]}), rules()).await);
    let bag = errors(json!({"airports": ["NYC", "LAS"]}), rules()).await;
    assert_eq!(
        bag.first("airports.1"),
        Some("The selected airports.1 is invalid.")
    );
    assert!(
        !passes(
            json!({"topping": "sprinkles"}),
            rules! { "topping" => [Rule::not_in(["sprinkles", "cherries"])] }
        )
        .await
    );
    assert!(
        passes(
            json!({"roles": ["admin", "editor"]}),
            rules! { "roles" => [Rule::contains(["admin"])] }
        )
        .await
    );
    assert!(
        !passes(
            json!({"roles": ["admin"]}),
            rules! { "roles" => [Rule::doesnt_contain(["admin"])] }
        )
        .await
    );
    assert!(
        !passes(
            json!({"user": {"admin": true}}),
            rules! { "user" => [Rule::array(["name"])] }
        )
        .await
    );
    assert!(
        passes(
            json!({"user": {"name": "T"}}),
            rules! { "user" => [Rule::array_keys(["name", "username"])] }
        )
        .await
    );
}

// ----------------------------------------------------------------------
// Passwords
// ----------------------------------------------------------------------

#[tokio::test]
async fn password_complexity() {
    let _c = container();
    let rule = || rules! { "password" => ["required", Password::min(8).letters().mixed_case().numbers().symbols()] };
    assert!(passes(json!({"password": "Secret-123"}), rule()).await);

    let bag = errors(json!({"password": "short"}), rule()).await;
    assert_eq!(
        bag.get("password"),
        vec![
            "The password field must be at least 8 characters.",
            "The password field must contain at least one uppercase and one lowercase letter.",
            "The password field must contain at least one symbol.",
            "The password field must contain at least one number.",
        ]
    );

    let bag = errors(
        json!({"password": "12345678"}),
        rules! { "password" => [Password::min(8).letters()] },
    )
    .await;
    assert_eq!(
        bag.get("password"),
        vec!["The password field must contain at least one letter."]
    );

    let bag = errors(
        json!({"password": "Password"}),
        rules! { "password" => [Password::min(8).max(4)] },
    )
    .await;
    assert_eq!(
        bag.first("password"),
        Some("The password field must not be greater than 4 characters.")
    );
}

#[tokio::test]
async fn password_presence_rules() {
    let _c = container();
    // Not required: a missing password passes, a required one fails once.
    assert!(passes(json!({}), rules! { "password" => [Password::min(8)] }).await);
    let bag = errors(
        json!({}),
        rules! { "password" => ["required", Password::min(8)] },
    )
    .await;
    assert_eq!(bag.get("password"), vec!["The password field is required."]);
    let bag = errors(json!({}), rules! { "password" => [Password::required()] }).await;
    assert_eq!(bag.get("password"), vec!["The password field is required."]);
    assert!(
        passes(
            json!({"password": null}),
            rules! { "password" => ["nullable", Password::min(8)] }
        )
        .await
    );

    let mut validator = Validator::make(
        json!({"password": "abc"}),
        rules! { "password" => [Password::min(8)] },
    )
    .attributes([("password", "passphrase")]);
    validator.passes().await;
    assert_eq!(
        validator.errors().first("password"),
        Some("The passphrase field must be at least 8 characters.")
    );
}

#[tokio::test]
async fn password_defaults_live_in_the_container() {
    let _c = container();
    assert_eq!(
        Password::defaults().to_password_rules_string(),
        "minlength: 8;"
    );
    Password::set_defaults(|| Password::min(12).numbers());
    let bag = errors(
        json!({"password": "abcdefghijk"}),
        rules! { "password" => [Password::defaults()] },
    )
    .await;
    assert_eq!(
        bag.get("password"),
        vec![
            "The password field must be at least 12 characters.",
            "The password field must contain at least one number.",
        ]
    );
}

struct Leaked;

#[async_trait]
impl UncompromisedVerifier for Leaked {
    async fn verify(&self, password: &str, _threshold: usize) -> bool {
        password != "password123"
    }
}

#[tokio::test]
async fn uncompromised_passwords_use_the_bound_verifier() {
    let (container, _guard) = container();
    // Without a verifier, `uncompromised()` is a no-op.
    assert!(
        passes(
            json!({"password": "password123"}),
            rules! { "password" => [Password::min(8).uncompromised()] }
        )
        .await
    );

    container.instance_arc::<dyn UncompromisedVerifier>(Arc::new(Leaked));
    let bag = errors(
        json!({"password": "password123"}),
        rules! { "password" => [Password::min(8).uncompromised()] },
    )
    .await;
    assert_eq!(
        bag.first("password"),
        Some("The given password has appeared in a data leak. Please choose a different password.")
    );
    assert!(
        passes(
            json!({"password": "correct horse"}),
            rules! { "password" => [Password::min(8).uncompromised()] }
        )
        .await
    );
}

// ----------------------------------------------------------------------
// Message resolvers
// ----------------------------------------------------------------------

struct Dutch;

impl MessageResolver for Dutch {
    fn get(&self, key: &str) -> Option<Value> {
        match key {
            "validation.required" => Some(json!(":Attribute is verplicht.")),
            "validation.attributes" => Some(json!({"email": "e-mailadres"})),
            "validation.custom" => Some(json!({"name": {"max": "Naam te lang"}})),
            "validation.values.type.cc" => Some(json!("creditcard")),
            _ => None,
        }
    }
}

#[tokio::test]
async fn message_resolvers_bound_in_the_container_translate_messages() {
    let (container, _guard) = container();
    container.instance_arc::<dyn MessageResolver>(Arc::new(Dutch));
    let mut validator = Validator::make(
        json!({"name": "abcdef", "type": "cc"}),
        rules! {
            "email" => "required",
            "name" => "max:3",
            "number" => "required_if:type,cc",
            "age" => "integer|min:18",
        },
    );
    let _ = validator.passes().await;
    let bag = validator.errors();
    assert_eq!(bag.first("email"), Some("E-mailadres is verplicht."));
    assert_eq!(bag.first("name"), Some("Naam te lang"));
    assert_eq!(
        bag.first("number"),
        Some("The number field is required when type is creditcard.")
    );
}

#[tokio::test]
async fn message_resolvers_can_be_registered_on_the_factory() {
    let _c = container();
    Validator::resolve_messages_using(Dutch);
    let bag = errors(json!({}), [("email", "required")]).await;
    assert_eq!(bag.first("email"), Some("E-mailadres is verplicht."));
}
