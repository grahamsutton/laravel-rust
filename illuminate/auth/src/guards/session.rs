//! The session guard: Laravel's classic, cookie-and-session based
//! authentication for web applications.

use std::any::Any;
use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_cookie::CookieQueue;
use illuminate_cookie::facades::Cookie;
use illuminate_hashing::Hash;
use illuminate_http::{HttpException, async_trait};
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Result, Str, Value, ValueExt, json};

use super::{AttemptCallback, Context, Guard, Known, Shared};
use crate::providers::UserProvider;
use crate::support::{basic_credentials, hash_equals, hmac_sha256, sha1};
use crate::user::AuthUser;

/// The class name Laravel hashes into its session keys. Using the same
/// name keeps the keys compatible with sessions written by Laravel.
const SESSION_GUARD_CLASS: &str = "Illuminate\\Auth\\SessionGuard";

/// The default "remember me" lifetime in minutes (400 days).
const DEFAULT_REMEMBER_DURATION: i64 = 576_000;

/// Authenticates users with the session, plus an optional long-lived
/// "remember me" cookie.
///
/// The user's identifier is stored in the session under
/// [`get_name`](SessionGuard::get_name) (`login_web_59ba36...`). When the
/// user asks to be remembered, a `remember_web_59ba36...` cookie holding
/// `id|remember_token|password_hash` is queued; the `EncryptCookies`
/// middleware encrypts it on the way out.
pub struct SessionGuard {
    name: String,
    provider: Arc<dyn UserProvider>,
    shared: Arc<Shared>,
    remember_duration: i64,
    rehash_on_login: bool,
    hash_key: Option<String>,
}

impl SessionGuard {
    /// Create a session guard with the given name and user provider.
    pub fn new(name: impl Into<String>, provider: Arc<dyn UserProvider>) -> Self {
        Self::with_shared(
            name.into(),
            provider,
            Arc::new(Shared::new(Arc::new(Repository::empty()))),
            &Value::Null,
        )
    }

    pub(crate) fn with_shared(
        name: String,
        provider: Arc<dyn UserProvider>,
        shared: Arc<Shared>,
        config: &Value,
    ) -> Self {
        let remember_duration = config
            .get("remember")
            .and_then(ValueExt::to_i64_lossy)
            .unwrap_or(DEFAULT_REMEMBER_DURATION);
        let rehash_on_login = match shared.config.get("hashing.rehash_on_login") {
            Value::Null => true,
            value => value.truthy(),
        };
        let hash_key = match shared.config.get("app.key") {
            Value::String(key) if !key.is_empty() => Some(key),
            _ => None,
        };
        Self {
            name,
            provider,
            shared,
            remember_duration,
            rehash_on_login,
            hash_key,
        }
    }

    /// Set the number of minutes the "remember me" cookie should be valid.
    pub fn set_remember_duration(mut self, minutes: i64) -> Self {
        self.remember_duration = minutes;
        self
    }

    /// The number of minutes the "remember me" cookie is valid.
    pub fn remember_duration(&self) -> i64 {
        self.remember_duration
    }

    /// Set the key used to sign password hashes stored in the session and
    /// the "remember me" cookie (the application key by default).
    pub fn set_hash_key(mut self, key: impl Into<String>) -> Self {
        self.hash_key = Some(key.into());
        self
    }

    /// The session key holding the authenticated user's identifier.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_auth::{ArrayUserProvider, GenericUser, SessionGuard};
    ///
    /// let guard = SessionGuard::new("web", Arc::new(ArrayUserProvider::<GenericUser>::default()));
    ///
    /// assert_eq!(guard.get_name(), "login_web_59ba36addc2b2f9401580f014c7f58ea4e30989d");
    /// assert_eq!(guard.get_recaller_name(), "remember_web_59ba36addc2b2f9401580f014c7f58ea4e30989d");
    /// ```
    pub fn get_name(&self) -> String {
        format!("login_{}_{}", self.name, sha1(SESSION_GUARD_CLASS))
    }

    /// The name of the "remember me" cookie.
    pub fn get_recaller_name(&self) -> String {
        format!("remember_{}_{}", self.name, sha1(SESSION_GUARD_CLASS))
    }

    /// The session key holding the signed password hash used to detect
    /// password changes on other devices.
    pub fn password_hash_key(&self) -> String {
        format!("password_hash_{}", self.name)
    }

