//! Every auth middleware, through real requests.

use std::sync::Arc;

use illuminate_http::{Middleware, Request, Response};
use illuminate_session::RequestSessionExt;
use illuminate_support::{Carbon, json};

use super::{Browser, User, app, handler, whoami};
use crate::access::{AuthResponse, Gate, GateArgument, Policy};
use crate::middleware::{
    Authenticate, AuthenticateWithBasicAuth, Authorize, EnsureEmailIsVerified,
    RedirectIfAuthenticated, RequirePassword, middleware_aliases,
};
use crate::{Auth, AuthenticationException};

fn ok() -> illuminate_http::Destination {
    handler(|_| async { Ok(Response::new("ok")) })
}

#[tokio::test]
async fn authenticate_redirects_guests_to_the_login_page() {
    let _app = app();
    let mut browser = Browser::new();

    let response = browser
        .get(
            "/dashboard?tab=1",
            vec![Arc::new(Authenticate::new())],
            ok(),
        )
        .await;
    assert!(response.is_redirect());
    assert_eq!(response.target_url().unwrap(), "http://localhost/login");

    // The intended URL is remembered for after the login.
    let response = browser
        .get(
            "/",
            vec![],
            handler(|request: Request| async move {
                Ok(Response::new(
                    request
                        .session()
                        .get("url.intended")
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                ))
            }),
        )
        .await;
    assert_eq!(
        response.content_string(),
        "http://localhost/dashboard?tab=1"
    );
}

#[tokio::test]
async fn authenticate_responds_to_json_requests_with_a_401() {
    let _app = app();
    let mut browser = Browser::new();
    let response = browser
        .get_json("/api/user", vec![Arc::new(Authenticate::new())], ok())
        .await;
    assert_eq!(response.status_code(), 401);
    assert_eq!(response.json_body(), json!({"message": "Unauthenticated."}));
}

#[tokio::test]
async fn authenticate_throws_an_authentication_exception() {
    let _app = app();
    let request = Request::create("/dashboard", "GET");
    let error = Authenticate::using(&["web", "api"])
        .authenticate(&request)
        .await
        .unwrap_err();
    let exception = error.downcast_ref::<AuthenticationException>().unwrap();
    assert_eq!(exception.guards(), ["web", "api"]);
    assert_eq!(exception.redirect_path(), Some("/login"));
    assert_eq!(exception.message(), "Unauthenticated.");
}

#[tokio::test]
async fn the_guest_redirect_can_be_customized() {
    let _app = app();
    let mut browser = Browser::new();

    Auth::resolve_routes_using(|name| (name == "login").then(|| "/sign-in".to_string()));
    let response = browser
        .get("/", vec![Arc::new(Authenticate::new())], ok())
        .await;
    assert_eq!(response.target_url().unwrap(), "http://localhost/sign-in");

    Authenticate::redirect_using(|request| Some(format!("/auth?from={}", request.path())));
    let response = browser
        .get("/secret", vec![Arc::new(Authenticate::new())], ok())
        .await;
    assert_eq!(
        response.target_url().unwrap(),
        "http://localhost/auth?from=secret"
    );

    AuthenticationException::redirect_using(|_| None);
    let response = browser
        .get("/", vec![Arc::new(Authenticate::new())], ok())
        .await;
    assert_eq!(response.status_code(), 401);
}

#[tokio::test]
async fn authenticate_checks_each_guard_and_uses_the_first_match() {
    let _app = app();
    let mut browser = Browser::new();
    let request = browser.request("GET", "/api/user?api_token=abigail-token", json!({}), false);
    let response = browser
        .send(
            request,
            vec![Arc::new(Authenticate::from_parameters(&[
                "web".into(),
                "api".into(),
            ]))],
            handler(|request: Request| async move {
                assert_eq!(Auth::get_default_driver(), "api");
                assert_eq!(request.attribute("_auth_id"), json!(2));
                let user: User = Auth::user().await.unwrap();
                Ok(Response::new(user.name))
            }),
        )
        .await;
    assert_eq!(response.content_string(), "Abigail");
}

