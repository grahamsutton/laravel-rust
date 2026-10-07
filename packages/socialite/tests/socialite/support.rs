//! The application every test runs: a container with `services.*`
//! credentials for every driver, Socialite registered, stray HTTP requests
//! prevented, and helpers for requests with sessions.

use std::future::Future;
use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_http::{Request, with_request};
use illuminate_http_client::{Http, Request as ClientRequest};
use illuminate_session::{ArraySessionHandler, RequestSessionExt, Store};
use illuminate_support::{Str, Value, json};
use laravel_socialite::{DRIVERS, SocialiteServiceProvider};

pub struct App {
    pub container: Arc<Container>,
    _guard: LocalInstanceGuard,
}

impl Drop for App {
    fn drop(&mut self) {
        Str::create_random_strings_normally();
    }
}

impl App {
    pub fn config(&self) -> Arc<Repository> {
        self.container.make::<Repository>()
    }
}

pub fn merge(base: &mut Value, extra: Value) {
    match (base, extra) {
        (Value::Object(base), Value::Object(extra)) => {
            for (key, value) in extra {
                match base.get_mut(&key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        merge(existing, value)
                    }
                    _ => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, extra) => *base = extra,
    }
}

/// The application, with every driver configured.
pub fn app() -> App {
    app_with(json!({}))
}

/// The application, with extra configuration merged over the defaults.
pub fn app_with(extra: Value) -> App {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());

    let mut services = serde_json::Map::new();
    for driver in DRIVERS {
        services.insert(
            driver.to_string(),
            json!({
                "client_id": format!("{driver}-client-id"),
                "client_secret": format!("{driver}-secret"),
                "redirect": format!("https://laravel.test/auth/{driver}/callback"),
            }),
        );
    }
    services["twitter"]["oauth"] = json!(2);

    let mut config = json!({
        "app": {"name": "Laravel", "url": "https://laravel.test"},
        "services": services,
    });
    merge(&mut config, extra);
    container.instance(Repository::new(config));

    SocialiteServiceProvider.register(&container);
    SocialiteServiceProvider.boot(&container);

    Http::prevent_stray_requests();

    App {
        container,
        _guard: guard,
    }
}

/// A fresh session.
pub fn session() -> Arc<Store> {
    Arc::new(Store::new(
        "laravel_session",
        Arc::new(ArraySessionHandler::new(120)),
        None,
    ))
}

/// A `GET` request with the session.
pub fn get(uri: &str, session: &Arc<Store>) -> Request {
    let request = Request::create(uri, "GET");
    request.set_session(session.clone());
    request
}

/// Run the future while handling the request.
pub async fn handling<F: Future>(request: Request, future: F) -> F::Output {
    with_request(request, future).await
}

/// Make `Str::random` return predictable strings: the state is a string
/// of `s`, the PKCE code verifier a string of `v`.
pub fn predictable_random_strings() {
    Str::create_random_strings_using(|length| match length {
        40 => "s".repeat(40),
        96 => "v".repeat(96),
        other => "x".repeat(other),
    });
}

/// The decoded query of a URL, in order.
pub fn query(url: &str) -> Vec<(String, String)> {
    let Some((_, query)) = url.split_once('?') else {
        return Vec::new();
    };
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key), decode(value))
        })
        .collect()
}

/// One decoded query parameter of a URL.
pub fn query_param(url: &str, key: &str) -> Option<String> {
    query(url)
        .into_iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap();
                out.push(u8::from_str_radix(hex, 16).unwrap());
                index += 2;
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8(out).unwrap()
}

/// The requests sent through the `Http` facade.
pub fn sent() -> Vec<ClientRequest> {
    Http::recorded()
        .into_iter()
        .map(|(request, _)| request)
        .collect()
}

/// The one request sent to the URL.
pub fn sent_to(url: &str) -> ClientRequest {
    let requests: Vec<ClientRequest> = sent()
        .into_iter()
        .filter(|request| request.url() == url || request.url().starts_with(&format!("{url}?")))
        .collect();
    assert_eq!(
        requests.len(),
        1,
        "expected one request to [{url}], sent: {:?}",
        sent()
            .iter()
            .map(|request| request.url().to_string())
            .collect::<Vec<_>>()
    );
    requests.into_iter().next().unwrap()
}

/// A JSON response for `Http::fake_urls`.
pub fn ok(body: Value) -> illuminate_http_client::FakeResponse {
    Http::response(body, 200, &[])
}
