use illuminate_foundation::Application;
use illuminate_foundation::testing::{AssertableJson, TestApp};
use illuminate_http::{Cookie, Request, Response, cookie};
use illuminate_routing::{Redirect, Route, back};
use illuminate_support::{Value, json};
use illuminate_validation::{Validator, rules};

struct UserController;

impl UserController {
    async fn show() -> &'static str {
        "User"
    }
}

fn test_app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let builder = Application::configure_detached(dir.path()).with_routing(|routing| {
        routing.web(|| {
            Route::get("/status/{code}", |request: Request| async move {
                let code: u16 = request.route("code").unwrap_or_default().parse().unwrap_or(200);
                Response::make("", code)
            });
            Route::get("/headers", || async {
                Response::new("ok").with_header("Cache-Control", "max-age=3600, public")
            });
            Route::get("/cookies", || async {
                Response::new("ok")
                    .cookie(Cookie::new("session_cookie", "value"))
                    .cookie(cookie("remember", "1", 60))
                    .cookie(Cookie::forget("old"))
            });
            Route::get("/html", || async {
                "<ul>\n  <li>Taylor &amp; Abigail</li>\n  <li><b>James</b></li>\n</ul>"
            });
            Route::get("/empty", || async { Response::no_content() });
            Route::post("/back", || async { back() });
            Route::post("/posts", |request: Request| async move {
                Validator::make(request.all(), rules! { "title" => "required", "body" => "required" })
                    .validate()
                    .await?;
                Ok::<_, illuminate_support::Error>(back())
            });
            Route::post("/old-input", || async { back().with_input(json!({"name": "Taylor"})) });
            Route::get("/somewhere", || async { Response::redirect("/users?page=2&sort=name") });
            Route::get("/unsubscribe/{user}", || async { "Unsubscribed" }).name("unsubscribe");
            Route::get("/signed", || async { Redirect::signed_route("unsubscribe", json!({"user": 1})) });
            Route::get("/unsigned", || async { Response::redirect("/unsubscribe/1") });
            Route::get("/users/{user}", UserController::show);
            Route::get("/go-to-user", || async { Response::redirect("/users/5") });
            Route::get("/stream", || async {
                Response::stream(futures::stream::iter(vec![
                    Ok::<_, illuminate_support::Error>("Hello, ".into()),
                    Ok("World".into()),
                ]))
            });
            Route::get("/stream-json", || async {
                Response::stream(futures::stream::iter(vec![
                    Ok::<_, illuminate_support::Error>(r#"{"data":"#.into()),
                    Ok("[1,2]}".into()),
                ]))
            });
            Route::get("/json/user", || async {
                json!({
                    "id": 1,
                    "name": "Victoria Faith",
                    "email": "victoria@gmail.com",
                    "status": "active",
                    "score": 9.5,
                    "admin": false,
                    "deleted_at": null,
                    "tags": ["laravel", "rust"],
                    "meta": {"created": "2024", "source": "web"}
                })
            });
            Route::get("/json/users", || async {
                json!({
                    "meta": {"total": 3},
                    "users": [
                        {"id": 1, "name": "Victoria Faith", "email": "victoria@gmail.com"},
                        {"id": 2, "name": "Taylor Otwell", "email": "taylor@laravel.com"},
                        {"id": 3, "name": "Abigail Otwell", "email": "abigail@laravel.com"}
                    ]
                })
            });
            Route::get("/json/list", || async {
                json!([{"id": 1, "name": "Taylor"}, {"id": 2, "name": "Abigail"}, {"id": 3, "name": "James"}])
            });
            Route::post("/api/users", |request: Request| async move {
                Validator::make(request.all(), rules! { "name" => "required", "email" => "required|email" })
                    .validate()
                    .await?;
                Ok::<_, illuminate_support::Error>(json!({"created": true}))
            });
        });
    });
    (TestApp::new(builder), dir)
}

// ----------------------------------------------------------------------
// Status codes
// ----------------------------------------------------------------------

