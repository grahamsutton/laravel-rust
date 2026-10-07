//! Exchanging codes for tokens, refreshing tokens, and users from tokens.

use illuminate_http_client::{Http, RequestException};
use illuminate_support::json;
use laravel_socialite::Socialite;

use crate::support::*;

/// Store a known state (and verifier) in a session, and handle the callback.
async fn callback(
    driver: &str,
    configure: impl FnOnce(laravel_socialite::Provider) -> laravel_socialite::Provider,
    verifier: Option<&str>,
) -> illuminate_support::Result<laravel_socialite::User> {
    let session = session();
    session.put("state", "the-state");
    if let Some(verifier) = verifier {
        session.put("code_verifier", verifier);
    }
    let result = handling(
        get("/auth/callback?code=the-code&state=the-state", &session),
        async { configure(Socialite::driver(driver)).user().await },
    )
    .await;
    if verifier.is_some() {
        assert!(!session.has("code_verifier"), "the verifier is pulled");
    }
    result
}

#[tokio::test]
async fn the_code_is_exchanged_with_a_form_request() {
    let _app = app();
    Http::fake_urls([
        (
            "https://github.com/login/oauth/access_token",
            ok(json!({
                "access_token": "gho_token",
                "refresh_token": "ghr_refresh",
                "expires_in": 28800,
                "scope": "user:email,read:org",
                "token_type": "bearer",
            })),
        ),
        (
            "https://api.github.com/user",
            ok(json!({"id": 1, "login": "taylorotwell"})),
        ),
        ("https://api.github.com/user/emails", ok(json!([]))),
    ]);

    let user = callback("github", |github| github, None).await.unwrap();

    let token = sent_to("https://github.com/login/oauth/access_token");
    assert_eq!(token.method(), "POST");
    assert!(token.is_form());
    assert!(token.has_header_value("Accept", "application/json"));
    assert_eq!(
        token.data(),
        &json!({
            "grant_type": "authorization_code",
            "client_id": "github-client-id",
            "client_secret": "github-secret",
            "code": "the-code",
            "redirect_uri": "https://laravel.test/auth/github/callback",
        })
    );

    let profile = sent_to("https://api.github.com/user");
    assert_eq!(profile.method(), "GET");
    assert!(profile.has_header_value("Authorization", "token gho_token"));
    assert!(profile.has_header_value("Accept", "application/vnd.github.v3+json"));

    assert_eq!(user.token, "gho_token");
    assert_eq!(user.refresh_token.as_deref(), Some("ghr_refresh"));
    assert_eq!(user.expires_in, Some(28800));
    assert_eq!(user.approved_scopes, ["user:email", "read:org"]);
    assert_eq!(user.access_token_response_body["token_type"], "bearer");
}

#[tokio::test]
async fn custom_parameters_are_sent_with_the_token_request() {
    let _app = app();
    Http::fake_urls([
        (
            "https://www.googleapis.com/oauth2/v4/token",
            ok(json!({"access_token": "ya29", "scope": "openid email"})),
        ),
        (
            "https://www.googleapis.com/oauth2/v3/userinfo*",
            ok(json!({"sub": "1"})),
        ),
    ]);

    let user = callback(
        "google",
        |google| google.with(json!({"access_type": "offline"})),
        None,
    )
    .await
    .unwrap();

    let token = sent_to("https://www.googleapis.com/oauth2/v4/token");
    assert_eq!(token["access_type"], "offline");
    assert_eq!(token["grant_type"], "authorization_code");
    assert_eq!(user.approved_scopes, ["openid", "email"]);
}

#[tokio::test]
async fn pkce_sends_the_code_verifier() {
    let _app = app();
    Http::fake_urls([
        (
            "https://github.com/login/oauth/access_token",
            ok(json!({"access_token": "gho_token"})),
        ),
        ("https://api.github.com/user", ok(json!({"id": 1}))),
        ("https://api.github.com/user/emails", ok(json!([]))),
    ]);

    callback(
        "github",
        |github| github.enable_pkce(),
        Some("the-verifier"),
    )
    .await
    .unwrap();

    let token = sent_to("https://github.com/login/oauth/access_token");
    assert_eq!(token["code_verifier"], "the-verifier");
}

#[tokio::test]
async fn x_authenticates_its_token_requests_with_basic_auth() {
    let _app = app();
    Http::fake_urls([
        (
            "https://api.x.com/2/oauth2/token",
            ok(json!({
                "token_type": "bearer",
                "expires_in": 7200,
                "access_token": "x-token",
                "scope": "users.read tweet.read",
                "refresh_token": "x-refresh",
            })),
        ),
        (
            "https://api.x.com/2/users/me*",
            ok(json!({"data": {"id": "2244994945", "name": "X Dev", "username": "XDevelopers"}})),
        ),
    ]);

    let user = callback("x", |x| x, Some("the-verifier")).await.unwrap();

    let token = sent_to("https://api.x.com/2/oauth2/token");
    assert!(token.has_header_value("Authorization", "Basic eC1jbGllbnQtaWQ6eC1zZWNyZXQ="));
    assert!(token.has_header_value("Accept", "application/json"));
    assert_eq!(token["grant_type"], "authorization_code");
    assert_eq!(token["code"], "the-code");
    assert_eq!(token["code_verifier"], "the-verifier");

    assert_eq!(user.token, "x-token");
    assert_eq!(user.approved_scopes, ["users.read", "tweet.read"]);
}

