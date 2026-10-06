//! Password resets: reset tokens, the password broker, and the `Password`
//! facade.

mod broker;
mod repository;

pub use broker::{PasswordBroker, PasswordBrokerManager, PasswordStatus, TokenRepositoryFactory};
pub use repository::{ArrayTokenRepository, TokenRepository};

use std::future::Future;
use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, try_app};
use illuminate_support::{Result, Value};

use crate::user::{AuthUser, FromAuthUser};

/// Resolve the application's password broker manager, registering one if
/// the application hasn't.
pub fn password_brokers() -> Arc<PasswordBrokerManager> {
    if let Some(manager) = try_app::<PasswordBrokerManager>() {
        return manager;
    }
    let container = Container::get_instance();
    container.singleton_if::<PasswordBrokerManager>(|c| {
        let config = c
            .try_make::<Repository>()
            .unwrap_or_else(|_| Arc::new(Repository::empty()));
        Arc::new(PasswordBrokerManager::new(config))
    });
    container.make::<PasswordBrokerManager>()
}

/// The `Password` facade: send password reset links and reset passwords
/// with the default broker (`auth.defaults.passwords`).
///
/// ```ignore
/// let status = Password::send_reset_link(&request.only(&["email"]), |user: User, token| async move {
///     Mail::to(&user.email).send(ResetPasswordMail::new(token)).await
/// }).await?;
///
/// if status == Password::RESET_LINK_SENT {
///     return Ok(back().with("status", status.as_str()));
/// }
/// ```
pub struct Password;

impl Password {
    /// The reset link was sent (`passwords.sent`).
    pub const RESET_LINK_SENT: PasswordStatus = PasswordStatus::ResetLinkSent;
    /// The password was reset (`passwords.reset`).
    pub const PASSWORD_RESET: PasswordStatus = PasswordStatus::PasswordReset;
    /// No user matches the credentials (`passwords.user`).
    pub const INVALID_USER: PasswordStatus = PasswordStatus::InvalidUser;
    /// The token is invalid or expired (`passwords.token`).
    pub const INVALID_TOKEN: PasswordStatus = PasswordStatus::InvalidToken;
    /// A token was requested too recently (`passwords.throttled`).
    pub const RESET_THROTTLED: PasswordStatus = PasswordStatus::ResetThrottled;

    /// The broker manager behind the facade.
    pub fn manager() -> Arc<PasswordBrokerManager> {
        password_brokers()
    }

    /// Get a password broker by name (`None` for the default).
    pub fn broker(name: Option<&str>) -> Result<Arc<PasswordBroker>> {
        password_brokers().broker(name)
    }

    /// Register a token repository driver (`database`, `cache`, ...).
    pub fn token_repository(
        driver: impl Into<String>,
        factory: impl Fn(&Container, &Value, &[u8]) -> Arc<dyn TokenRepository> + Send + Sync + 'static,
    ) {
        password_brokers().extend(driver, factory);
    }

    /// Send a password reset link to the user matching the credentials.
    /// The callback receives the user and the plain-text token, and should
    /// deliver the link.
    pub async fn send_reset_link<U, F, Fut>(
        credentials: &Value,
        callback: F,
    ) -> Result<PasswordStatus>
    where
        U: FromAuthUser + Send,
        F: FnOnce(U, String) -> Fut + Send,
        Fut: Future<Output = Result<()>> + Send,
    {
        Self::broker(None)?
            .send_reset_link(credentials, callback)
            .await
    }

    /// Reset the password of the user matching the credentials (`email`,
    /// `password`, `token`). The callback receives the user and the new
    /// plain-text password, and should save it.
    pub async fn reset<U, F, Fut>(credentials: &Value, callback: F) -> Result<PasswordStatus>
    where
        U: FromAuthUser + Send,
        F: FnOnce(U, String) -> Fut + Send,
        Fut: Future<Output = Result<()>> + Send,
    {
        Self::broker(None)?.reset(credentials, callback).await
    }

    /// Create a reset token for the user.
    pub async fn create_token(user: &AuthUser) -> Result<String> {
        Self::broker(None)?.create_token(user).await
    }

    /// Delete the user's reset tokens.
    pub async fn delete_token(user: &AuthUser) -> Result<()> {
        Self::broker(None)?.delete_token(user).await
    }

    /// Determine if the token is valid for the user.
    pub async fn token_exists(user: &AuthUser, token: &str) -> Result<bool> {
        Self::broker(None)?.token_exists(user, token).await
    }

    /// Get the user matching the credentials (the `token` is ignored).
    pub async fn get_user(credentials: &Value) -> Result<Option<AuthUser>> {
        Self::broker(None)?.get_user(credentials).await
    }
}
