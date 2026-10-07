//! The application every test runs: an in-memory SQLite database with
//! Eloquent users, sessions, cookies, auth, the router, and Sanctum.

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::Arc;

use illuminate_auth::{
    Auth, AuthServiceProvider, AuthUser, Authenticatable, AuthenticationException, RequestAuthExt,
    UserProvider,
};
use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_cookie::{AddQueuedCookiesToResponse, CookieServiceProvider, EncryptCookies};
use illuminate_database::eloquent::*;
use illuminate_database::{DatabaseManager, DatabaseServiceProvider, Migration};
use illuminate_encryption::Encrypter;
use illuminate_http::exceptions::default_render;
use illuminate_http::{
    ExceptionHandler, HeaderMap, HeaderName, HeaderValue, HttpException, Json, Request, Response,
    async_trait,
};
use illuminate_routing::{RouteMiddleware, Router, RoutingServiceProvider};
use illuminate_session::{
    SessionServiceProvider, StartSession, TokenMismatchException, ValidateCsrfToken,
};
use illuminate_support::{Error, Result};

use laravel_sanctum::{
    AccessToken, CreatePersonalAccessTokensTable, EnsureFrontendRequestsAreStateful, Sanctum,
    SanctumServiceProvider,
};

// ----------------------------------------------------------------------
// Models
// ----------------------------------------------------------------------

#[derive(Debug, Clone, Default, Model)]
#[fillable(name, email, password)]
#[hidden(password, remember_token)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
    pub password: String,
    pub remember_token: Option<String>,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,
    pub original: Original,
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
}

#[derive(Debug, Clone, Default, Model)]
#[fillable(name)]
pub struct Admin {
    pub id: u64,
    pub name: String,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,
    pub original: Original,
}

impl Authenticatable for Admin {
    fn auth_identifier(&self) -> Value {
        json!(self.id)
    }

    fn auth_password(&self) -> String {
        String::new()
    }
}

/// Retrieves users through an Eloquent model (the foundation's
/// `EloquentUserProvider`, in miniature).
pub struct EloquentUsers<M>(PhantomData<fn() -> M>);

impl<M> EloquentUsers<M> {
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

#[async_trait]
impl<M: Model + Authenticatable> UserProvider for EloquentUsers<M> {
    async fn retrieve_by_id(&self, identifier: &Value) -> Result<Option<AuthUser>> {
        Ok(M::find(identifier.clone()).await?.map(AuthUser::new))
    }

    async fn update_remember_token(&self, _user: &AuthUser, _token: &str) -> Result<()> {
        Ok(())
    }

    async fn retrieve_by_credentials(&self, credentials: &Value) -> Result<Option<AuthUser>> {
        match credentials.get("email") {
            Some(email) => Ok(M::where_("email", email.clone())
                .first()
                .await?
                .map(AuthUser::new)),
            None => Ok(None),
        }
    }
}

// ----------------------------------------------------------------------
// The application
// ----------------------------------------------------------------------

/// Renders exceptions the way the foundation's handler does.
struct Handler;

impl ExceptionHandler for Handler {
    fn report(&self, _error: &Error) {}

    fn render(&self, request: &Request, error: Error) -> Response {
        if let Some(exception) = error.downcast_ref::<AuthenticationException>() {
            return exception.render(request);
        }
        if let Some(exception) = error.downcast_ref::<TokenMismatchException>() {
            let http = HttpException::with_message(419, exception.message.clone());
            return default_render(request, http.into());
        }
        default_render(request, error)
    }
}

pub struct App {
    pub container: Arc<Container>,
    _guard: LocalInstanceGuard,
}

impl Drop for App {
    fn drop(&mut self) {
        // Leave the thread as we found it (tests may share threads).
        Carbon::set_thread_test_now(None);
        illuminate_support::Str::create_random_strings_normally();
    }
}

impl App {
    pub fn router(&self) -> Arc<Router> {
        self.container.make::<Router>()
    }

