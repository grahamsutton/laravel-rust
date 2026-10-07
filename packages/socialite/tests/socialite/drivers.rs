//! Creating providers: the built-in drivers, configuration, custom drivers,
//! and package discovery.

use std::sync::Arc;

use illuminate_config::{Repository, config};
use illuminate_container::{Container, discovered_providers};
use illuminate_http_client::Http;
use illuminate_support::json;
use laravel_socialite::{
    BitbucketProvider, DRIVERS, DriverMissingConfigurationException, FacebookProvider,
    GithubProvider, GitlabProvider, GoogleProvider, LinkedInOpenIdProvider, Provider,
    SlackOpenIdProvider, SlackProvider, Socialite, SocialiteManager, TwitterProvider, XProvider,
};

use crate::support::*;

#[test]
fn every_driver_is_built_from_its_services_configuration() {
    let _app = app();

    for driver in DRIVERS {
        let provider = Socialite::try_driver(driver).unwrap();
        assert_eq!(provider.get_client_id(), format!("{driver}-client-id"));
        assert_eq!(provider.get_client_secret(), format!("{driver}-secret"));
        assert_eq!(
            provider.get_redirect_url(),
            format!("https://laravel.test/auth/{driver}/callback")
        );
    }

    let is = |driver: &str, check: fn(&Provider) -> bool| {
        assert!(check(&Socialite::driver(driver)), "{driver}");
    };
    is("github", |p| p.driver_ref::<GithubProvider>().is_some());
    is("google", |p| p.driver_ref::<GoogleProvider>().is_some());
    is("facebook", |p| p.driver_ref::<FacebookProvider>().is_some());
    is("x", |p| p.driver_ref::<XProvider>().is_some());
    is("twitter", |p| p.driver_ref::<TwitterProvider>().is_some());
    is("twitter-oauth-2", |p| {
        p.driver_ref::<TwitterProvider>().is_some()
    });
    is("linkedin-openid", |p| {
        p.driver_ref::<LinkedInOpenIdProvider>().is_some()
    });
    is("gitlab", |p| p.driver_ref::<GitlabProvider>().is_some());
    is("bitbucket", |p| {
        p.driver_ref::<BitbucketProvider>().is_some()
    });
    is("slack", |p| p.driver_ref::<SlackProvider>().is_some());
    is("slack-openid", |p| {
        p.driver_ref::<SlackOpenIdProvider>().is_some()
    });

    // Built-in driver names are case-insensitive, like PHP's methods.
    assert!(
        Socialite::driver("GitHub")
            .driver_ref::<GithubProvider>()
            .is_some()
    );
    // `with` is an alias of `driver`.
    assert!(
        Socialite::with("github")
            .driver_ref::<GithubProvider>()
            .is_some()
    );
}

#[test]
fn each_driver_has_its_default_scopes() {
    let _app = app();
    let scopes = |driver: &str| Socialite::driver(driver).get_scopes().to_vec();

    assert_eq!(scopes("github"), ["user:email"]);
    assert_eq!(scopes("google"), ["openid", "profile", "email"]);
    assert_eq!(scopes("facebook"), ["email"]);
    assert_eq!(scopes("x"), ["users.read", "tweet.read"]);
    assert_eq!(scopes("linkedin-openid"), ["openid", "profile", "email"]);
    assert_eq!(scopes("gitlab"), ["read_user"]);
    assert_eq!(scopes("bitbucket"), ["email"]);
    assert_eq!(
        scopes("slack"),
        [
            "identity.basic",
            "identity.email",
            "identity.team",
            "identity.avatar"
        ]
    );
    assert_eq!(scopes("slack-openid"), ["openid", "email", "profile"]);

    assert!(Socialite::driver("x").uses_pkce());
    assert!(!Socialite::driver("github").uses_pkce());
}

#[tokio::test]
async fn unknown_drivers_are_not_supported() {
    let _app = app();

    let error = Socialite::try_driver("myspace").unwrap_err();
    assert_eq!(error.to_string(), "Driver [myspace] not supported.");

    let error = Socialite::try_driver("").unwrap_err();
    assert_eq!(error.to_string(), "No Socialite driver was specified.");
    assert_eq!(
        Socialite::manager()
            .get_default_driver()
            .unwrap_err()
            .to_string(),
        "No Socialite driver was specified."
    );

    // `driver` hands the error to the provider's requests...
    let provider = Socialite::driver("myspace");
    assert_eq!(
        provider.redirect().unwrap_err().to_string(),
        "Driver [myspace] not supported."
    );
    assert_eq!(
        provider.user().await.unwrap_err().to_string(),
        "Driver [myspace] not supported."
    );
    assert_eq!(
        provider
            .user_from_token("token")
            .await
            .unwrap_err()
            .to_string(),
        "Driver [myspace] not supported."
    );
    assert_eq!(
        provider
            .refresh_token("token")
            .await
            .unwrap_err()
            .to_string(),
        "Driver [myspace] not supported."
    );
}

