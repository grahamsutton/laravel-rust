//! The Socialite manager: builds providers from `config/services`.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use illuminate_config::Repository;
use illuminate_container::{Container, try_app};
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Result, Value, ValueExt};

use crate::exceptions::DriverMissingConfigurationException;
use crate::support::short_type_name;
use crate::testing::{FakeDriver, FakeProvider};
use crate::two::{
    BitbucketProvider, FacebookProvider, GithubProvider, GitlabProvider, GoogleProvider,
    LinkedInOpenIdProvider, OAuth2Provider, Provider, SlackOpenIdProvider, SlackProvider,
    TwitterProvider, User, XProvider,
};

/// Creates a custom driver's provider (see [`SocialiteManager::extend`]).
pub type ProviderCreator = Arc<dyn Fn(&Container) -> Result<Provider> + Send + Sync>;

/// The drivers Socialite ships with, and the `services.*` key each one is
/// configured under.
pub const DRIVERS: [&str; 11] = [
    "facebook",
    "x",
    "twitter",
    "twitter-oauth-2",
    "linkedin-openid",
    "google",
    "github",
    "gitlab",
    "bitbucket",
    "slack",
    "slack-openid",
];

/// The Socialite manager (Laravel's `SocialiteManager`): creates a provider
/// for each driver from its `services.{driver}` configuration, and holds
/// custom drivers and testing fakes.
///
/// It's registered in the container by the
/// [`SocialiteServiceProvider`](crate::SocialiteServiceProvider) and used
/// through the [`Socialite`](crate::Socialite) facade.
///
/// Unlike Laravel's manager, providers aren't cached: every
/// [`driver`](SocialiteManager::driver) call builds a new one, so options set
/// for one request never leak into the next.
#[derive(Default)]
pub struct SocialiteManager {
    custom_creators: RwLock<HashMap<String, ProviderCreator>>,
    fakes: RwLock<HashMap<String, Arc<FakeProvider>>>,
}

impl SocialiteManager {
    /// Create a new manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a driver's provider.
    ///
    /// Custom drivers ([`extend`](SocialiteManager::extend)) win over the
    /// built-in ones, and fakes ([`fake`](SocialiteManager::fake)) win over
    /// both.
    pub fn driver(&self, driver: &str) -> Result<Provider> {
        if driver.is_empty() {
            return Err(InvalidArgumentException::new("No Socialite driver was specified.").into());
        }
        if let Some(fake) = self.fakes.read().unwrap().get(driver).cloned() {
            return Ok(Provider::new(FakeDriver(fake), "", "", ""));
        }
        let creator = self.custom_creators.read().unwrap().get(driver).cloned();
        if let Some(creator) = creator {
            return creator(&Container::get_instance());
        }
        self.create_driver(driver)
    }

    /// Get a driver's provider (an alias of [`driver`](SocialiteManager::driver)).
    pub fn with(&self, driver: &str) -> Result<Provider> {
        self.driver(driver)
    }

    /// Socialite has no default driver: always an error.
    pub fn get_default_driver(&self) -> Result<String> {
        Err(InvalidArgumentException::new("No Socialite driver was specified.").into())
    }

    /// Register a custom driver creator. The closure receives the container
    /// and usually returns [`SocialiteManager::build_provider`]'s provider.
    pub fn extend<F>(&self, driver: impl Into<String>, creator: F) -> &Self
    where
        F: Fn(&Container) -> Result<Provider> + Send + Sync + 'static,
    {
        self.custom_creators
            .write()
            .unwrap()
            .insert(driver.into(), Arc::new(creator));
        self
    }

    /// Build an OAuth 2 provider from its configuration: the required
    /// `client_id`, `client_secret` and `redirect` keys, and the optional
    /// `guzzle` HTTP options.
    ///
    /// Relative redirect URLs (`/auth/callback`) are made absolute with the
    /// URL generator.
    pub fn build_provider<D: OAuth2Provider>(&self, driver: D, config: &Value) -> Result<Provider> {
        let missing: Vec<&str> = ["client_id", "client_secret", "redirect"]
            .into_iter()
            .filter(|key| config.get(key).is_none())
            .collect();
        if !missing.is_empty() {
            return Err(DriverMissingConfigurationException::make(
                short_type_name::<D>(),
                &missing,
            )
            .into());
        }

        let provider = Provider::new(
            driver,
            config["client_id"].to_string_lossy(),
            config["client_secret"].to_string_lossy(),
            Self::format_redirect_url(config),
        );
        Ok(match config.get("guzzle") {
            Some(options @ Value::Object(_)) => provider.set_http_options(options.clone()),
            _ => provider,
        })
    }

    /// Format the redirect URL of a provider's configuration: paths starting
    /// with `/` become absolute URLs to the application.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_config::Repository;
    /// use illuminate_container::Container;
    /// use illuminate_support::json;
    /// use laravel_socialite::SocialiteManager;
    ///
    /// let container = Arc::new(Container::new());
    /// let _guard = Container::set_local_instance(container.clone());
    /// container.instance(Repository::new(json!({"app": {"url": "https://laravel.test"}})));
    ///
    /// assert_eq!(
    ///     SocialiteManager::format_redirect_url(&json!({"redirect": "/auth/github/callback"})),
    ///     "https://laravel.test/auth/github/callback",
    /// );
    /// assert_eq!(
    ///     SocialiteManager::format_redirect_url(&json!({"redirect": "https://example.com/callback"})),
    ///     "https://example.com/callback",
    /// );
    /// ```
    pub fn format_redirect_url(config: &Value) -> String {
        let redirect = config["redirect"].to_string_lossy();
        if redirect.starts_with('/') {
            illuminate_routing::url(&redirect)
        } else {
            redirect
        }
    }

