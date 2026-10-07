//! Faking providers in tests: `Socialite::fake` and `User::fake`.

use std::sync::Arc;

use illuminate_container::Container;
use illuminate_http::{Json, Request};
use illuminate_http_client::Http;
use illuminate_routing::Router;
use illuminate_support::{Error, json};
use laravel_socialite::{Socialite, User};

use crate::support::*;

/// The application's routes, as a user would write them.
fn routes() -> Router {
    let router = Router::new();
    router.get("/auth/github/redirect", || async {
        Socialite::driver("github").scopes(["read:org"]).redirect()
    });
    router.get("/auth/github/callback", || async {
        let user = Socialite::driver("github").user().await?;
        Ok::<_, Error>(Json(json!({
            "id": user.id,
            "name": user.name,
            "email": user.email,
            "token": user.token,
            "refresh_token": user.refresh_token,
            "expires_in": user.expires_in,
            "approved_scopes": user.approved_scopes,
        })))
    });
    router
}

#[tokio::test]
async fn the_redirect_may_be_faked() {
    let _app = app();
    let fake = Socialite::fake("github");

    // No session, no configuration needed: the fake doesn't redirect for real.
    let response = routes()
        .dispatch(Request::create("/auth/github/redirect", "GET"))
        .await;

    assert_eq!(response.status().as_u16(), 302);
    assert_eq!(
        response.target_url().unwrap(),
        "https://socialite.fake/github/authorize"
    );
    assert!(fake.redirected());
    assert_eq!(fake.redirects()[0].scopes, ["read:org"]);
    assert_eq!(fake.driver(), "github");
    Http::assert_nothing_sent();
}

#[tokio::test]
async fn the_callback_may_be_faked() {
    let _app = app();
    Socialite::fake_with(
        "github",
        User::fake(json!({
            "id": "github-123",
            "name": "Jason Beggs",
            "email": "jason@example.com",
        })),
    );

    let response = routes()
        .dispatch(Request::create("/auth/github/callback", "GET"))
        .await;

    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response.json_body(),
        json!({
            "id": "github-123",
            "name": "Jason Beggs",
            "email": "jason@example.com",
            "token": "fake-token",
            "refresh_token": "fake-refresh-token",
            "expires_in": 3600,
            "approved_scopes": [],
        })
    );
    Http::assert_nothing_sent();
}

#[tokio::test]
async fn fake_token_values_may_be_overridden() {
    let _app = app();
    Socialite::fake_with(
        "github",
        User::fake(json!({
            "id": "github-123",
            "name": "Jason Beggs",
            "email": "jason@example.com",
            "token": "fake-token",
            "refreshToken": "fake-refresh-token",
            "expiresIn": 3600,
            "approvedScopes": ["read", "write"],
        })),
    );

    let user = Socialite::driver("github").user().await.unwrap();
    assert_eq!(user.token, "fake-token");
    assert_eq!(user.refresh_token.as_deref(), Some("fake-refresh-token"));
    assert_eq!(user.expires_in, Some(3600));
    assert_eq!(user.approved_scopes, ["read", "write"]);
    assert_eq!(user["name"], "Jason Beggs");
}

#[tokio::test]
async fn fakes_without_a_user_return_a_fake_one() {
    let _app = app();
    Socialite::fake("google");

    let user = Socialite::driver("google")
        .stateless()
        .user()
        .await
        .unwrap();

    assert!(!user.id.is_empty());
    assert_eq!(user.token, "fake-token");
}

#[tokio::test]
async fn faked_providers_answer_every_request() {
    let _app = app();
    let fake = Socialite::fake_with("slack", User::fake(json!({"id": "U1"})));

    // Driver-specific options are ignored by the fake...
    let response = Socialite::driver("slack")
        .as_bot_user()
        .set_scopes(["chat:write"])
        .with(json!({"team": "T1"}))
        .stateless()
        .redirect()
        .unwrap();
    assert_eq!(
        response.target_url().unwrap(),
        "https://socialite.fake/slack/authorize"
    );
    let redirect = &fake.redirects()[0];
    assert_eq!(redirect.scopes, ["chat:write"]);
    assert_eq!(redirect.parameters["team"], "T1");
    assert!(redirect.stateless);

    let user = Socialite::driver("slack")
        .user_from_token("xoxp-known")
        .await
        .unwrap();
    assert_eq!(user.id, "U1");
    assert_eq!(user.token, "xoxp-known");

    let token = Socialite::driver("slack")
        .refresh_token("old-refresh")
        .await
        .unwrap();
    assert_eq!(token.token, "fake-token");
    assert_eq!(token.refresh_token, "old-refresh");
    assert_eq!(token.expires_in, Some(3600));

    // The fake user may be changed.
    fake.set_user(User::fake(json!({"id": "U2"})));
    assert_eq!(Socialite::driver("slack").user().await.unwrap().id, "U2");

    Http::assert_nothing_sent();
}

#[tokio::test]
async fn only_the_faked_driver_is_faked() {
    let _app = app();
    Socialite::fake("github");

    let response = Socialite::driver("gitlab").stateless().redirect().unwrap();
    assert!(
        response
            .target_url()
            .unwrap()
            .starts_with("https://gitlab.com/oauth/authorize")
    );

    Socialite::manager().forget_fake("github");
    let response = Socialite::driver("github").stateless().redirect().unwrap();
    assert!(
        response
            .target_url()
            .unwrap()
            .starts_with("https://github.com/")
    );
}

#[test]
fn fakes_belong_to_the_test_container() {
    let _app = app();
    Socialite::fake("github");
    assert!(Socialite::manager().get_fake("github").is_some());

    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container);
    assert!(Socialite::manager().get_fake("github").is_none());
}

#[test]
fn faked_drivers_need_no_configuration() {
    let _app = app_with(json!({"services": {"github": null}}));
    assert!(Socialite::try_driver("github").is_err());

    Socialite::fake("github");
    assert!(Socialite::try_driver("github").is_ok());
}
