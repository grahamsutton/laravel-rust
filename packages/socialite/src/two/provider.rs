//! The OAuth 2 provider: Laravel's `Two\AbstractProvider`, and the
//! [`OAuth2Provider`] trait every driver implements.

use std::any::Any;
use std::fmt;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use illuminate_http::{Request, Response, current_request};
use illuminate_http_client::{Http, PendingRequest};
use illuminate_session::{RequestSessionExt, Store};
use illuminate_support::error::RuntimeException;
use illuminate_support::{Error, Map, Result, Str, Value, ValueExt, to_value};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::exceptions::InvalidStateException;
use crate::support::{QueryEncoding, get, get_string, string_list, unique};
use crate::two::{Token, User};

/// An OAuth 2 driver: the provider-specific half of Laravel's
/// `Two\AbstractProvider` subclasses (`GithubProvider`, `GoogleProvider`,
/// and every community provider).
///
/// A driver says where to send the user, where to exchange the code for a
/// token, how to fetch the user with that token, and how to map the
/// provider's user onto a [`User`]. Everything else — state, PKCE, scopes,
/// token exchange, refreshing tokens — is handled by [`Provider`], which
/// every hook receives so it can use the shared building blocks
/// ([`Provider::build_auth_url_from_base`], [`Provider::get_http_client`],
/// ...). Override the other hooks to change the shared behavior, calling the
/// `default_*` methods of [`Provider`] for Laravel's `parent::` behavior.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_http_client::Http;
/// use illuminate_support::{json, Result, Value};
/// use laravel_socialite::{async_trait, OAuth2Provider, Provider, Socialite, User};
///
/// pub struct DiscordProvider;
///
/// #[async_trait]
/// impl OAuth2Provider for DiscordProvider {
///     fn default_scopes(&self) -> Vec<String> {
///         vec!["identify".into(), "email".into()]
///     }
///
///     fn scope_separator(&self) -> &str {
///         " "
///     }
///
///     fn get_auth_url(&self, provider: &Provider, state: Option<&str>) -> String {
///         provider.build_auth_url_from_base("https://discord.com/oauth2/authorize", state)
///     }
///
///     fn get_token_url(&self, _provider: &Provider) -> String {
///         "https://discord.com/api/oauth2/token".into()
///     }
///
///     async fn get_user_by_token(&self, provider: &Provider, token: &str) -> Result<Value> {
///         let response = provider
///             .get_http_client()
///             .with_token(token, "Bearer")
///             .get("https://discord.com/api/users/@me")
///             .await?;
///
///         Ok(response.json())
///     }
///
///     fn map_user_to_object(&self, _provider: &Provider, user: Value) -> User {
///         User::new().set_raw(user.clone()).map(json!({
///             "id": user["id"],
///             "nickname": user["username"],
///             "name": user["global_name"],
///             "email": user["email"],
///         }))
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container.clone());
/// Http::fake_urls([(
///     "https://discord.com/api/users/@me",
///     Http::response(json!({"id": "80351110224678912", "username": "nelly", "global_name": "Nelly"}), 200, &[]),
/// )]);
///
/// let discord = Socialite::build_provider(DiscordProvider, &json!({
///     "client_id": "client-id",
///     "client_secret": "client-secret",
///     "redirect": "https://laravel.test/auth/discord/callback",
/// }))?;
///
/// let user = discord.user_from_token("token").await?;
///
/// assert_eq!(user.id, "80351110224678912");
/// assert_eq!(user.get_nickname(), Some("nelly"));
/// assert_eq!(user.token, "token");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[async_trait]
pub trait OAuth2Provider: Any + Send + Sync {
    /// The scopes requested by default (Laravel's `$scopes` property).
    fn default_scopes(&self) -> Vec<String> {
        Vec::new()
    }

    /// The separator used to join the requested scopes (Laravel's
    /// `$scopeSeparator`, a comma by default), and to split the scopes the
    /// provider says were approved.
    fn scope_separator(&self) -> &str {
        ","
    }

    /// How the authorization URL's query string is encoded (Laravel's
    /// `$encodingType`).
    fn encoding_type(&self) -> QueryEncoding {
        QueryEncoding::Rfc1738
    }

