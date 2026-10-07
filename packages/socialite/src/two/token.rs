//! A refreshed OAuth 2 access token.

use serde::{Deserialize, Serialize};

/// A new access token, returned by
/// [`Provider::refresh_token`](crate::Provider::refresh_token) (Laravel's
/// `Two\Token`).
///
/// ```
/// use laravel_socialite::Token;
///
/// let token = Token::new("new-token", "refresh-token", Some(3600), ["read:user"]);
///
/// assert_eq!(token.token, "new-token");
/// assert_eq!(token.refresh_token, "refresh-token");
/// assert_eq!(token.expires_in, Some(3600));
/// assert_eq!(token.approved_scopes, ["read:user"]);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    /// The new access token.
    pub token: String,
    /// The refresh token: the provider's new one, or the one you refreshed
    /// with when the provider doesn't rotate them.
    pub refresh_token: String,
    /// The number of seconds the access token is valid for.
    pub expires_in: Option<i64>,
    /// The scopes the token was granted.
    pub approved_scopes: Vec<String>,
}

impl Token {
    /// Create a new token.
    pub fn new(
        token: impl Into<String>,
        refresh_token: impl Into<String>,
        expires_in: Option<i64>,
        approved_scopes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            token: token.into(),
            refresh_token: refresh_token.into(),
            expires_in,
            approved_scopes: approved_scopes.into_iter().map(Into::into).collect(),
        }
    }
}