    /// Fake the driver: it redirects to `https://socialite.fake/{driver}/authorize`
    /// and returns the given user (a [`User::fake`] when `None`).
    pub fn fake(&self, driver: impl Into<String>, user: Option<User>) -> Arc<FakeProvider> {
        let driver = driver.into();
        let user = user.unwrap_or_else(|| User::fake(Value::Null));
        let fake = Arc::new(FakeProvider::new(driver.clone(), user));
        self.fakes.write().unwrap().insert(driver, fake.clone());
        fake
    }

    /// Stop faking the driver.
    pub fn forget_fake(&self, driver: &str) {
        self.fakes.write().unwrap().remove(driver);
    }

    /// Get the fake for the driver, if it's faked.
    pub fn get_fake(&self, driver: &str) -> Option<Arc<FakeProvider>> {
        self.fakes.read().unwrap().get(driver).cloned()
    }

    // ------------------------------------------------------------------
    // The built-in drivers
    // ------------------------------------------------------------------

    fn create_driver(&self, driver: &str) -> Result<Provider> {
        match driver.to_ascii_lowercase().as_str() {
            "github" => self.create_github_driver(),
            "google" => self.create_google_driver(),
            "facebook" => self.create_facebook_driver(),
            "x" => self.create_x_driver(),
            "twitter" => self.create_twitter_driver(),
            "twitter-oauth-2" => self.create_twitter_oauth2_driver(),
            "linkedin-openid" => self.create_linkedin_openid_driver(),
            "gitlab" => self.create_gitlab_driver(),
            "bitbucket" => self.create_bitbucket_driver(),
            "slack" => self.create_slack_driver(),
            "slack-openid" => self.create_slack_openid_driver(),
            _ => Err(
                InvalidArgumentException::new(format!("Driver [{driver}] not supported.")).into(),
            ),
        }
    }

    fn config(key: &str) -> Value {
        try_app::<Repository>()
            .map(|config| config.get(key))
            .unwrap_or(Value::Null)
    }

    /// Create an instance of the GitHub driver (`services.github`).
    pub fn create_github_driver(&self) -> Result<Provider> {
        self.build_provider(GithubProvider, &Self::config("services.github"))
    }

    /// Create an instance of the Google driver (`services.google`).
    pub fn create_google_driver(&self) -> Result<Provider> {
        self.build_provider(GoogleProvider, &Self::config("services.google"))
    }

    /// Create an instance of the Facebook driver (`services.facebook`).
    pub fn create_facebook_driver(&self) -> Result<Provider> {
        self.build_provider(FacebookProvider::new(), &Self::config("services.facebook"))
    }

    /// Create an instance of the X driver (`services.x`).
    pub fn create_x_driver(&self) -> Result<Provider> {
        self.build_provider(XProvider::new(), &Self::config("services.x"))
    }

    /// Create an instance of the Twitter driver (`services.twitter`), which
    /// must be configured for OAuth 2 (`'oauth' => 2`).
    pub fn create_twitter_driver(&self) -> Result<Provider> {
        let config = Self::config("services.twitter");
        if config.get("oauth").and_then(ValueExt::to_i64_lossy) == Some(2) {
            return self.build_provider(TwitterProvider::new(), &config);
        }
        Err(InvalidArgumentException::new(
            "The [twitter] driver uses OAuth 1, which is not supported. Use the [x] driver, or set [services.twitter.oauth] to 2.",
        )
        .into())
    }

    /// Create an instance of the Twitter OAuth 2 driver (`services.twitter-oauth-2`).
    pub fn create_twitter_oauth2_driver(&self) -> Result<Provider> {
        self.build_provider(
            TwitterProvider::new(),
            &Self::config("services.twitter-oauth-2"),
        )
    }

    /// Create an instance of the LinkedIn OpenID driver (`services.linkedin-openid`).
    pub fn create_linkedin_openid_driver(&self) -> Result<Provider> {
        self.build_provider(
            LinkedInOpenIdProvider,
            &Self::config("services.linkedin-openid"),
        )
    }

    /// Create an instance of the GitLab driver (`services.gitlab`, with an
    /// optional `host`).
    pub fn create_gitlab_driver(&self) -> Result<Provider> {
        let config = Self::config("services.gitlab");
        let host = config["host"].to_string_lossy();
        Ok(self
            .build_provider(GitlabProvider::new(), &config)?
            .set_host(host))
    }

    /// Create an instance of the Bitbucket driver (`services.bitbucket`).
    pub fn create_bitbucket_driver(&self) -> Result<Provider> {
        self.build_provider(BitbucketProvider, &Self::config("services.bitbucket"))
    }

    /// Create an instance of the Slack driver (`services.slack`).
    pub fn create_slack_driver(&self) -> Result<Provider> {
        self.build_provider(SlackProvider::new(), &Self::config("services.slack"))
    }

    /// Create an instance of the Slack OpenID driver (`services.slack-openid`).
    pub fn create_slack_openid_driver(&self) -> Result<Provider> {
        self.build_provider(SlackOpenIdProvider, &Self::config("services.slack-openid"))
    }
}

impl fmt::Debug for SocialiteManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut custom: Vec<String> = self
            .custom_creators
            .read()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        custom.sort();
        let mut fakes: Vec<String> = self.fakes.read().unwrap().keys().cloned().collect();
        fakes.sort();
        f.debug_struct("SocialiteManager")
            .field("custom_creators", &custom)
            .field("fakes", &fakes)
            .finish()
    }
}