#[tokio::test]
async fn status_code_helpers() {
    let (mut app, _dir) = test_app();

    app.get("/status/304").await.assert_not_modified();
    app.get("/status/307").await.assert_temporary_redirect();
    app.get("/status/308").await.assert_permanent_redirect();
    app.get("/status/406")
        .await
        .assert_not_acceptable()
        .assert_client_error();
    app.get("/status/408").await.assert_request_timeout();
    app.get("/status/415").await.assert_unsupported_media_type();
    app.get("/status/424").await.assert_failed_dependency();
    app.get("/empty").await.assert_no_content();
}

#[tokio::test]
#[should_panic(expected = "Expected response status code [424] but received 200.")]
async fn status_code_helpers_fail_with_laravels_message() {
    let (mut app, _dir) = test_app();
    app.get("/status/200").await.assert_failed_dependency();
}

#[tokio::test]
#[should_panic(expected = "Expected response status code [>=500, < 600] but received 404.")]
async fn server_error_assertions_fail_with_laravels_message() {
    let (mut app, _dir) = test_app();
    app.get("/status/404").await.assert_server_error();
}

// ----------------------------------------------------------------------
// Headers & cookies
// ----------------------------------------------------------------------

#[tokio::test]
async fn headers_can_be_asserted_to_contain_a_value() {
    let (mut app, _dir) = test_app();
    app.get("/headers")
        .await
        .assert_header_contains("Cache-Control", "max-age=3600")
        .assert_header_contains("cache-control", "public");
}

#[tokio::test]
#[should_panic(
    expected = "Header [Cache-Control] was found, but [max-age=3600, public] does not contain [private]."
)]
async fn header_contains_fails_when_the_value_is_missing() {
    let (mut app, _dir) = test_app();
    app.get("/headers")
        .await
        .assert_header_contains("Cache-Control", "private");
}

#[tokio::test]
#[should_panic(expected = "Header [X-Missing] not present on response.")]
async fn header_contains_fails_when_the_header_is_missing() {
    let (mut app, _dir) = test_app();
    app.get("/headers")
        .await
        .assert_header_contains("X-Missing", "anything");
}

#[tokio::test]
async fn cookies_can_be_asserted_not_to_be_expired() {
    let (mut app, _dir) = test_app();
    app.get("/cookies")
        .await
        .assert_cookie_not_expired("session_cookie")
        .assert_cookie_not_expired("remember")
        .assert_cookie_expired("old");
}

#[tokio::test]
#[should_panic(expected = "Cookie [old] is expired, it expired at")]
async fn cookie_not_expired_fails_for_expired_cookies() {
    let (mut app, _dir) = test_app();
    app.get("/cookies").await.assert_cookie_not_expired("old");
}

#[tokio::test]
#[should_panic(expected = "Cookie [missing] not present on response.")]
async fn cookie_not_expired_fails_for_missing_cookies() {
    let (mut app, _dir) = test_app();
    app.get("/cookies")
        .await
        .assert_cookie_not_expired("missing");
}

// ----------------------------------------------------------------------
// Content
// ----------------------------------------------------------------------

#[tokio::test]
async fn html_and_text_can_be_seen_in_order() {
    let (mut app, _dir) = test_app();
    app.get("/html")
        .await
        .assert_see_html("<li><b>James</b></li>")
        .assert_dont_see_html("<li>James</li>")
        .assert_see_html_in_order(&["<li>Taylor", "<b>James</b>"])
        .assert_see_in_order(&["Taylor & Abigail", "James"])
        .assert_see_text("Taylor & Abigail James")
        .assert_see_text_in_order(&["Taylor", "Abigail", "James"])
        .assert_dont_see_text("Dries");
}

#[tokio::test]
#[should_panic(expected = "contains \"<li>Taylor\" in specified order.")]
async fn see_html_in_order_fails_when_out_of_order() {
    let (mut app, _dir) = test_app();
    app.get("/html")
        .await
        .assert_see_html_in_order(&["<b>James</b>", "<li>Taylor"]);
}

