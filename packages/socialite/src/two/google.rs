//! Google.

use async_trait::async_trait;
use illuminate_support::{Result, Value, json};

use crate::support::get_value;
use crate::two::{OAuth2Provider, Provider, User};

/// The Google driver (`services.google`), using OpenID Connect's
/// `openid`, `profile` and `email` scopes.
///
/// Optional parameters such as `hd` (a hosted domain), `prompt` or
/// `access_type` may be passed with [`Provider::with`]:
///
/// ```
/// use illuminate_support::json;
/// use laravel_socialite::{GoogleProvider, Provider};
///
/// let google = Provider::new(GoogleProvider::new(), "id", "secret", "https://laravel.test/callback")
///     .with(json!({"hd": "example.com"}))
///     .stateless();
///
/// assert_eq!(
///     google.get_auth_url(None),
///     "https://accounts.google.com/o/oauth2/auth?client_id=id\
///      &redirect_uri=https%3A%2F%2Flaravel.test%2Fcallback\
///      &scope=openid+profile+email&response_type=code&hd=example.com",
/// );
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct GoogleProvider;

impl GoogleProvider {
    /// Create a new Google driver.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl OAuth2Provider for GoogleProvider {
    fn default_scopes(&self) -> Vec<String> {
        vec!["openid".into(), "profile".into(), "email".into()]
    }

    fn scope_separator(&self) -> &str {
        " "
    }

    fn get_auth_url(&self, provider: &Provider, state: Option<&str>) -> String {
        provider.build_auth_url_from_base("https://accounts.google.com/o/oauth2/auth", state)
    }

    fn get_token_url(&self, _provider: &Provider) -> String {
        "https://www.googleapis.com/oauth2/v4/token".into()
    }

    async fn get_user_by_token(&self, provider: &Provider, token: &str) -> Result<Value> {
        let response = provider
            .get_http_client()
            .with_header("Accept", "application/json")
            .with_token(token, "Bearer")
            .get_with(
                "https://www.googleapis.com/oauth2/v3/userinfo",
                json!({"prettyPrint": "false"}),
            )
            .await?;
        Ok(response.json())
    }

    fn map_user_to_object(&self, _provider: &Provider, mut user: Value) -> User {
        // Fields kept for backwards compatibility with Google's older API.
        let id = get_value(&user, "sub");
        let verified = get_value(&user, "email_verified");
        let link = get_value(&user, "profile");
        if let Value::Object(raw) = &mut user {
            raw.insert("id".into(), id.clone());
            raw.insert("verified_email".into(), verified);
            raw.insert("link".into(), link);
        }

        let avatar = get_value(&user, "picture");
        User::new().set_raw(user.clone()).map(json!({
            "id": id,
            "nickname": get_value(&user, "nickname"),
            "name": get_value(&user, "name"),
            "email": get_value(&user, "email"),
            "avatar": avatar,
            "avatar_original": avatar,
        }))
    }
}
