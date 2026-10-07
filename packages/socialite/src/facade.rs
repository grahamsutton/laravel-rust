//! The `Socialite` facade.

use std::sync::Arc;

use illuminate_container::{Container, try_app};
use illuminate_support::{Result, Value};

use crate::manager::SocialiteManager;
use crate::testing::FakeProvider;
use crate::two::{OAuth2Provider, Provider, User};

/// The `Socialite` facade: authenticate users with OAuth providers.
///
/// ```ignore
/// use laravel_socialite::Socialite;
///
/// Route::get("/auth/redirect", || async {
///     Socialite::driver("github").redirect()
/// });
///
/// Route::get("/auth/callback", || async {
///     let user = Socialite::driver("github").user().await?;
///
///     // user.token
///     Ok::<_, Error>(redirect("/dashboard"))
/// });
/// ```
///
/// The facade resolves the [`SocialiteManager`] from the container at call
/// time (registering one if the application hasn't), so every test
/// container gets its own custom drivers and fakes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Socialite;

impl Socialite {
    /// Get the Socialite manager from the container, registering one if the
    /// application hasn't yet.
    pub fn manager() -> Arc<SocialiteManager> {
        if let Some(manager) = try_app::<SocialiteManager>() {
            return manager;
        }
        let container = Container::get_instance();
        container.singleton_if::<SocialiteManager>(|_| Arc::new(SocialiteManager::new()));
        container.make::<SocialiteManager>()
    }

    /// Get a provider for the driver: `facebook`, `x`, `linkedin-openid`,
    /// `google`, `github`, `gitlab`, `bitbucket`, `slack`, `slack-openid`,
    /// or a custom driver.
    ///
    /// When the provider can't be created — an unknown driver, or missing
    /// `services.*` configuration — the error is returned by the provider's
    /// requests ([`Provider::redirect`], [`Provider::user`], ...). Use
    /// [`Socialite::try_driver`] to get it right away.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_config::Repository;
    /// use illuminate_container::Container;
    /// use illuminate_support::json;
    /// use laravel_socialite::Socialite;
    ///
    /// let container = Arc::new(Container::new());
    /// let _guard = Container::set_local_instance(container.clone());
    /// container.instance(Repository::new(json!({"services": {"github": {
    ///     "client_id": "client-id",
    ///     "client_secret": "client-secret",
    ///     "redirect": "https://laravel.test/auth/callback",
    /// }}})));
    ///
    /// let response = Socialite::driver("github").stateless().redirect()?;
    /// assert!(response.target_url().unwrap().starts_with("https://github.com/login/oauth/authorize?client_id=client-id&"));
    ///
    /// let error = Socialite::driver("myspace").redirect().unwrap_err();
    /// assert_eq!(error.to_string(), "Driver [myspace] not supported.");
    /// # Ok::<(), illuminate_support::Error>(())
    /// ```
    pub fn driver(driver: &str) -> Provider {
        Self::try_driver(driver).unwrap_or_else(Provider::failed)
    }

    /// Get a provider for the driver, or the error creating it.
    pub fn try_driver(driver: &str) -> Result<Provider> {
        Self::manager().driver(driver)
    }

    /// Get a provider for the driver (an alias of [`Socialite::driver`]).
    pub fn with(driver: &str) -> Provider {
        Self::driver(driver)
    }

    /// Build an OAuth 2 provider from a driver and its configuration
    /// (`client_id`, `client_secret`, `redirect`, and optional `guzzle`
    /// HTTP options) — for custom drivers.
    pub fn build_provider<D: OAuth2Provider>(driver: D, config: &Value) -> Result<Provider> {
        Self::manager().build_provider(driver, config)
    }

    /// Register a custom driver.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_config::{config, Repository};
    /// use illuminate_container::Container;
    /// use illuminate_support::json;
    /// use laravel_socialite::{GitlabProvider, Socialite};
    ///
    /// let container = Arc::new(Container::new());
    /// let _guard = Container::set_local_instance(container.clone());
    /// container.instance(Repository::new(json!({"services": {"gitlab-ee": {
    ///     "client_id": "client-id",
    ///     "client_secret": "client-secret",
    ///     "redirect": "https://laravel.test/auth/callback",
    /// }}})));
    ///
    /// Socialite::extend("gitlab-ee", |_app| {
    ///     Ok(Socialite::build_provider(GitlabProvider::new(), &config("services.gitlab-ee"))?
    ///         .set_host("https://gitlab.example.com"))
    /// });
    ///
    /// let provider = Socialite::driver("gitlab-ee");
    /// assert_eq!(provider.get_token_url(), "https://gitlab.example.com/oauth/token");
    /// ```
    pub fn extend<F>(driver: impl Into<String>, creator: F)
    where
        F: Fn(&Container) -> Result<Provider> + Send + Sync + 'static,
    {
        Self::manager().extend(driver, creator);
    }

    /// Fake the driver for testing: [`Provider::redirect`] redirects to
    /// `https://socialite.fake/{driver}/authorize`, and [`Provider::user`]
    /// returns a [`User::fake`] user.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_container::Container;
    /// use laravel_socialite::Socialite;
    ///
    /// let container = Arc::new(Container::new());
    /// let _guard = Container::set_local_instance(container.clone());
    ///
    /// Socialite::fake("github");
    ///
    /// let response = Socialite::driver("github").redirect()?;
    /// assert!(response.is_redirect());
    /// assert_eq!(response.target_url().unwrap(), "https://socialite.fake/github/authorize");
    /// # Ok::<(), illuminate_support::Error>(())
    /// ```
    pub fn fake(driver: impl Into<String>) -> Arc<FakeProvider> {
        Self::manager().fake(driver, None)
    }

    /// Fake the driver for testing, returning the given user from
    /// [`Provider::user`] (usually built with [`User::fake`]).
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
    /// Socialite::fake_with("github", User::fake(json!({
    ///     "id": "github-123",
    ///     "name": "Jason Beggs",
    ///     "email": "jason@example.com",
    /// })));
    ///
    /// let user = Socialite::driver("github").user().await?;
    ///
    /// assert_eq!(user.id, "github-123");
    /// assert_eq!(user.get_email(), Some("jason@example.com"));
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn fake_with(driver: impl Into<String>, user: User) -> Arc<FakeProvider> {
        Self::manager().fake(driver, Some(user))
    }
}
