//! The password broker and its manager.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, RwLock};

use illuminate_config::Repository;
use illuminate_container::Container;
use illuminate_support::error::{InvalidArgumentException, RuntimeException};
use illuminate_support::{Result, Value, ValueExt};

use super::repository::{ArrayTokenRepository, TokenRepository};
use crate::providers::UserProvider;
use crate::support::{decode_app_key, plain_password};
use crate::user::{AuthUser, FromAuthUser};

/// The outcome of a password broker operation. Its string form is the
/// translation key Laravel uses for the message (`passwords.sent`, ...).
///
/// ```
/// use illuminate_auth::PasswordStatus;
///
/// assert_eq!(PasswordStatus::ResetLinkSent.as_str(), "passwords.sent");
/// assert_eq!(PasswordStatus::InvalidToken.to_string(), "passwords.token");
/// assert!(PasswordStatus::PasswordReset == "passwords.reset");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PasswordStatus {
    /// The reset link was sent.
    ResetLinkSent,
    /// The password was reset.
    PasswordReset,
    /// No user matches the given credentials.
    InvalidUser,
    /// The reset token is invalid or expired.
    InvalidToken,
    /// A reset link was requested too recently.
    ResetThrottled,
}

impl PasswordStatus {
    /// The status's translation key.
    pub fn as_str(&self) -> &'static str {
        match self {
            PasswordStatus::ResetLinkSent => "passwords.sent",
            PasswordStatus::PasswordReset => "passwords.reset",
            PasswordStatus::InvalidUser => "passwords.user",
            PasswordStatus::InvalidToken => "passwords.token",
            PasswordStatus::ResetThrottled => "passwords.throttled",
        }
    }

    /// Determine if the operation succeeded.
    pub fn is_successful(&self) -> bool {
        matches!(
            self,
            PasswordStatus::ResetLinkSent | PasswordStatus::PasswordReset
        )
    }
}

impl fmt::Display for PasswordStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq<&str> for PasswordStatus {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl From<PasswordStatus> for Value {
    fn from(status: PasswordStatus) -> Self {
        Value::String(status.as_str().to_string())
    }
}

/// Sends password reset links and resets passwords, using a user provider
/// to find users and a token repository to keep track of reset tokens.
pub struct PasswordBroker {
    tokens: Arc<dyn TokenRepository>,
    users: Arc<dyn UserProvider>,
}

impl PasswordBroker {
    /// Create a broker.
    pub fn new(tokens: Arc<dyn TokenRepository>, users: Arc<dyn UserProvider>) -> Self {
        Self { tokens, users }
    }

    /// Send a password reset link to the user matching the credentials.
    ///
    /// The callback receives the user (as `AuthUser` or your concrete user
    /// type) and the plain-text token, and should deliver the reset link.
    pub async fn send_reset_link<U, F, Fut>(
        &self,
        credentials: &Value,
        callback: F,
    ) -> Result<PasswordStatus>
    where
        U: FromAuthUser + Send,
        F: FnOnce(U, String) -> Fut + Send,
        Fut: Future<Output = Result<()>> + Send,
    {
        let Some(user) = self.get_user(credentials).await? else {
            return Ok(PasswordStatus::InvalidUser);
        };
        if self.tokens.recently_created_token(&user).await? {
            return Ok(PasswordStatus::ResetThrottled);
        }
        let token = self.tokens.create(&user).await?;
        callback(convert(&user)?, token).await?;
        Ok(PasswordStatus::ResetLinkSent)
    }

    /// Reset the password of the user matching the credentials (`email`,
    /// `password`, `token`).
    ///
    /// The callback receives the user and the new plain-text password, and
    /// should save it; the token is deleted afterwards.
    pub async fn reset<U, F, Fut>(&self, credentials: &Value, callback: F) -> Result<PasswordStatus>
    where
        U: FromAuthUser + Send,
        F: FnOnce(U, String) -> Fut + Send,
        Fut: Future<Output = Result<()>> + Send,
    {
        let user = match self.validate_reset(credentials).await? {
            Ok(user) => user,
            Err(status) => return Ok(status),
        };
        let password = plain_password(credentials).unwrap_or_default();
        callback(convert(&user)?, password).await?;
        self.tokens.delete(&user).await?;
        Ok(PasswordStatus::PasswordReset)
    }

    async fn validate_reset(
        &self,
        credentials: &Value,
    ) -> Result<std::result::Result<AuthUser, PasswordStatus>> {
        let Some(user) = self.get_user(credentials).await? else {
            return Ok(Err(PasswordStatus::InvalidUser));
        };
        let token = credentials
            .get("token")
            .map(ValueExt::to_string_lossy)
            .unwrap_or_default();
        if token.is_empty() || !self.tokens.exists(&user, &token).await? {
            return Ok(Err(PasswordStatus::InvalidToken));
        }
        Ok(Ok(user))
    }

    /// Get the user matching the credentials (the `token` is ignored).
    pub async fn get_user(&self, credentials: &Value) -> Result<Option<AuthUser>> {
        let mut credentials = credentials.clone();
        if let Value::Object(map) = &mut credentials {
            map.shift_remove("token");
        }
        self.users.retrieve_by_credentials(&credentials).await
    }