#[tokio::test]
#[should_panic(expected = "contains \"Taylor\" in specified order.")]
async fn see_text_in_order_fails_when_out_of_order() {
    let (mut app, _dir) = test_app();
    app.get("/html")
        .await
        .assert_see_text_in_order(&["James", "Taylor"]);
}

#[tokio::test]
#[should_panic(expected = "Failed asserting that the response does not contain [<b>James</b>].")]
async fn dont_see_html_fails_when_present() {
    let (mut app, _dir) = test_app();
    app.get("/html").await.assert_dont_see_html("<b>James</b>");
}

// ----------------------------------------------------------------------
// Redirects
// ----------------------------------------------------------------------

#[tokio::test]
async fn redirects_can_be_asserted_to_contain_a_uri() {
    let (mut app, _dir) = test_app();
    app.get("/somewhere")
        .await
        .assert_redirect_contains("/users")
        .assert_redirect_contains("sort=name");
}

#[tokio::test]
#[should_panic(expected = "does not contain [/posts].")]
async fn redirect_contains_fails_for_other_locations() {
    let (mut app, _dir) = test_app();
    app.get("/somewhere")
        .await
        .assert_redirect_contains("/posts");
}

#[tokio::test]
async fn redirects_back_can_be_asserted() {
    let (mut app, _dir) = test_app();

    app.from("/form")
        .post("/back", json!({}))
        .await
        .assert_redirect_back()
        .assert_redirect_back_without_errors();

    app.from("/form")
        .post("/posts", json!({"title": "Hello"}))
        .await
        .assert_redirect_back()
        .assert_redirect_back_with_errors(&["body"])
        .assert_redirect_back_with_errors(&[]);
}

#[tokio::test]
#[should_panic(expected = "matches the expected location [http://localhost/form]")]
async fn redirect_back_fails_for_other_locations() {
    let (mut app, _dir) = test_app();
    app.from("/form");
    app.get("/somewhere").await.assert_redirect_back();
}

#[tokio::test]
#[should_panic(expected = "Session has unexpected errors")]
async fn redirect_back_without_errors_fails_when_there_are_errors() {
    let (mut app, _dir) = test_app();
    app.from("/form")
        .post("/posts", json!({}))
        .await
        .assert_redirect_back_without_errors();
}

#[tokio::test]
async fn redirects_to_signed_routes_can_be_asserted() {
    let (mut app, _dir) = test_app();
    app.get("/signed")
        .await
        .assert_redirect_to_signed_route(None, ())
        .assert_redirect_to_signed_route("unsubscribe", json!({"user": 1}));
}

#[tokio::test]
#[should_panic(expected = "The response is not a redirect to a signed route.")]
async fn redirect_to_signed_route_fails_without_a_signature() {
    let (mut app, _dir) = test_app();
    app.get("/unsigned")
        .await
        .assert_redirect_to_signed_route(None, ());
}

#[tokio::test]
#[should_panic(expected = "matches the expected route [http://localhost/unsubscribe/2]")]
async fn redirect_to_signed_route_fails_for_other_routes() {
    let (mut app, _dir) = test_app();
    app.get("/signed")
        .await
        .assert_redirect_to_signed_route("unsubscribe", json!({"user": 2}));
}

#[tokio::test]
async fn redirects_to_controller_actions_can_be_asserted() {
    let (mut app, _dir) = test_app();
    app.get("/go-to-user")
        .await
        .assert_redirect_to_action("UserController@show", json!({"user": 5}));
}

#[tokio::test]
#[should_panic(expected = "Action PostController@index not defined.")]
async fn redirect_to_action_fails_for_unknown_actions() {
    let (mut app, _dir) = test_app();
    app.get("/go-to-user")
        .await
        .assert_redirect_to_action("PostController@index", ());
}

// ----------------------------------------------------------------------
// Streamed responses
// ----------------------------------------------------------------------

