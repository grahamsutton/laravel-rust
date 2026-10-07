//! Redirecting to each provider: authorization URLs, scopes, separators,
//! custom parameters, PKCE and stateless redirects.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use illuminate_support::json;
use laravel_socialite::Socialite;
use sha2::{Digest, Sha256};

use crate::support::*;

const STATE: &str = "ssssssssssssssssssssssssssssssssssssssss";

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Redirect to the driver's provider (configured by the callback) and
/// return the authorization URL.
async fn redirect_url(
    driver: &str,
    configure: impl FnOnce(laravel_socialite::Provider) -> laravel_socialite::Provider,
) -> String {
    let session = session();
    let response = handling(get("/auth/redirect", &session), async {
        configure(Socialite::driver(driver)).redirect()
    })
    .await
    .unwrap();
    assert_eq!(response.status().as_u16(), 302);
    response.target_url().unwrap()
}

#[tokio::test]
async fn github_redirects_with_its_default_scope() {
    let _app = app();
    predictable_random_strings();
    let session = session();

    let response = handling(get("/auth/redirect", &session), async {
        Socialite::driver("github").redirect()
    })
    .await
    .unwrap();

    assert!(response.is_redirect());
    assert_eq!(
        response.target_url().unwrap(),
        format!(
            "https://github.com/login/oauth/authorize?client_id=github-client-id\
             &redirect_uri=https%3A%2F%2Flaravel.test%2Fauth%2Fgithub%2Fcallback\
             &scope=user%3Aemail&response_type=code&state={STATE}"
        )
    );
    assert_eq!(session.get("state"), json!(STATE));
    assert!(!session.has("code_verifier"));
}

#[tokio::test]
async fn google_joins_scopes_with_spaces() {
    let _app = app();
    predictable_random_strings();
    let url = redirect_url("google", |google| google).await;
    assert_eq!(
        url,
        format!(
            "https://accounts.google.com/o/oauth2/auth?client_id=google-client-id\
             &redirect_uri=https%3A%2F%2Flaravel.test%2Fauth%2Fgoogle%2Fcallback\
             &scope=openid+profile+email&response_type=code&state={STATE}"
        )
    );
}

#[tokio::test]
async fn facebook_uses_its_graph_version_and_dialog_options() {
    let _app = app();
    predictable_random_strings();
    let url = redirect_url("facebook", |facebook| facebook).await;
    assert_eq!(
        url,
        format!(
            "https://www.facebook.com/v3.3/dialog/oauth?client_id=facebook-client-id\
             &redirect_uri=https%3A%2F%2Flaravel.test%2Fauth%2Ffacebook%2Fcallback\
             &scope=email&response_type=code&state={STATE}"
        )
    );

    let url = redirect_url("facebook", |facebook| facebook.as_popup().re_request()).await;
    assert_eq!(query_param(&url, "display").as_deref(), Some("popup"));
    assert_eq!(query_param(&url, "auth_type").as_deref(), Some("rerequest"));

    let url = redirect_url("facebook", |facebook| {
        facebook.re_request().re_authenticate()
    })
    .await;
    assert_eq!(
        query_param(&url, "auth_type").as_deref(),
        Some("reauthenticate")
    );
}

#[tokio::test]
async fn x_uses_pkce_and_rfc_3986_encoding() {
    let _app = app();
    predictable_random_strings();
    let session = session();

    let response = handling(get("/auth/redirect", &session), async {
        Socialite::driver("x").redirect()
    })
    .await
    .unwrap();

    let verifier = "v".repeat(96);
    assert_eq!(session.get("code_verifier"), json!(verifier));
    assert_eq!(
        response.target_url().unwrap(),
        format!(
            "https://x.com/i/oauth2/authorize?client_id=x-client-id\
             &redirect_uri=https%3A%2F%2Flaravel.test%2Fauth%2Fx%2Fcallback\
             &scope=users.read%20tweet.read&response_type=code&state={STATE}\
             &code_challenge={}&code_challenge_method=S256",
            challenge(&verifier)
        )
    );
}

#[tokio::test]
async fn x_always_sends_a_state() {
    let _app = app();
    let url = redirect_url("x", |x| x.stateless()).await;
    assert_eq!(query_param(&url, "state").as_deref(), Some("state"));
}

