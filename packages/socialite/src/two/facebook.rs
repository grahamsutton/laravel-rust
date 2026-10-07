//! Facebook.

use async_trait::async_trait;
use illuminate_support::error::RuntimeException;
use illuminate_support::{Map, Result, Value, ValueExt, json};

use crate::jwt::Jwt;
use crate::support::{get_value, hmac_sha256_hex, optional_string};
use crate::two::{OAuth2Provider, Provider, User};

/// The URL of Facebook's Limited Login public keys.
const JWKS_URL: &str = "https://limited.facebook.com/.well-known/oauth/openid/jwks/";

/// The Facebook driver (`services.facebook`).
///
/// Requests the `email` scope and the `name`, `email`, `gender`, `verified`
/// and `link` fields by default. Every Graph API request is signed with an
/// `appsecret_proof`.
///
/// ```
/// use laravel_socialite::{FacebookProvider, Provider};
///
/// let facebook = Provider::new(FacebookProvider::new(), "id", "secret", "https://laravel.test/callback")
///     .fields(["name", "email", "first_name"])
///     .as_popup()
///     .re_request()
///     .stateless();
///
/// let url = facebook.get_auth_url(None);
///
/// assert!(url.starts_with("https://www.facebook.com/v3.3/dialog/oauth?client_id=id&"));
/// assert!(url.ends_with("&display=popup&auth_type=rerequest"));
/// ```
///
/// Tokens from Facebook Limited Login (iOS) are OpenID Connect ID tokens:
/// [`Provider::user_from_token`] verifies their signature against
/// Facebook's public keys, and
/// [`Provider::user_from_token_with_nonce`] checks the nonce too.
#[derive(Clone, Debug)]
pub struct FacebookProvider {
    /// The base Facebook Graph URL.
    pub graph_url: String,
    /// The Graph API version for the request.
    pub version: String,
    /// The user fields being requested.
    pub fields: Vec<String>,
    /// Display the dialog in a popup view.
    pub popup: bool,
    /// Re-request a declined permission.
    pub re_request: bool,
    /// Re-authenticate the user.
    pub re_authenticate: bool,
}

impl Default for FacebookProvider {
    fn default() -> Self {
        Self {
            graph_url: "https://graph.facebook.com".into(),
            version: "v3.3".into(),
            fields: ["name", "email", "gender", "verified", "link"]
                .map(String::from)
                .to_vec(),
            popup: false,
            re_request: false,
            re_authenticate: false,
        }
    }
}

impl FacebookProvider {
    /// Create a new Facebook driver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Determine if the given token is an OpenID Connect ID token (a JWT).
    pub fn is_oidc_token(token: &str) -> bool {
        token.split('.').count() == 3
    }

    /// Get the user for a Graph API access token.
    pub async fn get_user_from_access_token(
        &self,
        provider: &Provider,
        token: &str,
    ) -> Result<Value> {
        let mut query = Map::new();
        query.insert("access_token".into(), token.into());
        query.insert("fields".into(), self.fields.join(",").into());
        if !provider.get_client_secret().is_empty() {
            query.insert(
                "appsecret_proof".into(),
                hmac_sha256_hex(provider.get_client_secret().as_bytes(), token.as_bytes()).into(),
            );
        }

        let response = provider
            .get_http_client()
            .with_header("Accept", "application/json")
            .get_with(
                format!("{}/{}/me", self.graph_url, self.version),
                Value::Object(query),
            )
            .await?;
        Ok(response.json())
    }

    /// Get the user for a Limited Login OpenID Connect token, verifying its
    /// signature, audience, issuer, expiration — and nonce, when given.
    pub async fn get_user_by_oidc_token(
        &self,
        provider: &Provider,
        token: &str,
        nonce: Option<&str>,
    ) -> Result<Value> {
        let jwt = Jwt::parse(token)?;
        let kid = jwt.header["kid"].to_string_lossy();
        let keys = provider.get_http_client().get(JWKS_URL).await?.json();
        let key = keys["keys"]
            .as_array()
            .and_then(|keys| keys.iter().find(|key| key["kid"].to_string_lossy() == kid))
            .ok_or_else(|| {
                RuntimeException::new(format!(
                    "Unable to find the public key [{kid}] of the token."
                ))
            })?;

        let mut data = jwt.verify(key)?;

        if data["aud"].as_str() != Some(provider.get_client_id()) {
            return Err(RuntimeException::new("Token has incorrect audience.").into());
        }
        if data["iss"].as_str() != Some("https://www.facebook.com") {
            return Err(RuntimeException::new("Token has incorrect issuer.").into());
        }
        if let Some(nonce) = nonce
            && data["nonce"].as_str() != Some(nonce)
        {
            return Err(RuntimeException::new("Token has incorrect nonce.").into());
        }

        if let Value::Object(user) = &mut data {
            let sub = user.get("sub").cloned().unwrap_or(Value::Null);
            user.insert("id".into(), sub);
            if let Some(given_name) = user.get("given_name").cloned() {
                user.insert("first_name".into(), given_name);
            }
            if let Some(family_name) = user.get("family_name").cloned() {
                user.insert("last_name".into(), family_name);
            }
        }
        Ok(data)
    }
}

