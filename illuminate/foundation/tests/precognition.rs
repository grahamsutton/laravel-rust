//! Laravel Precognition: predicting the outcome of a request.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use illuminate_foundation::Application;
use illuminate_foundation::testing::TestApp;
use illuminate_http::{HttpException, Middleware, Next, Request, Response, async_trait};
use illuminate_routing::{
    FromRequest, Input, Redirect, Route, RouteMiddleware, route_parameter_for,
};
use illuminate_session::RequestSessionExt;
use illuminate_support::{Result, Value, json};
use illuminate_validation::{FormRequest, Rules, Validated, ValidatesRequests, rules};

/// Counts how many times each handler body actually ran.
#[derive(Default)]
struct SideEffects {
    users_created: AtomicUsize,
    users_updated: AtomicUsize,
    teams_created: AtomicUsize,
    interactions: AtomicUsize,
}

impl SideEffects {
    fn get(counter: &AtomicUsize) -> usize {
        counter.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct StoreUserRequest;

impl FormRequest for StoreUserRequest {
    fn rules(&self, request: &Request) -> Rules {
        rules! {
            "name" => "required|string|max:255",
            "email" => "required|email",
            "password" => if request.is_precognitive() {
                "required|min:8"
            } else {
                "required|min:8|confirmed"
            },
        }
    }
}

#[derive(Default)]
struct UpdateUserRequest;

#[async_trait]
impl FormRequest for UpdateUserRequest {
    fn rules(&self, _request: &Request) -> Rules {
        rules! { "name" => "required|string" }
    }

    async fn authorize(&self, request: &Request) -> bool {
        request.route("user").as_deref() != Some("2")
    }
}

/// A "model" bound from the `{user}` route parameter.
struct User {
    id: u64,
}

#[async_trait]
impl FromRequest for User {
    async fn from_request(request: &Request) -> Result<Self> {
        match route_parameter_for(request, "user") {
            Some((id, _)) if id == "1" || id == "2" => Ok(User { id: id.parse()? }),
            _ => Err(HttpException::new(404).into()),
        }
    }
}

/// An extractor validating its input with `request.validate(...)`.
struct NewTeam {
    name: String,
}

#[async_trait]
impl FromRequest for NewTeam {
    async fn from_request(request: &Request) -> Result<Self> {
        let validated = request
            .validate(rules! { "name" => "required|min:3", "slug" => "required|alpha_dash" })
            .await?;
        Ok(NewTeam {
            name: validated["name"].as_str().unwrap_or_default().to_string(),
        })
    }
}

#[derive(serde::Deserialize)]
struct Search {
    term: String,
}

/// Counts "interactions", skipping precognitive requests (the docs'
/// "Managing Side-Effects" example).
struct InteractionMiddleware(Arc<SideEffects>);

#[async_trait]
impl Middleware for InteractionMiddleware {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        if !request.is_precognitive() {
            self.0.interactions.fetch_add(1, Ordering::SeqCst);
        }
        Ok(next.run(request).await)
    }
}

struct UserController;

impl UserController {
    async fn update(user: User, request: Validated<UpdateUserRequest>) -> String {
        current_effects()
            .users_updated
            .fetch_add(1, Ordering::SeqCst);
        format!(
            "Updated user {} to {}",
            user.id,
            request.validated()["name"]
        )
    }
}

fn current_effects() -> Arc<SideEffects> {
    illuminate_container::app::<SideEffects>()
}

fn app() -> (TestApp, Arc<SideEffects>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let effects = Arc::new(SideEffects::default());
    let shared = effects.clone();
    let builder = Application::configure_detached(dir.path()).with_routing(move |routing| {
        let effects = shared.clone();
        routing.web(move || {
            let created = effects.clone();
            Route::post("/users", move |request: Validated<StoreUserRequest>| {
                let created = created.clone();
                async move {
                    created.users_created.fetch_add(1, Ordering::SeqCst);
                    Response::json(request.validated()).with_status(201)
                }
            })
            .middleware("precognitive");

            Route::put("/users/{user}", UserController::update)
                .middleware("precognitive")
                .missing(|| async { Redirect::to("/users") });

            let teams = effects.clone();
            Route::post("/teams", move |team: NewTeam| {
                let teams = teams.clone();
                async move {
                    teams.teams_created.fetch_add(1, Ordering::SeqCst);
                    format!("Created {}", team.name)
                }
            })
            .middleware(vec![
                RouteMiddleware::named("precognitive"),
                RouteMiddleware::of(InteractionMiddleware(effects.clone())),
            ]);

            let teams = effects.clone();
            Route::post("/teams/inline", move |request: Request| {
                let teams = teams.clone();
                async move {
                    request.validate(rules! { "name" => "required" }).await?;
                    teams.teams_created.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, illuminate_support::Error>("Created")
                }
            })
            .middleware("precognitive");

            Route::get("/search", |Input(search): Input<Search>| async move {
                format!("Searching for {}", search.term)
            })
            .middleware("precognitive");

            Route::post("/session", |request: Request| async move {
                request.session().put("written", "by the handler");
                "Written"
            })
            .middleware(vec![
                RouteMiddleware::named("precognitive"),
                RouteMiddleware::of(|request: Request, next: Next| async move {
                    request.session().put("touched", "by middleware");
                    Ok(next.run(request).await)
                }),
            ]);
            Route::get("/session", |request: Request| async move {
                let session = request.session();
                format!("{} / {}", session.get("touched"), session.get("written"))
            });

            let plain = effects.clone();
            Route::post("/plain", move || {
                let plain = plain.clone();
                async move {
                    plain.users_created.fetch_add(1, Ordering::SeqCst);
                    "Plain"
                }
            });
        });
    });
    let app = TestApp::new(builder);
    app.app().instance_arc::<SideEffects>(effects.clone());
    (app, effects, dir)
}

fn valid_user() -> Value {
    json!({
        "name": "Taylor Otwell",
        "email": "taylor@laravel.com",
        "password": "secret-password",
    })
}

#[tokio::test]
async fn successful_precognitive_requests_skip_the_handler() {
    let (mut app, effects, _dir) = app();

    let response = app
        .with_precognition()
        .post_json("/users", valid_user())
        .await;

    response
        .assert_successful_precognition()
        .assert_header("Precognition", Some("true"))
        .assert_header("Vary", Some("Precognition"));
    assert_eq!(response.content(), "");
    assert_eq!(SideEffects::get(&effects.users_created), 0);

    // The real submission runs the handler (and the stricter rules).
    let mut submission = valid_user();
    submission["password_confirmation"] = json!("secret-password");
    let response = app.flush_headers().post_json("/users", submission).await;
    response
        .assert_created()
        .assert_header("Vary", Some("Precognition"))
        .assert_header_missing("Precognition")
        .assert_header_missing("Precognition-Success");
    assert_eq!(SideEffects::get(&effects.users_created), 1);
}

#[tokio::test]
async fn validation_failures_are_returned_as_usual() {
    let (mut app, effects, _dir) = app();

    let response = app
        .with_precognition()
        .post_json(
            "/users",
            json!({"name": "", "email": "not-an-email", "password": "short"}),
        )
        .await;

    response
        .assert_unprocessable()
        .assert_header("Precognition", Some("true"))
        .assert_header("Vary", Some("Precognition"))
        .assert_header_missing("Precognition-Success")
        .assert_json_validation_errors(&["name", "email", "password"])
        .assert_json_path(
            "errors.email.0",
            "The email field must be a valid email address.",
        )
        .assert_json_path(
            "errors.password.0",
            "The password field must be at least 8 characters.",
        );
    assert_eq!(
        response.json_path("message"),
        json!("The name field is required. (and 2 more errors)")
    );
    assert_eq!(SideEffects::get(&effects.users_created), 0);
}

#[tokio::test]
async fn rules_may_differ_for_precognitive_requests() {
    let (mut app, _effects, _dir) = app();

    // The password needn't be confirmed while predicting...
    app.with_precognition()
        .post_json("/users", valid_user())
        .await
        .assert_no_content();

    // ...but it must be on submission.
    app.flush_headers()
        .post_json("/users", valid_user())
        .await
        .assert_unprocessable()
        .assert_json_validation_errors(&["password"])
        .assert_json_path(
            "errors.password.0",
            "The password field confirmation does not match.",
        );
}

#[tokio::test]
async fn validate_only_limits_validation_to_the_listed_fields() {
    let (mut app, effects, _dir) = app();
    app.with_precognition();

    // Only the email is validated, so the empty name doesn't matter.
    let response = app
        .with_header("Precognition-Validate-Only", "email")
        .post_json("/users", json!({"name": "", "email": "taylor@laravel.com"}))
        .await;
    response
        .assert_successful_precognition()
        .assert_header("Precognition", Some("true"));

    let response = app
        .with_header("Precognition-Validate-Only", "email,password")
        .post_json(
            "/users",
            json!({"name": "", "email": "nope", "password": "short"}),
        )
        .await;
    response
        .assert_unprocessable()
        .assert_header("Precognition", Some("true"))
        .assert_json_validation_errors(&["email", "password"])
        .assert_json_missing_validation_errors(&["name"]);

    assert_eq!(SideEffects::get(&effects.users_created), 0);
}

#[tokio::test]
async fn controller_handlers_bind_models_and_authorize() {
    let (mut app, effects, _dir) = app();
    app.with_precognition();

    app.put_json("/users/1", json!({"name": "Taylor"}))
        .await
        .assert_successful_precognition();

    app.put_json("/users/1", json!({}))
        .await
        .assert_unprocessable()
        .assert_header("Precognition", Some("true"))
        .assert_json_validation_errors(&["name"]);

    // Form request authorization still runs.
    app.put_json("/users/2", json!({"name": "Taylor"}))
        .await
        .assert_forbidden()
        .assert_header("Precognition", Some("true"));

    // Bindings that can't be resolved use the route's `missing` handler.
    app.put_json("/users/3", json!({"name": "Taylor"}))
        .await
        .assert_redirect(Some("/users"))
        .assert_header("Precognition", Some("true"))
        .assert_header_missing("Precognition-Success");

    assert_eq!(SideEffects::get(&effects.users_updated), 0);

    app.flush_headers()
        .put_json("/users/1", json!({"name": "Taylor"}))
        .await
        .assert_ok()
        .assert_content("Updated user 1 to \"Taylor\"");
    assert_eq!(SideEffects::get(&effects.users_updated), 1);
}

#[tokio::test]
async fn extractors_using_request_validation_are_predicted() {
    let (mut app, effects, _dir) = app();
    app.with_precognition();

    app.post_json("/teams", json!({"name": "La", "slug": "not a slug"}))
        .await
        .assert_unprocessable()
        .assert_json_validation_errors(&["name", "slug"]);

    app.with_header("Precognition-Validate-Only", "name")
        .post_json("/teams", json!({"name": "Laravel", "slug": "not a slug"}))
        .await
        .assert_successful_precognition();

    app.flush_headers()
        .with_precognition()
        .post_json("/teams", json!({"name": "Laravel", "slug": "laravel"}))
        .await
        .assert_no_content();

    // Precognitive requests don't count as interactions.
    assert_eq!(SideEffects::get(&effects.interactions), 0);
    assert_eq!(SideEffects::get(&effects.teams_created), 0);

    app.flush_headers()
        .post_json("/teams", json!({"name": "Laravel", "slug": "laravel"}))
        .await
        .assert_ok()
        .assert_content("Created Laravel");
    assert_eq!(SideEffects::get(&effects.interactions), 1);
    assert_eq!(SideEffects::get(&effects.teams_created), 1);
}

#[tokio::test]
async fn validation_inside_the_handler_body_is_not_predicted() {
    let (mut app, effects, _dir) = app();

    // Just like Laravel, only the handler's arguments are resolved: move
    // validation into a form request (or an extractor) to predict it.
    app.with_precognition()
        .post_json("/teams/inline", json!({}))
        .await
        .assert_no_content();
    assert_eq!(SideEffects::get(&effects.teams_created), 0);

    app.flush_headers()
        .post_json("/teams/inline", json!({}))
        .await
        .assert_unprocessable();
}

#[tokio::test]
async fn input_extractors_are_resolved() {
    let (mut app, _effects, _dir) = app();
    app.with_precognition();

    app.get("/search?term=eloquent").await.assert_no_content();
    app.get_json("/search")
        .await
        .assert_unprocessable()
        .assert_header("Precognition", Some("true"));
}

#[tokio::test]
async fn precognitive_requests_do_not_persist_the_session() {
    let (mut app, _effects, _dir) = app();

    app.with_precognition()
        .post_json("/session", json!({}))
        .await
        .assert_no_content();
    app.flush_headers()
        .get("/session")
        .await
        .assert_content("null / null");

    app.post_json("/session", json!({})).await.assert_ok();
    app.get("/session")
        .await
        .assert_content("\"by middleware\" / \"by the handler\"");
}

#[tokio::test]
async fn routes_without_the_middleware_ignore_the_header() {
    let (mut app, effects, _dir) = app();

    let response = app
        .with_precognition()
        .post_json("/plain", json!({}))
        .await;
    response
        .assert_ok()
        .assert_content("Plain")
        .assert_header_missing("Precognition")
        .assert_header_missing("Vary");
    assert_eq!(SideEffects::get(&effects.users_created), 1);
}

#[tokio::test]
async fn the_header_must_be_exactly_true() {
    let (mut app, effects, _dir) = app();

    // Not a precognitive request: the submission's rules apply.
    app.with_header("Precognition", "1")
        .post_json("/users", valid_user())
        .await
        .assert_unprocessable()
        .assert_header_missing("Precognition")
        .assert_header("Vary", Some("Precognition"))
        .assert_json_validation_errors(&["password"]);

    let mut submission = valid_user();
    submission["password_confirmation"] = json!("secret-password");
    app.with_header("Precognition", "false")
        .post_json("/users", submission)
        .await
        .assert_created()
        .assert_header_missing("Precognition-Success");
    assert_eq!(SideEffects::get(&effects.users_created), 1);
}

#[tokio::test]
async fn the_precognitive_alias_may_be_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let builder = Application::configure_detached(dir.path())
        .with_middleware(|middleware| {
            middleware.alias("precognitive", |_: &[String]| {
                illuminate_http::middleware_fn(|request: Request, next: Next| async move {
                    Ok(next.run(request).await.with_header("X-Custom", "yes"))
                })
            });
        })
        .with_routing(|routing| {
            routing.web(|| {
                Route::post("/users", || async { "Created" }).middleware("precognitive");
            });
        });
    let mut app = TestApp::new(builder);

    app.with_precognition()
        .post_json("/users", json!({}))
        .await
        .assert_ok()
        .assert_header("X-Custom", Some("yes"))
        .assert_header_missing("Precognition");
}