#[tokio::test]
async fn twitter_oauth_2_uses_the_twitter_endpoints() {
    let _app = app();
    for driver in ["twitter-oauth-2", "twitter"] {
        let url = redirect_url(driver, |twitter| twitter).await;
        assert!(
            url.starts_with(&format!(
                "https://twitter.com/i/oauth2/authorize?client_id={driver}-client-id&"
            )),
            "{url}"
        );
        assert_eq!(
            query_param(&url, "code_challenge_method").as_deref(),
            Some("S256")
        );
    }
}

#[tokio::test]
async fn linkedin_openid_requests_openid_scopes() {
    let _app = app();
    predictable_random_strings();
    let url = redirect_url("linkedin-openid", |linkedin| linkedin).await;
    assert_eq!(
        url,
        format!(
            "https://www.linkedin.com/oauth/v2/authorization?client_id=linkedin-openid-client-id\
             &redirect_uri=https%3A%2F%2Flaravel.test%2Fauth%2Flinkedin-openid%2Fcallback\
             &scope=openid+profile+email&response_type=code&state={STATE}"
        )
    );
}

#[tokio::test]
async fn gitlab_redirects_to_gitlab_com_or_the_configured_host() {
    let _app = app();
    predictable_random_strings();
    let url = redirect_url("gitlab", |gitlab| gitlab).await;
    assert_eq!(
        url,
        format!(
            "https://gitlab.com/oauth/authorize?client_id=gitlab-client-id\
             &redirect_uri=https%3A%2F%2Flaravel.test%2Fauth%2Fgitlab%2Fcallback\
             &scope=read_user&response_type=code&state={STATE}"
        )
    );

    let _app = app_with(json!({"services": {"gitlab": {"host": "https://gitlab.example.com/"}}}));
    let url = redirect_url("gitlab", |gitlab| gitlab).await;
    assert!(
        url.starts_with("https://gitlab.example.com/oauth/authorize?client_id=gitlab-client-id&")
    );
    assert_eq!(
        Socialite::driver("gitlab").get_token_url(),
        "https://gitlab.example.com/oauth/token"
    );
}

#[tokio::test]
async fn bitbucket_requests_the_email_scope() {
    let _app = app();
    predictable_random_strings();
    let url = redirect_url("bitbucket", |bitbucket| bitbucket).await;
    assert_eq!(
        url,
        format!(
            "https://bitbucket.org/site/oauth2/authorize?client_id=bitbucket-client-id\
             &redirect_uri=https%3A%2F%2Flaravel.test%2Fauth%2Fbitbucket%2Fcallback\
             &scope=email&response_type=code&state={STATE}"
        )
    );
}

#[tokio::test]
async fn slack_requests_user_scopes_by_default() {
    let _app = app();
    predictable_random_strings();
    let url = redirect_url("slack", |slack| slack).await;
    assert_eq!(
        url,
        format!(
            "https://slack.com/oauth/v2/authorize?client_id=slack-client-id\
             &redirect_uri=https%3A%2F%2Flaravel.test%2Fauth%2Fslack%2Fcallback\
             &scope=&response_type=code&state={STATE}\
             &user_scope=identity.basic%2Cidentity.email%2Cidentity.team%2Cidentity.avatar"
        )
    );
}

#[tokio::test]
async fn slack_bot_users_request_bot_scopes() {
    let _app = app();
    predictable_random_strings();
    let url = redirect_url("slack", |slack| {
        slack
            .as_bot_user()
            .set_scopes(["chat:write", "chat:write.public", "chat:write.customize"])
    })
    .await;
    assert_eq!(
        url,
        format!(
            "https://slack.com/oauth/v2/authorize?client_id=slack-client-id\
             &redirect_uri=https%3A%2F%2Flaravel.test%2Fauth%2Fslack%2Fcallback\
             &scope=chat%3Awrite%2Cchat%3Awrite.public%2Cchat%3Awrite.customize\
             &response_type=code&state={STATE}"
        )
    );
}

#[tokio::test]
async fn slack_openid_requests_openid_scopes() {
    let _app = app();
    predictable_random_strings();
    let url = redirect_url("slack-openid", |slack| slack).await;
    assert_eq!(
        url,
        format!(
            "https://slack.com/openid/connect/authorize?client_id=slack-openid-client-id\
             &redirect_uri=https%3A%2F%2Flaravel.test%2Fauth%2Fslack-openid%2Fcallback\
             &scope=openid+email+profile&response_type=code&state={STATE}"
        )
    );
}

