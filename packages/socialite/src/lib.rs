//! # Laravel Socialite
//!
//! In addition to typical, form based authentication, Laravel also provides
//! a simple, convenient way to authenticate with OAuth providers using
//! Socialite. Socialite supports authentication via Facebook, X, LinkedIn,
//! Google, GitHub, GitLab, Bitbucket, and Slack — and community providers
//! are a small [`OAuth2Provider`] implementation away.
//!
//! ## Configuration
//!
//! Place each provider's credentials in your `config/services` file, under
//! the `facebook`, `x`, `linkedin-openid`, `google`, `github`, `gitlab`,
//! `bitbucket`, `slack` or `slack-openid` key:
//!
//! ```ignore
//! "github": {
//!     "client_id": env("GITHUB_CLIENT_ID", Value::Null),
//!     "client_secret": env("GITHUB_CLIENT_SECRET", Value::Null),
//!     "redirect": "http://example.com/callback-url",
//! },
//! ```
//!
//! A `redirect` starting with `/` is resolved to a fully qualified URL. The
//! `gitlab` driver also reads a `host` (for self-hosted instances), and every
//! driver accepts `guzzle` HTTP options (`timeout`, `connect_timeout`, ...).
//!
//! ## Routing
//!
//! You need two routes: one redirecting the user to the OAuth provider, and
//! one receiving the callback after they approve the request:
//!
//! ```ignore
//! use laravel_socialite::Socialite;
//!
//! Route::get("/auth/redirect", || async {
//!     Socialite::driver("github").redirect()
//! });
//!
//! Route::get("/auth/callback", || async {
//!     let github_user = Socialite::driver("github").user().await?;
//!
//!     let user = User::update_or_create(
//!         json!({"github_id": github_user.id}),
//!         json!({
//!             "name": github_user.name,
//!             "email": github_user.email,
//!             "github_token": github_user.token,
//!             "github_refresh_token": github_user.refresh_token,
//!         }),
//!     )
//!     .await?;
//!
//!     Auth::login(&user, false).await?;
//!
//!     Ok::<_, Error>(redirect("/dashboard"))
//! });
//! ```
//!
//! [`Provider::redirect`] stores a random `state` in the session (the
//! routes need the `web` middleware) and redirects to the provider;
//! [`Provider::user`] checks the `state` the provider sends back
//! ([`InvalidStateException`] when it doesn't match), exchanges the `code`
//! for an access token, and retrieves the [`User`]. Both work with the
//! request currently being handled.
//!
//! Here's the whole round trip, with the provider faked by `Http::fake`:
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_config::Repository;
//! use illuminate_container::Container;
//! use illuminate_http::{with_request, Request};
//! use illuminate_http_client::Http;
//! use illuminate_session::{ArraySessionHandler, RequestSessionExt, Store};
//! use illuminate_support::json;
//! use laravel_socialite::Socialite;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! # let container = Arc::new(Container::new());
//! # let _guard = Container::set_local_instance(container.clone());
//! container.instance(Repository::new(json!({"services": {"github": {
//!     "client_id": "client-id",
//!     "client_secret": "client-secret",
//!     "redirect": "https://laravel.test/auth/callback",
//! }}})));
//!
//! Http::fake_urls([
//!     ("https://github.com/login/oauth/access_token", Http::response(json!({
//!         "access_token": "gho_token", "scope": "user:email", "token_type": "bearer",
//!     }), 200, &[])),
//!     ("https://api.github.com/user", Http::response(json!({
//!         "id": 1, "node_id": "MDQ6", "login": "taylorotwell", "name": "Taylor Otwell",
//!         "avatar_url": "https://avatars.githubusercontent.com/u/463230",
//!     }), 200, &[])),
//!     ("https://api.github.com/user/emails", Http::response(json!([
//!         {"email": "taylor@laravel.com", "primary": true, "verified": true},
//!     ]), 200, &[])),
//! ]);
//!
//! let session = Arc::new(Store::new("laravel_session", Arc::new(ArraySessionHandler::new(120)), None));
//!
//! // GET /auth/redirect
//! let request = Request::create("/auth/redirect", "GET");
//! request.set_session(session.clone());
//! let response = with_request(request, async { Socialite::driver("github").redirect() }).await?;
//!
//! let state = session.get("state").as_str().unwrap().to_string();
//! assert!(response.target_url().unwrap().ends_with(&format!("&state={state}")));
//!
//! // GET /auth/callback?code=...&state=...
//! let request = Request::create(&format!("/auth/callback?code=abc&state={state}"), "GET");
//! request.set_session(session.clone());
//! let user = with_request(request, async { Socialite::driver("github").user().await }).await?;
//!
//! assert_eq!(user.get_id(), "1");
//! assert_eq!(user.get_nickname(), Some("taylorotwell"));
//! assert_eq!(user.get_email(), Some("taylor@laravel.com"));
//! assert_eq!(user.token, "gho_token");
//! assert_eq!(user.approved_scopes, ["user:email"]);
//! # Ok::<(), illuminate_support::Error>(())
//! # }).unwrap();
//! ```
//!
//! ## Scopes and optional parameters
//!
//! ```ignore
//! // Merge scopes with the driver's defaults...
//! Socialite::driver("github").scopes(["read:user", "public_repo"]).redirect()
//!
//! // Or replace them...
//! Socialite::driver("github").set_scopes(["read:user", "public_repo"]).redirect()
//!
//! // Optional parameters for the redirect request...
//! Socialite::driver("google").with(json!({"hd": "example.com"})).redirect()
//!
//! // Slack bot tokens: call `as_bot_user` before redirecting *and* in the callback...
//! Socialite::driver("slack").as_bot_user().set_scopes(["chat:write", "chat:write.public"]).redirect()
//! let token = Socialite::driver("slack").as_bot_user().user().await?.token;
//! ```
//!
//! [`Provider::stateless`] disables the session state verification (for
//! stateless APIs), [`Provider::enable_pkce`] turns on PKCE,
//! [`Provider::redirect_url`] changes the callback URL,
//! [`Provider::user_from_token`] retrieves a user with an access token you
//! already have, and [`Provider::refresh_token`] exchanges a refresh token
//! for a new [`Token`].
//!
//! ## Custom providers
//!
//! Implement [`OAuth2Provider`] and register it with [`Socialite::extend`],
//! building the provider from its configuration with
//! [`Socialite::build_provider`].
//!
//! ## Testing
//!
//! [`Socialite::fake`] makes a driver redirect to a fake authorization URL,
//! and [`Socialite::fake_with`] makes it return the given user:
//!
//! ```ignore
//! Socialite::fake_with("github", User::fake(json!({
//!     "id": "github-123",
//!     "name": "Jason Beggs",
//!     "email": "jason@example.com",
//! })));
//!
//! test.get("/auth/github/callback").await.assert_redirect("/dashboard");
//! ```
//!
//! Every HTTP request is sent through the `Http` facade, so `Http::fake()`
//! works too.

pub mod testing;
pub mod two;

mod exceptions;
mod facade;
mod jwt;
mod manager;
mod provider;
mod support;

pub use exceptions::{DriverMissingConfigurationException, InvalidStateException};
pub use facade::Socialite;
pub use manager::{DRIVERS, ProviderCreator, SocialiteManager};
pub use provider::SocialiteServiceProvider;
pub use support::QueryEncoding;
pub use testing::{FakeProvider, FakeRedirect};
pub use two::{
    BitbucketProvider, FacebookProvider, GithubProvider, GitlabProvider, GoogleProvider,
    LinkedInOpenIdProvider, OAuth2Provider, Provider, SlackOpenIdProvider, SlackProvider, Token,
    TwitterProvider, User, XProvider,
};

/// Re-exported so custom providers can implement [`OAuth2Provider`]
/// without their own dependency.
pub use async_trait::async_trait;

/// Everything you need to authenticate with OAuth providers, in one import.
pub mod prelude {
    pub use crate::facade::Socialite;
    pub use crate::two::{OAuth2Provider, Provider, Token, User};
}

illuminate_container::discover_provider!("laravel-socialite", SocialiteServiceProvider);
