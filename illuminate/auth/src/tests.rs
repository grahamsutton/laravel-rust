//! Integration tests: whole requests through the session, cookie, and auth
//! middleware, plus gates, policies, and password resets.

use std::future::Future;
use std::sync::Arc;

use indexmap::IndexMap;
use serde::Serialize;

use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_cookie::{AddQueuedCookiesToResponse, CookieServiceProvider, EncryptCookies};
use illuminate_encryption::Encrypter;
use illuminate_hashing::Hash;
use illuminate_http::exceptions::default_render;
use illuminate_http::{
    Destination, ExceptionHandler, HeaderMap, HeaderValue, Middleware, Request, Response,
    render_exception, run_middleware, with_request,
};
use illuminate_session::{RequestSessionExt, SessionServiceProvider, StartSession};
use illuminate_support::{Error, Result, Value, json};

use crate::access::AuthorizationException;
use crate::exceptions::AuthenticationException;
use crate::providers::ArrayUserProvider;
use crate::support::sha256;
use crate::user::{Authenticatable, GenericUser, MustVerifyEmail};
use crate::{Auth, AuthServiceProvider, UserProvider};

// ----------------------------------------------------------------------
// Fixtures
// ----------------------------------------------------------------------

/// A container with fast bcrypt hashing configured.
pub(crate) fn hashing_container() -> Arc<Container> {
    let container = Arc::new(Container::new());
    container.instance(Repository::new(
        json!({"hashing": {"bcrypt": {"rounds": 4}}}),
    ));
    container
}

/// A generic user with a hashed password (the hashing container must be current).
pub(crate) fn user(id: i64, email: &str, password: &str) -> GenericUser {
    GenericUser::new(json!({
        "id": id,
        "email": email,
        "password": Hash::make(password).unwrap(),
    }))
}

/// An application user, the way an Eloquent model would implement the
/// contracts.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct User {
    pub id: i64,
    pub name: String,
    pub email: String,
    #[serde(skip)]
    pub password: String,
    pub admin: bool,
    pub email_verified_at: Option<String>,
    pub api_token: Option<String>,
    pub hashed_token: Option<String>,
    #[serde(skip)]
    pub remember_token: Option<String>,
}

impl Authenticatable for User {
    fn auth_identifier(&self) -> Value {
        json!(self.id)
    }

    fn auth_password(&self) -> String {
        self.password.clone()
    }

    fn set_auth_password(&mut self, hashed: &str) {
        self.password = hashed.to_string();
    }

    fn remember_token(&self) -> Option<String> {
        self.remember_token.clone()
    }

    fn set_remember_token(&mut self, token: &str) {
        self.remember_token = Some(token.to_string());
    }

    fn must_verify_email(&self) -> Option<&dyn MustVerifyEmail> {
        Some(self)
    }
}

impl MustVerifyEmail for User {}

pub(crate) fn taylor() -> User {
    User {
        id: 1,
        name: "Taylor".into(),
        email: "taylor@laravel.com".into(),
        password: Hash::make("secret").unwrap(),
        admin: true,
        email_verified_at: Some("2024-01-01 00:00:00".into()),
        api_token: Some("taylor-token".into()),
        hashed_token: Some(sha256("hashed-secret")),
        remember_token: None,
    }
}

pub(crate) fn abigail() -> User {
    User {
        id: 2,
        name: "Abigail".into(),
        email: "abigail@laravel.com".into(),
        password: Hash::make("password").unwrap(),
        admin: false,
        email_verified_at: None,
        api_token: Some("abigail-token".into()),
        hashed_token: None,
        remember_token: None,
    }
}

fn config() -> Value {
    json!({
        "app": {"name": "Laravel", "key": "base64:AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8="},
        "hashing": {"bcrypt": {"rounds": 4}},
        "session": {"driver": "array", "cookie": "laravel_session", "lottery": [0, 100]},
        "auth": {
            "defaults": {"guard": "web", "passwords": "users"},
            "guards": {
                "web": {"driver": "session", "provider": "users"},
                "admin": {"driver": "session", "provider": "users", "remember": 60},
                "api": {"driver": "token", "provider": "users"},
                "hashed": {"driver": "token", "provider": "users", "hash": true, "input_key": "key", "storage_key": "hashed_token"},
                "generic": {"driver": "token", "provider": "generic"},
                "custom": {"driver": "custom-token", "provider": "users"},
                "broken": {"driver": "session", "provider": "eloquent-users"},
                "unknown": {"driver": "nope"},
            },
            "providers": {
                "users": {"driver": "memory"},
                "eloquent-users": {"driver": "eloquent", "model": "App\\Models\\User"},
                "generic": {"driver": "array", "users": [{"id": 10, "email": "generic@laravel.com", "api_token": "generic-token"}]},
            },
            "passwords": {
                "users": {"provider": "users", "driver": "array", "expire": 60, "throttle": 60},
                "database": {"provider": "users"},
            },
            "password_timeout": 10800,
        },
    })
}

/// Renders auth exceptions the way the foundation's handler does.
struct TestExceptionHandler;

impl ExceptionHandler for TestExceptionHandler {
    fn report(&self, _error: &Error) {}

    fn render(&self, request: &Request, error: Error) -> Response {
        if let Some(exception) = error.downcast_ref::<AuthenticationException>() {
            return exception.render(request);
        }
        if let Some(exception) = error.downcast_ref::<AuthorizationException>() {
            return default_render(request, exception.to_http_exception().into());
        }
        default_render(request, error)
    }
}

pub(crate) struct TestApp {
    pub users: Arc<ArrayUserProvider<User>>,
    _guard: LocalInstanceGuard,
}

impl TestApp {
    pub fn user(&self, id: i64) -> User {
        self.users.find(&json!(id)).unwrap()
    }
}