#[tokio::test]
async fn scopes_are_merged_or_replaced() {
    let _app = app();

    let url = redirect_url("github", |github| {
        github
            .scopes(["read:user", "public_repo"])
            .scopes(["read:user"])
    })
    .await;
    assert_eq!(
        query_param(&url, "scope").as_deref(),
        Some("user:email,read:user,public_repo")
    );

    let url = redirect_url("github", |github| {
        github.set_scopes(["read:user", "public_repo"])
    })
    .await;
    assert_eq!(
        query_param(&url, "scope").as_deref(),
        Some("read:user,public_repo")
    );

    let url = redirect_url("google", |google| {
        google.scopes(["https://www.googleapis.com/auth/calendar.readonly"])
    })
    .await;
    assert_eq!(
        query_param(&url, "scope").as_deref(),
        Some("openid profile email https://www.googleapis.com/auth/calendar.readonly")
    );
}

#[tokio::test]
async fn optional_parameters_are_added_to_the_url() {
    let _app = app();
    let url = redirect_url("google", |google| {
        google.with(json!({"hd": "example.com", "prompt": "consent"}))
    })
    .await;
    let query = query(&url);
    assert_eq!(
        &query[query.len() - 2..],
        [
            ("hd".to_string(), "example.com".to_string()),
            ("prompt".to_string(), "consent".to_string()),
        ]
    );

    // `with` replaces the previous parameters, and may be a list of pairs.
    let url = redirect_url("google", |google| {
        google
            .with(json!({"hd": "example.com"}))
            .with([("access_type", "offline")])
    })
    .await;
    assert_eq!(query_param(&url, "hd"), None);
    assert_eq!(query_param(&url, "access_type").as_deref(), Some("offline"));
}

#[tokio::test]
async fn the_redirect_url_may_be_changed() {
    let _app = app();
    let url = redirect_url("github", |github| {
        github.redirect_url("https://laravel.test/other/callback")
    })
    .await;
    assert_eq!(
        query_param(&url, "redirect_uri").as_deref(),
        Some("https://laravel.test/other/callback")
    );
}

#[tokio::test]
async fn relative_redirects_are_resolved_to_the_application() {
    let _app = app_with(json!({"services": {"github": {"redirect": "/auth/github/callback"}}}));
    let url = redirect_url("github", |github| github).await;
    // The request being handled is `http://localhost/auth/redirect`.
    assert_eq!(
        query_param(&url, "redirect_uri").as_deref(),
        Some("http://localhost/auth/github/callback")
    );

    // Outside of a request, the application URL is used.
    assert_eq!(
        Socialite::driver("github").get_redirect_url(),
        "https://laravel.test/auth/github/callback"
    );
}

#[tokio::test]
async fn pkce_may_be_enabled_for_any_provider() {
    let _app = app();
    predictable_random_strings();
    let session = session();

    let response = handling(get("/auth/redirect", &session), async {
        Socialite::driver("github").enable_pkce().redirect()
    })
    .await
    .unwrap();

    let url = response.target_url().unwrap();
    let verifier = session.get("code_verifier");
    assert_eq!(verifier, json!("v".repeat(96)));
    assert_eq!(
        query_param(&url, "code_challenge"),
        Some(challenge(verifier.as_str().unwrap()))
    );
    assert_eq!(
        query_param(&url, "code_challenge_method").as_deref(),
        Some("S256")
    );
    // RFC 7636: a 43 character, URL-safe challenge.
    assert_eq!(query_param(&url, "code_challenge").unwrap().len(), 43);
}

#[tokio::test]
async fn stateless_redirects_need_no_session() {
    let _app = app();

    // No request (and so no session) at all...
    let response = Socialite::driver("github").stateless().redirect().unwrap();
    let url = response.target_url().unwrap();
    assert_eq!(query_param(&url, "state"), None);
    assert_eq!(
        query_param(&url, "client_id").as_deref(),
        Some("github-client-id")
    );
}

#[tokio::test]
async fn redirecting_with_state_requires_a_session() {
    let _app = app();
    let error = handling(illuminate_http::Request::create("/", "GET"), async {
        Socialite::driver("github").redirect()
    })
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "Session store not set on request.");
}