#[async_trait]
impl OAuth2Provider for FacebookProvider {
    fn default_scopes(&self) -> Vec<String> {
        vec!["email".into()]
    }

    fn get_auth_url(&self, provider: &Provider, state: Option<&str>) -> String {
        provider.build_auth_url_from_base(
            &format!("https://www.facebook.com/{}/dialog/oauth", self.version),
            state,
        )
    }

    fn get_token_url(&self, _provider: &Provider) -> String {
        format!("{}/{}/oauth/access_token", self.graph_url, self.version)
    }

    fn get_code_fields(&self, provider: &Provider, state: Option<&str>) -> Map<String, Value> {
        let mut fields = provider.default_code_fields(state);
        if self.popup {
            fields.insert("display".into(), "popup".into());
        }
        if self.re_request {
            fields.insert("auth_type".into(), "rerequest".into());
        }
        if self.re_authenticate {
            fields.insert("auth_type".into(), "reauthenticate".into());
        }
        fields
    }

    async fn get_access_token_response(&self, provider: &Provider, code: &str) -> Result<Value> {
        let response = provider
            .get_http_client()
            .as_form()
            .post(
                provider.get_token_url(),
                Value::Object(provider.get_token_fields(code)),
            )
            .await?;

        // Facebook says `expires`; Socialite says `expires_in`.
        let mut data = response.json();
        if let Value::Object(body) = &mut data
            && body.get("expires_in").is_none_or(Value::is_null)
        {
            let expires = body.get("expires").cloned().unwrap_or(Value::Null);
            body.insert("expires_in".into(), expires);
        }
        Ok(data)
    }

    async fn get_user_by_token(&self, provider: &Provider, token: &str) -> Result<Value> {
        if Self::is_oidc_token(token) {
            return self.get_user_by_oidc_token(provider, token, None).await;
        }
        self.get_user_from_access_token(provider, token).await
    }

    fn map_user_to_object(&self, _provider: &Provider, user: Value) -> User {
        let (avatar, avatar_original) = match optional_string(user.get("sub")) {
            None => {
                let avatar = format!(
                    "{}/{}/{}/picture",
                    self.graph_url,
                    self.version,
                    user["id"].to_string_lossy()
                );
                let original = format!("{avatar}?width=1920");
                (Value::String(avatar), Value::String(original))
            }
            Some(_) => (get_value(&user, "picture"), get_value(&user, "picture")),
        };

        let id = match get_value(&user, "id") {
            Value::Null => get_value(&user, "sub"),
            id => id,
        };

        User::new().set_raw(user.clone()).map(json!({
            "id": id,
            "nickname": null,
            "name": get_value(&user, "name"),
            "email": get_value(&user, "email"),
            "avatar": avatar,
            "avatar_original": avatar_original,
            "profileUrl": get_value(&user, "link"),
        }))
    }
}

impl Provider {
    /// Set the user fields to request from Facebook (Facebook only).
    pub fn fields<I, S>(self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let fields: Vec<String> = fields.into_iter().map(Into::into).collect();
        self.with_driver(|facebook: &mut FacebookProvider| facebook.fields = fields)
    }

    /// Set the dialog to be displayed as a popup (Facebook only).
    pub fn as_popup(self) -> Self {
        self.with_driver(|facebook: &mut FacebookProvider| facebook.popup = true)
    }

    /// Re-request a declined permission (Facebook only).
    pub fn re_request(self) -> Self {
        self.with_driver(|facebook: &mut FacebookProvider| facebook.re_request = true)
    }