    /// Whether PKCE is enabled by default (Laravel's `$usesPKCE`).
    fn uses_pkce(&self) -> bool {
        false
    }

    /// Get the authentication URL for the provider — usually
    /// [`Provider::build_auth_url_from_base`] with the provider's
    /// authorization endpoint.
    fn get_auth_url(&self, provider: &Provider, state: Option<&str>) -> String;

    /// Get the token URL for the provider.
    fn get_token_url(&self, provider: &Provider) -> String;

    /// Get the raw user for the given access token.
    async fn get_user_by_token(&self, provider: &Provider, token: &str) -> Result<Value>;

    /// Map the raw user array to a Socialite [`User`].
    fn map_user_to_object(&self, provider: &Provider, user: Value) -> User;

    /// Get the query fields of the authorization URL. Defaults to
    /// [`Provider::default_code_fields`].
    fn get_code_fields(&self, provider: &Provider, state: Option<&str>) -> Map<String, Value> {
        provider.default_code_fields(state)
    }

    /// Get the headers of the access token request. Defaults to
    /// [`Provider::default_token_headers`].
    fn get_token_headers(&self, provider: &Provider, code: &str) -> Vec<(String, String)> {
        provider.default_token_headers(code)
    }

    /// Get the form fields of the access token request. Defaults to
    /// [`Provider::default_token_fields`].
    fn get_token_fields(&self, provider: &Provider, code: &str) -> Map<String, Value> {
        provider.default_token_fields(code)
    }

    /// Exchange the authorization code for the access token response.
    /// Defaults to [`Provider::default_access_token_response`].
    async fn get_access_token_response(&self, provider: &Provider, code: &str) -> Result<Value> {
        provider.default_access_token_response(code).await
    }

    /// Exchange a refresh token for a new access token response. Defaults
    /// to [`Provider::default_refresh_token_response`].
    async fn get_refresh_token_response(
        &self,
        provider: &Provider,
        refresh_token: &str,
    ) -> Result<Value> {
        provider.default_refresh_token_response(refresh_token).await
    }

    /// Redirect the user to the authentication page. Defaults to
    /// [`Provider::default_redirect`].
    fn redirect(&self, provider: &Provider) -> Result<Response> {
        provider.default_redirect()
    }

    /// Get the user for the current (callback) request. Defaults to
    /// [`Provider::default_user`].
    async fn user(&self, provider: &Provider) -> Result<User> {
        provider.default_user().await
    }

    /// Get the user for an access token you already have. Defaults to
    /// [`Provider::default_user_from_token`].
    async fn user_from_token(&self, provider: &Provider, token: &str) -> Result<User> {
        provider.default_user_from_token(token).await
    }
}

/// An OAuth 2 provider, ready to redirect users and retrieve them: what
/// [`Socialite::driver`](crate::Socialite::driver) returns.
///
/// `Provider` holds the state Laravel's `Two\AbstractProvider` keeps — the
/// client credentials, redirect URL, scopes, extra parameters, and the
/// `stateless` and PKCE switches — around the [`OAuth2Provider`] driver
/// that knows the provider's endpoints.
///
/// ```
/// use laravel_socialite::{GithubProvider, Provider};
///
/// let github = Provider::new(GithubProvider, "client-id", "client-secret", "https://laravel.test/callback")
///     .scopes(["read:user", "public_repo"])
///     .with([("allow_signup", "false")])
///     .stateless();
///
/// assert_eq!(github.get_scopes(), ["user:email", "read:user", "public_repo"]);
///
/// let response = github.redirect()?;
///
/// assert_eq!(
///     response.target_url().unwrap(),
///     "https://github.com/login/oauth/authorize?client_id=client-id\
///      &redirect_uri=https%3A%2F%2Flaravel.test%2Fcallback\
///      &scope=user%3Aemail%2Cread%3Auser%2Cpublic_repo&response_type=code&allow_signup=false",
/// );
/// # Ok::<(), illuminate_support::Error>(())
/// ```
///
/// Each call to [`Socialite::driver`](crate::Socialite::driver) builds a new
/// provider, so configuring one never leaks into another request.
pub struct Provider {
    driver: Box<dyn OAuth2Provider>,
    client_id: String,
    client_secret: String,
    redirect_url: String,
    scopes: Vec<String>,
    parameters: Map<String, Value>,
    stateless: bool,
    uses_pkce: bool,
    http_options: Value,
    request: Option<Request>,
    user: Mutex<Option<User>>,
}

