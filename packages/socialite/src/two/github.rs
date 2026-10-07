//! GitHub.

use async_trait::async_trait;
use illuminate_http_client::PendingRequest;
use illuminate_support::{Result, Value, json};

use crate::support::get_value;
use crate::two::{OAuth2Provider, Provider, User};

/// The GitHub driver (`services.github`).
///
/// Requests the `user:email` scope by default; while it is requested, the
/// user's primary, verified e-mail address is fetched from
/// `/user/emails` (GitHub only returns public addresses from `/user`).
///
/// ```
/// use laravel_socialite::{GithubProvider, Provider};
///
/// let github = Provider::new(GithubProvider, "id", "secret", "https://laravel.test/callback").stateless();
///
/// assert!(github.get_auth_url(None).starts_with("https://github.com/login/oauth/authorize?client_id=id&"));
/// assert_eq!(github.get_token_url(), "https://github.com/login/oauth/access_token");
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct GithubProvider;

impl GithubProvider {
    /// The request options for GitHub's API.
    fn request(provider: &Provider, token: &str) -> PendingRequest {
        provider
            .get_http_client()
            .with_header("Accept", "application/vnd.github.v3+json")
            .with_header("Authorization", format!("token {token}"))
    }

    /// Get the user's primary, verified e-mail address (`None` when it
    /// can't be retrieved).
    pub async fn get_email_by_token(&self, provider: &Provider, token: &str) -> Option<String> {
        let response = Self::request(provider, token)
            .get("https://api.github.com/user/emails")
            .await
            .ok()?;
        let Value::Array(emails) = response.json() else {
            return None;
        };
        emails
            .iter()
            .find(|email| email["primary"] == true && email["verified"] == true)
            .and_then(|email| email["email"].as_str().map(str::to_string))
    }
}

#[async_trait]
impl OAuth2Provider for GithubProvider {
    fn default_scopes(&self) -> Vec<String> {
        vec!["user:email".into()]
    }

    fn get_auth_url(&self, provider: &Provider, state: Option<&str>) -> String {
        provider.build_auth_url_from_base("https://github.com/login/oauth/authorize", state)
    }

    fn get_token_url(&self, _provider: &Provider) -> String {
        "https://github.com/login/oauth/access_token".into()
    }

    async fn get_user_by_token(&self, provider: &Provider, token: &str) -> Result<Value> {
        let mut user = Self::request(provider, token)
            .get("https://api.github.com/user")
            .await?
            .json();

        if provider
            .get_scopes()
            .iter()
            .any(|scope| scope == "user:email")
        {
            let email = self.get_email_by_token(provider, token).await;
            if let Value::Object(user) = &mut user {
                user.insert("email".into(), email.map_or(Value::Null, Value::String));
            }
        }

        Ok(user)
    }

    fn map_user_to_object(&self, _provider: &Provider, user: Value) -> User {
        User::new().set_raw(user.clone()).map(json!({
            "id": get_value(&user, "id"),
            "nodeId": get_value(&user, "node_id"),
            "nickname": get_value(&user, "login"),
            "name": get_value(&user, "name"),
            "email": get_value(&user, "email"),
            "avatar": get_value(&user, "avatar_url"),
        }))
    }
}
