//! Validating HTTP requests and form requests.

mod common;

use common::container;
use illuminate_http::{HeaderMap, HttpException, Request, UploadedFile};
use illuminate_support::{Str, Value, json};
use illuminate_validation::{
    CustomAttributes, CustomMessages, FormRequest, Rules, ValidatesRequests, ValidationException,
    Validator, async_trait, rules, validate_form_request,
};

fn post(input: Value) -> Request {
    Request::create_with("/posts", "POST", input, HeaderMap::new())
}

#[tokio::test]
async fn requests_validate_their_input() {
    let _c = container();
    let request = post(json!({"title": "Hello", "body": "World", "extra": true}));
    let validated = request
        .validate(rules! { "title" => "required|max:255", "body" => "required" })
        .await
        .unwrap();
    assert_eq!(validated, json!({"title": "Hello", "body": "World"}));
    assert_eq!(request.validated(), validated);
    assert_eq!(request.safe().only(&["title"]), json!({"title": "Hello"}));
}

#[tokio::test]
async fn failed_request_validation_returns_a_validation_exception() {
    let _c = container();
    let request = post(json!({"title": ""}));
    let error = request.validate([("title", "required")]).await.unwrap_err();
    let exception = error.downcast_ref::<ValidationException>().unwrap();
    assert_eq!(
        exception.errors.first("title"),
        Some("The title field is required.")
    );
    assert_eq!(exception.error_bag, "default");
    assert_eq!(request.validated(), json!({}));

    let error = request
        .validate_with_bag("post", [("title", "required")])
        .await
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<ValidationException>()
            .unwrap()
            .error_bag,
        "post"
    );
}

#[tokio::test]
async fn requests_validate_with_custom_messages_and_attributes() {
    let _c = container();
    let request = post(json!({}));
    let error = request
        .validate_with(
            [("email", "required")],
            [("email.required", "We need your :attribute!")],
            [("email", "email address")],
        )
        .await
        .unwrap_err();
    let exception = error.downcast::<ValidationException>().unwrap();
    assert_eq!(
        exception.errors.first("email"),
        Some("We need your email address!")
    );
}

#[tokio::test]
async fn requests_validate_uploaded_files() {
    let _c = container();
    let request = post(json!({"name": "Taylor"}));
    request.attach_file("avatar", UploadedFile::fake().create("me.jpg", 1));
    request.attach_file("photos", UploadedFile::fake().create("a.jpg", 10));
    request.attach_file("photos", UploadedFile::fake().create("b.pdf", 10));

    let error = request
        .validate(
            rules! { "name" => "required", "avatar" => "required|image", "photos.*" => "image" },
        )
        .await
        .unwrap_err();
    let exception = error.downcast::<ValidationException>().unwrap();
    assert_eq!(exception.errors.keys(), vec!["photos.1"]);

    let validated = request
        .validate(rules! { "name" => "required", "avatar" => "required|image|max:1" })
        .await
        .unwrap();
    assert_eq!(validated, json!({"name": "Taylor"}));
    assert_eq!(
        request
            .safe()
            .file("avatar")
            .unwrap()
            .client_original_name(),
        "me.jpg"
    );
}

#[tokio::test]
async fn request_validators_can_be_customized() {
    let _c = container();
    let request = post(json!({"games": 150}));
    let mut validator =
        request
            .validator([("games", "integer")])
            .sometimes("reason", "required", |input, _| {
                input["games"].as_i64() > Some(100)
            });
    assert!(validator.fails().await);
}

// ----------------------------------------------------------------------
// Form requests
// ----------------------------------------------------------------------

#[derive(Default)]
struct StorePostRequest;

#[async_trait]
impl FormRequest for StorePostRequest {
    fn rules(&self, _request: &Request) -> Rules {
        rules! {
            "title" => "required|max:255",
            "slug" => "required|alpha_dash",
            "body" => "required",
        }
    }

