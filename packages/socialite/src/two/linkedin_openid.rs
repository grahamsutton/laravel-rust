//! LinkedIn, with OpenID Connect.

use async_trait::async_trait;
use illuminate_support::{Result, Value, json};

use crate::support::get_value;
use crate::two::{OAuth2Provider, Provider, User};

/// The LinkedIn driver (`services.linkedin-openid`), using "Sign In with
/// LinkedIn using OpenID Connect" and its `openid`, `profile` and `email`
/// scopes.
///
/// ```
/// use laravel_socialite::{LinkedInOpenIdProvider, Provider};
///
/// let linkedin = Provider::new(LinkedInOpenIdProvider, "id", "secret", "https://laravel.test/callback").stateless();
///
/// assert!(linkedin.get_auth_url(None).starts_with("https://www.linkedin.com/oauth/v2/authorization?client_id=id&"));
/// assert_eq!(linkedin.get_token_url(), "https://www.linkedin.com/oauth/v2/accessToken");
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct LinkedInOpenIdProvider;

impl LinkedInOpenIdProvider {
    /// Get the user's basic profile from LinkedIn's `userinfo` endpoint.
    pub async fn get_basic_profile(&self, provider: &Provider, token: &str) -> Result<Value> {
        let response = provider
            .get_http_client()
            .with_token(token, "Bearer")
            .with_header("X-RestLi-Protocol-Version", "2.0.0")
            .get("https://api.linkedin.com/v2/userinfo")
            .await?;
        Ok(response.json())
    }
}

#[async_trait]
impl OAuth2Provider for LinkedInOpenIdProvider {
    fn default_scopes(&self) -> Vec<String> {
        vec!["openid".into(), "profile".into(), "email".into()]
    }

    fn scope_separator(&self) -> &str {
        " "
    }

    fn get_auth_url(&self, provider: &Provider, state: Option<&str>) -> String {
        provider.build_auth_url_from_base("https://www.linkedin.com/oauth/v2/authorization", state)
    }

    fn get_token_url(&self, _provider: &Provider) -> String {
        "https://www.linkedin.com/oauth/v2/accessToken".into()
    }

    async fn get_user_by_token(&self, provider: &Provider, token: &str) -> Result<Value> {
        self.get_basic_profile(provider, token).await
    }

    fn map_user_to_object(&self, _provider: &Provider, user: Value) -> User {
        User::new().set_raw(user.clone()).map(json!({
            "id": get_value(&user, "sub"),
            "nickname": null,
            "name": get_value(&user, "name"),
            "first_name": get_value(&user, "given_name"),
            "last_name": get_value(&user, "family_name"),
            "email": get_value(&user, "email"),
            "email_verified": get_value(&user, "email_verified"),
            "avatar": get_value(&user, "picture"),
            "avatar_original": get_value(&user, "picture"),
        }))
    }
}
