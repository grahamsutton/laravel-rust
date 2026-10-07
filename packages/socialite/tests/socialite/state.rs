//! The `state` stored when redirecting is checked on the callback.

use std::sync::Arc;

use illuminate_http::Request;
use illuminate_http_client::{Http, Request as ClientRequest};
use illuminate_session::Store;
use illuminate_support::json;
use laravel_socialite::{InvalidStateException, Socialite};

use crate::support::*;

fn fake_github() {
    Http::fake_urls([
        (
            "https://github.com/login/oauth/access_token",
            ok(json!({"access_token": "gho_token", "scope": "user:email", "token_type": "bearer"})),
        ),
        (
            "https://api.github.com/user",
            ok(
                json!({"id": 1, "node_id": "MDQ6", "login": "taylorotwell", "avatar_url": "https://a.test/t.png"}),
            ),
        ),
        (
            "https://api.github.com/user/emails",
            ok(json!([{"email": "taylor@laravel.com", "primary": true, "verified": true}])),
        ),
    ]);
}

async fn redirect(session: &Arc<Store>) -> String {
    handling(get("/auth/redirect", session), async {
        Socialite::driver("github").redirect()
    })
    .await
    .unwrap();
    session.get("state").as_str().unwrap().to_string()
}

async fn callback(
    uri: &str,
    session: &Arc<Store>,
) -> illuminate_support::Result<laravel_socialite::User> {
    handling(get(uri, session), async {
        Socialite::driver("github").user().await
    })
    .await
}

#[tokio::test]
async fn the_state_stored_in_the_session_must_be_returned() {
    let _app = app();
    fake_github();
    let session = session();

    let state = redirect(&session).await;
    assert_eq!(state.len(), 40);

    let user = callback(&format!("/auth/callback?code=abc&state={state}"), &session)
        .await
        .unwrap();
    assert_eq!(user.id, "1");

    // The state can only be used once...
    assert!(!session.has("state"));
    let error = callback(&format!("/auth/callback?code=abc&state={state}"), &session)
        .await
        .unwrap_err();
    assert!(error.is::<InvalidStateException>());
}

#[tokio::test]
async fn a_mismatching_state_is_rejected_before_any_request_is_sent() {
    let _app = app();
    fake_github();
    let session = session();
    redirect(&session).await;

    let error = callback("/auth/callback?code=abc&state=forged", &session)
        .await
        .unwrap_err();
    assert!(error.is::<InvalidStateException>());
    assert_eq!(error.to_string(), "Invalid state.");
    Http::assert_nothing_sent();
}

#[tokio::test]
async fn a_missing_state_is_rejected() {
    let _app = app();
    fake_github();

    // Missing from the request...
    let session = session();
    redirect(&session).await;
    let error = callback("/auth/callback?code=abc", &session)
        .await
        .unwrap_err();
    assert!(error.is::<InvalidStateException>());

    // Missing from the session (it expired, or the user never redirected)...
    let session = crate::support::session();
    let error = callback("/auth/callback?code=abc&state=", &session)
        .await
        .unwrap_err();
    assert!(error.is::<InvalidStateException>());
    let error = callback("/auth/callback?code=abc&state=anything", &session)
        .await
        .unwrap_err();
    assert!(error.is::<InvalidStateException>());

    Http::assert_nothing_sent();
}

#[tokio::test]
async fn the_state_requires_a_session() {
    let _app = app();
    fake_github();
    let error = handling(
        Request::create("/auth/callback?code=abc&state=x", "GET"),
        async { Socialite::driver("github").user().await },
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "Session store not set on request.");
}

#[tokio::test]
async fn stateless_providers_skip_the_check() {
    let _app = app();
    fake_github();

    let user = handling(Request::create("/auth/callback?code=abc", "GET"), async {
        Socialite::driver("github").stateless().user().await
    })
    .await
    .unwrap();

    assert_eq!(user.get_nickname(), Some("taylorotwell"));
    Http::assert_sent(|request: &ClientRequest| {
        request.url() == "https://github.com/login/oauth/access_token" && request["code"] == "abc"
    });
}

#[tokio::test]
async fn the_user_is_remembered_by_the_provider() {
    let _app = app();
    fake_github();
    let session = session();
    let state = redirect(&session).await;

    let (first, second) = handling(
        get(&format!("/auth/callback?code=abc&state={state}"), &session),
        async {
            let github = Socialite::driver("github");
            (github.user().await.unwrap(), github.user().await.unwrap())
        },
    )
    .await;

    assert_eq!(first, second);
    Http::assert_sent_count(3);
}

#[tokio::test]
async fn an_explicit_request_may_be_given() {
    let _app = app();
    fake_github();
    let session = session();
    session.put("state", "known-state");

    // No request is being handled: the provider uses the one it's given.
    let user = Socialite::driver("github")
        .set_request(get("/auth/callback?code=abc&state=known-state", &session))
        .user()
        .await
        .unwrap();

    assert_eq!(user.token, "gho_token");
}
