//! Shared helpers for the broadcasting integration tests.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{Hmac, Mac};
use md5::Md5;
use sha2::{Digest, Sha256};

use illuminate_broadcasting::BroadcastServiceProvider;
use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_http::{HeaderMap, HeaderValue, Request};
use illuminate_http_client::Http;
use illuminate_log::{Log, MessageLogged};
use illuminate_queue::{BusServiceProvider, QueueServiceProvider};
use illuminate_support::{Carbon, Map, Value, json};

pub const KEY: &str = "278d425bdf160c739803";
pub const SECRET: &str = "7ad3773142a6692b25b8";
pub const APP_ID: &str = "3";
pub const MASTER_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
pub const TIMESTAMP: i64 = 1353088179;

/// A test application with its own container.
pub struct TestApp {
    pub container: Arc<Container>,
    _guard: LocalInstanceGuard,
}

impl TestApp {
    pub fn config(&self) -> Arc<Repository> {
        self.container.make::<Repository>()
    }
}

impl Drop for TestApp {
    fn drop(&mut self) {
        Carbon::set_thread_test_now(None);
    }
}

/// Laravel's `config/broadcasting.php`, plus the queue, auth and logging
/// configuration the tests need.
pub fn config() -> Value {
    json!({
        "broadcasting": {
            "default": "pusher",
            "connections": {
                "pusher": {
                    "driver": "pusher",
                    "key": KEY,
                    "secret": SECRET,
                    "app_id": APP_ID,
                    "options": {
                        "cluster": "mt1",
                        "host": "api-mt1.pusher.com",
                        "port": 443,
                        "scheme": "https",
                        "encrypted": true,
                        "useTLS": true,
                        "encryption_master_key_base64": MASTER_KEY,
                    },
                    "client_options": {},
                },
                "reverb": {
                    "driver": "reverb",
                    "key": "reverb-key",
                    "secret": "reverb-secret",
                    "app_id": "1001",
                    "options": {
                        "host": "localhost",
                        "port": 8080,
                        "scheme": "http",
                        "useTLS": false,
                    },
                },
                "ably": {"driver": "ably", "key": "abcd.efgh:ijkl"},
                "log": {"driver": "log"},
                "null": {"driver": "null"},
            },
        },
        "queue": {
            "default": "sync",
            "connections": {
                "sync": {"driver": "sync"},
                "array": {"driver": "array", "queue": "default"},
            },
        },
        "auth": {
            "defaults": {"guard": "api"},
            "guards": {
                "api": {"driver": "token", "provider": "users"},
                "admin": {"driver": "token", "provider": "admins", "input_key": "admin_token"},
            },
            "providers": {
                "users": {"driver": "array", "users": [
                    {"id": 1, "name": "Taylor", "api_token": "taylor-token"},
                    {"id": 2, "name": "Abigail", "api_token": "abigail-token"},
                ]},
                "admins": {"driver": "array", "users": [
                    {"id": 99, "name": "Admin", "api_token": "admin-token"},
                ]},
            },
        },
        "logging": {
            "default": "null",
            "channels": {"null": {"driver": "null"}},
        },
    })
}

/// Boot a test application with the default configuration, faking every
/// outgoing HTTP request with an empty `200` response.
pub fn app() -> TestApp {
    let app = app_with(|_| {});
    Http::fake();
    app
}

/// Boot a test application, adjusting the configuration first. Time is
/// frozen at Pusher's documented timestamp, and stray HTTP requests are
/// prevented: fake the responses you need.
pub fn app_with(configure: impl FnOnce(&mut Value)) -> TestApp {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());

    let mut config = config();
    configure(&mut config);
    container.instance(Repository::new(config));

    QueueServiceProvider.register(&container);
    BusServiceProvider.register(&container);
    BroadcastServiceProvider.register(&container);
    BroadcastServiceProvider.boot(&container);

    Carbon::set_thread_test_now(Some(Carbon::create_from_timestamp(TIMESTAMP)));
    Http::prevent_stray_requests();

    TestApp {
        container,
        _guard: guard,
    }
}