#[tokio::test]
async fn streamed_responses_can_be_asserted() {
    let (mut app, _dir) = test_app();

    let response = app.get("/stream").await;
    response.assert_ok().assert_streamed();
    response
        .assert_streamed_content("Hello, World")
        .await
        .assert_ok();
    assert_eq!(
        response.streamed_content().await,
        "Hello, World",
        "the content is remembered"
    );

    app.get("/html").await.assert_not_streamed();

    let response = app.get("/stream-json").await;
    response
        .assert_streamed_json_content(json!({"data": [1, 2]}))
        .await;
    response.assert_json_path("data.1", 2);
}

#[tokio::test]
#[should_panic(expected = "Expected the response to be streamed, but it wasn't.")]
async fn assert_streamed_fails_for_buffered_responses() {
    let (mut app, _dir) = test_app();
    app.get("/html").await.assert_streamed();
}

#[tokio::test]
#[should_panic(expected = "Response was unexpectedly streamed.")]
async fn assert_not_streamed_fails_for_streamed_responses() {
    let (mut app, _dir) = test_app();
    app.get("/stream").await.assert_not_streamed();
}

#[tokio::test]
#[should_panic(
    expected = "Failed asserting that the streamed content [Hello, World] is identical to [Goodbye]."
)]
async fn assert_streamed_content_fails_for_other_content() {
    let (mut app, _dir) = test_app();
    app.get("/stream")
        .await
        .assert_streamed_content("Goodbye")
        .await;
}

#[tokio::test]
#[should_panic(expected = "The response is not a streamed response.")]
async fn streamed_content_fails_for_buffered_responses() {
    let (mut app, _dir) = test_app();
    app.get("/html").await.streamed_content().await;
}

// ----------------------------------------------------------------------
// Session & validation
// ----------------------------------------------------------------------

#[tokio::test]
async fn missing_old_input_can_be_asserted() {
    let (mut app, _dir) = test_app();
    app.from("/form")
        .post("/old-input", json!({}))
        .await
        .assert_session_has_input("name", Some(json!("Taylor")))
        .assert_session_missing_input("email");
}

#[tokio::test]
#[should_panic(expected = "Session has unexpected key [name].")]
async fn session_missing_input_fails_when_present() {
    let (mut app, _dir) = test_app();
    app.from("/form")
        .post("/old-input", json!({}))
        .await
        .assert_session_missing_input("name");
}

#[tokio::test]
async fn only_invalid_checks_session_errors() {
    let (mut app, _dir) = test_app();
    app.from("/form")
        .post("/posts", json!({}))
        .await
        .assert_only_invalid(&["title", "body"]);
}

#[tokio::test]
#[should_panic(expected = "Response has unexpected validation errors: 'body'")]
async fn only_invalid_fails_for_unexpected_session_errors() {
    let (mut app, _dir) = test_app();
    app.from("/form")
        .post("/posts", json!({}))
        .await
        .assert_only_invalid(&["title"]);
}

#[tokio::test]
async fn json_validation_errors_can_be_asserted() {
    let (mut app, _dir) = test_app();
    app.post_json("/api/users", json!({"name": "Taylor", "email": "nope"}))
        .await
        .assert_unprocessable()
        .assert_json_validation_error_for("email")
        .assert_only_json_validation_errors(&["email"])
        .assert_only_invalid(&["email"]);
}

#[tokio::test]
#[should_panic(expected = "Failed to find a validation error in the response for key: 'name'")]
async fn json_validation_error_for_fails_for_valid_keys() {
    let (mut app, _dir) = test_app();
    app.post_json("/api/users", json!({"name": "Taylor"}))
        .await
        .assert_json_validation_error_for("name");
}

#[tokio::test]
#[should_panic(expected = "Response has unexpected validation errors: 'email'")]
async fn only_json_validation_errors_fails_for_unexpected_errors() {
    let (mut app, _dir) = test_app();
    app.post_json("/api/users", json!({"title": "Hi"}))
        .await
        .assert_json_validation_errors(&["name", "email"])
        .assert_only_json_validation_errors(&["name"]);
}

// ----------------------------------------------------------------------
// JSON
// ----------------------------------------------------------------------