    async fn authorize(&self, request: &Request) -> bool {
        request.input("author") != json!("banned")
    }

    fn messages(&self) -> CustomMessages {
        [("title.required", "A title is required")].into()
    }

    fn attributes(&self) -> CustomAttributes {
        [("body", "post body")].into()
    }

    fn prepare_for_validation(&self, request: &Request) {
        let slug = Str::slug(&request.string("title"));
        request.merge(json!({ "slug": slug }));
    }

    fn after(&self, validator: &mut Validator, _request: &Request) {
        if validator.data()["title"] == "Forbidden" {
            validator
                .errors_mut()
                .add("title", "That title is not allowed.");
        }
    }

    fn passed_validation(&self, request: &Request) {
        request.set_attribute("passed", true);
    }

    fn error_bag(&self) -> String {
        "post".to_string()
    }

    fn redirect_to(&self, _request: &Request) -> Option<String> {
        Some("/posts/create".to_string())
    }
}

impl StorePostRequest {
    fn kind(&self) -> &'static str {
        "post"
    }
}

#[tokio::test]
async fn form_requests_validate_and_expose_the_data() {
    let _c = container();
    let request = post(json!({"title": "Hello World", "body": "Content", "author": "taylor"}));
    let validated = validate_form_request::<StorePostRequest>(&request)
        .await
        .unwrap();

    assert_eq!(
        validated.validated(),
        &json!({"title": "Hello World", "slug": "hello-world", "body": "Content"})
    );
    assert_eq!(validated.validated_key("slug"), json!("hello-world"));
    assert_eq!(
        validated.safe().except(&["body"]),
        json!({"title": "Hello World", "slug": "hello-world"})
    );
    assert_eq!(validated.input("author"), json!("taylor"));
    assert_eq!(validated.kind(), "post");
    assert_eq!(request.attribute("passed"), json!(true));
    assert_eq!(request.validated(), validated.validated().clone());
}

#[tokio::test]
async fn form_requests_report_failures_with_their_configuration() {
    let _c = container();
    let request = post(json!({"author": "taylor"}));
    let error = validate_form_request::<StorePostRequest>(&request)
        .await
        .err()
        .unwrap();
    let exception = error.downcast::<ValidationException>().unwrap();
    assert_eq!(exception.errors.first("title"), Some("A title is required"));
    assert_eq!(
        exception.errors.first("body"),
        Some("The post body field is required.")
    );
    assert_eq!(exception.error_bag, "post");
    assert_eq!(exception.redirect_to.as_deref(), Some("/posts/create"));

    let request = post(json!({"title": "Forbidden", "body": "x"}));
    let error = validate_form_request::<StorePostRequest>(&request)
        .await
        .err()
        .unwrap();
    let exception = error.downcast::<ValidationException>().unwrap();
    assert_eq!(
        exception.errors.get("title"),
        vec!["That title is not allowed."]
    );
}

#[tokio::test]
async fn unauthorized_form_requests_are_forbidden() {
    let _c = container();
    let request = post(json!({"title": "Hi", "body": "x", "author": "banned"}));
    let error = validate_form_request::<StorePostRequest>(&request)
        .await
        .err()
        .unwrap();
    let exception = error.downcast::<HttpException>().unwrap();
    assert_eq!(exception.status, 403);
    assert_eq!(exception.message(), "This action is unauthorized.");
}

#[derive(Default)]
struct StopsEarly;

impl FormRequest for StopsEarly {
    fn rules(&self, _request: &Request) -> Rules {
        rules! { "a" => "required", "b" => "required" }
    }

    fn stop_on_first_failure(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn form_requests_can_stop_on_the_first_failure() {
    let _c = container();
    let error = validate_form_request::<StopsEarly>(&post(json!({})))
        .await
        .err()
        .unwrap();
    let exception = error.downcast::<ValidationException>().unwrap();
    assert_eq!(exception.errors.keys(), vec!["a"]);
}