#[tokio::test]
async fn guests_pass_the_guest_middleware_and_users_are_redirected() {
    let _app = app();
    let mut browser = Browser::new();
    let guest = || -> Vec<Arc<dyn Middleware>> { vec![Arc::new(RedirectIfAuthenticated::new())] };

    let response = browser.get("/login", guest(), ok()).await;
    assert_eq!(response.content_string(), "ok");

    browser.login("taylor@laravel.com", "secret", false).await;
    let response = browser.get("/login", guest(), ok()).await;
    assert_eq!(response.target_url().unwrap(), "http://localhost");

    Auth::resolve_routes_using(|name| {
        (name == "home").then(|| "http://localhost/home".to_string())
    });
    let response = browser.get("/login", guest(), ok()).await;
    assert_eq!(response.target_url().unwrap(), "http://localhost/home");

    RedirectIfAuthenticated::redirect_using(|_| "/panel".to_string());
    let response = browser.get("/login", guest(), ok()).await;
    assert_eq!(response.target_url().unwrap(), "http://localhost/panel");

    // Other guards are checked when named.
    let response = browser
        .get(
            "/login",
            vec![Arc::new(RedirectIfAuthenticated::using(&["admin"]))],
            ok(),
        )
        .await;
    assert_eq!(response.content_string(), "ok");
}

#[tokio::test]
async fn basic_auth_prompts_for_credentials() {
    let _app = app();
    let mut browser = Browser::new();
    let basic = || -> Vec<Arc<dyn Middleware>> { vec![Arc::new(AuthenticateWithBasicAuth::new())] };

    let response = browser.get("/", basic(), whoami()).await;
    assert_eq!(response.status_code(), 401);
    assert_eq!(response.header("www-authenticate").unwrap(), "Basic");

    // taylor@laravel.com:wrong
    let request = browser.request("GET", "/", json!({}), false);
    request.set_header("authorization", "Basic dGF5bG9yQGxhcmF2ZWwuY29tOndyb25n");
    let response = browser.send(request, basic(), whoami()).await;
    assert_eq!(response.status_code(), 401);

    // taylor@laravel.com:secret
    let request = browser.request("GET", "/", json!({}), false);
    request.set_header(
        "authorization",
        "Basic dGF5bG9yQGxhcmF2ZWwuY29tOnNlY3JldA==",
    );
    let response = browser.send(request, basic(), whoami()).await;
    assert_eq!(response.content_string(), "taylor@laravel.com");

    // The login is kept in the session.
    assert_eq!(
        browser.get("/", basic(), whoami()).await.content_string(),
        "taylor@laravel.com"
    );
}

#[tokio::test]
async fn basic_auth_can_be_stateless() {
    let _app = app();
    let mut browser = Browser::new();
    let request = browser.request("GET", "/", json!({}), false);
    request.set_header(
        "authorization",
        "Basic dGF5bG9yQGxhcmF2ZWwuY29tOnNlY3JldA==",
    );
    let response = browser
        .send(
            request,
            vec![],
            handler(|_| async {
                Auth::guard("web").once_basic("email", &json!({})).await?;
                Ok(Response::new("once"))
            }),
        )
        .await;
    assert_eq!(response.content_string(), "once");
    assert_eq!(
        browser.get("/", vec![], whoami()).await.content_string(),
        "guest"
    );

    let response = browser
        .get(
            "/",
            vec![],
            handler(|_| async {
                Auth::guard("web").once_basic("email", &json!({})).await?;
                Ok(Response::new("once"))
            }),
        )
        .await;
    assert_eq!(response.status_code(), 401);
}

struct Post {
    user_id: i64,
}

struct PostPolicy;

impl Policy<Post> for PostPolicy {
    type User = User;

    fn create(&self, user: &User) -> Option<AuthResponse> {
        Some(user.admin.into())
    }

    fn update(&self, user: &User, post: &Post) -> Option<AuthResponse> {
        Some(if user.id == post.user_id {
            AuthResponse::allow()
        } else {
            AuthResponse::deny("You do not own this post.")
        })
    }