impl Provider {
    /// Create a new provider instance for the driver, with the client's
    /// credentials and redirect URL.
    pub fn new(
        driver: impl OAuth2Provider,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_url: impl Into<String>,
    ) -> Self {
        let scopes = unique(driver.default_scopes());
        let uses_pkce = driver.uses_pkce();
        Self {
            driver: Box::new(driver),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            redirect_url: redirect_url.into(),
            scopes,
            parameters: Map::new(),
            stateless: false,
            uses_pkce,
            http_options: Value::Null,
            request: None,
            user: Mutex::new(None),
        }
    }

    /// A provider that couldn't be created: every request it makes returns
    /// the error.
    pub(crate) fn failed(error: Error) -> Self {
        Self::new(Unresolved::new(error), "", "", "")
    }

    // ------------------------------------------------------------------
    // Configuring the request
    // ------------------------------------------------------------------

    /// Merge the scopes of the requested access.
    ///
    /// ```
    /// use laravel_socialite::{GithubProvider, Provider};
    ///
    /// let github = Provider::new(GithubProvider, "id", "secret", "/callback")
    ///     .scopes(["read:user", "public_repo"])
    ///     .scopes(["read:user"]);
    ///
    /// assert_eq!(github.get_scopes(), ["user:email", "read:user", "public_repo"]);
    /// ```
    pub fn scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let merged = std::mem::take(&mut self.scopes)
            .into_iter()
            .chain(scopes.into_iter().map(Into::into));
        self.scopes = unique(merged);
        self
    }

    /// Set the scopes of the requested access, replacing the defaults.
    ///
    /// ```
    /// use laravel_socialite::{GithubProvider, Provider};
    ///
    /// let github = Provider::new(GithubProvider, "id", "secret", "/callback")
    ///     .set_scopes(["read:user", "public_repo"]);
    ///
    /// assert_eq!(github.get_scopes(), ["read:user", "public_repo"]);
    /// ```
    pub fn set_scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.scopes = unique(scopes.into_iter().map(Into::into));
        self
    }

    /// Get the current scopes.
    pub fn get_scopes(&self) -> &[String] {
        &self.scopes
    }

    /// Set the custom parameters of the request — a JSON object or a list
    /// of pairs. They're added to the authorization URL and the token
    /// request (and replace any parameters set before).
    ///
    /// Be careful not to pass reserved keywords such as `state` or
    /// `response_type`.
    ///
    /// ```
    /// use illuminate_support::json;
    /// use laravel_socialite::{GoogleProvider, Provider};
    ///
    /// let google = Provider::new(GoogleProvider::new(), "id", "secret", "/callback")
    ///     .with(json!({"hd": "example.com"}));
    ///
    /// assert_eq!(google.get_parameters()["hd"], "example.com");
    /// ```
    pub fn with(mut self, parameters: impl Serialize) -> Self {
        self.parameters = parameters_from(to_value(&parameters));
        self
    }

    /// Get the custom parameters of the request.
    pub fn get_parameters(&self) -> &Map<String, Value> {
        &self.parameters
    }

    /// Indicate that the provider should operate as stateless: no `state`
    /// is stored in (or checked against) the session — handy for APIs
    /// without cookie based sessions.
    pub fn stateless(mut self) -> Self {
        self.stateless = true;
        self
    }

    /// Determine if the provider is operating as stateless.
    pub fn is_stateless(&self) -> bool {
        self.stateless
    }

    /// Determine if the provider is operating with state.
    pub fn uses_state(&self) -> bool {
        !self.stateless
    }

    /// Enable PKCE ("Proof Key for Code Exchange") for this request: a
    /// `code_verifier` is stored in the session, its S256 `code_challenge`
    /// is sent with the redirect, and the verifier with the token request.
    pub fn enable_pkce(mut self) -> Self {
        self.uses_pkce = true;
        self
    }

    /// Determine if the provider uses PKCE.
    pub fn uses_pkce(&self) -> bool {
        self.uses_pkce
    }

    /// Set the redirect URL (the callback the provider sends the user back to).
    pub fn redirect_url(mut self, url: impl Into<String>) -> Self {
        self.redirect_url = url.into();
        self
    }

    /// Get the redirect URL.
    pub fn get_redirect_url(&self) -> &str {
        &self.redirect_url
    }

    /// Get the client ID.
    pub fn get_client_id(&self) -> &str {
        &self.client_id
    }

    /// Get the client secret.
    pub fn get_client_secret(&self) -> &str {
        &self.client_secret
    }

    /// Set the request the provider works with. By default it uses the
    /// request currently being handled (an empty request outside of HTTP).
    pub fn set_request(mut self, request: Request) -> Self {
        self.request = Some(request);
        self
    }

    /// Get the request the provider works with.
    pub fn get_request(&self) -> Request {
        self.request
            .clone()
            .or_else(current_request)
            .unwrap_or_default()
    }

    /// Set the (Guzzle-style) options of the HTTP requests sent to the
    /// provider: `timeout`, `connect_timeout`, `verify`, `http_errors`, ...
    /// (a provider's `guzzle` configuration).
    pub fn set_http_options(mut self, options: Value) -> Self {
        self.http_options = options;
        self
    }

    /// Get the options of the HTTP requests sent to the provider.
    pub fn get_http_options(&self) -> &Value {
        &self.http_options
    }

    /// The separator of the driver's scopes.
    pub fn scope_separator(&self) -> &str {
        self.driver.scope_separator()
    }

    /// How the driver's authorization URL is encoded.
    pub fn encoding_type(&self) -> QueryEncoding {
        self.driver.encoding_type()
    }

    /// Get the driver, if it is a `T`.
    ///
    /// ```
    /// use laravel_socialite::{GithubProvider, GitlabProvider, Provider};
    ///
    /// let github = Provider::new(GithubProvider, "id", "secret", "/callback");
    ///
    /// assert!(github.driver_ref::<GithubProvider>().is_some());
    /// assert!(github.driver_ref::<GitlabProvider>().is_none());
    /// ```
    pub fn driver_ref<T: OAuth2Provider>(&self) -> Option<&T> {
        let driver: &dyn Any = &*self.driver;
        driver.downcast_ref::<T>()
    }

    /// Get the driver mutably, if it is a `T`.
    pub fn driver_mut<T: OAuth2Provider>(&mut self) -> Option<&mut T> {
        let driver: &mut dyn Any = &mut *self.driver;
        driver.downcast_mut::<T>()
    }

    /// Configure the driver, when it is a `T` — how provider-specific
    /// options are set (other drivers, like Socialite's fakes, are left
    /// alone).
    ///
    /// ```
    /// use laravel_socialite::{GitlabProvider, Provider};
    ///
    /// let gitlab = Provider::new(GitlabProvider::new(), "id", "secret", "/callback")
    ///     .with_driver(|gitlab: &mut GitlabProvider| gitlab.host = "https://gitlab.example.com".into());
    ///
    /// assert_eq!(gitlab.driver_ref::<GitlabProvider>().unwrap().host, "https://gitlab.example.com");
    /// ```
    pub fn with_driver<T: OAuth2Provider>(mut self, callback: impl FnOnce(&mut T)) -> Self {
        if let Some(driver) = self.driver_mut::<T>() {
            callback(driver);
        }
        self
    }

    // ------------------------------------------------------------------
    // Authenticating
    // ------------------------------------------------------------------

    /// Redirect the user to the authentication page for the provider.
    ///
    /// A random `state` is stored in the session (unless the provider is
    /// [stateless](Provider::stateless)), as is the PKCE `code_verifier`
    /// when PKCE is enabled; without a session, an error is returned.
    pub fn redirect(&self) -> Result<Response> {
        self.driver.redirect(self)
    }

    /// Get the user for the authenticated (callback) request: the `state`
    /// is checked against the session ([`InvalidStateException`] when it
    /// doesn't match), the `code` is exchanged for an access token, and
    /// the user is fetched with it.
    ///
    /// The user is remembered, so calling `user` again returns it without
    /// another round trip.
    pub async fn user(&self) -> Result<User> {
        if let Some(user) = self.user.lock().unwrap().clone() {
            return Ok(user);
        }
        let user = self.driver.user(self).await?;
        *self.user.lock().unwrap() = Some(user.clone());
        Ok(user)
    }

    /// Get a user instance from a known access token.
    pub async fn user_from_token(&self, token: &str) -> Result<User> {
        self.driver.user_from_token(self, token).await
    }

    /// Refresh a user's access token with a refresh token.
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<Token> {
        let response = self.get_refresh_token_response(refresh_token).await?;
        Ok(Token::new(
            get_string(&response, "access_token").unwrap_or_default(),
            get_string(&response, "refresh_token").unwrap_or_else(|| refresh_token.to_string()),
            get(&response, "expires_in").and_then(ValueExt::to_i64_lossy),
            self.parse_approved_scopes(&response),
        ))
    }

    /// Get the access token response for the given code.
    pub async fn get_access_token_response(&self, code: &str) -> Result<Value> {
        self.driver.get_access_token_response(self, code).await
    }

    /// Get the refresh token response for the given refresh token.
    pub async fn get_refresh_token_response(&self, refresh_token: &str) -> Result<Value> {
        self.driver
            .get_refresh_token_response(self, refresh_token)
            .await
    }

    // ------------------------------------------------------------------
    // The driver's hooks
    // ------------------------------------------------------------------

    /// Get the authentication URL for the provider.
    pub fn get_auth_url(&self, state: Option<&str>) -> String {
        self.driver.get_auth_url(self, state)
    }

    /// Get the token URL for the provider.
    pub fn get_token_url(&self) -> String {
        self.driver.get_token_url(self)
    }

    /// Get the raw user for the given access token.
    pub async fn get_user_by_token(&self, token: &str) -> Result<Value> {
        self.driver.get_user_by_token(self, token).await
    }

    /// Map the raw user array to a Socialite [`User`].
    pub fn map_user_to_object(&self, user: Value) -> User {
        self.driver.map_user_to_object(self, user)
    }

    /// Get the query fields of the authorization URL.
    pub fn get_code_fields(&self, state: Option<&str>) -> Map<String, Value> {
        self.driver.get_code_fields(self, state)
    }

    /// Get the headers of the access token request.
    pub fn get_token_headers(&self, code: &str) -> Vec<(String, String)> {
        self.driver.get_token_headers(self, code)
    }

    /// Get the form fields of the access token request.
    pub fn get_token_fields(&self, code: &str) -> Map<String, Value> {
        self.driver.get_token_fields(self, code)
    }

    // ------------------------------------------------------------------
    // Building blocks
    // ------------------------------------------------------------------

    /// Build the authentication URL for the provider from the given base
    /// URL and the [code fields](Provider::get_code_fields).
    pub fn build_auth_url_from_base(&self, url: &str, state: Option<&str>) -> String {
        let query = self
            .encoding_type()
            .build_query(&self.get_code_fields(state));
        format!("{url}?{query}")
    }

    /// Format the given scopes.
    pub fn format_scopes(&self, scopes: &[String], separator: &str) -> String {
        scopes.join(separator)
    }

    /// Get the session of the provider's request.
    pub fn session(&self) -> Result<Arc<Store>> {
        self.get_request()
            .try_session()
            .ok_or_else(|| RuntimeException::new("Session store not set on request.").into())
    }

    /// Get a new HTTP client request for talking to the provider, through
    /// the `Http` facade (so `Http::fake()` works in tests) with the
    /// provider's [HTTP options](Provider::set_http_options).
    ///
    /// Like Guzzle, client and server errors are returned as errors unless
    /// the options set `http_errors` to `false`.
    pub fn get_http_client(&self) -> PendingRequest {
        let mut client = Http::new_request();
        if let Value::Object(options) = &self.http_options
            && !options.is_empty()
        {
            client = client.with_options(self.http_options.clone());
        }
        if matches!(
            self.http_options.get("http_errors"),
            Some(Value::Bool(false))
        ) {
            client
        } else {
            client.throw()
        }
    }

    /// Get the `code` from the request.
    pub fn get_code(&self) -> Option<String> {
        match self.get_request().input("code") {
            Value::Null => None,
            Value::String(code) => Some(code),
            other => Some(other.to_string_lossy()),
        }
    }

    /// Get a fresh, random `state` (40 characters).
    pub fn get_state(&self) -> String {
        Str::random(40)
    }

    /// Generate a random PKCE code verifier (96 characters).
    pub fn get_code_verifier(&self) -> String {
        Str::random(96)
    }

    /// Generate the PKCE code challenge for the `code_verifier` in the
    /// session: the URL-safe, unpadded base64 SHA-256 of the verifier.
    pub fn get_code_challenge(&self) -> String {
        let verifier = self
            .get_request()
            .try_session()
            .map(|session| session.get("code_verifier").to_string_lossy())
            .unwrap_or_default();
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
    }

    /// Get the hash method used to calculate the PKCE code challenge.
    pub fn get_code_challenge_method(&self) -> &'static str {
        "S256"
    }

    /// Determine if the current request / session has a mismatching
    /// `state` (always `false` when stateless). The session's `state` is
    /// pulled, so it can only be used once.
    pub fn has_invalid_state(&self) -> Result<bool> {
        if self.is_stateless() {
            return Ok(false);
        }
        let request = self.get_request();
        let session = request
            .try_session()
            .ok_or_else(|| RuntimeException::new("Session store not set on request."))?;
        let state = session.pull("state");
        if !state.truthy() {
            return Ok(true);
        }
        Ok(request.input("state").as_str() != state.as_str())
    }

    /// Create the user from the access token response and the raw user,
    /// setting the token, refresh token, expiration and approved scopes.
    pub fn user_instance(&self, response: &Value, user: Value) -> User {
        self.map_user_to_object(user)
            .set_access_token_response_body(response.clone())
            .set_token(get_string(response, "access_token").unwrap_or_default())
            .set_refresh_token(get_string(response, "refresh_token"))
            .set_expires_in(get(response, "expires_in").and_then(ValueExt::to_i64_lossy))
            .set_approved_scopes(self.parse_approved_scopes(response))
    }

    /// The scopes an access token response says were approved (its
    /// `scope`, split on the driver's separator).
    pub fn parse_approved_scopes(&self, response: &Value) -> Vec<String> {
        string_list(get(response, "scope"), self.scope_separator())
    }

    // ------------------------------------------------------------------
    // Laravel's `AbstractProvider` behavior (the drivers' `parent::`)
    // ------------------------------------------------------------------

    /// Store the `state` (and PKCE `code_verifier`) in the session and
    /// redirect to the [authentication URL](Provider::get_auth_url).
    pub fn default_redirect(&self) -> Result<Response> {
        let mut state = None;
        if self.uses_state() {
            let value = self.get_state();
            self.session()?.put("state", value.clone());
            state = Some(value);
        }
        if self.uses_pkce() {
            self.session()?
                .put("code_verifier", self.get_code_verifier());
        }
        Ok(Response::redirect(self.get_auth_url(state.as_deref())))
    }

    /// The standard authorization URL fields: `client_id`,
    /// `redirect_uri`, `scope`, `response_type`, `state`, the PKCE
    /// challenge, and the [custom parameters](Provider::with).
    pub fn default_code_fields(&self, state: Option<&str>) -> Map<String, Value> {
        let mut fields = Map::new();
        fields.insert("client_id".into(), self.client_id.clone().into());
        fields.insert("redirect_uri".into(), self.redirect_url.clone().into());
        fields.insert(
            "scope".into(),
            self.format_scopes(&self.scopes, self.scope_separator())
                .into(),
        );
        fields.insert("response_type".into(), "code".into());
        if self.uses_state() {
            fields.insert(
                "state".into(),
                state.map_or(Value::Null, |state| state.into()),
            );
        }
        if self.uses_pkce() {
            fields.insert("code_challenge".into(), self.get_code_challenge().into());
            fields.insert(
                "code_challenge_method".into(),
                self.get_code_challenge_method().into(),
            );
        }
        for (key, value) in &self.parameters {
            fields.insert(key.clone(), value.clone());
        }
        fields
    }

    /// The standard token request headers: `Accept: application/json`.
    pub fn default_token_headers(&self, _code: &str) -> Vec<(String, String)> {
        vec![("Accept".into(), "application/json".into())]
    }

    /// The standard token request fields: `grant_type`, `client_id`,
    /// `client_secret`, `code`, `redirect_uri`, the PKCE `code_verifier`
    /// (pulled from the session), and the [custom parameters](Provider::with).
    pub fn default_token_fields(&self, code: &str) -> Map<String, Value> {
        let mut fields = Map::new();
        fields.insert("grant_type".into(), "authorization_code".into());
        fields.insert("client_id".into(), self.client_id.clone().into());
        fields.insert("client_secret".into(), self.client_secret.clone().into());
        fields.insert("code".into(), code.into());
        fields.insert("redirect_uri".into(), self.redirect_url.clone().into());
        if self.uses_pkce() {
            let verifier = self
                .get_request()
                .try_session()
                .map_or(Value::Null, |session| session.pull("code_verifier"));
            fields.insert("code_verifier".into(), verifier);
        }
        for (key, value) in &self.parameters {
            fields.insert(key.clone(), value.clone());
        }
        fields
    }

    /// `POST` the [token fields](Provider::get_token_fields) as a form to
    /// the [token URL](Provider::get_token_url) and decode the JSON response.
    pub async fn default_access_token_response(&self, code: &str) -> Result<Value> {
        let response = self
            .get_http_client()
            .with_headers(self.get_token_headers(code))
            .as_form()
            .post(
                self.get_token_url(),
                Value::Object(self.get_token_fields(code)),
            )
            .await?;
        Ok(response.json())
    }

    /// `POST` a `refresh_token` grant to the [token URL](Provider::get_token_url)
    /// and decode the JSON response.
    pub async fn default_refresh_token_response(&self, refresh_token: &str) -> Result<Value> {
        let mut fields = Map::new();
        fields.insert("grant_type".into(), "refresh_token".into());
        fields.insert("refresh_token".into(), refresh_token.into());
        fields.insert("client_id".into(), self.client_id.clone().into());
        fields.insert("client_secret".into(), self.client_secret.clone().into());
        let response = self
            .get_http_client()
            .with_header("Accept", "application/json")
            .as_form()
            .post(self.get_token_url(), Value::Object(fields))
            .await?;
        Ok(response.json())
    }

    /// Check the `state`, exchange the `code` for an access token, fetch the
    /// user with it, and build the [`User`].
    pub async fn default_user(&self) -> Result<User> {
        if self.has_invalid_state()? {
            return Err(InvalidStateException.into());
        }
        let code = self.get_code().unwrap_or_default();
        let response = self.get_access_token_response(&code).await?;
        let token = get_string(&response, "access_token").unwrap_or_default();
        let user = self.get_user_by_token(&token).await?;
        Ok(self.user_instance(&response, user))
    }

    /// Fetch the user with the given access token and build the [`User`].
    pub async fn default_user_from_token(&self, token: &str) -> Result<User> {
        let user = self.get_user_by_token(token).await?;
        Ok(self.map_user_to_object(user).set_token(token))
    }
}