    pub fn config(&self) -> Arc<Repository> {
        self.container.make::<Repository>()
    }
}

fn merge(base: &mut Value, extra: Value) {
    match (base, extra) {
        (Value::Object(base), Value::Object(extra)) => {
            for (key, value) in extra {
                match base.get_mut(&key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        merge(existing, value)
                    }
                    _ => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, extra) => *base = extra,
    }
}

pub async fn app() -> App {
    app_with(json!({})).await
}

/// Boot the application with extra configuration merged over the defaults.
pub async fn app_with(extra: Value) -> App {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());

    let mut config = json!({
        "app": {
            "name": "Laravel",
            "url": "http://localhost",
            "frontend_url": "http://localhost:3000",
            "key": "base64:AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=",
        },
        "database": {
            "default": "sqlite",
            "connections": {"sqlite": {"driver": "sqlite", "database": ":memory:"}},
        },
        "session": {"driver": "array", "cookie": "laravel_session", "lottery": [0, 100]},
        "auth": {
            "defaults": {"guard": "web"},
            "guards": {
                "web": {"driver": "session", "provider": "users"},
            },
            "providers": {
                "users": {"driver": "eloquent", "model": "App\\Models\\User"},
                "admins": {"driver": "eloquent", "model": "Admin"},
            },
        },
    });
    merge(&mut config, extra);
    container.instance(Repository::new(config));
    container.instance(Encrypter::new([7u8; 32], "aes-256-cbc").unwrap());
    container.instance_arc::<dyn ExceptionHandler>(Arc::new(Handler));

    let providers: Vec<Box<dyn ServiceProvider>> = vec![
        Box::new(DatabaseServiceProvider),
        Box::new(SessionServiceProvider),
        Box::new(CookieServiceProvider),
        Box::new(AuthServiceProvider),
        Box::new(RoutingServiceProvider),
        Box::new(SanctumServiceProvider),
    ];
    for provider in &providers {
        provider.register(&container);
    }

    Auth::provider("eloquent", |_app, config| {
        let model = config
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("User");
        if model.ends_with("Admin") {
            Arc::new(EloquentUsers::<Admin>::new()) as Arc<dyn UserProvider>
        } else {
            Arc::new(EloquentUsers::<User>::new())
        }
    });

    let router = container.make::<Router>();
    for (alias, factory) in illuminate_auth::middleware_aliases() {
        router.alias_middleware_factory(alias, factory);
    }
    for (alias, factory) in laravel_sanctum::middleware_aliases() {
        router.alias_middleware_factory(alias, factory);
    }
    router.middleware_group(
        "web",
        vec![
            RouteMiddleware::of(EncryptCookies::new()),
            RouteMiddleware::of(AddQueuedCookiesToResponse::new()),
            RouteMiddleware::of(StartSession::new()),
            RouteMiddleware::of(ValidateCsrfToken::new()),
        ],
    );
    router.middleware_group(
        "api",
        vec![RouteMiddleware::of(EnsureFrontendRequestsAreStateful::new())],
    );

    for provider in &providers {
        provider.boot(&container);
    }

    migrate().await;

    App {
        container,
        _guard: guard,
    }
}

async fn migrate() {
    let schema = DatabaseManager::resolve()
        .default_connection()
        .get_schema_builder();
    schema
        .create("users", |table| {
            table.id();
            table.string("name");
            table.string("email").unique();
            table.string("password").default("");
            table.string("remember_token").nullable();
            table.timestamps();
        })
        .await
        .unwrap();
    schema
        .create("admins", |table| {
            table.id();
            table.string("name");
            table.timestamps();
        })
        .await
        .unwrap();
    CreatePersonalAccessTokensTable.up().await.unwrap();
}

pub async fn taylor() -> User {
    User::create(json!({
        "name": "Taylor",
        "email": "taylor@laravel.com",
        "password": "secret-hash",
    }))
    .await
    .unwrap()
}

pub async fn abigail() -> User {
    User::create(json!({
        "name": "Abigail",
        "email": "abigail@laravel.com",
        "password": "another-hash",
    }))
    .await
    .unwrap()
}

/// `GET /api/user` (in the `api` group, behind `auth:sanctum`): who is
/// authenticated, and with which token.
pub fn user_route(app: &App) {
    app.router()
        .get("/api/user", |request: Request| async move {
            let user = request.auth_user().unwrap();
            let token = Sanctum::access_token_for(&user);
            let kind = if user.is::<User>() {
                "User"
            } else if user.is::<Admin>() {
                "Admin"
            } else {
                "unknown"
            };
            Json(json!({
                "type": kind,
                "id": user.id(),
                "name": user.to_value()["name"],
                "token": token.as_ref().and_then(|token| token.as_personal()).map(|token| token.id),
                "transient": token.as_ref().map(AccessToken::is_transient),
                "can_update": token.as_ref().is_some_and(|token| token.can("server:update")),
                "auth_id": request.attribute("_auth_id"),
                "stateful": request.attribute("sanctum"),
            }))
        })
        .middleware(("api", "auth:sanctum"));
}

/// Freeze "now" for the test (the current-thread runtime runs every task
/// on this thread).
pub fn travel_to(date: &str) -> Carbon {
    let now = Carbon::parse(date).unwrap();
    Carbon::set_thread_test_now(Some(now));
    now
}

// ----------------------------------------------------------------------
// A browser: a cookie jar in front of the router
// ----------------------------------------------------------------------

pub struct Browser {
    router: Arc<Router>,
    cookies: BTreeMap<String, String>,
}

impl Browser {
    pub fn new(app: &App) -> Self {
        Self {
            router: app.router(),
            cookies: BTreeMap::new(),
        }
    }

    pub fn cookie(&self, name: &str) -> Option<String> {
        self.cookies.get(name).cloned()
    }

    pub fn request(
        &self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        input: Value,
    ) -> Request {
        let mut map = HeaderMap::new();
        if !self.cookies.is_empty() {
            let cookie = self
                .cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            map.insert("cookie", HeaderValue::from_str(&cookie).unwrap());
        }
        for (name, value) in headers {
            map.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        Request::create_with(uri, method, input, map)
    }

    pub async fn send(&mut self, request: Request) -> Response {
        let response = self.router.dispatch(request).await;
        for cookie in response.cookies() {
            if cookie.is_cleared() {
                self.cookies.remove(&cookie.name);
            } else {
                self.cookies
                    .insert(cookie.name.clone(), cookie.value.clone());
            }
        }
        response
    }

    pub async fn get(&mut self, uri: &str, headers: &[(&str, &str)]) -> Response {
        let request = self.request("GET", uri, headers, json!({}));
        self.send(request).await
    }

    pub async fn post(&mut self, uri: &str, headers: &[(&str, &str)], input: Value) -> Response {
        let request = self.request("POST", uri, headers, input);
        self.send(request).await
    }
}

/// Send a JSON API request with the given bearer token.
pub async fn api(app: &App, method: &str, uri: &str, token: &str) -> Response {
    let authorization = format!("Bearer {token}");
    let request = Browser::new(app).request(
        method,
        uri,
        &[
            ("authorization", authorization.as_str()),
            ("accept", "application/json"),
        ],
        json!({}),
    );
    app.router().dispatch(request).await
}

/// Send a JSON API request without credentials.
pub async fn guest(app: &App, method: &str, uri: &str) -> Response {
    let request =
        Browser::new(app).request(method, uri, &[("accept", "application/json")], json!({}));
    app.router().dispatch(request).await
}
