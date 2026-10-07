//! Validating precognitive requests (Laravel Precognition).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::container;
use illuminate_http::exceptions::HttpResponseException;
use illuminate_http::{HeaderMap, Request, Response};
use illuminate_support::{Error, Result, Value, json};
use illuminate_validation::{
    FormRequest, Rules, ValidatesRequests, ValidationException, Validator, rules,
    validate_form_request, validate_form_request_with,
};

fn post(input: Value) -> Request {
    Request::create_with("/users", "POST", input, HeaderMap::new())
}

/// A request the `precognitive` middleware accepted.
fn precognitive(input: Value, validate_only: Option<&str>) -> Request {
    let request = post(input);
    request.set_header("Precognition", "true");
    if let Some(validate_only) = validate_only {
        request.set_header("Precognition-Validate-Only", validate_only);
    }
    request.set_attribute("precognitive", true);
    request
}

fn errors_of(error: &Error) -> Vec<String> {
    let exception = error
        .downcast_ref::<ValidationException>()
        .unwrap_or_else(|| panic!("expected a validation exception, got: {error}"));
    exception
        .errors
        .keys()
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn success_response(error: &Error) -> Response {
    error
        .downcast_ref::<HttpResponseException>()
        .and_then(HttpResponseException::take_response)
        .unwrap_or_else(|| panic!("expected a precognition response, got: {error}"))
}

fn registration_rules() -> Rules {
    rules! {
        "name" => "required|string",
        "email" => "required|email",
        "password" => "required|min:8",
    }
}

#[tokio::test]
async fn precognitive_requests_validate_only_the_listed_attributes() {
    let _c = container();
    let request = precognitive(json!({"name": "", "email": "nope"}), Some("email"));

    let error = request.validate(registration_rules()).await.unwrap_err();
    assert_eq!(errors_of(&error), ["email"]);
}

#[tokio::test]
async fn passing_the_listed_attributes_ends_the_request_successfully() {
    let _c = container();
    let request = precognitive(json!({"email": "taylor@laravel.com"}), Some("email,name"));

    let error = request
        .validate(
            rules! { "email" => "required|email", "name" => "string", "password" => "required" },
        )
        .await
        .unwrap_err();
    let response = success_response(&error);
    assert_eq!(response.status_code(), 204);
    assert_eq!(
        response.header("Precognition-Success").as_deref(),
        Some("true")
    );
    // Validation didn't "complete", so nothing was stored on the request.
    assert_eq!(request.validated(), json!({}));
}

#[tokio::test]
async fn precognitive_requests_without_the_header_validate_everything() {
    let _c = container();
    let request = precognitive(json!({"name": "", "email": "nope"}), None);
    let error = request.validate(registration_rules()).await.unwrap_err();
    assert_eq!(errors_of(&error), ["name", "email", "password"]);

    let request = precognitive(json!({"name": "Taylor"}), None);
    let validated = request.validate([("name", "required")]).await.unwrap();
    assert_eq!(validated, json!({"name": "Taylor"}));
}

#[tokio::test]
async fn the_header_is_ignored_unless_the_request_is_precognitive() {
    let _c = container();
    let request = post(json!({"name": "", "email": "nope"}));
    request.set_header("Precognition", "true");
    request.set_header("Precognition-Validate-Only", "email");

    let error = request.validate(registration_rules()).await.unwrap_err();
    assert_eq!(errors_of(&error), ["name", "email", "password"]);
}

#[tokio::test]
async fn validate_only_supports_wildcards() {
    let _c = container();
    let input = json!({
        "team": "",
        "users": [{"name": "", "email": "nope"}, {"name": "", "email": "also-nope"}],
    });
    let rules = || {
        rules! {
            "team" => "required",
            "users.*.name" => "required",
            "users.*.email" => "email",
        }
    };

    let error = precognitive(input.clone(), Some("users.*.email"))
        .validate(rules())
        .await
        .unwrap_err();
    assert_eq!(errors_of(&error), ["users.0.email", "users.1.email"]);

    let error = precognitive(input.clone(), Some("users.1.email,team"))
        .validate(rules())
        .await
        .unwrap_err();
    assert_eq!(errors_of(&error), ["team", "users.1.email"]);

    let error = precognitive(input, Some("users.*.*"))
        .validate(rules())
        .await
        .unwrap_err();
    assert_eq!(
        errors_of(&error),
        [
            "users.0.name",
            "users.1.name",
            "users.0.email",
            "users.1.email"
        ]
    );
}

#[tokio::test]
async fn the_error_bag_is_kept_for_precognitive_failures() {
    let _c = container();
    let request = precognitive(json!({}), Some("name"));
    let error = request
        .validate_with_bag("register", registration_rules())
        .await
        .unwrap_err();
    let exception = error.downcast_ref::<ValidationException>().unwrap();
    assert_eq!(exception.error_bag, "register");
    assert_eq!(errors_of(&error), ["name"]);
}

#[derive(Default)]
struct StoreUserRequest {
    passed: Arc<AtomicUsize>,
}

impl FormRequest for StoreUserRequest {
    fn rules(&self, request: &Request) -> Rules {
        rules! {
            "name" => "required",
            "email" => "required|email",
            "password" => if request.is_precognitive() { "required|min:8" } else { "required|min:8|confirmed" },
        }
    }

    fn with_validator(&self, validator: Validator, _request: &Request) -> Validator {
        validator.sometimes("reason", "required", |input, _| input["name"] == "Taylor")
    }

    fn passed_validation(&self, _request: &Request) {
        self.passed.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn form_requests_may_customize_rules_for_precognitive_requests() {
    let _c = container();
    let input = json!({"name": "Jess", "email": "jess@laravel.com", "password": "secret-password"});

    let validated = validate_form_request::<StoreUserRequest>(&precognitive(input.clone(), None))
        .await
        .unwrap();
    assert_eq!(validated.validated()["password"], "secret-password");

    let error = validate_form_request::<StoreUserRequest>(&post(input))
        .await
        .err()
        .unwrap();
    assert_eq!(errors_of(&error), ["password"]);
}

#[tokio::test]
async fn form_requests_validate_only_the_listed_attributes() {
    let _c = container();
    let passed = Arc::new(AtomicUsize::new(0));
    let form = || StoreUserRequest {
        passed: passed.clone(),
    };

    // Rules added by `with_validator` are filtered too.
    let request = precognitive(json!({"name": "Taylor", "email": "nope"}), Some("email"));
    let error = validate_form_request_with(form(), &request)
        .await
        .err()
        .unwrap();
    assert_eq!(errors_of(&error), ["email"]);

    let request = precognitive(
        json!({"name": "Taylor", "email": "taylor@laravel.com"}),
        Some("email,name"),
    );
    let error = validate_form_request_with(form(), &request)
        .await
        .err()
        .unwrap();
    assert_eq!(success_response(&error).status_code(), 204);
    assert_eq!(passed.load(Ordering::SeqCst), 0);
    assert_eq!(request.validated(), json!({}));

    // A precognitive request without the header validates everything.
    let request = precognitive(
        json!({"name": "Taylor", "email": "taylor@laravel.com"}),
        None,
    );
    let error = validate_form_request_with(form(), &request)
        .await
        .err()
        .unwrap();
    assert_eq!(errors_of(&error), ["password", "reason"]);
}

#[tokio::test]
async fn validators_can_filter_their_rules() -> Result<()> {
    let _c = container();
    let mut validator = Validator::make(
        json!({"name": "", "items": [{"sku": ""}, {"sku": "A1"}]}),
        rules! { "name" => "required", "items.*.sku" => "required" },
    )
    .sometimes("notes", "required", |_, _| true)
    .filter_rules(|attribute| attribute.starts_with("items."));

    assert!(validator.fails().await);
    assert_eq!(validator.errors().keys(), vec!["items.0.sku"]);

    let mut validator = Validator::make(
        json!({"name": "Taylor", "age": "x"}),
        [("name", "required"), ("age", "integer")],
    )
    .filter_rules(|attribute| attribute != "age");
    assert_eq!(validator.try_validate().await?, json!({"name": "Taylor"}));
    Ok(())
}