    /// Sign a password hash for storage in the session or cookie.
    pub fn hash_password_for_cookie(&self, password_hash: &str) -> String {
        let key = self
            .hash_key
            .as_deref()
            .unwrap_or("base-key-for-password-hash-mac");
        hmac_sha256(password_hash, key.as_bytes())
    }

    /// The user most recently retrieved by `attempt`, `once`, or `validate`.
    pub fn last_attempted(&self) -> Option<AuthUser> {
        self.shared.context().guard(&self.name).last_attempted
    }

    /// The "remember me" cookie of the current request, if it has one.
    pub fn recaller(&self) -> Option<Recaller> {
        self.recaller_for(&self.shared.context())
    }

    fn recaller_for(&self, context: &Context) -> Option<Recaller> {
        let request = context.request.as_ref()?;
        request
            .cookie(&self.get_recaller_name())
            .filter(|value| !value.is_empty())
            .map(Recaller::new)
    }

    async fn user_from_recaller(
        &self,
        context: &Context,
        recaller: &Recaller,
    ) -> Result<Option<AuthUser>> {
        if !recaller.valid() || context.guard(&self.name).recall_attempted {
            return Ok(None);
        }
        context
            .state
            .update(&self.name, |state| state.recall_attempted = true);

        let user = self
            .provider
            .retrieve_by_token(&Value::String(recaller.id()), &recaller.token())
            .await?;
        let via_remember = user.is_some();
        context
            .state
            .update(&self.name, |state| state.via_remember = via_remember);

        let Some(user) = user else {
            return Ok(None);
        };
        let password = user.auth_password();
        if password.is_empty() {
            return Ok(None);
        }
        let hash = recaller.hash();
        let valid = hash_equals(&self.hash_password_for_cookie(&password), &hash)
            || hash_equals(&password, &hash);
        Ok(valid.then_some(user))
    }

    /// Put the user's identifier in the session and migrate it to a new ID
    /// (preventing session fixation).
    async fn update_session(&self, context: &Context, id: &Value) -> Result<()> {
        if let Some(session) = context.session() {
            session.put(&self.get_name(), id.clone());
            session.regenerate(true).await?;
        }
        Ok(())
    }

    fn store_password_hash(&self, context: &Context, user: &AuthUser) {
        let password = user.auth_password();
        if password.is_empty() {
            return;
        }
        if let Some(session) = context.session() {
            session.put(
                &self.password_hash_key(),
                self.hash_password_for_cookie(&password),
            );
        }
    }

    async fn ensure_remember_token_is_set(&self, user: AuthUser) -> Result<AuthUser> {
        if user.remember_token().is_some() {
            return Ok(user);
        }
        self.cycle_remember_token(user).await
    }

    async fn cycle_remember_token(&self, user: AuthUser) -> Result<AuthUser> {
        let token = Str::random(60);
        let user = user.with_remember_token(&token);
        self.provider.update_remember_token(&user, &token).await?;
        Ok(user)
    }

    fn cookie_queue(&self, context: &Context) -> Arc<CookieQueue> {
        match &context.request {
            Some(request) => CookieQueue::for_request(request),
            None => Cookie::jar().queue_store(),
        }
    }

    fn queue_recaller_cookie(&self, context: &Context, user: &AuthUser) {
        let value = format!(
            "{}|{}|{}",
            user.id().to_string_lossy(),
            user.remember_token().unwrap_or_default(),
            self.hash_password_for_cookie(&user.auth_password()),
        );
        let cookie = Cookie::make(self.get_recaller_name(), value, self.remember_duration);
        self.cookie_queue(context).queue(cookie);
    }

    fn clear_user_data_from_storage(&self, context: &Context) {
        if let Some(session) = context.session() {
            session.remove(&self.get_name());
        }
        let queue = self.cookie_queue(context);
        queue.unqueue(&self.get_recaller_name(), None);
        if self.recaller_for(context).is_some() {
            queue.queue(Cookie::forget(self.get_recaller_name()));
        }
    }

    async fn rehash_password_if_required(
        &self,
        user: AuthUser,
        credentials: &Value,
    ) -> Result<AuthUser> {
        if !self.rehash_on_login {
            return Ok(user);
        }
        Ok(self
            .provider
            .rehash_password_if_required(&user, credentials, false)
            .await?
            .unwrap_or(user))
    }

    /// Retrieve the user matching the credentials and check their password,
    /// remembering them as the last attempted user.
    async fn retrieve_and_validate(
        &self,
        context: &Context,
        credentials: &Value,
    ) -> Result<Option<AuthUser>> {
        let user = self.provider.retrieve_by_credentials(credentials).await?;
        context
            .state
            .update(&self.name, |state| state.last_attempted = user.clone());
        match user {
            Some(user)
                if self
                    .provider
                    .validate_credentials(&user, credentials)
                    .await? =>
            {
                Ok(Some(user))
            }
            _ => Ok(None),
        }
    }