/// A fully configured application: session, cookies, encryption, auth.
pub(crate) fn app() -> TestApp {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(config()));
    container.instance(Encrypter::new([7u8; 32], "aes-256-cbc").unwrap());
    container.instance_arc::<dyn ExceptionHandler>(Arc::new(TestExceptionHandler));
    SessionServiceProvider.register(&container);
    CookieServiceProvider.register(&container);
    AuthServiceProvider.register(&container);

    let users = Arc::new(ArrayUserProvider::new(vec![taylor(), abigail()]));
    let provider = users.clone();
    Auth::provider("memory", move |_app, _config| provider.clone());

    TestApp {
        users,
        _guard: guard,
    }
}

/// Turn an async closure returning `Result<Response>` into a destination.
pub(crate) fn handler<F, Fut>(callback: F) -> Destination
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Response>> + Send + 'static,
{
    Arc::new(move |request| {
        let future = callback(request);
        Box::pin(async move {
            match future.await {
                Ok(response) => response,
                Err(error) => render_exception(error),
            }
        })
    })
}

/// A destination that reports the authenticated user's email.
pub(crate) fn whoami() -> Destination {
    handler(|_request| async {
        let user: Option<User> = Auth::user().await;
        Ok(Response::new(
            user.map(|user| user.email)
                .unwrap_or_else(|| "guest".into()),
        ))
    })
}

/// A browser: remembers cookies between requests.
pub(crate) struct Browser {
    cookies: IndexMap<String, String>,
    encrypt: bool,
}

impl Browser {
    pub fn new() -> Self {
        Self {
            cookies: IndexMap::new(),
            encrypt: true,
        }
    }

    pub fn cookie(&self, name: &str) -> Option<&String> {
        self.cookies.get(name)
    }

    pub fn forget(&mut self, name: &str) {
        self.cookies.shift_remove(name);
    }

    pub fn set_cookie(&mut self, name: &str, value: &str) {
        self.cookies.insert(name.to_string(), value.to_string());
    }

    pub fn without_encryption(mut self) -> Self {
        self.encrypt = false;
        self
    }

    pub fn request(&self, method: &str, uri: &str, input: Value, json: bool) -> Request {
        let mut headers = HeaderMap::new();
        if !self.cookies.is_empty() {
            let header = self
                .cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            headers.insert("cookie", HeaderValue::from_str(&header).unwrap());
        }
        if json {
            headers.insert("accept", HeaderValue::from_static("application/json"));
        }
        Request::create_with(uri, method, input, headers)
    }

    /// Send a request through the web middleware, the route middleware,
    /// and the destination.
    pub async fn send(
        &mut self,
        request: Request,
        route: Vec<Arc<dyn Middleware>>,
        destination: Destination,
    ) -> Response {
        let mut middleware: Vec<Arc<dyn Middleware>> = Vec::new();
        if self.encrypt {
            middleware.push(Arc::new(EncryptCookies::new()));
        }
        middleware.push(Arc::new(AddQueuedCookiesToResponse::new()));
        middleware.push(Arc::new(StartSession::new()));
        middleware.extend(route);

        let response = with_request(
            request.clone(),
            run_middleware(request, middleware, destination),
        )
        .await;
        for cookie in response.cookies() {
            if cookie.is_cleared() {
                self.cookies.shift_remove(&cookie.name);
            } else {
                self.cookies
                    .insert(cookie.name.clone(), cookie.value.clone());
            }
        }
        response
    }

    pub async fn get(
        &mut self,
        uri: &str,
        route: Vec<Arc<dyn Middleware>>,
        destination: Destination,
    ) -> Response {
        let request = self.request("GET", uri, json!({}), false);
        self.send(request, route, destination).await
    }

    pub async fn get_json(
        &mut self,
        uri: &str,
        route: Vec<Arc<dyn Middleware>>,
        destination: Destination,
    ) -> Response {
        let request = self.request("GET", uri, json!({}), true);
        self.send(request, route, destination).await
    }

    pub async fn post(&mut self, uri: &str, input: Value, destination: Destination) -> Response {
        let request = self.request("POST", uri, input, false);
        self.send(request, Vec::new(), destination).await
    }

    /// Log in through a request, the way a login form would.
    pub async fn login(&mut self, email: &str, password: &str, remember: bool) -> Response {
        let input = json!({"email": email, "password": password, "remember": remember});
        self.post(
            "/login",
            input,
            handler(|request: Request| async move {
                let credentials = request.only(&["email", "password"]);
                if Auth::attempt(&credentials, request.boolean("remember")).await? {
                    Ok(Response::redirect("/dashboard"))
                } else {
                    Ok(Response::new("failed").with_status(422))
                }
            }),
        )
        .await
    }
}

/// Run a closure as if a request were being handled (with a started
/// session), returning the request so its state can be inspected.
pub(crate) async fn in_request<F, Fut, T>(callback: F) -> (Request, T)
where
    F: FnOnce(Request) -> Fut,
    Fut: Future<Output = T>,
{
    let request = Request::create("/", "GET");
    let session = illuminate_session::Session::manager().driver().unwrap();
    session.start().await.unwrap();
    request.set_session(session);
    let output = with_request(request.clone(), callback(request.clone())).await;
    (request, output)
}

mod flows;
mod gates;
mod middleware;
mod passwords;
mod remember;

#[tokio::test]
async fn the_provider_trait_is_usable_through_arcs() {
    let _app = app();
    let provider: Arc<dyn UserProvider> = Arc::new(ArrayUserProvider::new(vec![taylor()]));
    let shared = Arc::new(provider);
    assert!(shared.retrieve_by_id(&json!(1)).await.unwrap().is_some());
}