    /// Create a reset token for the user.
    pub async fn create_token(&self, user: &AuthUser) -> Result<String> {
        self.tokens.create(user).await
    }

    /// Delete the user's reset tokens.
    pub async fn delete_token(&self, user: &AuthUser) -> Result<()> {
        self.tokens.delete(user).await
    }

    /// Determine if the token is valid for the user.
    pub async fn token_exists(&self, user: &AuthUser, token: &str) -> Result<bool> {
        self.tokens.exists(user, token).await
    }

    /// The token repository.
    pub fn get_repository(&self) -> Arc<dyn TokenRepository> {
        self.tokens.clone()
    }
}

fn convert<U: FromAuthUser>(user: &AuthUser) -> Result<U> {
    U::from_auth_user(user).ok_or_else(|| {
        RuntimeException::new(format!(
            "The password reset user is a [{}], not a [{}].",
            user.type_name(),
            std::any::type_name::<U>()
        ))
        .into()
    })
}

/// Builds a token repository: `(app, broker config, decoded app key)`.
pub type TokenRepositoryFactory =
    Arc<dyn Fn(&Container, &Value, &[u8]) -> Arc<dyn TokenRepository> + Send + Sync>;

/// Builds the password brokers configured under `auth.passwords`.
///
/// Each broker names a user provider (`auth.providers.<name>`) and a token
/// repository `driver`. The in-memory `array` driver is built in;
/// `database` and `cache` repositories are registered by the framework with
/// [`extend`](PasswordBrokerManager::extend).
pub struct PasswordBrokerManager {
    config: Arc<Repository>,
    brokers: RwLock<HashMap<String, Arc<PasswordBroker>>>,
    creators: RwLock<HashMap<String, TokenRepositoryFactory>>,
}

impl PasswordBrokerManager {
    /// Create a manager reading the given configuration.
    pub fn new(config: Arc<Repository>) -> Self {
        let manager = Self {
            config,
            brokers: RwLock::new(HashMap::new()),
            creators: RwLock::new(HashMap::new()),
        };
        manager.extend("array", |_app, config, key| {
            let expire = config
                .get("expire")
                .and_then(ValueExt::to_i64_lossy)
                .unwrap_or(60);
            let throttle = config
                .get("throttle")
                .and_then(ValueExt::to_i64_lossy)
                .unwrap_or(0);
            Arc::new(ArrayTokenRepository::new(
                key.to_vec(),
                expire * 60,
                throttle,
            ))
        });
        manager
    }

    /// Register a token repository driver.
    pub fn extend(
        &self,
        driver: impl Into<String>,
        factory: impl Fn(&Container, &Value, &[u8]) -> Arc<dyn TokenRepository> + Send + Sync + 'static,
    ) -> &Self {
        self.creators
            .write()
            .unwrap()
            .insert(driver.into(), Arc::new(factory));
        self
    }

    /// Get a broker by name (`None` for `auth.defaults.passwords`).
    pub fn broker(&self, name: Option<&str>) -> Result<Arc<PasswordBroker>> {
        let name = match name {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => self.get_default_driver(),
        };
        if let Some(broker) = self.brokers.read().unwrap().get(&name) {
            return Ok(broker.clone());
        }
        let broker = Arc::new(self.resolve(&name)?);
        Ok(self
            .brokers
            .write()
            .unwrap()
            .entry(name)
            .or_insert(broker)
            .clone())
    }

    fn resolve(&self, name: &str) -> Result<PasswordBroker> {
        let config = self.config.get(&format!("auth.passwords.{name}"));
        if config.is_null() {
            return Err(InvalidArgumentException::new(format!(
                "Password resetter [{name}] is not defined."
            ))
            .into());
        }

        let provider_name = config.get("provider").map(ValueExt::to_string_lossy);
        let users = crate::facade::manager()
            .create_user_provider(provider_name.as_deref())?
            .ok_or_else(|| {
                InvalidArgumentException::new(format!(
                    "Password resetter [{name}] requires a user provider."
                ))
            })?;

        let driver = match config.get("driver") {
            Some(Value::String(driver)) if !driver.is_empty() => driver.clone(),
            _ => "database".to_string(),
        };
        let creator = self.creators.read().unwrap().get(&driver).cloned();
        let Some(creator) = creator else {
            return Err(InvalidArgumentException::new(format!(
                "Password reset token repository [{driver}] is not defined."
            ))
            .into());
        };
        let key = decode_app_key(&self.config.get("app.key").to_string_lossy());
        let tokens = creator(&Container::get_instance(), &config, &key);
        Ok(PasswordBroker::new(tokens, users))
    }

    /// The default broker's name.
    pub fn get_default_driver(&self) -> String {
        match self.config.get("auth.defaults.passwords") {
            Value::String(name) if !name.is_empty() => name,
            _ => "users".to_string(),
        }
    }

    /// Forget the brokers that have been built.
    pub fn forget_brokers(&self) {
        self.brokers.write().unwrap().clear();
    }
}