    fn set_user_in(&self, context: &Context, user: AuthUser) {
        self.shared.remember_user(context, &self.name, Some(user));
    }

    fn failed_basic_response() -> illuminate_support::Error {
        HttpException::with_message(401, "Invalid credentials.")
            .header("WWW-Authenticate", "Basic")
            .into()
    }

    fn basic_credentials_for(
        &self,
        context: &Context,
        field: &str,
        extra: &Value,
    ) -> Option<Value> {
        let request = context.request.as_ref()?;
        let (user, password) = basic_credentials(request)?;
        let mut credentials = json!({ field: user, "password": password });
        if let (Value::Object(target), Value::Object(extra)) = (&mut credentials, extra) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        Some(credentials)
    }

    async fn logout_with(&self, cycle_token: bool) -> Result<()> {
        let user = self.try_user().await?;
        let context = self.shared.context();
        self.clear_user_data_from_storage(&context);

        if cycle_token && let Some(user) = user.filter(|user| user.remember_token().is_some()) {
            self.cycle_remember_token(user).await?;
        }

        context.state.update(&self.name, |state| {
            state.user = None;
            state.resolved = true;
            state.logged_out = true;
        });
        self.shared.forget_acting_as(Some(&self.name));
        self.shared.sync_request_user(&context);
        Ok(())
    }
}

#[async_trait]
impl Guard for SessionGuard {
    fn name(&self) -> &str {
        &self.name
    }

    async fn try_user(&self) -> Result<Option<AuthUser>> {
        let context = self.shared.context();
        match self.shared.known_user(&context, &self.name) {
            Known::User(user) => return Ok(Some(user)),
            Known::Guest => return Ok(None),
            Known::Unknown => {}
        }

        // First we will try to load the user using the identifier in the
        // session. Otherwise we will check for a "remember me" cookie.
        let mut user = None;
        if let Some(session) = context.session() {
            let id = session.get(&self.get_name());
            if !id.is_null() {
                user = self.provider.retrieve_by_id(&id).await?;
            }
        }

        if user.is_none()
            && let Some(recaller) = self.recaller_for(&context)
        {
            user = self.user_from_recaller(&context, &recaller).await?;
            if let Some(user) = &user {
                self.update_session(&context, &user.id()).await?;
            }
        }

        self.shared
            .remember_user(&context, &self.name, user.clone());
        Ok(user)
    }

    async fn id(&self) -> Option<Value> {
        let context = self.shared.context();
        if context.guard(&self.name).logged_out {
            return None;
        }
        match self.user().await {
            Some(user) => Some(user.id()),
            None => context
                .session()
                .map(|session| session.get(&self.get_name()))
                .filter(|id| !id.is_null()),
        }
    }

    async fn validate(&self, credentials: &Value) -> Result<bool> {
        let context = self.shared.context();
        Ok(self
            .retrieve_and_validate(&context, credentials)
            .await?
            .is_some())
    }

    fn set_user(&self, user: AuthUser) {
        self.set_user_in(&self.shared.context(), user);
    }

    fn forget_user(&self) {
        self.shared.forget_user(&self.shared.context(), &self.name);
    }

    fn provider(&self) -> Option<Arc<dyn UserProvider>> {
        Some(self.provider.clone())
    }

    async fn attempt(&self, credentials: &Value, remember: bool) -> Result<bool> {
        self.attempt_when(credentials, &[], remember).await
    }

    async fn attempt_when(
        &self,
        credentials: &Value,
        callbacks: &[AttemptCallback],
        remember: bool,
    ) -> Result<bool> {
        let context = self.shared.context();
        let Some(user) = self.retrieve_and_validate(&context, credentials).await? else {
            return Ok(false);
        };
        if !callbacks.iter().all(|callback| callback(&user)) {
            return Ok(false);
        }
        let user = self.rehash_password_if_required(user, credentials).await?;
        self.login(user, remember).await?;
        Ok(true)
    }

    async fn once(&self, credentials: &Value) -> Result<bool> {
        let context = self.shared.context();
        let Some(user) = self.retrieve_and_validate(&context, credentials).await? else {
            return Ok(false);
        };
        let user = self.rehash_password_if_required(user, credentials).await?;
        self.set_user_in(&context, user);
        Ok(true)
    }

