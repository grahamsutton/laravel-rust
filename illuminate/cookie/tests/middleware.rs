use std::sync::{Arc, Mutex};

use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_cookie::facades::Cookie;
use illuminate_cookie::{AddQueuedCookiesToResponse, CookieJar, CookieServiceProvider, CookieValuePrefix, EncryptCookies};
use illuminate_encryption::{EncryptionServiceProvider, Encrypter, MissingAppKeyException};
use illuminate_http::{Cookie as HttpCookie, HeaderMap, HeaderValue, Middleware, Request, Response, run_middleware};
use illuminate_support::{Value, json};

const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// The cookie `theme=dark`, encrypted by PHP exactly the way Laravel's
/// `EncryptCookies` middleware does it (prefix + `encryptString`).
const LARAVEL_THEME_COOKIE: &str = "eyJpdiI6IngxUWJuN1RVMndVZW00RXE1bnN0T2c9PSIsInZhbHVlIjoiTzRFVjljYUdKaW1vUWgyblhHbXJmTnd1RHJXNkxRSTZaZ2d6VHJrR0R4WldOOENzQTl4d0JaZ1p1VkpEemdzcyIsIm1hYyI6IjJhYmZlMmE5ZTk3NjdiMjA3MTg3MTU3YzM2ZTRjZDQ3YzQzZTIxNTBlYWZlMjNlMDlhMjVmYzQ0MWNjMDlmZGQiLCJ0YWciOiIifQ==";

fn app(config: Value) -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    container.instance(Repository::new(config));
    EncryptionServiceProvider.register(&container);
    CookieServiceProvider.register(&container);
    let guard = Container::set_local_instance(container.clone());
    (container, guard)
}

fn default_app() -> (Arc<Container>, LocalInstanceGuard) {
    app(json!({
        "app": {"key": KEY, "cipher": "AES-256-CBC"},
        "session": {"path": "/", "domain": null, "secure": false, "same_site": "lax"},
    }))
}

fn request_with_cookies(cookies: &str) -> Request {
    let mut headers = HeaderMap::new();
    headers.insert("cookie", HeaderValue::from_str(cookies).unwrap());
    Request::create_with("/", "GET", json!({}), headers)
}

/// Run the request through the middleware, recording the cookies the handler saw.
async fn run(
    request: Request,
    middleware: Vec<Arc<dyn Middleware>>,
    respond: impl Fn() -> Response + Send + Sync + 'static,
) -> (Response, Vec<(String, String)>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = seen.clone();
    let respond = Arc::new(respond);
    let response = run_middleware(
        request,
        middleware,
        Arc::new(move |request: Request| {
            let recorder = recorder.clone();
            let respond = respond.clone();
            Box::pin(async move {
                *recorder.lock().unwrap() = request.cookies().into_iter().collect();
                respond()
            })
        }),
    )
    .await;
    let seen = seen.lock().unwrap().clone();
    (response, seen)
}

#[tokio::test]
async fn it_decrypts_cookies_encrypted_by_laravel() {
    let (_app, _guard) = default_app();
    let request = request_with_cookies(&format!("theme={LARAVEL_THEME_COOKIE}"));

    let (_, seen) = run(request, vec![Arc::new(EncryptCookies::new())], Response::default).await;

    assert_eq!(seen, vec![("theme".to_string(), "dark".to_string())]);
}

#[tokio::test]
async fn outgoing_cookies_are_encrypted_with_the_value_prefix() {
    let (app, _guard) = default_app();

    let (response, _) = run(Request::create("/", "GET"), vec![Arc::new(EncryptCookies::new())], || {
        Response::new("ok").with_cookie(HttpCookie::new("color", "blue"))
    })
    .await;

    let encrypted = response.get_cookie("color").unwrap().value.clone();
    assert_ne!(encrypted, "blue");
    let decrypted = app.make::<Encrypter>().decrypt_string(&encrypted).unwrap();
    assert_eq!(decrypted, format!("{}blue", CookieValuePrefix::create("color", KEY.as_bytes())));

    // Sending it back in, the application sees the plain value.
    let request = request_with_cookies(&format!("color={encrypted}"));
    let (_, seen) = run(request, vec![Arc::new(EncryptCookies::new())], Response::default).await;
    assert_eq!(seen, vec![("color".to_string(), "blue".to_string())]);
}