#[tokio::test]
async fn drivers_require_their_credentials() {
    let app = app_with(json!({"services": {"github": null}}));
    app.config().set(
        "services.gitlab",
        json!({"client_id": "id", "client_secret": null, "redirect": null}),
    );
    app.config()
        .set("services.bitbucket", json!({"client_id": "id"}));

    let error = Socialite::try_driver("github").unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<DriverMissingConfigurationException>()
            .unwrap(),
        &DriverMissingConfigurationException::make(
            "GithubProvider",
            &["client_id", "client_secret", "redirect"]
        )
    );
    assert_eq!(
        error.to_string(),
        "Missing required configuration keys [client_id, client_secret, redirect] for [GithubProvider] OAuth provider."
    );

    let error = Socialite::try_driver("bitbucket").unwrap_err();
    assert_eq!(
        error.to_string(),
        "Missing required configuration keys [client_secret, redirect] for [BitbucketProvider] OAuth provider."
    );

    // Keys that are present (even when empty) satisfy the check, like Laravel.
    Socialite::try_driver("gitlab").unwrap();

    // The error is kept for the provider's requests.
    let error = Socialite::driver("github").redirect().unwrap_err();
    assert!(error.is::<DriverMissingConfigurationException>());
}

#[test]
fn twitter_must_be_configured_for_oauth_2() {
    let _app = app_with(json!({"services": {"twitter": {"oauth": 1}}}));
    let error = Socialite::try_driver("twitter").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("OAuth 1, which is not supported")
    );
}

#[tokio::test]
async fn custom_drivers_may_be_registered() {
    let _app = app_with(json!({"services": {"company-gitlab": {
        "client_id": "company-id",
        "client_secret": "company-secret",
        "redirect": "/auth/company/callback",
        "host": "https://git.company.test",
    }}}));

    Socialite::extend("company-gitlab", |_app: &Container| {
        let config = config("services.company-gitlab");
        Ok(Socialite::build_provider(GitlabProvider::new(), &config)?
            .set_host(config["host"].as_str().unwrap_or_default()))
    });

    let provider = Socialite::driver("company-gitlab");
    assert_eq!(provider.get_client_id(), "company-id");
    assert_eq!(
        provider.get_redirect_url(),
        "https://laravel.test/auth/company/callback"
    );
    assert_eq!(
        provider.get_token_url(),
        "https://git.company.test/oauth/token"
    );

    let response = provider.stateless().redirect().unwrap();
    assert!(
        response
            .target_url()
            .unwrap()
            .starts_with("https://git.company.test/oauth/authorize?client_id=company-id&")
    );
}

#[tokio::test]
async fn custom_drivers_win_over_built_in_ones() {
    let _app = app();
    Socialite::extend("github", |_app| {
        Ok(Provider::new(
            GithubProvider,
            "overridden",
            "secret",
            "https://laravel.test/cb",
        ))
    });
    assert_eq!(Socialite::driver("github").get_client_id(), "overridden");

    // Creator errors are returned like any other.
    Socialite::extend("broken", |_app| {
        Err(illuminate_support::error::RuntimeException::new("Broken!").into())
    });
    assert_eq!(
        Socialite::driver("broken")
            .redirect()
            .unwrap_err()
            .to_string(),
        "Broken!"
    );
}

#[test]
fn build_provider_applies_http_options() {
    let _app = app();
    let provider = Socialite::build_provider(
        GithubProvider,
        &json!({
            "client_id": "id",
            "client_secret": "secret",
            "redirect": "https://laravel.test/callback",
            "guzzle": {"timeout": 3},
        }),
    )
    .unwrap();
    assert_eq!(provider.get_http_options(), &json!({"timeout": 3}));

    let error = Socialite::build_provider(GithubProvider, &json!({})).unwrap_err();
    assert!(error.is::<DriverMissingConfigurationException>());
}

#[test]
fn providers_are_built_fresh_every_time() {
    let _app = app();
    let first = Socialite::driver("github").scopes(["repo"]).stateless();
    let second = Socialite::driver("github");
    assert_eq!(first.get_scopes(), ["user:email", "repo"]);
    assert_eq!(second.get_scopes(), ["user:email"]);
    assert!(!second.is_stateless());
}

#[test]
fn managers_belong_to_their_container() {
    let _app = app();
    Socialite::extend("custom", |_app| {
        Ok(Provider::new(GithubProvider, "custom", "", ""))
    });
    assert!(Socialite::try_driver("custom").is_ok());

    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({})));
    assert!(Socialite::try_driver("custom").is_err());
    assert!(container.bound::<SocialiteManager>());
}

#[test]
fn socialite_is_discovered_as_a_package() {
    let package = discovered_providers()
        .into_iter()
        .find(|provider| provider.package == "laravel-socialite")
        .expect("laravel-socialite registers its provider for discovery");

    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    let provider = (package.provider)();
    assert_eq!(provider.name(), "SocialiteServiceProvider");
    provider.register(&container);
    provider.boot(&container);
    assert!(container.bound::<SocialiteManager>());
    assert!(Arc::ptr_eq(
        &Socialite::manager(),
        &container.make::<SocialiteManager>()
    ));
}

#[tokio::test]
async fn requests_go_through_the_http_facade() {
    let _app = app();
    // Nothing is faked: stray requests are prevented by the test app.
    let error = Socialite::driver("github")
        .user_from_token("token")
        .await
        .unwrap_err();
    assert!(error.is::<illuminate_http_client::StrayRequestException>());
    Http::assert_nothing_sent();
}