    fn delete(&self, user: &User, post: &Post) -> Option<AuthResponse> {
        Some(if user.id == post.user_id {
            AuthResponse::allow()
        } else {
            AuthResponse::deny_as_not_found()
        })
    }
}

#[tokio::test]
async fn the_can_middleware_authorizes_abilities() {
    let _app = app();
    Gate::define("view-dashboard", |user: &User| user.admin);
    Gate::policy::<Post, _>(PostPolicy);
    Authorize::resolve_argument_using(|request, parameter| match parameter {
        "post" => {
            let owner = request.route("post")?.parse().ok()?;
            Some(GateArgument::owned(Post { user_id: owner }))
        }
        _ => None,
    });

    let can = |parameters: &[&str]| -> Vec<Arc<dyn Middleware>> {
        let parameters: Vec<String> = parameters.iter().map(|p| p.to_string()).collect();
        vec![
            Arc::new(Authenticate::new()),
            Authorize::factory()(&parameters),
        ]
    };
    let with_route = |browser: &Browser, owner: &str| {
        let request = browser.request("GET", "/posts", json!({}), false);
        request.set_route_parameter("post", owner);
        request
    };

    let mut taylor = Browser::new();
    taylor.login("taylor@laravel.com", "secret", false).await;
    let mut abigail = Browser::new();
    abigail
        .login("abigail@laravel.com", "password", false)
        .await;

    // Abilities without models.
    assert_eq!(
        taylor
            .get("/", can(&["view-dashboard"]), ok())
            .await
            .status_code(),
        200
    );
    let response = abigail.get_json("/", can(&["view-dashboard"]), ok()).await;
    assert_eq!(response.status_code(), 403);
    assert_eq!(
        response.json_body(),
        json!({"message": "This action is unauthorized."})
    );

    // Route parameters resolved into models.
    let request = with_route(&taylor, "1");
    assert_eq!(
        taylor
            .send(request, can(&["update", "post"]), ok())
            .await
            .status_code(),
        200
    );
    let request = with_route(&abigail, "1");
    let response = abigail.send(request, can(&["update", "post"]), ok()).await;
    assert_eq!(response.status_code(), 403);
    assert!(
        response
            .content_string()
            .contains("You do not own this post.")
    );

    // Denials may hide the resource.
    let request = with_route(&abigail, "1");
    assert_eq!(
        abigail
            .send(request, can(&["delete", "post"]), ok())
            .await
            .status_code(),
        404
    );

    // Model types, by class name.
    assert_eq!(
        taylor
            .get("/", can(&["create", "App\\Models\\Post"]), ok())
            .await
            .status_code(),
        200
    );
    assert_eq!(
        taylor
            .get("/", can(&["create", "Post"]), ok())
            .await
            .status_code(),
        200
    );
    assert_eq!(
        abigail
            .get("/", can(&["create", "App\\Models\\Post"]), ok())
            .await
            .status_code(),
        403
    );

    // Guests are denied.
    let mut guest = Browser::new();
    let response = guest
        .get(
            "/",
            vec![Authorize::factory()(&["view-dashboard".into()])],
            ok(),
        )
        .await;
    assert_eq!(response.status_code(), 403);

    let middleware = Authorize::using("update", &["post", "'literal'"]);
    assert_eq!(middleware.ability(), "update");
    assert_eq!(middleware.models(), ["post", "'literal'"]);
    let request = Request::create("/", "GET");
    let arguments = middleware.gate_arguments(&request);
    // Without a route parameter (or a resolved model), the name itself is used.
    assert_eq!(arguments[0].as_value(), Some(&json!("post")));
    assert_eq!(arguments[1].as_value(), Some(&json!("literal")));
}