#[tokio::test]
async fn invalid_cookies_are_dropped() {
    let (app, _guard) = default_app();
    let encrypter = app.make::<Encrypter>();

    // Encrypted for another cookie name, so the prefix doesn't match.
    let swapped = encrypter
        .encrypt_string(&format!("{}admin", CookieValuePrefix::create("role", KEY.as_bytes())))
        .unwrap();
    // Encrypted without any prefix at all.
    let unprefixed = encrypter.encrypt_string("admin").unwrap();

    let request = request_with_cookies(&format!(
        "tampered=not-encrypted; user={swapped}; legacy={unprefixed}; theme={LARAVEL_THEME_COOKIE}"
    ));
    let (_, seen) = run(request, vec![Arc::new(EncryptCookies::new())], Response::default).await;

    assert_eq!(seen, vec![("theme".to_string(), "dark".to_string())]);
}

#[tokio::test]
async fn excepted_cookies_are_left_alone() {
    let (_app, _guard) = default_app();
    let middleware = EncryptCookies::new().except(["plain"]);
    let request = request_with_cookies("plain=readable");

    let (response, seen) = run(request, vec![Arc::new(middleware)], || {
        Response::new("ok").with_cookie(HttpCookie::new("plain", "still-readable"))
    })
    .await;

    assert_eq!(seen, vec![("plain".to_string(), "readable".to_string())]);
    assert_eq!(response.get_cookie("plain").unwrap().value, "still-readable");

    let mut middleware = EncryptCookies::new();
    middleware.disable_for("other");
    assert!(middleware.is_disabled("other"));
}

#[tokio::test]
async fn previous_keys_still_decrypt_cookies() {
    let (_app, _guard) = default_app();
    let encrypter = Encrypter::new([1u8; 32], "aes-256-cbc")
        .unwrap()
        .previous_keys([KEY.as_bytes()])
        .unwrap();
    let request = request_with_cookies(&format!("theme={LARAVEL_THEME_COOKIE}"));

    let (_, seen) = run(request, vec![Arc::new(EncryptCookies::with_encrypter(Arc::new(encrypter)))], Response::default).await;

    assert_eq!(seen, vec![("theme".to_string(), "dark".to_string())]);
}

#[tokio::test]
async fn queued_cookies_are_attached_and_encrypted() {
    let (app, _guard) = default_app();

    let (response, _) = run(
        Request::create("/", "GET"),
        vec![Arc::new(EncryptCookies::new()), Arc::new(AddQueuedCookiesToResponse)],
        || {
            Cookie::queue_make("name", "value", 60);
            Cookie::queue(Cookie::forever("remember", "me"));
            Cookie::expire("old");
            Response::new("ok")
        },
    )
    .await;

    let encrypter = app.make::<Encrypter>();
    let name = response.get_cookie("name").unwrap();
    assert_eq!(name.minutes, Some(60));
    assert_eq!(
        CookieValuePrefix::remove(&encrypter.decrypt_string(&name.value).unwrap()),
        "value"
    );
    assert_eq!(response.get_cookie("remember").unwrap().minutes, Some(576_000));
    assert!(response.get_cookie("old").unwrap().is_cleared());

    // The queue belonged to that request: nothing leaks into the jar.
    assert!(app.make::<CookieJar>().get_queued_cookies().is_empty());
}

#[tokio::test]
async fn a_missing_key_renders_an_error() {
    let (_app, _guard) = app(json!({"app": {"key": null}}));

    let (response, _) = run(Request::create("/", "GET"), vec![Arc::new(EncryptCookies::new())], Response::default).await;

    assert_eq!(response.status_code(), 500);
    assert!(response.exception().unwrap().downcast_ref::<MissingAppKeyException>().is_some());
}

#[tokio::test]
async fn the_facade_reads_request_cookies() {
    let (_app, _guard) = default_app();
    let request = request_with_cookies("theme=dark");
    let theme = illuminate_http::with_request(request, async { (Cookie::get("theme"), Cookie::has("missing")) }).await;
    assert_eq!(theme, (Some("dark".to_string()), false));
    assert_eq!(Cookie::make("a", "b", 5).minutes, Some(5));
    assert!(Cookie::forget("a").is_cleared());
    assert!(Cookie::jar().get_queued_cookies().is_empty());
}

#[test]
fn the_provider_reads_session_configuration() {
    let (app, _guard) = app(json!({"session": {"path": "/app", "domain": ".laravel.com", "secure": true, "same_site": "none"}}));
    let cookie = app.make::<CookieJar>().make("a", "b", 1);
    assert_eq!(cookie.path, "/app");
    assert_eq!(cookie.domain.as_deref(), Some(".laravel.com"));
    assert!(cookie.secure);
    assert_eq!(cookie.same_site, Some(illuminate_http::SameSite::None));
    assert_eq!(illuminate_cookie::cookie("x", "y", 2).path, "/app");
    Cookie::unqueue("x");
    assert!(Cookie::queued("x").is_none());
}