    async fn once_using_id(&self, id: &Value) -> Result<Option<AuthUser>> {
        let user = self.provider.retrieve_by_id(id).await?;
        if let Some(user) = &user {
            self.set_user(user.clone());
        }
        Ok(user)
    }

    async fn login(&self, user: AuthUser, remember: bool) -> Result<()> {
        let context = self.shared.context();
        self.update_session(&context, &user.id()).await?;
        self.store_password_hash(&context, &user);

        // If the user should be permanently "remembered" by the application
        // we will queue a permanent cookie that contains the encrypted copy
        // of the user identifier.
        let user = if remember {
            let user = self.ensure_remember_token_is_set(user).await?;
            self.queue_recaller_cookie(&context, &user);
            user
        } else {
            user
        };

        self.set_user_in(&context, user);
        Ok(())
    }

    async fn login_using_id(&self, id: &Value, remember: bool) -> Result<Option<AuthUser>> {
        let user = self.provider.retrieve_by_id(id).await?;
        if let Some(user) = &user {
            self.login(user.clone(), remember).await?;
        }
        Ok(user)
    }

    async fn logout(&self) -> Result<()> {
        self.logout_with(true).await
    }

    async fn logout_current_device(&self) -> Result<()> {
        self.logout_with(false).await
    }

    async fn logout_other_devices(&self, password: &str) -> Result<()> {
        let Some(user) = self.try_user().await? else {
            return Ok(());
        };
        if !Hash::check(password, &user.auth_password()) {
            return Err(InvalidArgumentException::new(
                "The given password does not match the current password.",
            )
            .into());
        }

        let user = self
            .provider
            .rehash_password_if_required(&user, &json!({ "password": password }), true)
            .await?
            .unwrap_or(user);

        let context = self.shared.context();
        self.set_user_in(&context, user.clone());
        self.store_password_hash(&context, &user);

        let queued = self
            .cookie_queue(&context)
            .queued(&self.get_recaller_name(), None)
            .is_some();
        if self.recaller_for(&context).is_some() || queued {
            self.queue_recaller_cookie(&context, &user);
        }
        Ok(())
    }

    fn via_remember(&self) -> bool {
        self.shared.context().guard(&self.name).via_remember
    }

    async fn basic(&self, field: &str, extra: &Value) -> Result<()> {
        if self.check().await {
            return Ok(());
        }
        let context = self.shared.context();
        if let Some(credentials) = self.basic_credentials_for(&context, field, extra) {
            let has_user = credentials
                .get(field)
                .is_some_and(|user| !user.to_string_lossy().is_empty());
            if has_user && self.attempt(&credentials, false).await? {
                return Ok(());
            }
        }
        Err(Self::failed_basic_response())
    }

    async fn once_basic(&self, field: &str, extra: &Value) -> Result<()> {
        let context = self.shared.context();
        let credentials = self
            .basic_credentials_for(&context, field, extra)
            .unwrap_or_else(|| json!({ field: null, "password": null }));
        if self.once(&credentials).await? {
            Ok(())
        } else {
            Err(Self::failed_basic_response())
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The value of a "remember me" cookie: `id|token|password_hash`.
///
/// ```
/// use illuminate_auth::Recaller;
///
/// let recaller = Recaller::new("1|token|hash");
///
/// assert!(recaller.valid());
/// assert_eq!(recaller.id(), "1");
/// assert_eq!(recaller.token(), "token");
/// assert_eq!(recaller.hash(), "hash");
/// assert!(!Recaller::new("1|").valid());
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recaller {
    value: String,
}

impl Recaller {
    /// Wrap a cookie value.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }

    /// The user identifier.
    pub fn id(&self) -> String {
        self.segment(0)
    }

    /// The "remember me" token.
    pub fn token(&self) -> String {
        self.segment(1)
    }

    /// The signed password hash.
    pub fn hash(&self) -> String {
        self.value.split('|').nth(2).unwrap_or_default().to_string()
    }

    /// Determine if the value has every segment.
    pub fn valid(&self) -> bool {
        let segments = self.segments();
        self.value.contains('|')
            && segments.len() >= 3
            && !segments[0].trim().is_empty()
            && !segments[1].trim().is_empty()
    }

    /// Every `|`-separated segment.
    pub fn segments(&self) -> Vec<String> {
        self.value.split('|').map(str::to_string).collect()
    }

    fn segment(&self, index: usize) -> String {
        self.value
            .splitn(3, '|')
            .nth(index)
            .unwrap_or_default()
            .to_string()
    }
}
