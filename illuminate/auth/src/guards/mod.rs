//! Authentication guards: how users are authenticated for each request.

mod request;
mod session;
mod token;

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use illuminate_config::Repository;
use illuminate_container::Container;
use illuminate_http::{Request, async_trait, current_request};
use illuminate_session::{RequestSessionExt, Store};
use illuminate_support::error::{InvalidArgumentException, RuntimeException};
use illuminate_support::{Result, Value, ValueExt};

use crate::exceptions::AuthenticationException;
use crate::manager::ProviderFactory;
use crate::providers::UserProvider;
use crate::state::{AuthState, GuardState};
use crate::user::AuthUser;

pub(crate) use request::request_callback as request_callback_for;
pub use request::{IntoUserResult, RequestGuard, RequestGuardCallback};
pub use session::{Recaller, SessionGuard};
pub use token::TokenGuard;

/// A callback deciding whether a user who passed `attempt_when`'s
/// credential check may log in.
pub type AttemptCallback = Arc<dyn Fn(&AuthUser) -> bool + Send + Sync>;

/// An authentication guard — Laravel's `Guard` and `StatefulGuard`
/// contracts in one.
///
/// Every guard can tell you who the current user is. Stateful guards (the
/// `session` guard) can also log users in and out; the other guards answer
/// those calls with an error.
///
/// Guards are shared between every request the application is handling,
/// so they keep what they learn about a request in that request's
/// [`AuthState`] — never in themselves.
#[async_trait]
pub trait Guard: Send + Sync + 'static {
    /// The guard's name (its key in the `auth.guards` configuration).
    fn name(&self) -> &str;

    /// Get the currently authenticated user, reporting any failure of the
    /// underlying user provider.
    async fn try_user(&self) -> Result<Option<AuthUser>>;

    /// Get the currently authenticated user.
    async fn user(&self) -> Option<AuthUser> {
        self.try_user().await.ok().flatten()
    }

    /// Determine if the current user is authenticated.
    async fn check(&self) -> bool {
        self.user().await.is_some()
    }

    /// Determine if the current user is a guest.
    async fn guest(&self) -> bool {
        !self.check().await
    }

    /// The identifier of the currently authenticated user.
    async fn id(&self) -> Option<Value> {
        self.user().await.map(|user| user.id())
    }

    /// Get the authenticated user, or fail with an [`AuthenticationException`].
    async fn authenticate(&self) -> Result<AuthUser> {
        match self.try_user().await? {
            Some(user) => Ok(user),
            None => Err(AuthenticationException::new(vec![self.name().to_string()], None).into()),
        }
    }

    /// Validate a user's credentials without logging them in.
    async fn validate(&self, credentials: &Value) -> Result<bool>;

    /// Determine if the guard has a user for the current request (without
    /// trying to resolve one).
    fn has_user(&self) -> bool {
        AuthState::current().user(self.name()).is_some()
    }

    /// Set the current user.
    fn set_user(&self, user: AuthUser);

    /// Forget the current user (it will be resolved again when needed).
    fn forget_user(&self);

    /// The user provider used by the guard.
    fn provider(&self) -> Option<Arc<dyn UserProvider>>;

    /// Attempt to authenticate a user using the given credentials, logging
    /// them in (and optionally remembering them) when they're valid.
    async fn attempt(&self, _credentials: &Value, _remember: bool) -> Result<bool> {
        Err(unsupported(self.name(), "attempt"))
    }

    /// Like [`attempt`](Guard::attempt), but every callback must also
    /// approve the user.
    async fn attempt_when(
        &self,
        _credentials: &Value,
        _callbacks: &[AttemptCallback],
        _remember: bool,
    ) -> Result<bool> {
        Err(unsupported(self.name(), "attempt_when"))
    }

    /// Log a user into the application for this request only.
    async fn once(&self, _credentials: &Value) -> Result<bool> {
        Err(unsupported(self.name(), "once"))
    }

    /// Log the given user ID into the application for this request only.
    async fn once_using_id(&self, _id: &Value) -> Result<Option<AuthUser>> {
        Err(unsupported(self.name(), "once_using_id"))
    }

    /// Log a user into the application.
    async fn login(&self, _user: AuthUser, _remember: bool) -> Result<()> {
        Err(unsupported(self.name(), "login"))
    }

    /// Log the given user ID into the application.
    async fn login_using_id(&self, _id: &Value, _remember: bool) -> Result<Option<AuthUser>> {
        Err(unsupported(self.name(), "login_using_id"))
    }

    /// Log the user out of the application.
    async fn logout(&self) -> Result<()> {
        Err(unsupported(self.name(), "logout"))
    }

    /// Log the user out of the application on their current device only
    /// (their "remember me" token is left alone).
    async fn logout_current_device(&self) -> Result<()> {
        Err(unsupported(self.name(), "logout_current_device"))
    }

    /// Invalidate the user's sessions on their other devices by rehashing
    /// their password (requires the `auth.session` middleware).
    async fn logout_other_devices(&self, _password: &str) -> Result<()> {
        Err(unsupported(self.name(), "logout_other_devices"))
    }

    /// Determine if the user was authenticated via the "remember me" cookie.
    fn via_remember(&self) -> bool {
        false
    }

    /// Authenticate the request with HTTP Basic credentials (`field` names
    /// the username column), logging the user in.
    async fn basic(&self, _field: &str, _extra: &Value) -> Result<()> {
        Err(unsupported(self.name(), "basic"))
    }

    /// Authenticate the request with HTTP Basic credentials, for this
    /// request only.
    async fn once_basic(&self, _field: &str, _extra: &Value) -> Result<()> {
        Err(unsupported(self.name(), "once_basic"))
    }

    /// The guard as `Any`, to reach driver-specific methods.
    fn as_any(&self) -> &dyn Any;
}