    /// Re-authenticate the user (Facebook only).
    pub fn re_authenticate(self) -> Self {
        self.with_driver(|facebook: &mut FacebookProvider| facebook.re_authenticate = true)
    }

    /// Get a user from a Facebook Limited Login token, checking the nonce
    /// used to start the login. Other tokens (and other drivers) behave
    /// like [`Provider::user_from_token`].
    pub async fn user_from_token_with_nonce(&self, token: &str, nonce: &str) -> Result<User> {
        match self.driver_ref::<FacebookProvider>() {
            Some(facebook) if FacebookProvider::is_oidc_token(token) => {
                let user = facebook
                    .get_user_by_oidc_token(self, token, Some(nonce))
                    .await?;
                Ok(self.map_user_to_object(user).set_token(token))
            }
            _ => self.user_from_token(token).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use illuminate_container::Container;
    use illuminate_http_client::Http;

    use super::*;
    use crate::jwt::tests::{EXPIRED_TOKEN, EXPONENT, MODULUS, TOKEN, WRONG_AUDIENCE_TOKEN};

    fn facebook() -> Provider {
        Provider::new(
            FacebookProvider::new(),
            "client-id",
            "client-secret",
            "https://laravel.test/auth/facebook/callback",
        )
    }

    fn fake_public_keys() {
        Http::fake_urls([(
            JWKS_URL,
            Http::response(
                json!({"keys": [
                    {"kid": "another-kid", "kty": "RSA", "alg": "RS256", "n": "AQAB", "e": "AQAB"},
                    {"kid": "test-kid", "kty": "RSA", "alg": "RS256", "use": "sig", "n": MODULUS, "e": EXPONENT},
                ]}),
                200,
                &[],
            ),
        )]);
    }

    #[tokio::test]
    async fn limited_login_tokens_are_verified_against_facebooks_keys() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container);
        fake_public_keys();

        let user = facebook().user_from_token(TOKEN).await.unwrap();

        assert_eq!(user.id, "10229");
        assert_eq!(user.get_name(), Some("Taylor Otwell"));
        assert_eq!(user.get_email(), Some("taylor@laravel.com"));
        assert_eq!(
            user.get_avatar(),
            Some("https://platform-lookaside.fbsbx.com/taylor.jpg")
        );
        assert_eq!(
            user.attribute("avatar_original"),
            Some(&json!("https://platform-lookaside.fbsbx.com/taylor.jpg"))
        );
        assert_eq!(user["first_name"], "Taylor");
        assert_eq!(user["last_name"], "Otwell");
        assert_eq!(user.token, TOKEN);

        Http::assert_sent_count(1);
        Http::assert_not_sent(|request| request.url().contains("graph.facebook.com"));
    }

    #[tokio::test]
    async fn the_nonce_may_be_checked() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container);
        fake_public_keys();

        let user = facebook()
            .user_from_token_with_nonce(TOKEN, "nonce-123")
            .await
            .unwrap();
        assert_eq!(user.id, "10229");

        let error = facebook()
            .user_from_token_with_nonce(TOKEN, "another-nonce")
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Token has incorrect nonce.");
    }

    #[tokio::test]
    async fn invalid_limited_login_tokens_are_rejected() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container);
        fake_public_keys();

        let error = facebook().user_from_token(EXPIRED_TOKEN).await.unwrap_err();
        assert_eq!(error.to_string(), "Expired token");

        let error = facebook()
            .user_from_token(WRONG_AUDIENCE_TOKEN)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Token has incorrect audience.");

        // A signature made for another payload...
        let mut segments: Vec<&str> = TOKEN.split('.').collect();
        let other: Vec<&str> = WRONG_AUDIENCE_TOKEN.split('.').collect();
        segments[2] = other[2];
        let error = facebook()
            .user_from_token(&segments.join("."))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Signature verification failed");

        // A key Facebook doesn't publish...
        Http::fake_urls([(JWKS_URL, Http::response(json!({"keys": []}), 200, &[]))]);
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container);
        Http::fake_urls([(JWKS_URL, Http::response(json!({"keys": []}), 200, &[]))]);
        let error = facebook().user_from_token(TOKEN).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Unable to find the public key [test-kid] of the token."
        );
    }

    #[test]
    fn oidc_tokens_are_recognized() {
        assert!(FacebookProvider::is_oidc_token("a.b.c"));
        assert!(!FacebookProvider::is_oidc_token("EAAGm0PX4ZCpsBA"));
    }
}
