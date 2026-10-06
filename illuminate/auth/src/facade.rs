//! The `Auth` facade and the `auth()` helper.

use std::future::Future;
use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, try_app};
use illuminate_http::Request;
use illuminate_support::{Result, Value};

use crate::guards::{Guard, IntoUserResult};
use crate::manager::AuthManager;
use crate::providers::UserProvider;
use crate::user::{AuthUser, AuthUserRef, FromAuthUser};

/// Resolve the application's auth manager, registering one (configured
/// from the container's `Repository`) if the application hasn't.
pub fn manager() -> Arc<AuthManager> {
    if let Some(manager) = try_app::<AuthManager>() {
        return manager;
    }
    let container = Container::get_instance();
    container.singleton_if::<AuthManager>(|c| {
        let config = c
            .try_make::<Repository>()
            .unwrap_or_else(|_| Arc::new(Repository::empty()));
        Arc::new(AuthManager::new(config))
    });
    container.make::<AuthManager>()
}

/// Get the auth manager — Laravel's `auth()` helper.
///
/// ```ignore
/// let user: Option<User> = auth().user().await;
/// let admin = auth().guard(Some("admin"))?;
/// ```
pub fn auth() -> Arc<AuthManager> {
    manager()
}

/// The `Auth` facade: authenticate users and find out who they are.
///
/// Calls that don't name a guard use the default one. Anything that may
/// need to look the user up (in the session, a cookie, the database) is
/// `async`.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_auth::{Auth, GenericUser};
/// use illuminate_config::Repository;
/// use illuminate_container::Container;
/// use illuminate_support::json;
///
/// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// # runtime.block_on(async {
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({
///     "auth": {
///         "defaults": {"guard": "web"},
///         "guards": {"web": {"driver": "session", "provider": "users"}},
///         "providers": {"users": {"driver": "array", "users": [{"id": 1, "name": "Taylor"}]}},
///     },
/// })));
///
/// assert!(Auth::guest().await);
///
/// Auth::login_using_id(1, false).await.unwrap();
///
/// assert!(Auth::check().await);
/// assert_eq!(Auth::id().await, Some(json!(1)));
///
/// let user: Option<GenericUser> = Auth::user().await;
/// assert_eq!(user.unwrap().get("name"), json!("Taylor"));
/// # });
/// ```
pub struct Auth;

impl Auth {
    /// The auth manager behind the facade.
    pub fn manager() -> Arc<AuthManager> {
        manager()
    }

    /// Get a guard by name.
    ///
    /// # Panics
    ///
    /// Panics when the guard (or its user provider) isn't configured, just
    /// like Laravel throws. Use [`Auth::try_guard`] to handle that yourself.
    pub fn guard(name: &str) -> Arc<dyn Guard> {
        match manager().guard(Some(name)) {
            Ok(guard) => guard,
            Err(error) => panic!("{error}"),
        }
    }

    /// Get a guard by name, reporting configuration problems as errors.
    pub fn try_guard(name: &str) -> Result<Arc<dyn Guard>> {
        manager().guard(Some(name))
    }

    /// The default guard.
    pub fn default_guard() -> Result<Arc<dyn Guard>> {
        manager().guard(None)
    }

    /// Use the given guard for the rest of the current request.
    pub fn should_use(name: &str) {
        manager().should_use(Some(name));
    }

    /// The default guard's name.
    pub fn get_default_driver() -> String {
        manager().get_default_driver()
    }

    /// Set the application's default guard.
    pub fn set_default_driver(name: &str) {
        manager().set_default_driver(name);
    }

    /// Register a custom guard driver.
    pub fn extend(
        driver: impl Into<String>,
        factory: impl Fn(&Container, &str, &Value) -> Result<Arc<dyn Guard>> + Send + Sync + 'static,
    ) {
        manager().extend(driver, factory);
    }

    /// Register a custom user provider driver.
    pub fn provider(
        driver: impl Into<String>,
        factory: impl Fn(&Container, &Value) -> Arc<dyn UserProvider> + Send + Sync + 'static,
    ) {
        manager().provider(driver, factory);
    }

    /// Register a guard driver that authenticates requests with a closure.
    ///
    /// ```ignore
    /// Auth::via_request("custom-token", |request: Request| async move {
    ///     User::where_("token", request.string("token")).first().await
    /// });
    /// ```
    pub fn via_request<F, Fut, R>(driver: impl Into<String>, callback: F)
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoUserResult + 'static,
    {
        manager().via_request(driver, callback);
    }

    /// Create the configured user provider (`None` for the default one).
    pub fn create_user_provider(name: Option<&str>) -> Result<Option<Arc<dyn UserProvider>>> {
        manager().create_user_provider(name)
    }

    /// Replace how "the current user" is resolved for gates.
    pub fn resolve_users_using<F, Fut>(resolver: F)
    where
        F: Fn(Option<String>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<AuthUser>> + Send + 'static,
    {
        manager().resolve_users_using(resolver);
    }

    /// Teach the auth middleware how to turn route names (`login`,
    /// `dashboard`, `verification.notice`, `password.confirm`, ...) into
    /// URLs. Without a resolver, Laravel's conventional paths are used.
    pub fn resolve_routes_using(resolver: impl Fn(&str) -> Option<String> + Send + Sync + 'static) {
        manager().hooks().set_route_url_resolver(resolver);
    }

    /// Determine if the current user is authenticated.
    pub async fn check() -> bool {
        manager().check().await
    }

    /// Determine if the current user is a guest.
    pub async fn guest() -> bool {
        manager().guest().await
    }

    /// The currently authenticated user, as a concrete type:
    /// `let user: Option<User> = Auth::user().await;`
    pub async fn user<U: FromAuthUser>() -> Option<U> {
        manager().user::<U>().await
    }

    /// The currently authenticated user, of whatever type it is.
    pub async fn auth_user() -> Option<AuthUser> {
        manager().auth_user().await
    }

    /// The currently authenticated user as a JSON value.
    pub async fn user_value() -> Option<Value> {
        manager().user_value().await
    }

    /// The ID of the currently authenticated user.
    pub async fn id() -> Option<Value> {
        manager().id().await
    }

    /// Get the authenticated user, or fail with an `AuthenticationException`.
    pub async fn authenticate() -> Result<AuthUser> {
        manager().authenticate().await
    }

    /// Validate a user's credentials without logging them in.
    pub async fn validate(credentials: &Value) -> Result<bool> {
        manager().validate(credentials).await
    }

    /// Attempt to authenticate a user using the given credentials.
    ///
    /// ```ignore
    /// if Auth::attempt(&request.only(&["email", "password"]), request.boolean("remember")).await? {
    ///     request.session().regenerate(false).await?;
    ///     return Ok(redirect_intended("/dashboard"));
    /// }
    /// ```
    pub async fn attempt(credentials: &Value, remember: bool) -> Result<bool> {
        manager().attempt(credentials, remember).await
    }

    /// Attempt to authenticate a user, letting the callback inspect the
    /// user before they're logged in.
    pub async fn attempt_when<U, F>(credentials: &Value, callback: F, remember: bool) -> Result<bool>
    where
        U: AuthUserRef,
        F: Fn(&U) -> bool + Send + Sync + 'static,
    {
        manager().attempt_when(credentials, callback, remember).await
    }

    /// Log a user in for this request only (no session or cookies).
    pub async fn once(credentials: &Value) -> Result<bool> {
        manager().once(credentials).await
    }

    /// Log the given user ID in for this request only.
    pub async fn once_using_id(id: impl Into<Value>) -> Result<Option<AuthUser>> {
        manager().once_using_id(id).await
    }

    /// Log a user into the application.
    pub async fn login(user: impl Into<AuthUser>, remember: bool) -> Result<()> {
        manager().login(user, remember).await
    }

    /// Log the given user ID into the application.
    pub async fn login_using_id(id: impl Into<Value>, remember: bool) -> Result<Option<AuthUser>> {
        manager().login_using_id(id, remember).await
    }

    /// Log the user out of the application.
    pub async fn logout() -> Result<()> {
        manager().logout().await
    }

    /// Log the user out on their current device only.
    pub async fn logout_current_device() -> Result<()> {
        manager().logout_current_device().await
    }

    /// Invalidate the user's sessions on their other devices.
    pub async fn logout_other_devices(password: &str) -> Result<()> {
        manager().logout_other_devices(password).await
    }

    /// Determine if the user was authenticated via "remember me".
    pub fn via_remember() -> bool {
        manager().via_remember()
    }

    /// Determine if the default guard has a user (without resolving one).
    pub fn has_user() -> bool {
        manager().has_user()
    }

    /// Set the current user.
    pub fn set_user(user: impl Into<AuthUser>) {
        manager().set_user(user);
    }

    /// Forget the current user.
    pub fn forget_user() {
        manager().forget_user();
    }

    /// Authenticate as the given user (for the rest of the test), on the
    /// given guard (`None` for the default one) — Laravel's `actingAs`.
    pub fn acting_as(user: impl Into<AuthUser>, guard: Option<&str>) {
        manager().acting_as(user, guard);
    }

    /// Stop acting as a user.
    pub fn forget_acting_as() {
        manager().forget_acting_as();
    }

    /// Forget every guard that has been built.
    pub fn forget_guards() {
        manager().forget_guards();
    }
}
