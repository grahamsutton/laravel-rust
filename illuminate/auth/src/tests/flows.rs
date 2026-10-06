//! Logging in and out across requests, and the guards.

use std::sync::Arc;

use illuminate_http::{Request, Response};
use illuminate_session::RequestSessionExt;
use illuminate_support::{Value, json};

use super::{Browser, User, app, handler, in_request, whoami};
use crate::middleware::Authenticate;
use crate::{Auth, AuthUser, GenericUser, Guard, RequestAuthExt, SessionGuard};

#[tokio::test]
async fn users_can_log_in_and_stay_logged_in() {
    let _app = app();
    let mut browser = Browser::new();

    // A first visit starts a session.
    browser.get("/", vec![], whoami()).await;
    let guest_session = browser.cookie("laravel_session").cloned().unwrap();

    let response = browser.login("taylor@laravel.com", "secret", false).await;
    assert!(response.is_redirect());

    // Logging in migrates the session to a new ID.
    assert_ne!(browser.cookie("laravel_session").unwrap(), &guest_session);

    let response = browser
        .get("/dashboard", vec![Arc::new(Authenticate::new())], whoami())
        .await;
    assert_eq!(response.status_code(), 200);
    assert_eq!(response.content_string(), "taylor@laravel.com");
}

#[tokio::test]
async fn the_session_holds_the_login_key() {
    let _app = app();
    let (request, _) = in_request(|_| async {
        assert!(
            Auth::attempt(
                &json!({"email": "taylor@laravel.com", "password": "secret"}),
                false
            )
            .await
            .unwrap()
        );
    })
    .await;

    let session = request.session();
    assert_eq!(
        session.get("login_web_59ba36addc2b2f9401580f014c7f58ea4e30989d"),
        json!(1)
    );
    assert!(session.get("password_hash_web").is_string());

    // The request knows its user, synchronously.
    let user: User = request.user().unwrap();
    assert_eq!(user.name, "Taylor");
    assert_eq!(request.user_id(), Some(json!(1)));
    assert_eq!(request.attribute("_auth_id"), json!(1));
    assert!(request.auth_user().unwrap().is::<User>());
    assert!(request.user::<GenericUser>().is_none());
}

#[tokio::test]
async fn invalid_credentials_are_rejected() {
    let _app = app();
    in_request(|request: Request| async move {
        let wrong = json!({"email": "taylor@laravel.com", "password": "wrong"});
        assert!(!Auth::attempt(&wrong, false).await.unwrap());
        assert!(Auth::guest().await);
        assert!(!Auth::validate(&wrong).await.unwrap());

        let missing = json!({"email": "nobody@laravel.com", "password": "secret"});
        assert!(!Auth::attempt(&missing, false).await.unwrap());

        // The guard remembers who it last tried.
        let guard = Auth::guard("web");
        guard.validate(&wrong).await.unwrap();
        let session_guard = guard.downcast_ref::<SessionGuard>().unwrap();
        assert_eq!(session_guard.last_attempted().unwrap().id(), json!(1));

        assert!(
            Auth::validate(&json!({"email": "taylor@laravel.com", "password": "secret"}))
                .await
                .unwrap()
        );
        assert!(Auth::guest().await, "validating never logs the user in");
        assert!(request.attribute("_auth_id").is_null());
    })
    .await;
}

#[tokio::test]
async fn attempt_when_lets_callbacks_veto_the_login() {
    let _app = app();
    in_request(|_| async {
        let credentials = json!({"email": "abigail@laravel.com", "password": "password"});
        let admins_only = |user: &User| user.admin;
        assert!(
            !Auth::attempt_when(&credentials, admins_only, false)
                .await
                .unwrap()
        );
        assert!(Auth::guest().await);

        let verified = |user: &User| user.email_verified_at.is_none();
        assert!(
            Auth::attempt_when(&credentials, verified, false)
                .await
                .unwrap()
        );
        assert_eq!(Auth::id().await, Some(json!(2)));
    })
    .await;
}

