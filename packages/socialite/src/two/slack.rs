//! Slack, with user or bot tokens.

use async_trait::async_trait;
use illuminate_support::{Map, Result, Value, ValueExt, json};

use crate::exceptions::InvalidStateException;
use crate::support::{get, get_string, get_value};
use crate::two::{OAuth2Provider, Provider, User};

/// The Slack driver (`services.slack`), using Slack's "Sign in with Slack"
/// v2 flow.
///
/// By default it requests a *user* token (`xoxp-`) with the
/// `identity.basic`, `identity.email`, `identity.team` and
/// `identity.avatar` user scopes. Call [`Provider::as_bot_user`] — both
/// before redirecting and when retrieving the user — for a *bot* token
/// (`xoxb-`) instead:
///
/// ```
/// use laravel_socialite::{Provider, SlackProvider};
///
/// let slack = Provider::new(SlackProvider::new(), "id", "secret", "https://laravel.test/callback").stateless();
/// assert!(slack.get_auth_url(None).contains(
///     "&scope=&response_type=code&user_scope=identity.basic%2Cidentity.email%2Cidentity.team%2Cidentity.avatar",
/// ));
///
/// let bot = Provider::new(SlackProvider::new(), "id", "secret", "https://laravel.test/callback")
///     .as_bot_user()
///     .set_scopes(["chat:write", "chat:write.public", "chat:write.customize"])
///     .stateless();
/// assert!(bot.get_auth_url(None).contains("&scope=chat%3Awrite%2Cchat%3Awrite.public%2Cchat%3Awrite.customize&"));
/// ```
#[derive(Clone, Debug, Default)]
pub struct SlackProvider {
    /// Whether a bot token is requested (Laravel's `scope` scope key)
    /// instead of a user token (`user_scope`).
    pub bot: bool,
}

impl SlackProvider {
    /// Create a new Slack driver, for user tokens.
    pub fn new() -> Self {
        Self::default()
    }

    /// The query key the requested scopes are sent as: `user_scope` for
    /// user tokens, `scope` for bot tokens.
    pub fn scope_key(&self) -> &'static str {
        if self.bot { "scope" } else { "user_scope" }
    }
}

#[async_trait]
impl OAuth2Provider for SlackProvider {
    fn default_scopes(&self) -> Vec<String> {
        [
            "identity.basic",
            "identity.email",
            "identity.team",
            "identity.avatar",
        ]
        .map(String::from)
        .to_vec()
    }

    fn get_auth_url(&self, provider: &Provider, state: Option<&str>) -> String {
        provider.build_auth_url_from_base("https://slack.com/oauth/v2/authorize", state)
    }

    fn get_token_url(&self, _provider: &Provider) -> String {
        "https://slack.com/api/oauth.v2.access".into()
    }

    fn get_code_fields(&self, provider: &Provider, state: Option<&str>) -> Map<String, Value> {
        let mut fields = provider.default_code_fields(state);
        if !self.bot {
            fields.insert("scope".into(), "".into());
            fields.insert(
                "user_scope".into(),
                provider
                    .format_scopes(provider.get_scopes(), provider.scope_separator())
                    .into(),
            );
        }
        fields
    }

    async fn get_access_token_response(&self, provider: &Provider, code: &str) -> Result<Value> {
        let response = provider.default_access_token_response(code).await?;
        // A user token lives under `authed_user`; a bot token at the top.
        if !self.bot
            && let Some(authed_user @ Value::Object(_)) = get(&response, "authed_user")
        {
            return Ok(authed_user.clone());
        }
        Ok(response)
    }

    async fn user(&self, provider: &Provider) -> Result<User> {
        if !self.bot {
            return provider.default_user().await;
        }
        if provider.has_invalid_state()? {
            return Err(InvalidStateException.into());
        }
        let code = provider.get_code().unwrap_or_default();
        let response = provider.get_access_token_response(&code).await?;
        Ok(User::new()
            .set_access_token_response_body(response.clone())
            .set_token(get_string(&response, "access_token").unwrap_or_default())
            .set_refresh_token(get_string(&response, "refresh_token"))
            .set_expires_in(get(&response, "expires_in").and_then(ValueExt::to_i64_lossy))
            .set_approved_scopes(provider.parse_approved_scopes(&response)))
    }

    async fn get_user_by_token(&self, provider: &Provider, token: &str) -> Result<Value> {
        let response = provider
            .get_http_client()
            .with_token(token, "Bearer")
            .get("https://slack.com/api/users.identity")
            .await?;
        Ok(response.json())
    }

    fn map_user_to_object(&self, _provider: &Provider, user: Value) -> User {
        User::new().set_raw(user.clone()).map(json!({
            "id": get_value(&user, "user.id"),
            "name": get_value(&user, "user.name"),
            "email": get_value(&user, "user.email"),
            "avatar": get_value(&user, "user.image_512"),
            "organization_id": get_value(&user, "team.id"),
        }))
    }
}

impl Provider {
    /// Request a Slack *bot* token (`xoxb-`) instead of a user token (Slack
    /// only). Call it before redirecting *and* before retrieving the user;
    /// the user then only carries the token.
    pub fn as_bot_user(self) -> Self {
        self.with_driver(|slack: &mut SlackProvider| slack.bot = true)
    }
}