impl dyn Guard {
    /// Get the currently authenticated user as a concrete type.
    ///
    /// ```ignore
    /// let admin: Option<Admin> = Auth::guard("admin").user_as().await;
    /// ```
    pub async fn user_as<U: crate::user::FromAuthUser>(&self) -> Option<U> {
        self.user().await.and_then(|user| U::from_auth_user(&user))
    }

    /// Borrow the guard as its concrete driver (`SessionGuard`, ...).
    pub fn downcast_ref<G: Guard>(&self) -> Option<&G> {
        self.as_any().downcast_ref::<G>()
    }
}

fn unsupported(guard: &str, method: &str) -> illuminate_support::Error {
    RuntimeException::new(format!(
        "Method [{method}] is not supported by the [{guard}] guard."
    ))
    .into()
}

/// What the auth manager shares with the guards it creates: configuration,
/// the fallback state used outside of requests, and test overrides.
pub(crate) struct Shared {
    pub(crate) config: Arc<Repository>,
    pub(crate) fallback: Arc<AuthState>,
    acting_as: RwLock<HashMap<String, AuthUser>>,
    default_guard: RwLock<Option<String>>,
    provider_creators: RwLock<HashMap<String, ProviderFactory>>,
}

impl Shared {
    pub(crate) fn new(config: Arc<Repository>) -> Self {
        Self {
            config,
            fallback: Arc::new(AuthState::new()),
            acting_as: RwLock::new(HashMap::new()),
            default_guard: RwLock::new(None),
            provider_creators: RwLock::new(HashMap::new()),
        }
    }

    pub(crate) fn register_provider(&self, driver: String, factory: ProviderFactory) {
        self.provider_creators
            .write()
            .unwrap()
            .insert(driver, factory);
    }

    pub(crate) fn has_provider(&self, driver: &str) -> bool {
        self.provider_creators.read().unwrap().contains_key(driver)
    }

    /// The default user provider's name (`auth.defaults.provider`).
    pub(crate) fn default_user_provider(&self) -> Option<String> {
        match self.config.get("auth.defaults.provider") {
            Value::String(name) if !name.is_empty() => Some(name),
            _ => None,
        }
    }

    /// Create the user provider configured under `auth.providers.<name>`.
    pub(crate) fn create_user_provider(
        &self,
        name: Option<&str>,
    ) -> Result<Option<Arc<dyn UserProvider>>> {
        let name = match name {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => match self.default_user_provider() {
                Some(name) => name,
                None => return Ok(None),
            },
        };
        let config = self.config.get(&format!("auth.providers.{name}"));
        if config.is_null() {
            return Ok(None);
        }
        let driver = option(&config, "driver").unwrap_or_default();
        let creator = self.provider_creators.read().unwrap().get(&driver).cloned();
        match creator {
            Some(creator) => Ok(Some(creator(&Container::get_instance(), &config))),
            None => Err(InvalidArgumentException::new(format!(
                "Authentication user provider [{driver}] is not defined."
            ))
            .into()),
        }
    }

