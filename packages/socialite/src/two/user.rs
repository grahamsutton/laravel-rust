//! The user retrieved from an OAuth 2 provider.

use std::ops::Index;

use illuminate_support::{Map, Str, Value, ValueExt};
use serde::{Deserialize, Serialize};

use crate::support::{optional_string, string_list};

/// A user retrieved from an OAuth provider (Laravel's `Two\User`).
///
/// Every provider fills in the same handful of properties — `id`,
/// `nickname`, `name`, `email` and `avatar` — along with the OAuth token
/// details. Everything the provider returned is kept in [`User::raw`], and
/// you may index the user to read it:
///
/// ```
/// use illuminate_support::json;
/// use laravel_socialite::User;
///
/// let user = User::new()
///     .set_raw(json!({"id": 1, "login": "taylorotwell", "company": "Laravel"}))
///     .map(json!({"id": 1, "nickname": "taylorotwell", "name": "Taylor Otwell"}))
///     .set_token("gho_secret")
///     .set_refresh_token(Some("ghr_refresh".into()))
///     .set_expires_in(Some(28800));
///
/// assert_eq!(user.get_id(), "1");
/// assert_eq!(user.get_nickname(), Some("taylorotwell"));
/// assert_eq!(user.get_name(), Some("Taylor Otwell"));
/// assert_eq!(user.get_email(), None);
/// assert_eq!(user.token, "gho_secret");
/// assert_eq!(user.refresh_token.as_deref(), Some("ghr_refresh"));
/// assert_eq!(user.expires_in, Some(28800));
///
/// // The provider's raw response...
/// assert_eq!(user["company"], "Laravel");
/// assert!(user["missing"].is_null());
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct User {
    /// The user's unique identifier with the provider (`""` when the
    /// provider didn't return one, like Slack bot tokens).
    pub id: String,
    /// The user's nickname / username.
    pub nickname: Option<String>,
    /// The user's full name.
    pub name: Option<String>,
    /// The user's e-mail address.
    pub email: Option<String>,
    /// The user's avatar image URL.
    pub avatar: Option<String>,
    /// The raw user array returned by the provider (Laravel's `$user->user`).
    pub raw: Value,
    /// Every attribute the provider mapped, including provider-specific
    /// extras such as `avatar_original`, `nodeId` or `organization_id`.
    pub attributes: Map<String, Value>,
    /// The user's access token.
    pub token: String,
    /// The refresh token that can be exchanged for a new access token.
    pub refresh_token: Option<String>,
    /// The number of seconds the access token is valid for.
    pub expires_in: Option<i64>,
    /// The scopes the user authorized (the provider's `scope` response).
    pub approved_scopes: Vec<String>,
    /// The provider's whole access token response.
    pub access_token_response_body: Value,
}

impl User {
    /// Create a new, empty user.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a fake user for testing, with fake OAuth token values
    /// (`fake-token`, `fake-refresh-token`, an hour until it expires) that
    /// the given attributes may override.
    ///
    /// Attributes may be spelled like Laravel's properties (`refreshToken`,
    /// `expiresIn`, `approvedScopes`) or in snake case.
    ///
    /// ```
    /// use illuminate_support::json;
    /// use laravel_socialite::User;
    ///
    /// let user = User::fake(json!({
    ///     "id": "github-123",
    ///     "name": "Jason Beggs",
    ///     "email": "jason@example.com",
    /// }));
    ///
    /// assert_eq!(user.id, "github-123");
    /// assert_eq!(user.token, "fake-token");
    /// assert_eq!(user.refresh_token.as_deref(), Some("fake-refresh-token"));
    /// assert_eq!(user.expires_in, Some(3600));
    /// assert_eq!(user["email"], "jason@example.com");
    ///
    /// let user = User::fake(json!({"token": "token", "approvedScopes": ["read", "write"]}));
    /// assert_eq!(user.token, "token");
    /// assert_eq!(user.approved_scopes, ["read", "write"]);
    /// ```
    pub fn fake(attributes: Value) -> Self {
        let attributes = match attributes {
            Value::Object(attributes) => attributes,
            _ => Map::new(),
        };
        let user = Self {
            id: Str::uuid().to_string(),
            token: "fake-token".into(),
            refresh_token: Some("fake-refresh-token".into()),
            expires_in: Some(3600),
            ..Self::default()
        };
        let raw = Value::Object(attributes.clone());
        user.map(Value::Object(attributes)).set_raw(raw)
    }

    /// Get the unique identifier for the user.
    pub fn get_id(&self) -> &str {
        &self.id
    }

    /// Get the nickname / username for the user.
    pub fn get_nickname(&self) -> Option<&str> {
        self.nickname.as_deref()
    }

    /// Get the full name of the user.
    pub fn get_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Get the e-mail address of the user.
    pub fn get_email(&self) -> Option<&str> {
        self.email.as_deref()
    }

    /// Get the avatar / image URL for the user.
    pub fn get_avatar(&self) -> Option<&str> {
        self.avatar.as_deref()
    }

    /// Get the raw user array.
    pub fn get_raw(&self) -> &Value {
        &self.raw
    }

    /// Set the raw user array from the provider.
    pub fn set_raw(mut self, user: Value) -> Self {
        self.raw = user;
        self
    }

