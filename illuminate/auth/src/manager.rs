//! The auth manager: builds guards and user providers from configuration.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use illuminate_config::Repository;
use illuminate_container::Container;
use illuminate_http::{BoxFuture, Request};
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Result, Value};

use crate::access::GateArgument;
use crate::guards::{
    AttemptCallback, Guard, IntoUserResult, RequestGuard, SessionGuard, Shared, TokenGuard, option,
    request_callback_for,
};
use crate::providers::{ArrayUserProvider, UserProvider};
use crate::state::AuthState;
use crate::user::{AuthUser, AuthUserRef, FromAuthUser};

/// Builds a custom guard: `(app, name, config)`.
pub type GuardFactory = Arc<dyn Fn(&Container, &str, &Value) -> Result<Arc<dyn Guard>> + Send + Sync>;

/// Builds a custom user provider: `(app, config)`.
pub type ProviderFactory = Arc<dyn Fn(&Container, &Value) -> Arc<dyn UserProvider> + Send + Sync>;

/// Resolves "the current user" for gates and other services: `(guard)`.
pub type UserResolver = Arc<dyn Fn(Option<String>) -> BoxFuture<'static, Option<AuthUser>> + Send + Sync>;

type GuestRedirect = Arc<dyn Fn(&Request) -> Option<String> + Send + Sync>;
type UserRedirect = Arc<dyn Fn(&Request) -> String + Send + Sync>;
type RouteUrl = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
type ArgumentResolver = Arc<dyn Fn(&Request, &str) -> Option<GateArgument<'static>> + Send + Sync>;

/// The application's authentication redirect and resolution hooks, shared
/// by the auth middleware (Laravel keeps these in static properties).
#[derive(Default)]
pub struct AuthHooks {
    guest_redirect: RwLock<Option<GuestRedirect>>,
    user_redirect: RwLock<Option<UserRedirect>>,
    route_url: RwLock<Option<RouteUrl>>,
    argument_resolver: RwLock<Option<ArgumentResolver>>,
}

impl AuthHooks {
    /// Set where unauthenticated users are redirected (`None` responds
    /// with a plain `401`).
    pub fn set_guest_redirect(&self, callback: impl Fn(&Request) -> Option<String> + Send + Sync + 'static) {
        *self.guest_redirect.write().unwrap() = Some(Arc::new(callback));
    }

    /// Set where authenticated users are sent by the `guest` middleware.
    pub fn set_user_redirect(&self, callback: impl Fn(&Request) -> String + Send + Sync + 'static) {
        *self.user_redirect.write().unwrap() = Some(Arc::new(callback));
    }

    /// Teach the auth middleware how to turn a route name into a URL.
    pub fn set_route_url_resolver(&self, callback: impl Fn(&str) -> Option<String> + Send + Sync + 'static) {
        *self.route_url.write().unwrap() = Some(Arc::new(callback));
    }

    /// Teach the `can` middleware how to turn a route parameter into a gate
    /// argument (route model binding).
    pub fn set_argument_resolver(
        &self,
        callback: impl Fn(&Request, &str) -> Option<GateArgument<'static>> + Send + Sync + 'static,
    ) {
        *self.argument_resolver.write().unwrap() = Some(Arc::new(callback));
    }

    /// Where an unauthenticated request should be redirected: the
    /// configured callback, the `login` route, or `/login`.
    pub fn guest_redirect(&self, request: &Request) -> Option<String> {
        let callback = self.guest_redirect.read().unwrap().clone();
        match callback {
            Some(callback) => callback(request),
            None => Some(self.route_url_or("login", "/login")),
        }
    }

    /// Where the `guest` middleware sends authenticated users: the
    /// configured callback, the `dashboard` or `home` route, or `/`.
    pub fn user_redirect(&self, request: &Request) -> String {
        let callback = self.user_redirect.read().unwrap().clone();
        if let Some(callback) = callback {
            return callback(request);
        }
        ["dashboard", "home"]
            .iter()
            .find_map(|name| self.route_url(name))
            .unwrap_or_else(|| "/".to_string())
    }

    /// The URL of a named route, when a resolver has been registered.
    pub fn route_url(&self, name: &str) -> Option<String> {
        let resolver = self.route_url.read().unwrap().clone();
        resolver.and_then(|resolver| resolver(name))
    }

    /// The URL of a named route, or a fallback path.
    pub fn route_url_or(&self, name: &str, fallback: &str) -> String {
        self.route_url(name).unwrap_or_else(|| fallback.to_string())
    }

    /// Resolve a `can` middleware parameter into a gate argument.
    pub fn resolve_argument(&self, request: &Request, parameter: &str) -> Option<GateArgument<'static>> {
        let resolver = self.argument_resolver.read().unwrap().clone();
        resolver.and_then(|resolver| resolver(request, parameter))
    }
}