#[tokio::test]
async fn facebook_renames_expires() {
    let _app = app();
    Http::fake_urls([
        (
            "https://graph.facebook.com/v3.3/oauth/access_token",
            ok(json!({"access_token": "fb-token", "token_type": "bearer", "expires": 5183944})),
        ),
        (
            "https://graph.facebook.com/v3.3/me*",
            ok(json!({"id": "10229", "name": "Taylor Otwell"})),
        ),
    ]);

    let user = callback("facebook", |facebook| facebook, None)
        .await
        .unwrap();

    let token = sent_to("https://graph.facebook.com/v3.3/oauth/access_token");
    assert!(token.is_form());
    assert_eq!(token["client_id"], "facebook-client-id");
    assert_eq!(user.expires_in, Some(5183944));
    assert_eq!(user.access_token_response_body["expires_in"], 5183944);
}

#[tokio::test]
async fn failed_token_requests_are_errors() {
    let _app = app();
    Http::fake_urls([(
        "https://github.com/login/oauth/access_token",
        Http::response(json!({"error": "bad_verification_code"}), 401, &[]),
    )]);

    let error = callback("github", |github| github, None).await.unwrap_err();

    let exception = error.downcast_ref::<RequestException>().unwrap();
    assert_eq!(exception.code(), 401);
}

#[tokio::test]
async fn http_errors_may_be_turned_off_with_guzzle_options() {
    let _app =
        app_with(json!({"services": {"gitlab": {"guzzle": {"http_errors": false, "timeout": 5}}}}));
    Http::fake_urls([(
        "https://gitlab.com/api/v4/user*",
        Http::response(json!({"message": "401 Unauthorized"}), 401, &[]),
    )]);

    let gitlab = Socialite::driver("gitlab");
    assert_eq!(gitlab.get_http_options()["timeout"], 5);

    let user = gitlab.user_from_token("expired").await.unwrap();
    assert_eq!(user["message"], "401 Unauthorized");
}

#[tokio::test]
async fn tokens_are_refreshed() {
    let _app = app();
    Http::fake_urls([(
        "https://github.com/login/oauth/access_token",
        ok(json!({
            "access_token": "gho_new",
            "expires_in": 28800,
            "refresh_token": "ghr_new",
            "scope": "user:email,repo",
        })),
    )]);

    let token = Socialite::driver("github")
        .refresh_token("ghr_old")
        .await
        .unwrap();

    assert_eq!(token.token, "gho_new");
    assert_eq!(token.refresh_token, "ghr_new");
    assert_eq!(token.expires_in, Some(28800));
    assert_eq!(token.approved_scopes, ["user:email", "repo"]);

    let request = sent_to("https://github.com/login/oauth/access_token");
    assert!(request.is_form());
    assert!(request.has_header_value("Accept", "application/json"));
    assert_eq!(
        request.data(),
        &json!({
            "grant_type": "refresh_token",
            "refresh_token": "ghr_old",
            "client_id": "github-client-id",
            "client_secret": "github-secret",
        })
    );
}

#[tokio::test]
async fn the_old_refresh_token_is_kept_when_the_provider_does_not_rotate_it() {
    let _app = app();
    Http::fake_urls([(
        "https://www.googleapis.com/oauth2/v4/token",
        ok(json!({"access_token": "ya29.new", "expires_in": 3599, "scope": "openid email"})),
    )]);

    let token = Socialite::driver("google")
        .refresh_token("1//refresh")
        .await
        .unwrap();

    assert_eq!(token.token, "ya29.new");
    assert_eq!(token.refresh_token, "1//refresh");
    assert_eq!(token.approved_scopes, ["openid", "email"]);
}

#[tokio::test]
async fn x_refreshes_with_basic_auth() {
    let _app = app();
    Http::fake_urls([(
        "https://api.x.com/2/oauth2/token",
        ok(
            json!({"access_token": "x-new", "refresh_token": "x-refresh-new", "expires_in": 7200, "scope": "users.read"}),
        ),
    )]);

    let token = Socialite::driver("x")
        .refresh_token("x-refresh")
        .await
        .unwrap();

    assert_eq!(token.token, "x-new");
    assert_eq!(token.refresh_token, "x-refresh-new");
    let request = sent_to("https://api.x.com/2/oauth2/token");
    assert!(request.has_header_value("Authorization", "Basic eC1jbGllbnQtaWQ6eC1zZWNyZXQ="));
    assert_eq!(
        request.data(),
        &json!({"grant_type": "refresh_token", "refresh_token": "x-refresh", "client_id": "x-client-id"})
    );
}

#[tokio::test]
async fn users_are_retrieved_from_known_tokens() {
    let _app = app();
    Http::fake_urls([
        (
            "https://api.github.com/user",
            ok(json!({"id": 1, "login": "taylorotwell"})),
        ),
        (
            "https://api.github.com/user/emails",
            ok(json!([{"email": "taylor@laravel.com", "primary": true, "verified": true}])),
        ),
    ]);

    let user = Socialite::driver("github")
        .user_from_token("gho_known")
        .await
        .unwrap();

    assert_eq!(user.token, "gho_known");
    assert_eq!(user.get_email(), Some("taylor@laravel.com"));
    assert_eq!(user.refresh_token, None);
    Http::assert_sent_count(2);
    Http::assert_not_sent(|request| request.url().contains("access_token"));
}