/// A request to the authorization endpoint.
pub fn auth_request(channel: &str, token: Option<&str>) -> Request {
    let mut input = json!({"channel_name": channel, "socket_id": "1234.1234"});
    if let Some(token) = token {
        input["api_token"] = Value::from(token);
    }
    Request::create_with("/broadcasting/auth", "POST", input, HeaderMap::new())
}

/// A request carrying the given input and headers.
pub fn request_with(input: Value, headers: &[(&'static str, &str)]) -> Request {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        map.insert(*name, HeaderValue::from_str(value).unwrap());
    }
    Request::create_with("/broadcasting/auth", "POST", input, map)
}

pub fn hmac_sha256(secret: &str, message: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(message.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// The shared secret of an encrypted channel, computed from Pusher's spec:
/// `sha256(channel . master_key)`.
pub fn shared_secret(channel: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(channel.as_bytes());
    hasher.update(BASE64.decode(MASTER_KEY).unwrap());
    hasher.finalize().into()
}

/// The requests sent through the HTTP client so far.
pub fn sent() -> Vec<illuminate_http_client::Request> {
    Http::recorded()
        .into_vec()
        .into_iter()
        .map(|(request, _)| request)
        .collect()
}

/// A request to Pusher's HTTP API, verified against the spec: the URL, the
/// `body_md5`, and the `auth_signature` recomputed from scratch.
pub struct PusherCall {
    pub endpoint: String,
    pub query: Map<String, Value>,
    pub body: Value,
    pub raw_body: String,
}

impl PusherCall {
    /// The decoded `data` of the event.
    pub fn data(&self) -> Value {
        serde_json::from_str(self.body["data"].as_str().unwrap()).unwrap()
    }
}

pub fn verify_pusher_call(
    request: &illuminate_http_client::Request,
    key: &str,
    secret: &str,
) -> PusherCall {
    assert_eq!(request.method(), "POST");
    assert!(request.has_header_value("Content-Type", "application/json"));

    let (endpoint, query_string) = request
        .url()
        .split_once('?')
        .expect("a signed query string");
    let mut query = Map::new();
    for pair in query_string.split('&') {
        let (name, value) = pair.split_once('=').unwrap();
        query.insert(name.to_string(), Value::from(value));
    }

    let raw_body = request.body().to_string();
    assert_eq!(query["auth_key"], key);
    assert_eq!(query["auth_timestamp"], TIMESTAMP.to_string());
    assert_eq!(query["auth_version"], "1.0");
    assert_eq!(
        query["body_md5"],
        hex::encode(Md5::digest(raw_body.as_bytes()))
    );

    // Recompute the signature from Pusher's spec.
    let mut params: Vec<(String, String)> = query
        .iter()
        .filter(|(name, _)| *name != "auth_signature")
        .map(|(name, value)| (name.clone(), value.as_str().unwrap().to_string()))
        .collect();
    params.sort();
    let path = endpoint
        .splitn(4, '/')
        .nth(3)
        .map(|path| format!("/{path}"))
        .unwrap();
    let path = path.trim_start_matches("/pusher").to_string();
    let string_to_sign = format!(
        "POST\n{path}\n{}",
        params
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("&")
    );
    assert_eq!(
        query["auth_signature"],
        hmac_sha256(secret, &string_to_sign)
    );

    PusherCall {
        endpoint: endpoint.to_string(),
        query,
        body: serde_json::from_str(&raw_body).unwrap(),
        raw_body,
    }
}

/// Record every message logged.
pub fn record_logs() -> Arc<Mutex<Vec<String>>> {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let recorder = logs.clone();
    Log::listen(move |record: &MessageLogged| {
        recorder.lock().unwrap().push(record.message.clone())
    });
    logs
}
