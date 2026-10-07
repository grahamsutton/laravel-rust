//! Slack, with OpenID Connect.

use async_trait::async_trait;
use illuminate_support::{Result, Value, json};

use crate::support::get_value;
use crate::two::{OAuth2Provider, Provider, User};

/// The Slack OpenID Connect driver (`services.slack-openid`), using
/// "Sign in with Slack" and its `openid`, `email` and `profile` scopes.
///
/// ```
/// use laravel_socialite::{Provider, SlackOpenIdProvider};
///
/// let slack = Provider::new(SlackOpenIdProvider, "id", "secret", "https://laravel.test/callback").stateless();
///
/// assert!(slack.get_auth_url(None).starts_with("https://slack.com/openid/connect/authorize?client_id=id&"));
/// assert!(slack.get_auth_url(None).contains("&scope=openid+email+profile&"));
/// assert_eq!(slack.get_token_url(), "https://slack.com/api/openid.connect.token");
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct SlackOpenIdProvider;

#[async_trait]
impl OAuth2Provider for SlackOpenIdProvider {
    fn default_scopes(&self) -> Vec<String> {
        vec!["openid".into(), "email".into(), "profile".into()]
    }

    fn scope_separator(&self) -> &str {
        " "
    }

    fn get_auth_url(&self, provider: &Provider, state: Option<&str>) -> String {
        provider.build_auth_url_from_base("https://slack.com/openid/connect/authorize", state)
    }

    fn get_token_url(&self, _provider: &Provider) -> String {
        "https://slack.com/api/openid.connect.token".into()
    }

    async fn get_user_by_token(&self, provider: &Provider, token: &str) -> Result<Value> {
        let response = provider
            .get_http_client()
            .with_token(token, "Bearer")
            .get("https://slack.com/api/openid.connect.userInfo")
            .await?;
        Ok(response.json())
    }

    fn map_user_to_object(&self, _provider: &Provider, user: Value) -> User {
        User::new().set_raw(user.clone()).map(json!({
            "id": get_value(&user, "sub"),
            "nickname": null,
            "name": get_value(&user, "name"),
            "email": get_value(&user, "email"),
            "avatar": get_value(&user, "picture"),
            "organization_id": get_value(&user, "https://slack.com/team_id"),
        }))
    }
}