impl fmt::Debug for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provider")
            .field("client_id", &self.client_id)
            .field("redirect_url", &self.redirect_url)
            .field("scopes", &self.scopes)
            .field("parameters", &self.parameters)
            .field("stateless", &self.stateless)
            .field("uses_pkce", &self.uses_pkce)
            .finish_non_exhaustive()
    }
}

/// Custom parameters from a JSON object or a list of `[key, value]` pairs.
fn parameters_from(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(parameters) => parameters,
        Value::Array(pairs) => pairs
            .into_iter()
            .filter_map(|pair| match pair {
                Value::Array(mut pair) if pair.len() == 2 => {
                    let value = pair.pop()?;
                    let key = pair.pop()?;
                    Some((key.to_string_lossy(), value))
                }
                _ => None,
            })
            .collect(),
        _ => Map::new(),
    }
}

/// The driver of a provider that couldn't be created: every request
/// returns the error (the first time the original error, then its message).
struct Unresolved {
    error: Mutex<Option<Error>>,
    message: String,
}

impl Unresolved {
    fn new(error: Error) -> Self {
        Self {
            message: error.to_string(),
            error: Mutex::new(Some(error)),
        }
    }

    fn error(&self) -> Error {
        self.error
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| RuntimeException::new(self.message.clone()).into())
    }
}