/// The auth manager — the service behind the `Auth` facade.
///
/// Guards are built lazily from `auth.guards.<name>` and cached; user
/// providers come from `auth.providers.<name>`. Calls that don't name a
/// guard go to the default one (`auth.defaults.guard`, or whatever the
/// current request chose with [`should_use`](AuthManager::should_use)).
pub struct AuthManager {
    shared: Arc<Shared>,
    guards: RwLock<HashMap<String, Arc<dyn Guard>>>,
    custom_creators: RwLock<HashMap<String, GuardFactory>>,
    user_resolver: RwLock<Option<UserResolver>>,
    hooks: AuthHooks,
}

impl AuthManager {
    /// Create a manager reading the given configuration.
    pub fn new(config: Arc<Repository>) -> Self {
        let manager = Self {
            shared: Arc::new(Shared::new(config)),
            guards: RwLock::new(HashMap::new()),
            custom_creators: RwLock::new(HashMap::new()),
            user_resolver: RwLock::new(None),
            hooks: AuthHooks::default(),
        };
        manager.provider("array", |_app, config| {
            Arc::new(ArrayUserProvider::from_value(config.get("users").unwrap_or(&Value::Null)))
        });
        manager
    }

    /// The configuration the manager reads.
    pub fn config(&self) -> Arc<Repository> {
        self.shared.config.clone()
    }

    /// The redirect and resolution hooks used by the auth middleware.
    pub fn hooks(&self) -> &AuthHooks {
        &self.hooks
    }

    /// The state used when no request is being handled.
    pub fn fallback_state(&self) -> Arc<AuthState> {
        self.shared.fallback.clone()
    }

    // ------------------------------------------------------------------
    // Guards
    // ------------------------------------------------------------------

    /// Get a guard by name (`None` for the default guard).
    pub fn guard(&self, name: Option<&str>) -> Result<Arc<dyn Guard>> {
        let name = match name {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => self.get_default_driver(),
        };
        if let Some(guard) = self.guards.read().unwrap().get(&name) {
            return Ok(guard.clone());
        }
        let guard = self.resolve(&name)?;
        Ok(self
            .guards
            .write()
            .unwrap()
            .entry(name)
            .or_insert(guard)
            .clone())
    }

    fn resolve(&self, name: &str) -> Result<Arc<dyn Guard>> {
        let config = self.shared.config.get(&format!("auth.guards.{name}"));
        if config.is_null() {
            return Err(InvalidArgumentException::new(format!("Auth guard [{name}] is not defined.")).into());
        }
        let driver = option(&config, "driver").unwrap_or_default();

        let creator = self.custom_creators.read().unwrap().get(&driver).cloned();
        if let Some(creator) = creator {
            return creator(&Container::get_instance(), name, &config);
        }

        match driver.as_str() {
            "session" => Ok(Arc::new(SessionGuard::with_shared(
                name.to_string(),
                self.required_provider(name, &config)?,
                self.shared.clone(),
                &config,
            ))),
            "token" => Ok(Arc::new(TokenGuard::with_shared(
                name.to_string(),
                self.required_provider(name, &config)?,
                self.shared.clone(),
                &config,
            ))),
            _ => Err(InvalidArgumentException::new(format!(
                "Auth driver [{driver}] for guard [{name}] is not defined."
            ))
            .into()),
        }
    }

    fn required_provider(&self, guard: &str, config: &Value) -> Result<Arc<dyn UserProvider>> {
        let provider = option(config, "provider");
        self.create_user_provider(provider.as_deref())?.ok_or_else(|| {
            InvalidArgumentException::new(format!("Auth guard [{guard}] requires a user provider.")).into()
        })
    }

    /// The default guard's name: the current request's choice, the
    /// application's, or `auth.defaults.guard`.
    pub fn get_default_driver(&self) -> String {
        self.shared.default_guard(&AuthState::current())
    }

    /// Set the default guard for the whole application.
    pub fn set_default_driver(&self, name: &str) {
        self.shared.set_default_guard(Some(name.to_string()));
    }