#[tokio::test]
async fn json_paths_can_be_asserted() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/users")
        .await
        .assert_json_paths(json!({"meta.total": 3, "users.0.name": "Victoria Faith"}))
        .assert_json_path_with("users.1.email", |email| {
            email
                .as_str()
                .is_some_and(|email| email.ends_with("@laravel.com"))
        })
        .assert_json_missing_paths(&["meta.page", "users.0.password", "users.*.password"])
        .assert_json_path_canonicalizing("users.*.id", json!([3, 1, 2]))
        .assert_json_paths_canonicalizing(
            json!({"users.*.name": ["Taylor Otwell", "Abigail Otwell", "Victoria Faith"]}),
        );
}

#[tokio::test]
#[should_panic(expected = "Found unexpected path [users.*.email] within the response JSON.")]
async fn json_missing_paths_supports_wildcards() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/users")
        .await
        .assert_json_missing_paths(&["users.*.email"]);
}

#[tokio::test]
#[should_panic(expected = "fulfills the expectations defined by the closure.")]
async fn json_path_with_fails_when_the_closure_returns_false() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/users")
        .await
        .assert_json_path_with("meta.total", |total| total == &json!(4));
}

#[tokio::test]
#[should_panic(expected = "ignoring order.")]
async fn json_path_canonicalizing_fails_for_other_values() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/users")
        .await
        .assert_json_path_canonicalizing("users.*.id", json!([1, 2]));
}

#[tokio::test]
async fn json_fragments_and_missing_fragments() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/users")
        .await
        .assert_json_fragments([json!({"id": 2}), json!({"name": "Abigail Otwell"})])
        .assert_json_missing(json!({"name": "Dries Vints", "id": 4}))
        .assert_json_missing_exact(json!({"id": 1, "name": "Dries Vints"}));
}

#[tokio::test]
#[should_panic(expected = "Found unexpected JSON fragment:")]
async fn json_missing_fails_when_any_pair_is_found() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/users")
        .await
        .assert_json_missing(json!({"name": "Dries Vints", "id": 1}));
}

#[tokio::test]
#[should_panic(expected = "Found unexpected JSON fragment:")]
async fn json_missing_exact_fails_when_every_pair_is_found() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/users")
        .await
        .assert_json_missing_exact(json!({"id": 1, "name": "Victoria Faith"}));
}

#[tokio::test]
async fn similar_json_ignores_order() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/list").await.assert_similar_json(json!([
        {"name": "James", "id": 3},
        {"id": 1, "name": "Taylor"},
        {"name": "Abigail", "id": 2}
    ]));
}

#[tokio::test]
#[should_panic(expected = "Failed asserting that two JSON values are similar.")]
async fn similar_json_fails_for_different_json() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/list")
        .await
        .assert_similar_json(json!([{"id": 1, "name": "Taylor"}]));
}

#[tokio::test]
async fn exact_json_structures_can_be_asserted() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/users")
        .await
        .assert_exact_json_structure(
            json!({"meta": ["total"], "users": {"*": ["id", "name", "email"]}}),
        );
    app.get_json("/json/list")
        .await
        .assert_exact_json_structure(json!({"*": ["id", "name"]}));
}

#[tokio::test]
#[should_panic(
    expected = "The JSON does not exactly match the expected structure: Expected exactly the keys [id, name] but found [email, id, name]."
)]
async fn exact_json_structure_fails_for_extra_keys() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/users")
        .await
        .assert_exact_json_structure(json!({"meta": ["total"], "users": {"*": ["id", "name"]}}));
}

// ----------------------------------------------------------------------
// Fluent JSON
// ----------------------------------------------------------------------

#[tokio::test]
async fn fluent_json_assertions() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/user").await.assert_json_fluent(|json| {
        json.where_("id", 1)
            .where_("name", "Victoria Faith")
            .where_fn("email", |email| {
                email
                    .as_str()
                    .is_some_and(|email| email.ends_with("@gmail.com"))
            })
            .where_not("status", "pending")
            .where_not_fn("score", |score| score.as_f64() > Some(10.0))
            .where_null("deleted_at")
            .where_not_null("tags")
            .where_type("score", "double")
            .where_type("admin", "boolean|null")
            .where_type("tags", ["array"])
            .where_contains("tags", "rust")
            .where_("meta", json!({"source": "web", "created": "2024"}))
            .missing("password")
    });
}

