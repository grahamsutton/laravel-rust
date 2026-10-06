//! "Remember me" cookies, and logging out other devices.

use std::sync::Arc;

use illuminate_http::{Request, Response};
use illuminate_session::RequestSessionExt;
use illuminate_support::json;

use super::{Browser, app, handler, in_request, whoami};
use crate::middleware::{Authenticate, AuthenticateSession};
use crate::{Auth, Recaller, SessionGuard};

const RECALLER: &str = "remember_web_59ba36addc2b2f9401580f014c7f58ea4e30989d";

#[tokio::test]
async fn remembered_users_are_logged_in_by_their_cookie() {
    let app = app();
    let mut browser = Browser::new();

    browser.login("taylor@laravel.com", "secret", true).await;

    // The token was generated and stored...
    let token = app.user(1).remember_token.clone().unwrap();
    assert_eq!(token.len(), 60);

    // ...and the (encrypted) cookie was queued.
    let encrypted = browser.cookie(RECALLER).cloned().unwrap();
    assert!(!encrypted.contains(&token), "the cookie is encrypted");

    // A new session (the browser was closed) is authenticated by the cookie.
    browser.forget("laravel_session");
    let response = browser
        .get(
            "/dashboard",
            vec![Arc::new(Authenticate::new())],
            handler(|request: Request| async move {
                assert!(Auth::via_remember());
                // The user was put back in the session.
                assert_eq!(
                    request
                        .session()
                        .get("login_web_59ba36addc2b2f9401580f014c7f58ea4e30989d"),
                    json!(1)
                );
                Ok(Response::new("remembered"))
            }),
        )
        .await;
    assert_eq!(response.content_string(), "remembered");

    // Logging out forgets the cookie and cycles the token.
    browser
        .get(
            "/logout",
            vec![],
            handler(|_| async {
                Auth::logout().await?;
                Ok(Response::new("bye"))
            }),
        )
        .await;
    assert!(browser.cookie(RECALLER).is_none());
    assert_ne!(app.user(1).remember_token.unwrap(), token);

    // The old cookie no longer works.
    browser.forget("laravel_session");
    browser.set_cookie(RECALLER, &encrypted);
    let response = browser.get("/", vec![], whoami()).await;
    assert_eq!(response.content_string(), "guest");
}

#[tokio::test]
async fn the_recaller_cookie_holds_the_id_token_and_password_hash() {
    let app = app();
    let mut browser = Browser::new().without_encryption();
    browser.login("abigail@laravel.com", "password", true).await;

    let value = browser.cookie(RECALLER).cloned().unwrap();
    let recaller = Recaller::new(value.clone());
    assert!(recaller.valid());
    assert_eq!(recaller.id(), "2");
    assert_eq!(recaller.token(), app.user(2).remember_token.unwrap());

    let guard = Auth::guard("web");
    let session_guard = guard.downcast_ref::<SessionGuard>().unwrap();
    assert_eq!(
        recaller.hash(),
        session_guard.hash_password_for_cookie(&app.user(2).password)
    );
    assert_eq!(session_guard.remember_duration(), 576_000);

    // A tampered password hash is rejected.
    let tampered = format!("{}|{}|nope", recaller.id(), recaller.token());
    let mut stranger = Browser::new().without_encryption();
    stranger.set_cookie(RECALLER, &tampered);
    let response = stranger.get("/", vec![], whoami()).await;
    assert_eq!(response.content_string(), "guest");

    // So is a cookie with a missing segment.
    let mut stranger = Browser::new().without_encryption();
    stranger.set_cookie(RECALLER, "2|");
    let response = stranger.get("/", vec![], whoami()).await;
    assert_eq!(response.content_string(), "guest");

    // The genuine cookie works.
    let mut device = Browser::new().without_encryption();
    device.set_cookie(RECALLER, &value);
    let response = device.get("/", vec![], whoami()).await;
    assert_eq!(response.content_string(), "abigail@laravel.com");
}

#[tokio::test]
async fn remember_durations_are_configurable() {
    let _app = app();
    let guard = Auth::guard("admin");
    assert_eq!(
        guard
            .downcast_ref::<SessionGuard>()
            .unwrap()
            .remember_duration(),
        60
    );

    let (_, cookie) = in_request(|request: Request| async move {
        guard.login_using_id(&json!(1), true).await.unwrap();
        illuminate_cookie::CookieQueue::for_request(&request).queued(
            "remember_admin_59ba36addc2b2f9401580f014c7f58ea4e30989d",
            None,
        )
    })
    .await;
    assert_eq!(cookie.unwrap().minutes, Some(60));
}

#[tokio::test]
async fn other_devices_can_be_logged_out() {
    let app = app();
    let mut laptop = Browser::new();
    let mut phone = Browser::new();
    laptop.login("taylor@laravel.com", "secret", false).await;
    phone.login("taylor@laravel.com", "secret", true).await;

    let protected = || -> Vec<Arc<dyn illuminate_http::Middleware>> {
        vec![
            Arc::new(Authenticate::new()),
            Arc::new(AuthenticateSession::new()),
        ]
    };

    // Both devices are logged in.
    assert_eq!(
        laptop
            .get("/", protected(), whoami())
            .await
            .content_string(),
        "taylor@laravel.com"
    );
    assert_eq!(
        phone.get("/", protected(), whoami()).await.content_string(),
        "taylor@laravel.com"
    );

    let original = app.user(1).password.clone();

    // The wrong password is refused.
    let response = laptop
        .get(
            "/",
            protected(),
            handler(|_| async {
                let error = Auth::logout_other_devices("wrong").await.unwrap_err();
                Ok(Response::new(error.to_string()))
            }),
        )
        .await;
    assert_eq!(
        response.content_string(),
        "The given password does not match the current password."
    );

    // The right one rehashes the password.
    laptop
        .get(
            "/",
            protected(),
            handler(|_| async {
                Auth::logout_other_devices("secret").await?;
                Ok(Response::new("done"))
            }),
        )
        .await;
    assert_ne!(app.user(1).password, original);

    // The laptop stays logged in; the phone is logged out.
    assert_eq!(
        laptop
            .get("/", protected(), whoami())
            .await
            .content_string(),
        "taylor@laravel.com"
    );
    let response = phone.get("/", protected(), whoami()).await;
    assert!(response.is_redirect());
    assert_eq!(response.target_url().unwrap(), "http://localhost/login");
    assert_eq!(
        phone.get("/", vec![], whoami()).await.content_string(),
        "guest"
    );
}

#[tokio::test]
async fn logging_out_the_current_device_keeps_the_remember_token() {
    let app = app();
    let mut browser = Browser::new();
    browser.login("taylor@laravel.com", "secret", true).await;
    let token = app.user(1).remember_token.clone();

    browser
        .get(
            "/",
            vec![],
            handler(|_| async {
                Auth::logout_current_device().await?;
                Ok(Response::new("bye"))
            }),
        )
        .await;
    assert_eq!(app.user(1).remember_token, token);
    assert!(browser.cookie(RECALLER).is_none());
    assert_eq!(
        browser.get("/", vec![], whoami()).await.content_string(),
        "guest"
    );
}
