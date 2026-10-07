//! Bitbucket.

use async_trait::async_trait;
use illuminate_support::{Result, Value, json};

use crate::support::get_value;
use crate::two::{OAuth2Provider, Provider, User};

/// The Bitbucket driver (`services.bitbucket`).
///
/// Requests the `email` scope by default; while it is requested, the
/// user's primary, confirmed e-mail address is fetched from `/user/emails`.
///
/// ```
/// use laravel_socialite::{BitbucketProvider, Provider};
///
/// let bitbucket = Provider::new(BitbucketProvider, "id", "secret", "https://laravel.test/callback").stateless();
///
/// assert!(bitbucket.get_auth_url(None).starts_with("https://bitbucket.org/site/oauth2/authorize?client_id=id&"));
/// assert_eq!(bitbucket.get_token_url(), "https://bitbucket.org/site/oauth2/access_token");
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct BitbucketProvider;

impl BitbucketProvider {
    /// Get the user's primary, confirmed e-mail address (`None` when it
    /// can't be retrieved).
    pub async fn get_email_by_token(&self, provider: &Provider, token: &str) -> Option<String> {
        let response = provider
            .get_http_client()
            .get_with(
                "https://api.bitbucket.org/2.0/user/emails",
                json!({"access_token": token}),
            )
            .await
            .ok()?;
        response.json()["values"]
            .as_array()?
            .iter()
            .find(|email| {
                email["type"] == "email"
                    && email["is_primary"] == true
                    && email["is_confirmed"] == true
            })
            .and_then(|email| email["email"].as_str().map(str::to_string))
    }
}

#[async_trait]
impl OAuth2Provider for BitbucketProvider {
    fn default_scopes(&self) -> Vec<String> {
        vec!["email".into()]
    }

    fn scope_separator(&self) -> &str {
        " "
    }

    fn get_auth_url(&self, provider: &Provider, state: Option<&str>) -> String {
        provider.build_auth_url_from_base("https://bitbucket.org/site/oauth2/authorize", state)
    }

    fn get_token_url(&self, _provider: &Provider) -> String {
        "https://bitbucket.org/site/oauth2/access_token".into()
    }

    async fn get_user_by_token(&self, provider: &Provider, token: &str) -> Result<Value> {
        let mut user = provider
            .get_http_client()
            .get_with(
                "https://api.bitbucket.org/2.0/user",
                json!({"access_token": token}),
            )
            .await?
            .json();

        if provider.get_scopes().iter().any(|scope| scope == "email") {
            let email = self.get_email_by_token(provider, token).await;
            if let Value::Object(user) = &mut user {
                user.insert("email".into(), email.map_or(Value::Null, Value::String));
            }
        }

        Ok(user)
    }

    fn map_user_to_object(&self, _provider: &Provider, user: Value) -> User {
        User::new().set_raw(user.clone()).map(json!({
            "id": get_value(&user, "uuid"),
            "nickname": get_value(&user, "username"),
            "name": get_value(&user, "display_name"),
            "email": get_value(&user, "email"),
            "avatar": get_value(&user, "links.avatar.href"),
        }))
    }
}