#[tokio::test]
async fn fluent_json_has_and_missing() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/user").await.assert_json_fluent(|json| {
        json.has_all(["id", "name", "email"])
            .has_any(["status", "nickname"])
            .missing_all(["password", "token"])
            .where_all(json!({"admin": false, "deleted_at": null}))
            .where_all_type(
                json!({"score": "double", "tags": "array", "meta.created": ["string", "null"]}),
            )
            .has_count("tags", 2)
            .has_scoped("meta", |meta| meta.where_("created", "2024").has("source"))
            .etc()
    });
}

#[tokio::test]
async fn fluent_json_collections() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/users")
        .await
        .assert_json_fluent(|json| {
            json.has("meta")
                .has_with("users", 3, |user| {
                    user.where_("id", 1).where_("name", "Victoria Faith").etc()
                })
                .has_scoped("users", |users| {
                    users.count(3).count_between(1, 5).each(|user| {
                        user.where_type("id", "integer")
                            .where_type("name", "string")
                            .etc()
                    })
                })
                .has_scoped("users.1", |user| user.where_("id", 2).etc())
        });

    app.get_json("/json/list").await.assert_json_fluent(|json| {
        json.count(3)
            .first(|user| user.where_("id", 1).where_("name", "Taylor"))
            .where_contains("name", json!(["Abigail", "James"]))
    });
}

#[tokio::test]
#[should_panic(expected = "Unexpected properties were found on the root level.")]
async fn fluent_json_fails_for_untouched_properties() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/user")
        .await
        .assert_json_fluent(|json| json.where_("id", 1));
}

#[tokio::test]
#[should_panic(expected = "Unexpected properties were found in scope [users.0].")]
async fn fluent_json_scopes_check_interaction() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/users")
        .await
        .assert_json_fluent(|json| json.has_with("users", 3, |user| user.where_("id", 1)).etc());
}

#[tokio::test]
#[should_panic(
    expected = "Property [users.0.id] does not match the expected value.\nFailed asserting that 1 is identical to 2."
)]
async fn fluent_json_where_fails_for_other_values() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/users")
        .await
        .assert_json_fluent(|json| {
            json.has_with("users", None, |user| user.where_("id", 2).etc())
                .etc()
        });
}

#[tokio::test]
#[should_panic(expected = "Property [id] does not match the expected value.")]
async fn fluent_json_where_is_strict_about_types() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/user")
        .await
        .assert_json_fluent(|json| json.where_("id", "1").etc());
}

#[tokio::test]
#[should_panic(expected = "Property [users] does not have the expected size.")]
async fn fluent_json_has_count_fails_for_other_sizes() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/users")
        .await
        .assert_json_fluent(|json| json.has_count("users", 2).etc());
}

#[tokio::test]
#[should_panic(expected = "Property [email] was found while it was expected to be missing.")]
async fn fluent_json_missing_fails_when_present() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/user")
        .await
        .assert_json_fluent(|json| json.missing("email").etc());
}

#[tokio::test]
#[should_panic(expected = "Property [nickname] does not exist.")]
async fn fluent_json_has_fails_when_missing() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/user")
        .await
        .assert_json_fluent(|json| json.has("nickname").etc());
}

#[tokio::test]
#[should_panic(expected = "Property [id] is not of expected type [string|null].")]
async fn fluent_json_where_type_fails_for_other_types() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/user")
        .await
        .assert_json_fluent(|json| json.where_type("id", "string|null").etc());
}

#[tokio::test]
#[should_panic(
    expected = "Property [status] contains a value that should be missing: [status, active]"
)]
async fn fluent_json_where_not_fails_for_the_value() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/user")
        .await
        .assert_json_fluent(|json| json.where_not("status", "active").etc());
}

