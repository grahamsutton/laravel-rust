//! Socialite's testing fakes (see [`Socialite::fake`](crate::Socialite::fake)).

use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use illuminate_http::Response;
use illuminate_support::{Map, Result, Value, json};

use crate::two::{OAuth2Provider, Provider, User};

/// A faked OAuth provider: instead of talking to the real provider, the
/// driver redirects to a fake authorization URL and returns the fake user.
///
/// Created by [`Socialite::fake`](crate::Socialite::fake) and
/// [`Socialite::fake_with`](crate::Socialite::fake_with); every
/// `Socialite::driver(...)` call for the faked driver uses it until the
/// test's container goes away.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_support::json;
/// use laravel_socialite::{Socialite, User};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container.clone());
/// let fake = Socialite::fake_with("github", User::fake(json!({"id": "github-123", "name": "Jason Beggs"})));
///
/// let response = Socialite::driver("github").scopes(["read:org"]).redirect()?;
/// assert_eq!(response.target_url().unwrap(), "https://socialite.fake/github/authorize");
/// assert_eq!(fake.redirects()[0].scopes, ["read:org"]);
///
/// let user = Socialite::driver("github").user().await?;
/// assert_eq!(user.id, "github-123");
/// assert_eq!(user.token, "fake-token");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Debug)]
pub struct FakeProvider {
    driver: String,
    user: RwLock<User>,
    redirects: Mutex<Vec<FakeRedirect>>,
}

/// A redirect made by a faked provider: how the application configured
/// the provider before redirecting.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeRedirect {
    /// The requested scopes.
    pub scopes: Vec<String>,
    /// The custom parameters (from [`Provider::with`]).
    pub parameters: Map<String, Value>,
    /// Whether the provider was stateless.
    pub stateless: bool,
    /// Whether PKCE was enabled.
    pub uses_pkce: bool,
    /// The redirect (callback) URL.
    pub redirect_url: String,
}

impl FakeProvider {
    /// Create a fake for the driver, returning the given user.
    pub fn new(driver: impl Into<String>, user: User) -> Self {
        Self {
            driver: driver.into(),
            user: RwLock::new(user),
            redirects: Mutex::new(Vec::new()),
        }
    }

    /// The name of the faked driver.
    pub fn driver(&self) -> &str {
        &self.driver
    }

    /// The fake authorization URL users are redirected to:
    /// `https://socialite.fake/{driver}/authorize`.
    pub fn authorize_url(&self) -> String {
        format!("https://socialite.fake/{}/authorize", self.driver)
    }

    /// The user the fake returns.
    pub fn user(&self) -> User {
        self.user.read().unwrap().clone()
    }

    /// Change the user the fake returns.
    pub fn set_user(&self, user: User) {
        *self.user.write().unwrap() = user;
    }

    /// The redirects made so far.
    pub fn redirects(&self) -> Vec<FakeRedirect> {
        self.redirects.lock().unwrap().clone()
    }

    /// Determine if the application redirected to the provider.
    pub fn redirected(&self) -> bool {
        !self.redirects.lock().unwrap().is_empty()
    }
}

/// The driver of a faked provider.
pub(crate) struct FakeDriver(pub(crate) Arc<FakeProvider>);

#[async_trait]
impl OAuth2Provider for FakeDriver {
    fn get_auth_url(&self, _provider: &Provider, _state: Option<&str>) -> String {
        self.0.authorize_url()
    }

    fn get_token_url(&self, _provider: &Provider) -> String {
        format!("https://socialite.fake/{}/token", self.0.driver)
    }

    async fn get_user_by_token(&self, _provider: &Provider, _token: &str) -> Result<Value> {
        Ok(self.0.user().raw)
    }

    fn map_user_to_object(&self, _provider: &Provider, _user: Value) -> User {
        self.0.user()
    }

    fn redirect(&self, provider: &Provider) -> Result<Response> {
        self.0.redirects.lock().unwrap().push(FakeRedirect {
            scopes: provider.get_scopes().to_vec(),
            parameters: provider.get_parameters().clone(),
            stateless: provider.is_stateless(),
            uses_pkce: provider.uses_pkce(),
            redirect_url: provider.get_redirect_url().to_string(),
        });
        Ok(Response::redirect(self.0.authorize_url()))
    }

    async fn user(&self, _provider: &Provider) -> Result<User> {
        Ok(self.0.user())
    }

    async fn user_from_token(&self, _provider: &Provider, token: &str) -> Result<User> {
        Ok(self.0.user().set_token(token))
    }

    async fn get_access_token_response(&self, _provider: &Provider, _code: &str) -> Result<Value> {
        let user = self.0.user();
        Ok(json!({
            "access_token": user.token,
            "refresh_token": user.refresh_token,
            "expires_in": user.expires_in,
            "scope": user.approved_scopes.join(","),
        }))
    }

    async fn get_refresh_token_response(
        &self,
        _provider: &Provider,
        refresh_token: &str,
    ) -> Result<Value> {
        let user = self.0.user();
        Ok(json!({
            "access_token": user.token,
            "refresh_token": refresh_token,
            "expires_in": user.expires_in,
            "scope": user.approved_scopes.join(","),
        }))
    }
}