    /// Map the given attributes onto the user: the known properties (`id`,
    /// `nickname`, `name`, `email`, `avatar`, and the token details) are
    /// set, and every attribute is kept in [`User::attributes`].
    pub fn map(mut self, attributes: Value) -> Self {
        let Value::Object(attributes) = attributes else {
            return self;
        };
        for (key, value) in &attributes {
            match key.as_str() {
                "id" => self.id = value.to_string_lossy(),
                "nickname" => self.nickname = optional_string(Some(value)),
                "name" => self.name = optional_string(Some(value)),
                "email" => self.email = optional_string(Some(value)),
                "avatar" => self.avatar = optional_string(Some(value)),
                "token" => self.token = value.to_string_lossy(),
                "refreshToken" | "refresh_token" => {
                    self.refresh_token = optional_string(Some(value))
                }
                "expiresIn" | "expires_in" => self.expires_in = value.to_i64_lossy(),
                "approvedScopes" | "approved_scopes" => {
                    self.approved_scopes = string_list(Some(value), " ")
                }
                _ => {}
            }
        }
        self.attributes.extend(attributes);
        self
    }

    /// Get one of the mapped attributes — including provider-specific ones
    /// like `avatar_original` (Google, Facebook, LinkedIn) or
    /// `organization_id` (Slack).
    pub fn attribute(&self, key: &str) -> Option<&Value> {
        self.attributes.get(key).filter(|value| !value.is_null())
    }

    /// Get all of the mapped attributes.
    pub fn get_attributes(&self) -> &Map<String, Value> {
        &self.attributes
    }

    /// Determine if the raw user array has the given key.
    pub fn has(&self, key: &str) -> bool {
        self.raw.get(key).is_some()
    }

    /// Set the token on the user.
    pub fn set_token(mut self, token: impl Into<String>) -> Self {
        self.token = token.into();
        self
    }

    /// Set the refresh token required to obtain a new access token.
    pub fn set_refresh_token(mut self, refresh_token: Option<String>) -> Self {
        self.refresh_token = refresh_token;
        self
    }

    /// Set the number of seconds the access token is valid for.
    pub fn set_expires_in(mut self, expires_in: Option<i64>) -> Self {
        self.expires_in = expires_in;
        self
    }

    /// Set the scopes that were approved by the user during authentication.
    pub fn set_approved_scopes(
        mut self,
        scopes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.approved_scopes = scopes.into_iter().map(Into::into).collect();
        self
    }

    /// Get the scopes that were approved by the user during authentication.
    pub fn get_approved_scopes(&self) -> &[String] {
        &self.approved_scopes
    }

    /// Set the access token response body.
    pub fn set_access_token_response_body(mut self, body: Value) -> Self {
        self.access_token_response_body = body;
        self
    }

    /// Get the access token response body.
    pub fn get_access_token_response_body(&self) -> &Value {
        &self.access_token_response_body
    }
}

/// Read the provider's raw user array: `user["login"]` (`null` when missing).
impl Index<&str> for User {
    type Output = Value;

    fn index(&self, key: &str) -> &Value {
        &self.raw[key]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn known_properties_are_mapped_and_extras_are_kept() {
        let user = User::new().map(json!({
            "id": 42,
            "nickname": null,
            "name": "Taylor",
            "email": "taylor@laravel.com",
            "avatar": "https://a.test/t.png",
            "avatar_original": "https://a.test/t-large.png",
        }));
        assert_eq!(user.get_id(), "42");
        assert_eq!(user.get_nickname(), None);
        assert_eq!(user.get_name(), Some("Taylor"));
        assert_eq!(user.get_email(), Some("taylor@laravel.com"));
        assert_eq!(user.get_avatar(), Some("https://a.test/t.png"));
        assert_eq!(
            user.attribute("avatar_original"),
            Some(&json!("https://a.test/t-large.png"))
        );
        assert_eq!(user.attribute("nickname"), None);
        assert_eq!(user.get_attributes().len(), 6);
    }

    #[test]
    fn token_details_may_be_set() {
        let user = User::new()
            .set_token("token")
            .set_refresh_token(Some("refresh".into()))
            .set_expires_in(Some(60))
            .set_approved_scopes(["read", "write"])
            .set_access_token_response_body(json!({"access_token": "token"}));
        assert_eq!(user.token, "token");
        assert_eq!(user.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(user.expires_in, Some(60));
        assert_eq!(user.get_approved_scopes(), ["read", "write"]);
        assert_eq!(
            user.get_access_token_response_body()["access_token"],
            "token"
        );
    }

    #[test]
    fn fake_users_have_fake_tokens_and_overrides() {
        let user = User::fake(json!({}));
        assert!(!user.id.is_empty());
        assert_eq!(user.token, "fake-token");
        assert_eq!(user.approved_scopes, Vec::<String>::new());

        let user = User::fake(json!({
            "id": "github-123",
            "refreshToken": "fake-refresh",
            "expiresIn": 7200,
            "approved_scopes": ["read"],
        }));
        assert_eq!(user.id, "github-123");
        assert_eq!(user.refresh_token.as_deref(), Some("fake-refresh"));
        assert_eq!(user.expires_in, Some(7200));
        assert_eq!(user.approved_scopes, ["read"]);
        assert!(user.has("id"));
        assert!(!user.has("name"));
    }

    #[test]
    fn users_serialize() {
        let user = User::new().map(json!({"id": "1", "name": "Taylor"}));
        let value = serde_json::to_value(&user).unwrap();
        assert_eq!(value["id"], "1");
        assert_eq!(value["name"], "Taylor");
        let back: User = serde_json::from_value(value).unwrap();
        assert_eq!(back, user);
    }
}