    /// Use the given guard (`None` for the configured default) for the rest
    /// of the current request — `request.user()`, gates and the `auth()`
    /// helpers all follow it.
    pub fn should_use(&self, name: Option<&str>) {
        let context = self.shared.context();
        let name = name
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.shared.app_default_guard());
        match &context.request {
            Some(_) => context.state.set_default_guard(Some(name)),
            None => self.shared.set_default_guard(Some(name)),
        }
        self.shared.sync_request_user(&context);
    }

    /// Register a custom guard driver.
    ///
    /// ```ignore
    /// Auth::extend("jwt", |_app, name, config| {
    ///     let provider = Auth::create_user_provider(config["provider"].as_str())?.unwrap();
    ///     Ok(Arc::new(JwtGuard::new(name, provider)))
    /// });
    /// ```
    pub fn extend(
        &self,
        driver: impl Into<String>,
        factory: impl Fn(&Container, &str, &Value) -> Result<Arc<dyn Guard>> + Send + Sync + 'static,
    ) -> &Self {
        self.custom_creators
            .write()
            .unwrap()
            .insert(driver.into(), Arc::new(factory));
        self.forget_guards();
        self
    }

    /// Register a guard driver that authenticates requests with a closure.
    pub fn via_request<F, Fut, R>(&self, driver: impl Into<String>, callback: F) -> &Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoUserResult + 'static,
    {
        let callback = request_callback_for(callback);
        let shared = self.shared.clone();
        self.extend(driver, move |_app, name, config| {
            let provider = shared.create_user_provider(option(config, "provider").as_deref())?;
            Ok(Arc::new(RequestGuard::with_shared(
                name.to_string(),
                callback.clone(),
                provider,
                shared.clone(),
            )))
        })
    }

    /// Register a custom user provider driver.
    ///
    /// ```ignore
    /// Auth::provider("eloquent", |_app, config| Arc::new(EloquentUserProvider::<User>::new(config)));
    /// ```
    pub fn provider(
        &self,
        driver: impl Into<String>,
        factory: impl Fn(&Container, &Value) -> Arc<dyn UserProvider> + Send + Sync + 'static,
    ) -> &Self {
        self.shared.register_provider(driver.into(), Arc::new(factory));
        self.forget_guards();
        self
    }

    /// Determine if a user provider driver has been registered.
    pub fn has_provider_driver(&self, driver: &str) -> bool {
        self.shared.has_provider(driver)
    }

    /// Create the user provider configured under `auth.providers.<name>`
    /// (`None` for `auth.defaults.provider`). Returns `Ok(None)` when no
    /// such provider is configured.
    pub fn create_user_provider(&self, name: Option<&str>) -> Result<Option<Arc<dyn UserProvider>>> {
        self.shared.create_user_provider(name)
    }

    /// The default user provider's name (`auth.defaults.provider`).
    pub fn get_default_user_provider(&self) -> Option<String> {
        self.shared.default_user_provider()
    }

    /// Determine if any guards have been built.
    pub fn has_resolved_guards(&self) -> bool {
        !self.guards.read().unwrap().is_empty()
    }

    /// Forget every guard that has been built.
    pub fn forget_guards(&self) -> &Self {
        self.guards.write().unwrap().clear();
        self
    }

    // ------------------------------------------------------------------
    // The user resolver
    // ------------------------------------------------------------------

    /// Replace how "the current user" is resolved for gates.
    pub fn resolve_users_using<F, Fut>(&self, resolver: F) -> &Self
    where
        F: Fn(Option<String>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<AuthUser>> + Send + 'static,
    {
        *self.user_resolver.write().unwrap() = Some(Arc::new(move |guard| Box::pin(resolver(guard))));
        self
    }

    /// The custom user resolver, if one was registered.
    pub fn user_resolver(&self) -> Option<UserResolver> {
        self.user_resolver.read().unwrap().clone()
    }

    /// Resolve the current user through the user resolver (by default, the
    /// given guard's user).
    pub async fn resolve_user(&self, guard: Option<&str>) -> Option<AuthUser> {
        match self.user_resolver() {
            Some(resolver) => resolver(guard.map(str::to_string)).await,
            None => self.guard(guard).ok()?.user().await,
        }
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    /// Authenticate as the given user for the rest of the test (every
    /// request handled by this application), and make the guard the
    /// default.
    pub fn acting_as(&self, user: impl Into<AuthUser>, guard: Option<&str>) {
        let user = user.into();
        let guard = guard
            .filter(|guard| !guard.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.get_default_driver());
        self.shared.set_acting_as(&guard, user.clone());
        let context = self.shared.context();
        context.state.update(&guard, |state| {
            state.user = Some(user);
            state.resolved = true;
            state.logged_out = false;
        });
        self.should_use(Some(&guard));
    }

    /// Stop authenticating as a user set with [`acting_as`](AuthManager::acting_as).
    pub fn forget_acting_as(&self) {
        self.shared.forget_acting_as(None);
        self.shared.fallback.flush();
    }

    // ------------------------------------------------------------------
    // The default guard
    // ------------------------------------------------------------------

    fn default_guard(&self) -> Arc<dyn Guard> {
        match self.guard(None) {
            Ok(guard) => guard,
            Err(error) => panic!("{error}"),
        }
    }

    /// Determine if the current user is authenticated.
    pub async fn check(&self) -> bool {
        self.default_guard().check().await
    }

    /// Determine if the current user is a guest.
    pub async fn guest(&self) -> bool {
        self.default_guard().guest().await
    }

    /// The currently authenticated user, as a concrete type (or `AuthUser`).
    pub async fn user<U: FromAuthUser>(&self) -> Option<U> {
        self.auth_user().await.and_then(|user| U::from_auth_user(&user))
    }

    /// The currently authenticated user.
    pub async fn auth_user(&self) -> Option<AuthUser> {
        self.default_guard().user().await
    }

    /// The currently authenticated user as a JSON value.
    pub async fn user_value(&self) -> Option<Value> {
        self.auth_user().await.map(|user| user.to_value())
    }

    /// The ID of the currently authenticated user.
    pub async fn id(&self) -> Option<Value> {
        self.default_guard().id().await
    }

    /// Get the authenticated user or fail with an `AuthenticationException`.
    pub async fn authenticate(&self) -> Result<AuthUser> {
        self.guard(None)?.authenticate().await
    }

    /// Validate a user's credentials.
    pub async fn validate(&self, credentials: &Value) -> Result<bool> {
        self.guard(None)?.validate(credentials).await
    }

    /// Attempt to authenticate a user using the given credentials.
    pub async fn attempt(&self, credentials: &Value, remember: bool) -> Result<bool> {
        self.guard(None)?.attempt(credentials, remember).await
    }

    /// Attempt to authenticate a user, running the callback to decide
    /// whether a user with valid credentials may log in.
    pub async fn attempt_when<U, F>(&self, credentials: &Value, callback: F, remember: bool) -> Result<bool>
    where
        U: AuthUserRef,
        F: Fn(&U) -> bool + Send + Sync + 'static,
    {
        let callback: AttemptCallback =
            Arc::new(move |user: &AuthUser| U::from_auth_user_ref(user).is_some_and(|user| callback(user)));
        self.guard(None)?
            .attempt_when(credentials, &[callback], remember)
            .await
    }

    /// Log a user in for this request only.
    pub async fn once(&self, credentials: &Value) -> Result<bool> {
        self.guard(None)?.once(credentials).await
    }

    /// Log the given user ID in for this request only.
    pub async fn once_using_id(&self, id: impl Into<Value>) -> Result<Option<AuthUser>> {
        self.guard(None)?.once_using_id(&id.into()).await
    }

    /// Log a user into the application.
    pub async fn login(&self, user: impl Into<AuthUser>, remember: bool) -> Result<()> {
        self.guard(None)?.login(user.into(), remember).await
    }

    /// Log the given user ID into the application.
    pub async fn login_using_id(&self, id: impl Into<Value>, remember: bool) -> Result<Option<AuthUser>> {
        self.guard(None)?.login_using_id(&id.into(), remember).await
    }

    /// Log the user out of the application.
    pub async fn logout(&self) -> Result<()> {
        self.guard(None)?.logout().await
    }

    /// Log the user out of the application on their current device only.
    pub async fn logout_current_device(&self) -> Result<()> {
        self.guard(None)?.logout_current_device().await
    }

    /// Invalidate the user's sessions on their other devices.
    pub async fn logout_other_devices(&self, password: &str) -> Result<()> {
        self.guard(None)?.logout_other_devices(password).await
    }

    /// Determine if the user was authenticated via "remember me".
    pub fn via_remember(&self) -> bool {
        self.default_guard().via_remember()
    }

    /// Determine if the default guard has a user (without resolving one).
    pub fn has_user(&self) -> bool {
        self.default_guard().has_user()
    }

    /// Set the current user.
    pub fn set_user(&self, user: impl Into<AuthUser>) {
        self.default_guard().set_user(user.into());
    }

    /// Forget the current user.
    pub fn forget_user(&self) {
        self.default_guard().forget_user();
    }

    /// The user the given request resolved for its default guard, without
    /// resolving anything (used by the synchronous request helpers).
    pub fn resolved_user(&self, state: &AuthState, guard: Option<&str>) -> Option<AuthUser> {
        let guard = guard
            .map(str::to_string)
            .unwrap_or_else(|| self.shared.default_guard(state));
        let snapshot = state.guard(&guard);
        if snapshot.logged_out {
            return None;
        }
        snapshot.user.or_else(|| self.shared.acting_as(&guard))
    }
}

impl std::fmt::Debug for AuthManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthManager")
            .field("default", &self.shared.app_default_guard())
            .field("guards", &self.guards.read().unwrap().keys().collect::<Vec<_>>())
            .finish()
    }
}