    /// The request being handled (if any) and its auth state.
    pub(crate) fn context(&self) -> Context {
        match current_request() {
            Some(request) => Context {
                state: AuthState::for_request(&request),
                request: Some(request),
            },
            None => Context {
                request: None,
                state: self.fallback.clone(),
            },
        }
    }

    /// The application-wide default guard (ignoring per-request overrides).
    pub(crate) fn app_default_guard(&self) -> String {
        if let Some(guard) = self.default_guard.read().unwrap().clone() {
            return guard;
        }
        match self.config.get("auth.defaults.guard") {
            Value::String(guard) if !guard.is_empty() => guard,
            _ => "web".to_string(),
        }
    }

    /// The default guard for the given state (a per-request override wins).
    pub(crate) fn default_guard(&self, state: &AuthState) -> String {
        state
            .default_guard()
            .unwrap_or_else(|| self.app_default_guard())
    }

    pub(crate) fn set_default_guard(&self, guard: Option<String>) {
        *self.default_guard.write().unwrap() = guard;
    }

    pub(crate) fn acting_as(&self, guard: &str) -> Option<AuthUser> {
        self.acting_as.read().unwrap().get(guard).cloned()
    }

    pub(crate) fn set_acting_as(&self, guard: &str, user: AuthUser) {
        self.acting_as
            .write()
            .unwrap()
            .insert(guard.to_string(), user);
    }

    pub(crate) fn forget_acting_as(&self, guard: Option<&str>) {
        let mut acting_as = self.acting_as.write().unwrap();
        match guard {
            Some(guard) => {
                acting_as.remove(guard);
            }
            None => acting_as.clear(),
        }
    }

    /// Keep the `_auth_id` request attribute (read by the `throttle`
    /// middleware) in sync with the request's default guard.
    pub(crate) fn sync_request_user(&self, context: &Context) {
        if let Some(request) = &context.request {
            let guard = self.default_guard(&context.state);
            let id = context
                .state
                .user(&guard)
                .map(|user| user.id())
                .unwrap_or(Value::Null);
            request.set_attribute("_auth_id", id);
        }
    }

    /// Record the user a guard resolved (or set) for the request.
    pub(crate) fn remember_user(&self, context: &Context, guard: &str, user: Option<AuthUser>) {
        context.state.update(guard, |state| {
            state.resolved = true;
            if user.is_some() {
                state.logged_out = false;
            }
            state.user = user;
        });
        self.sync_request_user(context);
    }

    /// Forget the user a guard resolved, so it will be resolved again.
    pub(crate) fn forget_user(&self, context: &Context, guard: &str) {
        context.state.update(guard, |state| {
            state.user = None;
            state.resolved = false;
        });
        self.sync_request_user(context);
    }

    /// The user an already-resolved guard knows about: its state, or a
    /// user set with `acting_as`. `None` means the guard must resolve.
    pub(crate) fn known_user(&self, context: &Context, guard: &str) -> Known {
        let state = context.state.guard(guard);
        if state.logged_out {
            return Known::Guest;
        }
        if let Some(user) = state.user {
            return Known::User(user);
        }
        if let Some(user) = self.acting_as(guard) {
            self.remember_user(context, guard, Some(user.clone()));
            return Known::User(user);
        }
        if state.resolved {
            return Known::Guest;
        }
        Known::Unknown
    }
}

/// What a guard already knows about the current request's user.
pub(crate) enum Known {
    User(AuthUser),
    Guest,
    Unknown,
}

/// The request a guard is working on, and its auth state.
pub(crate) struct Context {
    pub(crate) request: Option<Request>,
    pub(crate) state: Arc<AuthState>,
}

impl Context {
    pub(crate) fn session(&self) -> Option<Arc<Store>> {
        self.request
            .as_ref()
            .and_then(|request| request.try_session())
    }

    pub(crate) fn guard(&self, guard: &str) -> GuardState {
        self.state.guard(guard)
    }
}

/// Read a guard option from its configuration.
pub(crate) fn option(config: &Value, key: &str) -> Option<String> {
    match config.get(key) {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.to_string_lossy()).filter(|v| !v.is_empty()),
    }
}
