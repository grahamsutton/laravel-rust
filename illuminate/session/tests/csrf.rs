mod common;

use std::sync::Arc;

use common::{Browser, app, app_with, array_session, destination, web};
use illuminate_cookie::CookieValuePrefix;
use illuminate_encryption::Encrypter;
use illuminate_http::{HeaderMap, HeaderValue, Request, Response};
use illuminate_session::{
    RequestSessionExt, TokenMismatchException, ValidateCsrfToken, csrf_field,
};
use illuminate_support::json;

fn routes() -> illuminate_http::Destination {
    destination(|request: Request| async move {
        match request.path().as_str() {
            "token" => Response::new(request.session().token().unwrap()),
            "form" => Response::new(csrf_field().to_html().to_string()),
            _ => Response::new("ok"),
        }
    })
}

fn browser(csrf: ValidateCsrfToken) -> Browser {
    Browser::new(web(vec![Arc::new(csrf)]), routes())
}

fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.insert(*name, HeaderValue::from_str(value).unwrap());
    }
    headers
}

#[tokio::test]
async fn reading_requests_pass_and_receive_the_xsrf_cookie() {
    let (app, _guard) = app(array_session());
    let browser = browser(ValidateCsrfToken::new());

    let response = browser.get("/token").await;
    assert!(response.is_ok());

    let token = response.content_string();
    let cookie = response.get_cookie("XSRF-TOKEN").unwrap();
    assert!(
        !cookie.http_only,
        "JavaScript must be able to read the XSRF-TOKEN cookie"
    );
    assert_eq!(cookie.minutes, Some(120));

    // EncryptCookies encrypted it with the value prefix, like every other cookie.
    let decrypted = app
        .make::<Encrypter>()
        .decrypt_string(&cookie.value)
        .unwrap();
    assert_eq!(CookieValuePrefix::remove(&decrypted), token);

    for method in ["HEAD", "OPTIONS"] {
        assert!(
            browser
                .send("/", method, json!({}), HeaderMap::new())
                .await
                .is_ok()
        );
    }
}

#[tokio::test]
async fn posts_without_a_token_are_rejected() {
    let (_app, _guard) = app(array_session());
    let browser = browser(ValidateCsrfToken::new());
    browser.get("/token").await;

    let response = browser.post("/profile", json!({"name": "Taylor"})).await;

    assert_eq!(response.status_code(), 419);
    assert_eq!(response.content_string(), "Page Expired");
}

#[tokio::test]
async fn mismatches_are_token_mismatch_exceptions() {
    // Without the foundation's exception handler, the error still travels with the response.
    let (_app, _guard) =
        common::bare_app(json!({"app": {"key": common::KEY}, "session": array_session()}));
    let browser = browser(ValidateCsrfToken::new());
    browser.get("/token").await;

    let response = browser.post("/profile", json!({"_token": "wrong"})).await;

    let exception = response.exception().expect("the exception is attached");
    let mismatch = exception.downcast_ref::<TokenMismatchException>().unwrap();
    assert_eq!(mismatch.to_string(), "CSRF token mismatch.");
    assert_eq!(mismatch.status_code(), 419);
}

#[tokio::test]
async fn the_token_input_is_accepted() {
    let (_app, _guard) = app(array_session());
    let browser = browser(ValidateCsrfToken::new());
    let token = browser.get("/token").await.content_string();

    let response = browser.post("/profile", json!({"_token": token})).await;
    assert!(response.is_ok());
    assert!(response.get_cookie("XSRF-TOKEN").is_some());

    let response = browser
        .post("/profile", json!({"_token": "x".repeat(40)}))
        .await;
    assert_eq!(response.status_code(), 419);
}

