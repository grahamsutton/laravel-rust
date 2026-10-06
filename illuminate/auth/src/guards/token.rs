//! The token guard: simple API token authentication.

use std::any::Any;
use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_http::{Request, async_trait};
use illuminate_support::{Result, Value, ValueExt, json};

use super::{Context, Guard, Known, Shared, option};
use crate::providers::UserProvider;
use crate::support::{basic_credentials, sha256};
use crate::user::AuthUser;

/// Authenticates API requests with a token stored on the user.
///
/// The token is read from the `api_token` query string or input, the
/// bearer token, or the HTTP Basic password, and looked up in the user
/// provider by its `api_token` column. Set `hash` to store SHA-256 hashes of
/// tokens instead of the tokens themselves:
///
/// ```text
/// 'api' => ['driver' => 'token', 'provider' => 'users', 'hash' => true],
/// ```
pub struct TokenGuard {
    name: String,
    provider: Arc<dyn UserProvider>,
    shared: Arc<Shared>,
    input_key: String,
    storage_key: String,
    hash: bool,
}

impl TokenGuard {
    /// Create a token guard with Laravel's defaults (`api_token`, unhashed).
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
        Self {
            name,
            provider,
            shared,
            input_key: option(config, "input_key").unwrap_or_else(|| "api_token".into()),
            storage_key: option(config, "storage_key").unwrap_or_else(|| "api_token".into()),
            hash: config.get("hash").is_some_and(ValueExt::truthy),
        }
    }

    /// Use a different request key for the token.
    pub fn input_key(mut self, key: impl Into<String>) -> Self {
        self.input_key = key.into();
        self
    }

    /// Use a different storage column for the token.
    pub fn storage_key(mut self, key: impl Into<String>) -> Self {
        self.storage_key = key.into();
        self
    }

    /// Store SHA-256 hashes of tokens rather than the tokens themselves.
    pub fn hashed(mut self, hash: bool) -> Self {
        self.hash = hash;
        self
    }

    /// The token for the given request: the query string, the input, the
    /// bearer token, or the HTTP Basic password — whichever comes first.
    pub fn get_token_for_request(&self, request: &Request) -> Option<String> {
        let filled = |value: Value| Some(value.to_string_lossy()).filter(|token| !token.is_empty());
        filled(request.query(&self.input_key))
            .or_else(|| filled(request.input(&self.input_key)))
            .or_else(|| request.bearer_token())
            .or_else(|| basic_credentials(request).map(|(_, password)| password).filter(|p| !p.is_empty()))
    }

    async fn resolve(&self, context: &Context) -> Result<Option<AuthUser>> {
        let Some(request) = &context.request else {
            return Ok(None);
        };
        let Some(token) = self.get_token_for_request(request) else {
            return Ok(None);
        };
        let token = if self.hash { sha256(&token) } else { token };
        let mut credentials = json!({});
        credentials[&self.storage_key] = Value::String(token);
        self.provider.retrieve_by_credentials(&credentials).await
    }
}

#[async_trait]
impl Guard for TokenGuard {
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
        let user = self.resolve(&context).await?;
        self.shared.remember_user(&context, &self.name, user.clone());
        Ok(user)
    }

    async fn validate(&self, credentials: &Value) -> Result<bool> {
        let token = match credentials.get(&self.input_key) {
            Some(Value::String(token)) if !token.is_empty() => token.clone(),
            _ => return Ok(false),
        };
        let mut lookup = json!({});
        lookup[&self.storage_key] = Value::String(token);
        Ok(self.provider.retrieve_by_credentials(&lookup).await?.is_some())
    }

    fn set_user(&self, user: AuthUser) {
        self.shared.remember_user(&self.shared.context(), &self.name, Some(user));
    }

    fn forget_user(&self) {
        self.shared.forget_user(&self.shared.context(), &self.name);
    }

    fn provider(&self) -> Option<Arc<dyn UserProvider>> {
        Some(self.provider.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