#[async_trait]
impl OAuth2Provider for Unresolved {
    fn get_auth_url(&self, _provider: &Provider, _state: Option<&str>) -> String {
        String::new()
    }

    fn get_token_url(&self, _provider: &Provider) -> String {
        String::new()
    }

    async fn get_user_by_token(&self, _provider: &Provider, _token: &str) -> Result<Value> {
        Err(self.error())
    }

    fn map_user_to_object(&self, _provider: &Provider, user: Value) -> User {
        User::new().set_raw(user)
    }

    async fn get_access_token_response(&self, _provider: &Provider, _code: &str) -> Result<Value> {
        Err(self.error())
    }

    async fn get_refresh_token_response(
        &self,
        _provider: &Provider,
        _refresh_token: &str,
    ) -> Result<Value> {
        Err(self.error())
    }

    fn redirect(&self, _provider: &Provider) -> Result<Response> {
        Err(self.error())
    }

    async fn user(&self, _provider: &Provider) -> Result<User> {
        Err(self.error())
    }

    async fn user_from_token(&self, _provider: &Provider, _token: &str) -> Result<User> {
        Err(self.error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::two::GithubProvider;
    use illuminate_support::json;

    #[test]
    fn parameters_may_be_objects_or_pairs() {
        assert_eq!(
            parameters_from(json!({"hd": "example.com"}))["hd"],
            "example.com"
        );
        let pairs = parameters_from(json!([["prompt", "consent"], ["max_age", 0], ["bad"]]));
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs["max_age"], 0);
        assert!(parameters_from(json!("nope")).is_empty());
    }

    #[test]
    fn failed_providers_keep_returning_their_error() {
        let provider = Provider::failed(
            illuminate_support::error::InvalidArgumentException::new("Driver [x] not supported.")
                .into(),
        );
        let first = provider.redirect().unwrap_err();
        assert!(first.is::<illuminate_support::error::InvalidArgumentException>());
        let second = provider.redirect().unwrap_err();
        assert_eq!(second.to_string(), "Driver [x] not supported.");
    }

    #[test]
    fn code_fields_follow_laravels_order() {
        let provider = Provider::new(GithubProvider, "id", "secret", "https://laravel.test/cb")
            .with(json!({"scope": "overridden", "login": "taylor"}));
        let fields = provider.get_code_fields(Some("state"));
        let keys: Vec<&str> = fields.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "client_id",
                "redirect_uri",
                "scope",
                "response_type",
                "state",
                "login"
            ]
        );
        assert_eq!(fields["scope"], "overridden");
    }

    #[test]
    fn token_fields_include_the_parameters() {
        let provider = Provider::new(GithubProvider, "id", "secret", "https://laravel.test/cb")
            .with(json!({"audience": "api"}));
        let fields = provider.get_token_fields("code");
        assert_eq!(
            Value::Object(fields),
            json!({
                "grant_type": "authorization_code",
                "client_id": "id",
                "client_secret": "secret",
                "code": "code",
                "redirect_uri": "https://laravel.test/cb",
                "audience": "api",
            })
        );
        assert_eq!(
            provider.get_token_headers("code"),
            [("Accept".to_string(), "application/json".to_string())]
        );
    }

    #[test]
    fn approved_scopes_are_split_on_the_separator() {
        let provider = Provider::new(GithubProvider, "id", "secret", "/cb");
        assert_eq!(
            provider.parse_approved_scopes(&json!({"scope": "repo,gist"})),
            ["repo", "gist"]
        );
        assert!(
            provider
                .parse_approved_scopes(&json!({"scope": ""}))
                .is_empty()
        );
        assert!(provider.parse_approved_scopes(&json!({})).is_empty());
    }

    #[test]
    fn providers_debug_without_secrets() {
        let provider = Provider::new(GithubProvider, "id", "secret", "/cb");
        let debug = format!("{provider:?}");
        assert!(debug.contains("client_id: \"id\""));
        assert!(!debug.contains("secret"));
    }
}