#[tokio::test]
async fn users_can_log_out() {
    let _app = app();
    let mut browser = Browser::new();
    browser.login("taylor@laravel.com", "secret", false).await;

    let response = browser
        .get(
            "/logout",
            vec![],
            handler(|_| async {
                assert!(Auth::check().await);
                Auth::logout().await?;
                assert!(Auth::guest().await);
                assert!(Auth::user::<User>().await.is_none());
                assert_eq!(Auth::id().await, None);
                Ok(Response::redirect("/"))
            }),
        )
        .await;
    assert!(response.is_redirect());

    let response = browser
        .get("/dashboard", vec![Arc::new(Authenticate::new())], whoami())
        .await;
    assert!(response.is_redirect());
    assert_eq!(response.target_url().unwrap(), "http://localhost/login");
}

#[tokio::test]
async fn users_can_be_logged_in_by_instance_or_id() {
    let app = app();
    in_request(|_| async {
        let user = app.user(2);
        Auth::login(&user, false).await.unwrap();
        assert_eq!(Auth::user::<User>().await.unwrap().name, "Abigail");
        assert!(Auth::has_user());
        assert!(!Auth::via_remember());

        Auth::logout().await.unwrap();
        let logged_in = Auth::login_using_id(1, false).await.unwrap().unwrap();
        assert_eq!(logged_in.id(), json!(1));
        assert_eq!(Auth::user_value().await.unwrap()["name"], json!("Taylor"));

        assert!(Auth::login_using_id(99, false).await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
async fn once_authenticates_for_a_single_request() {
    let _app = app();
    let mut browser = Browser::new();

    browser
        .get(
            "/",
            vec![],
            handler(|request: Request| async move {
                assert!(
                    Auth::once(&json!({"email": "taylor@laravel.com", "password": "secret"}))
                        .await?
                );
                assert!(Auth::check().await);
                assert!(
                    request
                        .session()
                        .get("login_web_59ba36addc2b2f9401580f014c7f58ea4e30989d")
                        .is_null()
                );
                Ok(Response::new("ok"))
            }),
        )
        .await;

    let response = browser.get("/", vec![], whoami()).await;
    assert_eq!(response.content_string(), "guest");

    let response = browser
        .get(
            "/",
            vec![],
            handler(|_| async {
                let user = Auth::once_using_id(2).await?.unwrap();
                Ok(Response::new(
                    user.to_value()["email"].as_str().unwrap().to_string(),
                ))
            }),
        )
        .await;
    assert_eq!(response.content_string(), "abigail@laravel.com");
    assert_eq!(
        browser.get("/", vec![], whoami()).await.content_string(),
        "guest"
    );
}

#[tokio::test]
async fn users_can_be_set_and_forgotten() {
    let app = app();
    in_request(|request: Request| async move {
        Auth::set_user(&app.user(1));
        assert_eq!(Auth::id().await, Some(json!(1)));
        assert_eq!(request.attribute("_auth_id"), json!(1));

        Auth::forget_user();
        assert!(!Auth::has_user());
        assert!(request.attribute("_auth_id").is_null());
        // Nothing in the session, so the user can't be resolved again.
        assert!(Auth::guest().await);

        let error = Auth::authenticate().await.unwrap_err();
        assert_eq!(error.to_string(), "Unauthenticated.");
    })
    .await;
}

#[tokio::test]
async fn passwords_are_rehashed_when_the_work_factor_changes() {
    let app = app();
    let original = app.user(1).password.clone();
    assert!(original.starts_with("$2y$04$"));

    in_request(|_| async {
        illuminate_container::app::<illuminate_config::Repository>()
            .set("hashing.bcrypt.rounds", 5);
        illuminate_container::Container::get_instance()
            .forget_instance::<illuminate_hashing::HashManager>();
        assert!(
            Auth::attempt(
                &json!({"email": "taylor@laravel.com", "password": "secret"}),
                false
            )
            .await
            .unwrap()
        );
    })
    .await;

    let rehashed = app.user(1).password;
    assert!(rehashed.starts_with("$2y$05$"), "{rehashed}");
}

#[tokio::test]
async fn the_token_guard_reads_tokens_from_the_request() {
    let _app = app();
    let guard = Auth::guard("api");

    let request = Request::create("/api/user?api_token=taylor-token", "GET");
    let user = illuminate_http::with_request(request.clone(), guard.user())
        .await
        .unwrap();
    assert_eq!(user.id(), json!(1));
    assert_eq!(request.user_for::<User>("api").unwrap().name, "Taylor");

    let request = Request::create_with(
        "/api/user",
        "POST",
        json!({"api_token": "abigail-token"}),
        Default::default(),
    );
    let id = illuminate_http::with_request(request, guard.id()).await;
    assert_eq!(id, Some(json!(2)));

    let request = Request::create("/api/user", "GET");
    request.set_header("authorization", "Bearer taylor-token");
    assert!(illuminate_http::with_request(request, guard.check()).await);

    let request = Request::create("/api/user", "GET");
    request.set_header("authorization", "Bearer nope");
    assert!(illuminate_http::with_request(request, guard.guest()).await);

    assert!(
        guard
            .validate(&json!({"api_token": "taylor-token"}))
            .await
            .unwrap()
    );
    assert!(!guard.validate(&json!({"api_token": ""})).await.unwrap());
    assert!(!guard.validate(&json!({"api_token": "nope"})).await.unwrap());

    // Token guards can't log users in.
    let error = guard.attempt(&json!({}), false).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("not supported by the [api] guard")
    );
}

#[tokio::test]
async fn token_guards_can_hash_tokens() {
    let _app = app();
    let guard = Auth::guard("hashed");

    let request = Request::create("/api/user?key=hashed-secret", "GET");
    let user = illuminate_http::with_request(request, guard.user())
        .await
        .unwrap();
    assert_eq!(user.id(), json!(1));

    let request = Request::create("/api/user?key=hashed-secret-nope", "GET");
    assert!(illuminate_http::with_request(request, guard.guest()).await);

    // Basic auth passwords work as tokens too.
    let generic = Auth::guard("generic");
    let request = Request::create("/", "GET");
    request.set_header("authorization", "Basic Z2VuZXJpYzpnZW5lcmljLXRva2Vu");
    let user: GenericUser = illuminate_http::with_request(request, generic.user_as())
        .await
        .unwrap();
    assert_eq!(user.get("email"), json!("generic@laravel.com"));
}

#[tokio::test]
async fn request_guards_authenticate_with_a_closure() {
    let app = app();
    let taylor = app.user(1);
    Auth::via_request("custom-token", move |request: Request| {
        let taylor = taylor.clone();
        async move { (request.header("x-token").as_deref() == Some("let-me-in")).then_some(taylor) }
    });

    let guard = Auth::guard("custom");
    let request = Request::create("/", "GET");
    request.set_header("x-token", "let-me-in");
    let user: Option<User> = illuminate_http::with_request(request.clone(), guard.user_as()).await;
    assert_eq!(user.unwrap().name, "Taylor");
    assert!(
        illuminate_http::with_request(request, guard.validate(&json!({})))
            .await
            .unwrap()
    );
    assert!(guard.provider().is_some());

    let request = Request::create("/", "GET");
    assert!(illuminate_http::with_request(request, guard.guest()).await);
}

#[tokio::test]
async fn custom_guards_can_be_registered() {
    let _app = app();

    struct AlwaysGuard;

    #[illuminate_http::async_trait]
    impl Guard for AlwaysGuard {
        fn name(&self) -> &str {
            "always"
        }

        async fn try_user(&self) -> illuminate_support::Result<Option<AuthUser>> {
            Ok(Some(AuthUser::new(GenericUser::new(json!({"id": 42})))))
        }

        async fn validate(&self, _credentials: &Value) -> illuminate_support::Result<bool> {
            Ok(true)
        }

        fn set_user(&self, _user: AuthUser) {}

        fn forget_user(&self) {}

        fn provider(&self) -> Option<Arc<dyn crate::UserProvider>> {
            None
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    illuminate_container::app::<illuminate_config::Repository>()
        .set("auth.guards.always", json!({"driver": "always"}));
    Auth::extend("always", |_app, _name, _config| Ok(Arc::new(AlwaysGuard)));

    assert_eq!(Auth::guard("always").id().await, Some(json!(42)));
    assert!(Auth::manager().has_resolved_guards());
}

#[tokio::test]
async fn configuration_mistakes_are_reported() {
    let _app = app();
    let manager = Auth::manager();

    let error = manager.guard(Some("missing")).err().unwrap();
    assert_eq!(error.to_string(), "Auth guard [missing] is not defined.");

    let error = manager.guard(Some("unknown")).err().unwrap();
    assert_eq!(
        error.to_string(),
        "Auth driver [nope] for guard [unknown] is not defined."
    );

    let error = manager.guard(Some("broken")).err().unwrap();
    assert_eq!(
        error.to_string(),
        "Authentication user provider [eloquent] is not defined."
    );

    // Registering the driver fixes it.
    Auth::provider("eloquent", |_app, _config| {
        Arc::new(crate::ArrayUserProvider::<GenericUser>::default())
    });
    assert!(manager.guard(Some("broken")).is_ok());

    assert!(Auth::create_user_provider(Some("nope")).unwrap().is_none());
    assert!(Auth::create_user_provider(None).unwrap().is_none());
}

#[tokio::test]
#[should_panic(expected = "Auth guard [missing] is not defined.")]
async fn the_facade_panics_for_undefined_guards() {
    let _app = app();
    Auth::guard("missing");
}

#[tokio::test]
async fn guards_can_be_switched_for_the_request() {
    let app = app();
    let abigail = AuthUser::from(&app.user(2));
    in_request(|request: Request| async move {
        assert_eq!(Auth::get_default_driver(), "web");
        Auth::guard("admin").login(abigail, false).await.unwrap();
        assert!(Auth::guest().await, "the web guard is still a guest");
        assert!(request.auth_user().is_none());

        Auth::should_use("admin");
        assert_eq!(Auth::get_default_driver(), "admin");
        assert!(Auth::check().await);
        assert_eq!(request.user_id(), Some(json!(2)));
        assert_eq!(request.attribute("_auth_id"), json!(2));
    })
    .await;

    // Other requests still use the application default.
    in_request(|_| async {
        assert_eq!(Auth::get_default_driver(), "web");
    })
    .await;
}

#[tokio::test]
async fn acting_as_authenticates_every_request() {
    let app = app();
    Auth::acting_as(&app.user(1), None);

    // Outside of any request...
    assert_eq!(Auth::id().await, Some(json!(1)));

    // ...and in every request.
    let mut browser = Browser::new();
    let response = browser
        .get("/", vec![Arc::new(Authenticate::new())], whoami())
        .await;
    assert_eq!(response.content_string(), "taylor@laravel.com");

    // On another guard, which becomes the default.
    Auth::acting_as(&app.user(2), Some("api"));
    assert_eq!(Auth::get_default_driver(), "api");
    let response = browser
        .get("/", vec![Arc::new(Authenticate::using(&["api"]))], whoami())
        .await;
    assert_eq!(response.content_string(), "abigail@laravel.com");

    Auth::forget_acting_as();
    Auth::set_default_driver("web");
    let response = browser.get("/", vec![], whoami()).await;
    assert_eq!(response.content_string(), "guest");
}

#[tokio::test]
async fn the_id_falls_back_to_the_session() {
    let _app = app();
    in_request(|request: Request| async move {
        let guard = Auth::guard("web");
        let key = guard.downcast_ref::<SessionGuard>().unwrap().get_name();
        request.session().put(&key, 99);
        // User 99 doesn't exist, but the session still knows the id.
        assert!(guard.user().await.is_none());
        assert_eq!(guard.id().await, Some(json!(99)));
    })
    .await;
}

#[tokio::test]
async fn guards_work_without_a_session() {
    let app = app();
    // No request at all: the manager's own state is used.
    Auth::login(&app.user(1), false).await.unwrap();
    assert!(Auth::check().await);
    Auth::logout().await.unwrap();
    assert!(Auth::guest().await);
}