#[tokio::test]
#[should_panic(expected = "Property [email] was marked as invalid using a closure.")]
async fn fluent_json_where_fn_fails_when_the_closure_returns_false() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/user").await.assert_json_fluent(|json| {
        json.where_fn("email", |email| email == "taylor@laravel.com")
            .etc()
    });
}

#[tokio::test]
#[should_panic(expected = "None of properties [nickname, avatar] exist.")]
async fn fluent_json_has_any_fails_when_none_exist() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/user")
        .await
        .assert_json_fluent(|json| json.has_any(["nickname", "avatar"]).etc());
}

#[tokio::test]
#[should_panic(expected = "Property [tags] does not contain [php].")]
async fn fluent_json_where_contains_fails_for_missing_values() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/user")
        .await
        .assert_json_fluent(|json| json.where_contains("tags", "php").etc());
}

#[tokio::test]
#[should_panic(expected = "Property [name] is not scopeable.")]
async fn fluent_json_cannot_scope_onto_scalars() {
    let (mut app, _dir) = test_app();
    app.get_json("/json/user")
        .await
        .assert_json_fluent(|json| json.has_scoped("name", |name| name).etc());
}

#[test]
#[should_panic(
    expected = "Cannot scope directly onto the first element of the root level because it is empty."
)]
fn fluent_json_cannot_scope_onto_empty_lists() {
    AssertableJson::from_array(json!([])).first(|json| json);
}

#[test]
fn assertable_json_can_be_used_directly() {
    let json = AssertableJson::from_array(json!({"name": "Taylor", "roles": ["admin"]}))
        .where_("name", "Taylor")
        .when(true, |json| json.has_count("roles", 1))
        .unless(true, |json| json.missing("roles"));
    assert_eq!(json.get("roles.0"), json!("admin"));
    assert_eq!(json.path(), None);
    json.interacted();

    let value: Value = AssertableJson::from_array(json!([1, 2]))
        .count(2)
        .to_array();
    assert_eq!(value, json!([1, 2]));
}

#[tokio::test]
#[should_panic(expected = "Invalid JSON was returned from the route.")]
async fn json_validation_error_for_requires_json() {
    let (mut app, _dir) = test_app();
    app.get("/html")
        .await
        .assert_json_validation_error_for("name");
}

#[test]
#[should_panic(
    expected = "Cannot scope directly onto each element of property [tags] because it is empty."
)]
fn fluent_json_cannot_iterate_empty_properties() {
    AssertableJson::from_array(json!({"tags": []})).has_scoped("tags", |tags| tags.each(|tag| tag));
}

#[test]
#[should_panic(expected = "Property [deleted_at] should not be null.")]
fn fluent_json_where_not_null_fails_for_null() {
    AssertableJson::from_array(json!({"deleted_at": null})).where_not_null("deleted_at");
}

#[test]
#[should_panic(expected = "Property [name] should be null.")]
fn fluent_json_where_null_fails_for_values() {
    AssertableJson::from_array(json!({"name": "Taylor"})).where_null("name");
}

#[test]
#[should_panic(
    expected = "Root level does not have the expected size.\nFailed asserting that actual size 2 matches expected size 3."
)]
fn fluent_json_count_fails_at_the_root_level() {
    AssertableJson::from_array(json!([1, 2])).count(3);
}

#[test]
#[should_panic(expected = "Property [users] size is not less than or equal to [1].")]
fn fluent_json_count_between_fails_for_large_collections() {
    AssertableJson::from_array(json!({"users": [1, 2]}))
        .has_scoped("users", |users| users.count_between(0, 1).etc());
}

#[test]
#[should_panic(expected = "Header [Precognition-Success] not present on response.")]
fn successful_precognition_requires_the_success_header() {
    let response = illuminate_foundation::testing::TestResponse::new(
        illuminate_http::Response::no_content(),
        illuminate_http::Request::create("/", "POST"),
    );
    response.assert_successful_precognition();
}
