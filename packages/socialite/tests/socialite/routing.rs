//! The whole round trip through the router, with real sessions: redirect
//! to the provider, come back to the callback, and get the user.

use std::sync::Arc;

use illuminate_container::ServiceProvider;
use illuminate_http::{HeaderMap, HeaderValue, Json, Request, Response};
use illuminate_http_client::Http;
use illuminate_routing::{RouteMiddleware, Router};
use illuminate_session::{SessionServiceProvider, StartSession};
use illuminate_support::{Error, json};
use laravel_socialite::{InvalidStateException, Socialite};

use crate::support::*;

fn routes() -> Arc<Router> {
    let router = Router::new();
    router
        .get("/auth/redirect", || async {
            Socialite::driver("github").redirect()
        })
        .middleware(RouteMiddleware::of(StartSession::new()));
    router
        .get("/auth/callback", || async {
            let github_user = Socialite::driver("github").user().await?;
            Ok::<_, Error>(Json(json!({
                "github_id": github_user.id,
                "name": github_user.name,
                "email": github_user.email,
                "github_token": github_user.token,
                "github_refresh_token": github_user.refresh_token,
            })))
        })
        .middleware(RouteMiddleware::of(StartSession::new()));
    Arc::new(router)
}

/// A browser: remembers the session cookie between requests.
struct Browser {
    router: Arc<Router>,
    session_cookie: Option<String>,
}

impl Browser {
    async fn get(&mut self, uri: &str) -> Response {
        let mut headers = HeaderMap::new();
        if let Some(cookie) = &self.session_cookie {
            headers.insert(
                "cookie",
                HeaderValue::from_str(&format!("laravel_session={cookie}")).unwrap(),
            );
        }
        let request = Request::create_with(uri, "GET", json!({}), headers);
        let response = self.router.dispatch(request).await;
        if let Some(cookie) = response.get_cookie("laravel_session") {
            self.session_cookie = Some(cookie.value.clone());
        }
        response
    }
}

fn boot() -> App {
    let app = app_with(json!({"session": {
        "driver": "array",
        "cookie": "laravel_session",
        "lifetime": 120,
        "lottery": [0, 100],
    }}));
    SessionServiceProvider.register(&app.container);
    Http::fake_urls([
        (
            "https://github.com/login/oauth/access_token",
            ok(
                json!({"access_token": "gho_token", "refresh_token": "ghr_refresh", "scope": "user:email"}),
            ),
        ),
        (
            "https://api.github.com/user",
            ok(json!({"id": 1, "login": "taylorotwell", "name": "Taylor Otwell"})),
        ),
        (
            "https://api.github.com/user/emails",
            ok(json!([{"email": "taylor@laravel.com", "primary": true, "verified": true}])),
        ),
    ]);
    app
}

#[tokio::test]
async fn users_sign_in_through_the_provider() {
    let _app = boot();
    let mut browser = Browser {
        router: routes(),
        session_cookie: None,
    };

    let response = browser.get("/auth/redirect").await;
    assert_eq!(response.status().as_u16(), 302);
    let url = response.target_url().unwrap();
    assert!(
        url.starts_with("https://github.com/login/oauth/authorize?client_id=github-client-id&")
    );
    let state = query_param(&url, "state").unwrap();
    assert!(browser.session_cookie.is_some());

    // GitHub sends the user back with a code and the state...
    let response = browser
        .get(&format!("/auth/callback?code=the-code&state={state}"))
        .await;

    assert_eq!(
        response.status().as_u16(),
        200,
        "{}",
        response.content_string()
    );
    assert_eq!(
        response.json_body(),
        json!({
            "github_id": "1",
            "name": "Taylor Otwell",
            "email": "taylor@laravel.com",
            "github_token": "gho_token",
            "github_refresh_token": "ghr_refresh",
        })
    );
    Http::assert_sent(|request| {
        request.url() == "https://github.com/login/oauth/access_token"
            && request["code"] == "the-code"
    });
}

#[tokio::test]
async fn forged_callbacks_are_rejected() {
    let _app = boot();
    let mut browser = Browser {
        router: routes(),
        session_cookie: None,
    };
    browser.get("/auth/redirect").await;

    let response = browser
        .get("/auth/callback?code=the-code&state=forged")
        .await;

    assert_eq!(response.status().as_u16(), 500);
    assert!(response.exception().unwrap().is::<InvalidStateException>());
    Http::assert_nothing_sent();
}
