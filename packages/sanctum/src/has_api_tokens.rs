//! `HasApiTokens`: issuing and inspecting API tokens on your models.

use std::future::Future;

use illuminate_auth::Authenticatable;
use illuminate_database::eloquent::{Attributes, Model, MorphMany};
use illuminate_support::{Carbon, Result, Str, Value};
use serde::Serialize;

use crate::access_token::{AccessToken, TokenOwner, attach, token_for};
use crate::config;
use crate::facade::Sanctum;
use crate::personal_access_token::PersonalAccessToken;
use crate::support::{crc32b, key_string, sha256};

/// A freshly issued token: the model, and the plain-text token to hand to
/// the user — it can never be retrieved again.
///
/// Serializes like Laravel's `NewAccessToken`:
/// `{"accessToken": {...}, "plainTextToken": "1|..."}`.
#[derive(Clone, Debug, Serialize)]
pub struct NewAccessToken {
    /// The stored token.
    #[serde(rename = "accessToken")]
    pub access_token: PersonalAccessToken,
    /// The plain-text token: `{id}|{token}`.
    #[serde(rename = "plainTextToken")]
    pub plain_text_token: String,
}

impl NewAccessToken {
    /// Wrap a stored token and its plain-text value.
    pub fn new(access_token: PersonalAccessToken, plain_text_token: impl Into<String>) -> Self {
        Self {
            access_token,
            plain_text_token: plain_text_token.into(),
        }
    }

    /// The token as an array (`accessToken` and `plainTextToken`).
    pub fn to_array(&self) -> Value {
        illuminate_support::to_value(self)
    }

    /// The token as JSON.
    pub fn to_json(&self) -> String {
        self.to_array().to_string()
    }
}

/// API tokens for your authenticatable models — Laravel's `HasApiTokens`
/// trait.
///
/// Every Eloquent model that can log in has these methods; bring them into
/// scope with `use laravel_sanctum::HasApiTokens;`:
///
/// ```ignore
/// use laravel_sanctum::HasApiTokens;
///
/// Route::post("/tokens/create", |request: Request| async move {
///     let user: User = request.user().unwrap();
///     let token = user.create_token(&request.string("token_name"), &["*"]).await?;
///
///     Ok::<_, Error>(Json(json!({"token": token.plain_text_token})))
/// });
///
/// if user.token_can("server:update") {
///     // ...
/// }
///
/// // Revoke all tokens...
/// user.tokens().delete().await?;
///
/// // Revoke the token that was used to authenticate the current request...
/// user.current_access_token().unwrap().delete().await?;
///
/// // Revoke a specific token...
/// user.tokens().where_("id", token_id).delete().await?;
/// ```
pub trait HasApiTokens: Model + Authenticatable {
    /// The user's tokens (a `morph_many` relationship on `tokenable`).
    fn tokens(&self) -> MorphMany<Self, PersonalAccessToken> {
        Sanctum::use_tokenable_model::<Self>();
        self.morph_many("tokenable")
    }

    /// Determine if the current API token has a given ability. Always
    /// `true` for requests authenticated by your SPA's session.
    fn token_can(&self, ability: &str) -> bool {
        self.current_access_token()
            .is_some_and(|token| token.can(ability))
    }

    /// Determine if the current API token is missing a given ability.
    fn token_cant(&self, ability: &str) -> bool {
        !self.token_can(ability)
    }

    /// Create a new personal access token with the given abilities
    /// (`&["*"]` grants every ability).
    fn create_token(
        &self,
        name: &str,
        abilities: &[&str],
    ) -> impl Future<Output = Result<NewAccessToken>> + Send {
        issue_token(self, name, abilities, None)
    }

    /// Create a new personal access token that expires at the given time.
    ///
    /// ```ignore
    /// let token = user.create_token_with_expiry("token-name", &["*"], now().add_week()).await?;
    /// ```
    fn create_token_with_expiry(
        &self,
        name: &str,
        abilities: &[&str],
        expires_at: Carbon,
    ) -> impl Future<Output = Result<NewAccessToken>> + Send {
        issue_token(self, name, abilities, Some(expires_at))
    }

    /// Generate a token string: the `sanctum.token_prefix`, 40 random
    /// characters, and their CRC-32 checksum.
    fn generate_token_string(&self) -> String {
        let entropy = Str::random(40);
        format!("{}{}{}", config::token_prefix(), entropy, crc32b(&entropy))
    }

    /// The access token the user is currently authenticated with.
    fn current_access_token(&self) -> Option<AccessToken> {
        token_for(&TokenOwner::of(self))
    }

    /// Set the access token the user is authenticated with (for the request
    /// being handled).
    fn with_access_token(self, token: impl Into<AccessToken>) -> Self {
        attach(TokenOwner::of(&self), token.into());
        self
    }
}

impl<M: Model + Authenticatable> HasApiTokens for M {}

fn issue_token<M: HasApiTokens>(
    model: &M,
    name: &str,
    abilities: &[&str],
    expires_at: Option<Carbon>,
) -> impl Future<Output = Result<NewAccessToken>> + Send + 'static {
    let plain_text_token = model.generate_token_string();
    let tokens = model.tokens();
    let abilities: Vec<String> = abilities
        .iter()
        .map(|ability| ability.to_string())
        .collect();
    let mut attributes = Attributes::new();
    attributes
        .insert("name", name)
        .insert("token", sha256(&plain_text_token))
        .insert("abilities", abilities)
        .insert("expires_at", expires_at);

    async move {
        let token = tokens.create(attributes).await?;
        let plain_text_token = format!("{}|{plain_text_token}", key_string(&token.get_key()));
        Ok(NewAccessToken::new(token, plain_text_token))
    }
}