#[tokio::test]
async fn the_csrf_field_carries_the_token() {
    let (_app, _guard) = app(array_session());
    let browser = browser(ValidateCsrfToken::new());
    let token = browser.get("/token").await.content_string();

    let field = browser.get("/form").await.content_string();
    assert_eq!(
        field,
        format!(r#"<input type="hidden" name="_token" value="{token}" autocomplete="off">"#)
    );
}

#[tokio::test]
async fn the_x_csrf_token_header_is_accepted() {
    let (_app, _guard) = app(array_session());
    let browser = browser(ValidateCsrfToken::new());
    let token = browser.get("/token").await.content_string();

    let response = browser
        .send(
            "/profile",
            "PUT",
            json!({}),
            headers(&[("x-csrf-token", &token)]),
        )
        .await;
    assert!(response.is_ok());

    let response = browser
        .send(
            "/profile",
            "DELETE",
            json!({}),
            headers(&[("x-csrf-token", "nope")]),
        )
        .await;
    assert_eq!(response.status_code(), 419);
}

#[tokio::test]
async fn the_encrypted_x_xsrf_token_header_is_accepted() {
    let (_app, _guard) = app(array_session());
    let browser = browser(ValidateCsrfToken::new());
    browser.get("/token").await;

    // Axios copies the (encrypted) XSRF-TOKEN cookie into the X-XSRF-TOKEN header.
    let xsrf = browser.cookie("XSRF-TOKEN").unwrap();
    let response = browser
        .send(
            "/profile",
            "POST",
            json!({}),
            headers(&[("x-xsrf-token", &xsrf)]),
        )
        .await;
    assert!(response.is_ok());

    let response = browser
        .send(
            "/profile",
            "POST",
            json!({}),
            headers(&[("x-xsrf-token", "not-encrypted")]),
        )
        .await;
    assert_eq!(response.status_code(), 419);
}

#[tokio::test]
async fn regenerating_the_token_invalidates_the_old_one() {
    let (_app, _guard) = app(array_session());
    let browser = Browser::new(
        web(vec![Arc::new(ValidateCsrfToken::new())]),
        destination(|request: Request| async move {
            if request.path() == "logout" {
                request.session().regenerate_token();
            }
            Response::new(request.session().token().unwrap())
        }),
    );
    let token = browser.get("/").await.content_string();

    let new_token = browser
        .post("/logout", json!({"_token": token.clone()}))
        .await
        .content_string();
    assert_ne!(new_token, token);

    assert_eq!(
        browser
            .post("/profile", json!({"_token": token}))
            .await
            .status_code(),
        419
    );
    assert!(
        browser
            .post("/profile", json!({"_token": new_token}))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn excepted_uris_are_not_verified() {
    let (_app, _guard) = app(array_session());
    let browser =
        browser(ValidateCsrfToken::new().except(["/stripe/*", "http://localhost/foo/bar"]));

    assert!(browser.post("/stripe/webhook", json!({})).await.is_ok());
    assert!(browser.post("/foo/bar", json!({})).await.is_ok());
    assert_eq!(browser.post("/foo/baz", json!({})).await.status_code(), 419);
}

#[tokio::test]
async fn same_origin_requests_are_trusted() {
    let (_app, _guard) = app(array_session());
    let default = browser(ValidateCsrfToken::new());
    let same_origin = headers(&[("sec-fetch-site", "same-origin")]);
    let same_site = headers(&[("sec-fetch-site", "same-site")]);
    let cross_site = headers(&[("sec-fetch-site", "cross-site")]);

    assert!(
        default
            .send("/profile", "POST", json!({}), same_origin)
            .await
            .is_ok()
    );
    assert_eq!(
        default
            .send("/profile", "POST", json!({}), same_site.clone())
            .await
            .status_code(),
        419
    );
    assert_eq!(
        default
            .send("/profile", "POST", json!({}), cross_site.clone())
            .await
            .status_code(),
        419
    );

    let lenient = browser(ValidateCsrfToken::new().allow_same_site(true));
    assert!(
        lenient
            .send("/profile", "POST", json!({}), same_site)
            .await
            .is_ok()
    );

    let strict = browser(ValidateCsrfToken::new().use_origin_only(true));
    let response = strict.send("/profile", "POST", json!({}), cross_site).await;
    assert_eq!(response.status_code(), 403);
    let response = strict.get("/token").await;
    assert!(
        response.get_cookie("XSRF-TOKEN").is_none(),
        "origin-only mode sends no cookie"
    );
}

#[tokio::test]
async fn verification_can_be_disabled() {
    let (_app, _guard) = app(array_session());
    assert!(
        browser(ValidateCsrfToken::new().enabled(false))
            .post("/profile", json!({}))
            .await
            .is_ok()
    );

    let response = browser(ValidateCsrfToken::new().add_http_cookie(false))
        .get("/token")
        .await;
    assert!(response.get_cookie("XSRF-TOKEN").is_none());
}

#[tokio::test]
async fn verification_is_skipped_while_running_unit_tests() {
    let (_app, _guard) = app_with(json!({
        "app": {"key": common::KEY, "running_unit_tests": true},
        "session": array_session(),
    }));

    assert!(
        browser(ValidateCsrfToken::new())
            .post("/profile", json!({}))
            .await
            .is_ok()
    );

    let strict = browser(ValidateCsrfToken::new().verify_during_unit_tests());
    assert_eq!(strict.post("/profile", json!({})).await.status_code(), 419);
}

#[tokio::test]
async fn requests_without_a_session_never_match() {
    let (_app, _guard) = app(array_session());
    let browser = Browser::new(vec![Arc::new(ValidateCsrfToken::new())], routes());

    let response = browser.post("/profile", json!({"_token": ""})).await;
    assert_eq!(response.status_code(), 419);

    let response = browser.get("/").await;
    assert!(response.is_ok());
    assert!(response.get_cookie("XSRF-TOKEN").is_none());
}
