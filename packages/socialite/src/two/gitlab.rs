//! GitLab.

use async_trait::async_trait;
use illuminate_support::{Result, Value, json};

use crate::support::get_value;
use crate::two::{OAuth2Provider, Provider, User};

/// The GitLab driver (`services.gitlab`), for GitLab.com or your own
/// instance (the `host` configuration option).
///
/// ```
/// use laravel_socialite::{GitlabProvider, Provider};
///
/// let gitlab = Provider::new(GitlabProvider::new(), "id", "secret", "https://laravel.test/callback")
///     .set_host("https://gitlab.example.com/")
///     .stateless();
///
/// assert!(gitlab.get_auth_url(None).starts_with("https://gitlab.example.com/oauth/authorize?client_id=id&"));
/// assert_eq!(gitlab.get_token_url(), "https://gitlab.example.com/oauth/token");
/// ```
#[derive(Clone, Debug)]
pub struct GitlabProvider {
    /// The GitLab instance host (`https://gitlab.com` by default).
    pub host: String,
}

impl Default for GitlabProvider {
    fn default() -> Self {
        Self {
            host: "https://gitlab.com".into(),
        }
    }
}

impl GitlabProvider {
    /// Create a new GitLab driver for GitLab.com.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the GitLab instance host (blank hosts are ignored).
    pub fn set_host(&mut self, host: &str) {
        if !host.trim().is_empty() {
            self.host = host.trim_end_matches('/').to_string();
        }
    }
}

#[async_trait]
impl OAuth2Provider for GitlabProvider {
    fn default_scopes(&self) -> Vec<String> {
        vec!["read_user".into()]
    }

    fn scope_separator(&self) -> &str {
        " "
    }

    fn get_auth_url(&self, provider: &Provider, state: Option<&str>) -> String {
        provider.build_auth_url_from_base(&format!("{}/oauth/authorize", self.host), state)
    }

    fn get_token_url(&self, _provider: &Provider) -> String {
        format!("{}/oauth/token", self.host)
    }

    async fn get_user_by_token(&self, provider: &Provider, token: &str) -> Result<Value> {
        let response = provider
            .get_http_client()
            .get_with(
                format!("{}/api/v4/user", self.host),
                json!({"access_token": token}),
            )
            .await?;
        Ok(response.json())
    }

    fn map_user_to_object(&self, _provider: &Provider, user: Value) -> User {
        User::new().set_raw(user.clone()).map(json!({
            "id": get_value(&user, "id"),
            "nickname": get_value(&user, "username"),
            "name": get_value(&user, "name"),
            "email": get_value(&user, "email"),
            "avatar": get_value(&user, "avatar_url"),
        }))
    }
}

impl Provider {
    /// Set the GitLab instance host (GitLab only).
    pub fn set_host(self, host: impl AsRef<str>) -> Self {
        self.with_driver(|gitlab: &mut GitlabProvider| gitlab.set_host(host.as_ref()))
    }
}
