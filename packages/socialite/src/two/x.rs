//! X (formerly Twitter), with OAuth 2.

use async_trait::async_trait;
use illuminate_support::{Map, Result, Value, json};

use crate::support::{QueryEncoding, get_value};
use crate::two::{OAuth2Provider, Provider, User};

/// The X driver (`services.x`): OAuth 2 with PKCE.
///
/// Requests the `users.read` and `tweet.read` scopes, always uses PKCE,
/// and authenticates its token requests with HTTP basic auth.
///
/// ```
/// use laravel_socialite::{Provider, XProvider};
///
/// let x = Provider::new(XProvider::new(), "id", "secret", "https://laravel.test/callback").stateless();
///
/// let url = x.get_auth_url(None);
///
/// assert!(url.starts_with("https://x.com/i/oauth2/authorize?client_id=id&"));
/// assert!(url.contains("&scope=users.read%20tweet.read&"));
/// assert!(url.contains("&code_challenge_method=S256"));
/// ```
#[derive(Clone, Debug)]
pub struct XProvider {
    endpoints: Endpoints,
}

impl XProvider {
    /// Create a new X driver.
    pub fn new() -> Self {
        Self {
            endpoints: Endpoints {
                authorize: "https://x.com/i/oauth2/authorize",
                token: "https://api.x.com/2/oauth2/token",
                user: "https://api.x.com/2/users/me",
            },
        }
    }
}

impl Default for XProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// The Twitter OAuth 2 driver (`services.twitter-oauth-2`, or
/// `services.twitter` with `oauth` set to `2`): [`XProvider`] on the
/// `twitter.com` endpoints.
#[derive(Clone, Debug)]
pub struct TwitterProvider {
    endpoints: Endpoints,
}

impl TwitterProvider {
    /// Create a new Twitter OAuth 2 driver.
    pub fn new() -> Self {
        Self {
            endpoints: Endpoints {
                authorize: "https://twitter.com/i/oauth2/authorize",
                token: "https://api.twitter.com/2/oauth2/token",
                user: "https://api.twitter.com/2/users/me",
            },
        }
    }
}

impl Default for TwitterProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug)]
struct Endpoints {
    authorize: &'static str,
    token: &'static str,
    user: &'static str,
}

impl Endpoints {
    fn code_fields(&self, provider: &Provider, state: Option<&str>) -> Map<String, Value> {
        let mut fields = provider.default_code_fields(state);
        // X requires a state, even when Socialite doesn't check it.
        if provider.is_stateless() {
            fields.insert("state".into(), "state".into());
        }
        fields
    }

    async fn access_token_response(&self, provider: &Provider, code: &str) -> Result<Value> {
        let response = provider
            .get_http_client()
            .with_header("Accept", "application/json")
            .with_basic_auth(provider.get_client_id(), provider.get_client_secret())
            .as_form()
            .post(self.token, Value::Object(provider.get_token_fields(code)))
            .await?;
        Ok(response.json())
    }

    async fn refresh_token_response(
        &self,
        provider: &Provider,
        refresh_token: &str,
    ) -> Result<Value> {
        let response = provider
            .get_http_client()
            .with_header("Accept", "application/json")
            .with_basic_auth(provider.get_client_id(), provider.get_client_secret())
            .as_form()
            .post(
                self.token,
                json!({
                    "grant_type": "refresh_token",
                    "refresh_token": refresh_token,
                    "client_id": provider.get_client_id(),
                }),
            )
            .await?;
        Ok(response.json())
    }

    async fn user_by_token(&self, provider: &Provider, token: &str) -> Result<Value> {
        let response = provider
            .get_http_client()
            .with_token(token, "Bearer")
            .get_with(self.user, json!({"user.fields": "profile_image_url"}))
            .await?;
        Ok(get_value(&response.json(), "data"))
    }

    fn map_user(user: Value) -> User {
        User::new().set_raw(user.clone()).map(json!({
            "id": get_value(&user, "id"),
            "nickname": get_value(&user, "username"),
            "name": get_value(&user, "name"),
            "avatar": get_value(&user, "profile_image_url"),
        }))
    }
}

macro_rules! twitter_provider {
    ($provider:ty) => {
        #[async_trait]
        impl OAuth2Provider for $provider {
            fn default_scopes(&self) -> Vec<String> {
                vec!["users.read".into(), "tweet.read".into()]
            }

            fn scope_separator(&self) -> &str {
                " "
            }

            fn encoding_type(&self) -> QueryEncoding {
                QueryEncoding::Rfc3986
            }

            fn uses_pkce(&self) -> bool {
                true
            }

            fn get_auth_url(&self, provider: &Provider, state: Option<&str>) -> String {
                provider.build_auth_url_from_base(self.endpoints.authorize, state)
            }

            fn get_token_url(&self, _provider: &Provider) -> String {
                self.endpoints.token.into()
            }

            fn get_code_fields(
                &self,
                provider: &Provider,
                state: Option<&str>,
            ) -> Map<String, Value> {
                self.endpoints.code_fields(provider, state)
            }

            async fn get_access_token_response(
                &self,
                provider: &Provider,
                code: &str,
            ) -> Result<Value> {
                self.endpoints.access_token_response(provider, code).await
            }

            async fn get_refresh_token_response(
                &self,
                provider: &Provider,
                refresh_token: &str,
            ) -> Result<Value> {
                self.endpoints
                    .refresh_token_response(provider, refresh_token)
                    .await
            }

            async fn get_user_by_token(&self, provider: &Provider, token: &str) -> Result<Value> {
                self.endpoints.user_by_token(provider, token).await
            }

            fn map_user_to_object(&self, _provider: &Provider, user: Value) -> User {
                Endpoints::map_user(user)
            }
        }
    };
}

twitter_provider!(XProvider);
twitter_provider!(TwitterProvider);