#[tokio::test]
async fn unverified_users_are_sent_to_the_verification_notice() {
    let _app = app();
    let verified = || -> Vec<Arc<dyn Middleware>> { vec![Arc::new(EnsureEmailIsVerified::new())] };

    let mut guest = Browser::new();
    let response = guest.get("/billing", verified(), ok()).await;
    assert_eq!(
        response.target_url().unwrap(),
        "http://localhost/email/verify"
    );

    let mut abigail = Browser::new();
    abigail
        .login("abigail@laravel.com", "password", false)
        .await;
    let response = abigail.get("/billing", verified(), ok()).await;
    assert_eq!(
        response.target_url().unwrap(),
        "http://localhost/email/verify"
    );

    let response = abigail.get_json("/billing", verified(), ok()).await;
    assert_eq!(response.status_code(), 403);
    assert_eq!(
        response.json_body(),
        json!({"message": "Your email address is not verified."})
    );

    let response = abigail
        .get(
            "/billing",
            vec![EnsureEmailIsVerified::factory()(&["/please-verify".into()])],
            ok(),
        )
        .await;
    assert_eq!(
        response.target_url().unwrap(),
        "http://localhost/please-verify"
    );

    let mut taylor = Browser::new();
    taylor.login("taylor@laravel.com", "secret", false).await;
    assert_eq!(
        taylor
            .get("/billing", verified(), ok())
            .await
            .content_string(),
        "ok"
    );

    // Users that don't need verification always pass.
    let generic = crate::GenericUser::new(json!({"id": 5}));
    Auth::acting_as(&generic, None);
    assert_eq!(
        Browser::new()
            .get("/billing", verified(), ok())
            .await
            .content_string(),
        "ok"
    );
}

#[tokio::test]
async fn sensitive_actions_require_a_recent_password_confirmation() {
    let _app = app();
    let confirm = || -> Vec<Arc<dyn Middleware>> { vec![Arc::new(RequirePassword::new())] };
    let mut browser = Browser::new();
    browser.login("taylor@laravel.com", "secret", false).await;

    let response = browser.get("/settings", confirm(), ok()).await;
    assert_eq!(
        response.target_url().unwrap(),
        "http://localhost/confirm-password"
    );

    let response = browser.get_json("/settings", confirm(), ok()).await;
    assert_eq!(response.status_code(), 423);
    assert_eq!(
        response.json_body(),
        json!({"message": "Password confirmation required."})
    );

    // Confirm the password...
    browser
        .post(
            "/confirm-password",
            json!({}),
            handler(|request: Request| async move {
                request.session().password_confirmed();
                Ok(Response::new("confirmed"))
            }),
        )
        .await;
    assert_eq!(
        browser
            .get("/settings", confirm(), ok())
            .await
            .content_string(),
        "ok"
    );

    // ...which expires.
    browser
        .get(
            "/",
            vec![],
            handler(|request: Request| async move {
                let long_ago = Carbon::now().timestamp() - 10_801;
                request
                    .session()
                    .put("auth.password_confirmed_at", long_ago);
                Ok(Response::new("traveled"))
            }),
        )
        .await;
    assert!(
        browser
            .get("/settings", confirm(), ok())
            .await
            .is_redirect()
    );

    // The timeout can be given as a parameter.
    let lenient = RequirePassword::from_parameters(&["".into(), "20000".into()]);
    assert_eq!(lenient.timeout(), 20_000);
    assert_eq!(RequirePassword::new().timeout(), 10_800);
    assert_eq!(
        browser
            .get("/settings", vec![Arc::new(lenient)], ok())
            .await
            .content_string(),
        "ok"
    );
}

#[tokio::test]
async fn middleware_aliases_build_from_parameters() {
    let _app = app();
    let aliases = middleware_aliases();
    assert_eq!(aliases.len(), 7);
    let (_, factory) = aliases.iter().find(|(alias, _)| *alias == "auth").unwrap();
    let middleware = factory(&["api".into()]);

    let mut browser = Browser::new();
    let request = browser.request("GET", "/?api_token=taylor-token", json!({}), false);
    assert_eq!(
        browser
            .send(request, vec![middleware.clone()], whoami())
            .await
            .content_string(),
        "taylor@laravel.com"
    );
    let response = browser.get_json("/", vec![middleware], whoami()).await;
    assert_eq!(response.status_code(), 401);
}
