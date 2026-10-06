//! Password reset token repositories.

use std::collections::HashMap;
use std::sync::Mutex;

use illuminate_hashing::Hash;
use illuminate_http::async_trait;
use illuminate_support::{Carbon, Result, Str};

use crate::support::hmac_sha256;
use crate::user::AuthUser;

/// Stores password reset tokens — Laravel's `TokenRepositoryInterface`.
///
/// Tokens are keyed by the user's
/// [`email_for_password_reset`](crate::Authenticatable::email_for_password_reset).
/// The framework ships an in-memory [`ArrayTokenRepository`]; a database
/// (or cache) backed repository is registered with
/// `Password::token_repository`.
#[async_trait]
pub trait TokenRepository: Send + Sync {
    /// Create a new token for the user, replacing any existing one.
    async fn create(&self, user: &AuthUser) -> Result<String>;

    /// Determine if a (non-expired) token exists for the user.
    async fn exists(&self, user: &AuthUser, token: &str) -> Result<bool>;

    /// Determine if the user created a token too recently to get another.
    async fn recently_created_token(&self, user: &AuthUser) -> Result<bool>;

    /// Delete the user's token.
    async fn delete(&self, user: &AuthUser) -> Result<()>;

    /// Delete every expired token.
    async fn delete_expired(&self) -> Result<()>;
}

/// An in-memory token repository, perfect for tests and single-process
/// applications. Tokens are stored hashed, just like in the database.
///
/// ```
/// use illuminate_auth::{ArrayTokenRepository, AuthUser, GenericUser, TokenRepository};
/// use illuminate_support::json;
///
/// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// # runtime.block_on(async {
/// # let container = std::sync::Arc::new(illuminate_container::Container::new());
/// # let _guard = illuminate_container::Container::set_local_instance(container.clone());
/// # container.instance(illuminate_config::Repository::new(json!({"hashing": {"bcrypt": {"rounds": 4}}})));
/// let tokens = ArrayTokenRepository::new(b"app-key".to_vec(), 3600, 60);
/// let user = AuthUser::new(GenericUser::new(json!({"id": 1, "email": "taylor@laravel.com"})));
///
/// let token = tokens.create(&user).await.unwrap();
///
/// assert!(tokens.exists(&user, &token).await.unwrap());
/// assert!(!tokens.exists(&user, "wrong").await.unwrap());
/// assert!(tokens.recently_created_token(&user).await.unwrap());
/// # });
/// ```
pub struct ArrayTokenRepository {
    tokens: Mutex<HashMap<String, (String, i64)>>,
    hash_key: Vec<u8>,
    expires: i64,
    throttle: i64,
}

impl ArrayTokenRepository {
    /// Create a repository whose tokens expire after `expires` seconds and
    /// may be requested once every `throttle` seconds.
    pub fn new(hash_key: Vec<u8>, expires: i64, throttle: i64) -> Self {
        Self {
            tokens: Mutex::new(HashMap::new()),
            hash_key,
            expires,
            throttle,
        }
    }

    /// Generate a new random token (an HMAC of random bytes).
    pub fn create_new_token(&self) -> String {
        hmac_sha256(&Str::random(40), &self.hash_key)
    }

    /// Move the creation time of a user's token into the past (for tests).
    pub fn travel(&self, email: &str, seconds: i64) {
        if let Some((_, created_at)) = self.tokens.lock().unwrap().get_mut(email) {
            *created_at -= seconds;
        }
    }

    /// The number of stored tokens.
    pub fn count(&self) -> usize {
        self.tokens.lock().unwrap().len()
    }

    fn now() -> i64 {
        Carbon::now().timestamp()
    }

    fn record(&self, user: &AuthUser) -> Option<(String, i64)> {
        self.tokens
            .lock()
            .unwrap()
            .get(&user.email_for_password_reset())
            .cloned()
    }
}

#[async_trait]
impl TokenRepository for ArrayTokenRepository {
    async fn create(&self, user: &AuthUser) -> Result<String> {
        let token = self.create_new_token();
        let hashed = Hash::make(&token)?;
        self.tokens
            .lock()
            .unwrap()
            .insert(user.email_for_password_reset(), (hashed, Self::now()));
        Ok(token)
    }

    async fn exists(&self, user: &AuthUser, token: &str) -> Result<bool> {
        Ok(match self.record(user) {
            Some((hashed, created_at)) => {
                created_at + self.expires >= Self::now() && Hash::check(token, &hashed)
            }
            None => false,
        })
    }

    async fn recently_created_token(&self, user: &AuthUser) -> Result<bool> {
        if self.throttle <= 0 {
            return Ok(false);
        }
        Ok(self
            .record(user)
            .is_some_and(|(_, created_at)| created_at + self.throttle > Self::now()))
    }

    async fn delete(&self, user: &AuthUser) -> Result<()> {
        self.tokens
            .lock()
            .unwrap()
            .remove(&user.email_for_password_reset());
        Ok(())
    }

    async fn delete_expired(&self) -> Result<()> {
        let expired_at = Self::now() - self.expires;
        self.tokens
            .lock()
            .unwrap()
            .retain(|_, (_, created_at)| *created_at >= expired_at);
        Ok(())
    }
}
