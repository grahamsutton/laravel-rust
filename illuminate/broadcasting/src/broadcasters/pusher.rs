//! The Pusher (and Reverb) broadcaster, and a small client for Pusher's
//! HTTP API.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{Hmac, Mac};
use indexmap::IndexMap;
use md5::Md5;
use sha2::{Digest, Sha256};

use illuminate_http::Request;
use illuminate_http_client::Http;
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Carbon, Map, Result, Value, ValueExt, json};

use crate::authorization::{ChannelRegistry, access_denied};
use crate::broadcasters::{conventions, url_encode};
use crate::channel::Channel;
use crate::contracts::Broadcaster;
use crate::exceptions::BroadcastException;
use crate::secretbox;

/// An error returned by the Pusher SDK: invalid input, or an API failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PusherException {
    message: String,
    status: Option<u16>,
}

impl PusherException {
    /// Create an exception for invalid input.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
        }
    }

    /// Create an exception for a failed API call (Pusher's `ApiErrorException`).
    pub fn api(status: u16, body: impl Into<String>) -> Self {
        Self {
            message: body.into(),
            status: Some(status),
        }
    }

    /// The HTTP status of a failed API call.
    pub fn status(&self) -> Option<u16> {
        self.status
    }

    /// Determine if the exception is an API failure.
    pub fn is_api_error(&self) -> bool {
        self.status.is_some()
    }
}

impl fmt::Display for PusherException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PusherException {}

/// The resolved settings of a [`Pusher`] client.
#[derive(Clone, PartialEq)]
pub struct PusherSettings {
    /// The application key.
    pub auth_key: String,
    /// The application secret.
    pub secret: String,
    /// The application ID.
    pub app_id: String,
    /// `http` or `https`.
    pub scheme: String,
    /// The API host (`api-mt1.pusher.com`, your Reverb host, ...).
    pub host: String,
    /// The API port.
    pub port: u16,
    /// A path prefix for every API call.
    pub path: String,
    /// The request timeout, in seconds.
    pub timeout: f64,
    /// The connection timeout, in seconds.
    pub connect_timeout: f64,
    /// Whether TLS certificates are verified.
    pub verify: bool,
}

impl PusherSettings {
    /// The application's API path (`/apps/{app_id}`).
    pub fn base_path(&self) -> String {
        format!("/apps/{}", self.app_id)
    }

    /// The scheme, host, port and path prefix of every API URL.
    pub fn url_prefix(&self) -> String {
        format!("{}://{}:{}{}", self.scheme, self.host, self.port, self.path)
    }
}

impl fmt::Debug for PusherSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PusherSettings")
            .field("auth_key", &self.auth_key)
            .field("app_id", &self.app_id)
            .field("scheme", &self.scheme)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// A client for Pusher's HTTP API (and anything speaking it, like Reverb).
///
/// Requests go through the [`Http`] client, so `Http::fake()` intercepts
/// them in your tests.
///
/// ```
/// use illuminate_broadcasting::Pusher;
/// use illuminate_support::json;
///
/// let pusher = Pusher::new("278d425bdf160c739803", "7ad3773142a6692b25b8", "3", &json!({
///     "cluster": "eu",
/// })).unwrap();
///
/// assert_eq!(pusher.settings().url_prefix(), "https://api-eu.pusher.com:443");
///
/// let auth = pusher.authorize_channel("private-foobar", "1234.1234", None).unwrap();
/// assert_eq!(
///     auth["auth"],
///     "278d425bdf160c739803:58df8b0c36d6982b82c3ecf6b4662e34fe8c25bba48f5369f135bf843651c3a4"
/// );
/// ```
#[derive(Clone)]
pub struct Pusher {
    settings: PusherSettings,
    master_key: Option<[u8; 32]>,
}

impl Pusher {
    /// Create a client with Pusher's `options` (`cluster`, `host`, `port`,
    /// `scheme`, `useTLS`, `path`, `timeout`,
    /// `encryption_master_key_base64`).
    pub fn new(
        auth_key: impl Into<String>,
        secret: impl Into<String>,
        app_id: impl Into<String>,
        options: &Value,
    ) -> Result<Self> {
        let use_tls = options.get("useTLS").is_none_or(|value| value.truthy());

        let scheme = option_string(options, "scheme")
            .unwrap_or_else(|| if use_tls { "https" } else { "http" }.to_string());

        let port = match options.get("port").filter(|port| !port.is_blank()) {
            Some(value) => value
                .to_i64_lossy()
                .and_then(|port| u16::try_from(port).ok())
                .ok_or_else(|| {
                    InvalidArgumentException::new(format!(
                        "Invalid Pusher port [{}].",
                        value.to_string_lossy()
                    ))
                })?,
            None if scheme == "https" => 443,
            None => 80,
        };

        let host = option_string(options, "host")
            .or_else(|| {
                option_string(options, "cluster").map(|cluster| format!("api-{cluster}.pusher.com"))
            })
            .unwrap_or_else(|| "api-mt1.pusher.com".to_string());
        let host = host
            .strip_prefix("https://")
            .or_else(|| host.strip_prefix("http://"))
            .unwrap_or(&host)
            .to_string();

        let master_key = match option_string(options, "encryption_master_key_base64") {
            Some(encoded) => Some(parse_master_key(&encoded)?),
            None => None,
        };

        Ok(Self {
            settings: PusherSettings {
                auth_key: auth_key.into(),
                secret: secret.into(),
                app_id: app_id.into(),
                scheme,
                host,
                port,
                path: option_string(options, "path").unwrap_or_default(),
                timeout: options
                    .get("timeout")
                    .and_then(ValueExt::to_f64_lossy)
                    .unwrap_or(30.0),
                connect_timeout: 10.0,
                verify: true,
            },
            master_key,
        })
    }

    /// Create a client from a broadcasting connection's configuration (`key`,
    /// `secret`, `app_id`, `options` and `client_options`).
    pub fn from_config(config: &Value) -> Result<Self> {
        let credential = |name: &str| {
            option_string(config, name).ok_or_else(|| {
                InvalidArgumentException::new(format!(
                    "The Pusher [{name}] has not been configured."
                ))
            })
        };
        let options = config.get("options").cloned().unwrap_or(Value::Null);
        let mut pusher = Self::new(
            credential("key")?,
            credential("secret")?,
            credential("app_id")?,
            &options,
        )?;

        if let Some(client) = config.get("client_options") {
            if let Some(timeout) = client.get("timeout").and_then(ValueExt::to_f64_lossy) {
                pusher.settings.timeout = timeout;
            }
            if let Some(timeout) = client
                .get("connect_timeout")
                .and_then(ValueExt::to_f64_lossy)
            {
                pusher.settings.connect_timeout = timeout;
            }
            if client.get("verify").is_some_and(|verify| !verify.truthy()) {
                pusher.settings.verify = false;
            }
        }
        Ok(pusher)
    }

    /// The client's settings.
    pub fn settings(&self) -> &PusherSettings {
        &self.settings
    }

    // ------------------------------------------------------------------
    // Triggering events
    // ------------------------------------------------------------------

    /// Trigger an event on up to 100 channels, optionally excluding the
    /// connection with the given socket ID.
    ///
    /// Events on an encrypted channel (`private-encrypted-*`) are encrypted
    /// end-to-end with the channel's shared secret, and may only be sent to
    /// that single channel.
    pub async fn trigger(
        &self,
        channels: &[String],
        event: &str,
        data: &Value,
        socket_id: Option<&str>,
    ) -> Result<()> {
        validate_channels(channels)?;
        if let Some(socket_id) = socket_id {
            validate_socket_id(socket_id)?;
        }

        let encoded = serde_json::to_string(data)?;
        let data = match channels
            .iter()
            .find(|channel| is_encrypted_channel(channel))
        {
            Some(_) if channels.len() > 1 => {
                return Err(PusherException::new(
                    "You cannot trigger to multiple channels when using encrypted channels",
                )
                .into());
            }
            Some(channel) => self.encrypt_payload(channel, &encoded)?,
            None => encoded,
        };

        let mut body = Map::new();
        body.insert("name".into(), Value::from(event));
        body.insert("data".into(), Value::from(data));
        body.insert("channels".into(), json!(channels));
        if let Some(socket_id) = socket_id {
            body.insert("socket_id".into(), Value::from(socket_id));
        }

        self.post("/events", serde_json::to_string(&body)?).await
    }

    /// Send a signed `POST` request to the application's API.
    async fn post(&self, path: &str, body: String) -> Result<()> {
        let path = format!("{}{}", self.settings.base_path(), path);
        let mut query = IndexMap::new();
        query.insert(
            "body_md5".to_string(),
            hex::encode(Md5::digest(body.as_bytes())),
        );
        let query = self.sign(&path, "POST", query);

        let url = format!(
            "{}{}?{}",
            self.settings.url_prefix(),
            path,
            query
                .iter()
                .map(|(key, value)| format!("{}={}", url_encode(key), url_encode(value)))
                .collect::<Vec<_>>()
                .join("&")
        );

        let mut request = Http::with_body(body, "application/json")
            .timeout(self.settings.timeout)
            .connect_timeout(self.settings.connect_timeout);
        if !self.settings.verify {
            request = request.without_verifying();
        }

        let response = request
            .send("POST", url)
            .await
            .map_err(|error| PusherException::new(error.to_string()))?;
        if response.status() != 200 {
            return Err(
                PusherException::api(response.status(), response.body().to_string()).into(),
            );
        }
        Ok(())
    }

    /// Sign a request to the API, returning its query parameters.
    fn sign(
        &self,
        path: &str,
        method: &str,
        query: IndexMap<String, String>,
    ) -> IndexMap<String, String> {
        Self::build_auth_query_params(
            &self.settings.auth_key,
            &self.settings.secret,
            method,
            path,
            query,
            "1.0",
            Carbon::now().timestamp(),
        )
    }

    /// Build the authentication query parameters of an API request:
    /// `auth_key`, `auth_timestamp`, `auth_version`, your parameters, and
    /// the `auth_signature` — the HMAC-SHA256 of
    /// `"{METHOD}\n{path}\n{sorted query string}"` keyed by the secret.
    ///
    /// ```
    /// use illuminate_broadcasting::Pusher;
    /// use indexmap::IndexMap;
    ///
    /// let mut query = IndexMap::new();
    /// query.insert("body_md5".to_string(), "ec365a775a4cd0599faeb73354201b6f".to_string());
    ///
    /// let params = Pusher::build_auth_query_params(
    ///     "278d425bdf160c739803", "7ad3773142a6692b25b8", "POST", "/apps/3/events", query, "1.0", 1353088179,
    /// );
    ///
    /// assert_eq!(params["auth_signature"], "da454824c97ba181a32ccc17a72625ba02771f50b50e1e7430e47a1f3f457e6c");
    /// ```
    pub fn build_auth_query_params(
        auth_key: &str,
        secret: &str,
        method: &str,
        path: &str,
        query: IndexMap<String, String>,
        auth_version: &str,
        auth_timestamp: i64,
    ) -> IndexMap<String, String> {
        let mut params = IndexMap::new();
        params.insert("auth_key".to_string(), auth_key.to_string());
        params.insert("auth_timestamp".to_string(), auth_timestamp.to_string());
        params.insert("auth_version".to_string(), auth_version.to_string());
        for (key, value) in query {
            params.insert(key.to_lowercase(), value);
        }
        params.sort_keys();

        let string_to_sign = format!(
            "{}\n{}\n{}",
            method.to_uppercase(),
            path,
            params
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("&")
        );
        params.insert(
            "auth_signature".to_string(),
            hmac_sha256(secret, &string_to_sign),
        );
        params
    }

    // ------------------------------------------------------------------
    // Authorizing channels and users
    // ------------------------------------------------------------------

    /// Authorize a socket to subscribe to a private (or encrypted private)
    /// channel. `custom_data` is signed along, and returned as the
    /// `channel_data`.
    pub fn authorize_channel(
        &self,
        channel: &str,
        socket_id: &str,
        custom_data: Option<&str>,
    ) -> Result<Value> {
        validate_channel(channel)?;
        validate_socket_id(socket_id)?;

        let string_to_sign = match custom_data {
            Some(data) => format!("{socket_id}:{channel}:{data}"),
            None => format!("{socket_id}:{channel}"),
        };
        let mut response = Map::new();
        response.insert(
            "auth".into(),
            Value::from(format!(
                "{}:{}",
                self.settings.auth_key,
                hmac_sha256(&self.settings.secret, &string_to_sign)
            )),
        );
        if let Some(data) = custom_data {
            response.insert("channel_data".into(), Value::from(data));
        }
        if is_encrypted_channel(channel) {
            let secret = self.shared_secret(channel)?;
            response.insert("shared_secret".into(), Value::from(BASE64.encode(secret)));
        }
        Ok(Value::Object(response))
    }

    /// Authorize a socket to join a presence channel as the given user.
    pub fn authorize_presence_channel(
        &self,
        channel: &str,
        socket_id: &str,
        user_id: &str,
        user_info: Option<Value>,
    ) -> Result<Value> {
        let mut user_data = Map::new();
        user_data.insert("user_id".into(), Value::from(user_id));
        if let Some(info) = user_info.filter(ValueExt::truthy) {
            user_data.insert("user_info".into(), info);
        }
        let channel_data = serde_json::to_string(&user_data)?;
        self.authorize_channel(channel, socket_id, Some(&channel_data))
    }

    /// Authenticate a user for the connection (Pusher's user
    /// authentication): the user data must have an `id`.
    pub fn authenticate_user(&self, socket_id: &str, user_data: &Value) -> Result<Value> {
        validate_socket_id(socket_id)?;
        if user_data.get("id").is_none_or(|id| id.is_blank()) {
            return Err(PusherException::new("Invalid user data: the user must have an id").into());
        }

        let serialized = serde_json::to_string(user_data)?;
        let signature = hmac_sha256(
            &self.settings.secret,
            &format!("{socket_id}::user::{serialized}"),
        );
        Ok(json!({
            "auth": format!("{}:{}", self.settings.auth_key, signature),
            "user_data": serialized,
        }))
    }

    // ------------------------------------------------------------------
    // End-to-end encryption
    // ------------------------------------------------------------------

    /// The shared secret of an encrypted channel: the SHA-256 of the channel
    /// name followed by the master key.
    pub fn shared_secret(&self, channel: &str) -> Result<[u8; 32]> {
        if !is_encrypted_channel(channel) {
            return Err(PusherException::new(format!(
                "You must specify a channel of the form private-encrypted-* for E2E encryption. Got {channel}"
            ))
            .into());
        }
        let master_key = self.master_key.ok_or_else(|| {
            PusherException::new(
                "You must specify an encryption master key to use encrypted channels",
            )
        })?;

        let mut hasher = Sha256::new();
        hasher.update(channel.as_bytes());
        hasher.update(master_key);
        Ok(hasher.finalize().into())
    }

    /// Encrypt an event's (JSON) data for an encrypted channel:
    /// `{"nonce": "...", "ciphertext": "..."}`.
    pub fn encrypt_payload(&self, channel: &str, payload: &str) -> Result<String> {
        let secret = self.shared_secret(channel)?;
        let nonce: [u8; secretbox::NONCE_BYTES] = rand::random();
        let ciphertext = secretbox::seal(payload.as_bytes(), &nonce, &secret);
        Ok(serde_json::to_string(&json!({
            "nonce": BASE64.encode(nonce),
            "ciphertext": BASE64.encode(ciphertext),
        }))?)
    }

    /// Decrypt an encrypted channel's event data — what Echo does in the
    /// browser. Handy in tests.
    pub fn decrypt_payload(&self, channel: &str, data: &str) -> Result<String> {
        let secret = self.shared_secret(channel)?;
        let message: Value = serde_json::from_str(data)?;
        let decode = |key: &str| {
            BASE64
                .decode(
                    message
                        .get(key)
                        .map(ValueExt::to_string_lossy)
                        .unwrap_or_default(),
                )
                .map_err(|_| PusherException::new("The encrypted payload is malformed"))
        };
        let nonce: [u8; secretbox::NONCE_BYTES] = decode("nonce")?
            .try_into()
            .map_err(|_| PusherException::new("The encrypted payload is malformed"))?;
        let plaintext = secretbox::open(&decode("ciphertext")?, &nonce, &secret)
            .ok_or_else(|| PusherException::new("The encrypted payload could not be decrypted"))?;
        Ok(String::from_utf8(plaintext)?)
    }
}

impl fmt::Debug for Pusher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pusher")
            .field("settings", &self.settings)
            .field("encrypted_channels", &self.master_key.is_some())
            .finish()
    }
}

/// The Pusher broadcaster, also used for Laravel Reverb.
pub struct PusherBroadcaster {
    pusher: Pusher,
    channels: Arc<ChannelRegistry>,
    allow_jsonp: bool,
}

impl PusherBroadcaster {
    /// Create a new broadcaster instance.
    pub fn new(pusher: Pusher, channels: Arc<ChannelRegistry>) -> Self {
        Self {
            pusher,
            channels,
            allow_jsonp: false,
        }
    }

    /// Allow JSONP callbacks on authorization responses.
    pub fn allow_jsonp(mut self, allow: bool) -> Self {
        self.allow_jsonp = allow;
        self
    }

    /// The Pusher client.
    pub fn pusher(&self) -> &Pusher {
        &self.pusher
    }
}

#[async_trait]
impl Broadcaster for PusherBroadcaster {
    async fn auth(&self, request: &Request) -> Result<Value> {
        let channel_name = request.string("channel_name");
        let normalized = conventions::normalize_channel_name(&channel_name);

        if channel_name.is_empty()
            || (conventions::is_guarded_channel(&channel_name)
                && self
                    .channels
                    .retrieve_user(request, &normalized)
                    .await
                    .is_none())
        {
            return Err(access_denied());
        }

        self.channels
            .verify_user_can_access_channel(request, &normalized, self)
            .await
    }

    async fn valid_authentication_response(
        &self,
        request: &Request,
        result: Value,
    ) -> Result<Value> {
        let channel_name = request.string("channel_name");
        let socket_id = request.string("socket_id");

        if channel_name.starts_with("private") {
            return self
                .pusher
                .authorize_channel(&channel_name, &socket_id, None);
        }

        let normalized = conventions::normalize_channel_name(&channel_name);
        let user = self
            .channels
            .retrieve_user(request, &normalized)
            .await
            .ok_or_else(access_denied)?;

        self.pusher.authorize_presence_channel(
            &channel_name,
            &socket_id,
            &user.auth_identifier().to_string_lossy(),
            Some(result),
        )
    }

    async fn broadcast(
        &self,
        channels: &[Channel],
        event: &str,
        mut payload: Map<String, Value>,
    ) -> Result<()> {
        let socket = payload
            .shift_remove("socket")
            .filter(|socket| !socket.is_null())
            .map(|socket| socket.to_string_lossy());
        let data = Value::Object(payload);
        let names: Vec<String> = channels
            .iter()
            .map(|channel| channel.name().to_string())
            .collect();

        for chunk in names.chunks(100) {
            if let Err(error) = self
                .pusher
                .trigger(chunk, event, &data, socket.as_deref())
                .await
            {
                return Err(match error.downcast_ref::<PusherException>() {
                    Some(pusher) if pusher.is_api_error() => {
                        BroadcastException::new(format!("Pusher error: {pusher}.")).into()
                    }
                    _ => error,
                });
            }
        }
        Ok(())
    }

    async fn resolve_authenticated_user(&self, request: &Request) -> Result<Option<Value>> {
        let Some(user) = self.channels.resolve_authenticated_user(request).await else {
            return Ok(None);
        };
        self.pusher
            .authenticate_user(&request.string("socket_id"), &user)
            .map(Some)
    }

    fn allows_jsonp(&self) -> bool {
        self.allow_jsonp
    }
}

/// HMAC-SHA256, hex encoded.
pub(crate) fn hmac_sha256(secret: &str, message: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any size");
    mac.update(message.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn option_string(options: &Value, key: &str) -> Option<String> {
    options
        .get(key)
        .filter(|value| !value.is_blank())
        .map(ValueExt::to_string_lossy)
}

fn parse_master_key(encoded: &str) -> Result<[u8; 32]> {
    let key = BASE64.decode(encoded.trim()).map_err(|_| {
        PusherException::new("encryption_master_key_base64 must be a valid base64 string")
    })?;
    key.try_into().map_err(|_| {
        PusherException::new(
            "encryption_master_key_base64 must encode a key which is 32 bytes long",
        )
        .into()
    })
}

fn is_encrypted_channel(channel: &str) -> bool {
    channel.starts_with("private-encrypted-")
}

fn validate_channels(channels: &[String]) -> Result<()> {
    if channels.len() > 100 {
        return Err(PusherException::new(
            "An event can be triggered on a maximum of 100 channels in a single call.",
        )
        .into());
    }
    channels
        .iter()
        .try_for_each(|channel| validate_channel(channel))
}

fn validate_channel(channel: &str) -> Result<()> {
    let valid = !channel.is_empty()
        && channel.len() <= 200
        && channel
            .trim_start_matches('#')
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_=@,.;".contains(c))
        && !channel.trim_start_matches('#').is_empty();
    if valid {
        Ok(())
    } else {
        Err(PusherException::new(format!("Invalid channel name {channel}")).into())
    }
}

fn validate_socket_id(socket_id: &str) -> Result<()> {
    let valid = socket_id
        .split_once('.')
        .is_some_and(|(left, right)| is_digits(left) && is_digits(right));
    if valid {
        Ok(())
    } else {
        Err(PusherException::new(format!("Invalid socket ID {socket_id}")).into())
    }
}

fn is_digits(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pusher(options: Value) -> Pusher {
        Pusher::new(
            "278d425bdf160c739803",
            "7ad3773142a6692b25b8",
            "3",
            &options,
        )
        .unwrap()
    }

    #[test]
    fn settings_follow_the_sdk_defaults() {
        let settings = pusher(Value::Null).settings().clone();
        assert_eq!(settings.url_prefix(), "https://api-mt1.pusher.com:443");
        assert_eq!(settings.base_path(), "/apps/3");
        assert_eq!(settings.timeout, 30.0);

        let settings = pusher(json!({"cluster": "eu"})).settings().clone();
        assert_eq!(settings.host, "api-eu.pusher.com");

        let settings = pusher(json!({"useTLS": false})).settings().clone();
        assert_eq!(settings.url_prefix(), "http://api-mt1.pusher.com:80");

        let settings = pusher(json!({
            "host": "https://reverb.test", "port": "8080", "scheme": "http", "useTLS": false, "path": "/pusher",
        }))
        .settings()
        .clone();
        assert_eq!(settings.url_prefix(), "http://reverb.test:8080/pusher");

        // Laravel's default Pusher configuration.
        let settings = pusher(json!({
            "cluster": null, "host": "api-mt1.pusher.com", "port": 443, "scheme": "https", "encrypted": true, "useTLS": true,
        }))
        .settings()
        .clone();
        assert_eq!(settings.url_prefix(), "https://api-mt1.pusher.com:443");

        assert!(Pusher::new("k", "s", "1", &json!({"port": "nope"})).is_err());
        assert!(format!("{:?}", pusher(Value::Null)).contains("api-mt1.pusher.com"));
    }

    #[test]
    fn credentials_are_required() {
        let error = Pusher::from_config(&json!({"key": "k", "secret": "s"})).unwrap_err();
        assert_eq!(
            error.to_string(),
            "The Pusher [app_id] has not been configured."
        );

        let pusher = Pusher::from_config(&json!({
            "key": "k", "secret": "s", "app_id": 42,
            "client_options": {"timeout": 5, "connect_timeout": 2, "verify": false},
        }))
        .unwrap();
        assert_eq!(pusher.settings().app_id, "42");
        assert_eq!(pusher.settings().timeout, 5.0);
        assert_eq!(pusher.settings().connect_timeout, 2.0);
        assert!(!pusher.settings().verify);
    }

    #[test]
    fn requests_are_signed_like_the_pusher_docs() {
        let mut query = IndexMap::new();
        query.insert(
            "body_md5".to_string(),
            "ec365a775a4cd0599faeb73354201b6f".to_string(),
        );
        let params = Pusher::build_auth_query_params(
            "278d425bdf160c739803",
            "7ad3773142a6692b25b8",
            "POST",
            "/apps/3/events",
            query,
            "1.0",
            1353088179,
        );
        assert_eq!(
            params.keys().collect::<Vec<_>>(),
            vec![
                "auth_key",
                "auth_timestamp",
                "auth_version",
                "body_md5",
                "auth_signature"
            ]
        );
        assert_eq!(
            params["auth_signature"],
            "da454824c97ba181a32ccc17a72625ba02771f50b50e1e7430e47a1f3f457e6c"
        );
    }

    #[test]
    fn channels_are_authorized_like_the_pusher_docs() {
        let pusher = pusher(Value::Null);
        assert_eq!(
            pusher
                .authorize_channel("private-foobar", "1234.1234", None)
                .unwrap(),
            json!({"auth": "278d425bdf160c739803:58df8b0c36d6982b82c3ecf6b4662e34fe8c25bba48f5369f135bf843651c3a4"})
        );

        let channel_data = r#"{"user_id":10,"user_info":{"name":"Mr. Channels"}}"#;
        let auth = pusher
            .authorize_channel("presence-foobar", "1234.1234", Some(channel_data))
            .unwrap();
        assert_eq!(
            auth["auth"],
            "278d425bdf160c739803:31935e7d86dba64c2a90aed31fdc61869f9b22ba9d8863bba239c03ca481bc80"
        );
        assert_eq!(auth["channel_data"], channel_data);

        let presence = pusher
            .authorize_presence_channel(
                "presence-foobar",
                "1234.1234",
                "10",
                Some(json!({"name": "Mr. Channels"})),
            )
            .unwrap();
        assert_eq!(
            presence["channel_data"],
            r#"{"user_id":"10","user_info":{"name":"Mr. Channels"}}"#
        );

        let without_info = pusher
            .authorize_presence_channel("presence-foobar", "1234.1234", "10", Some(json!(true)))
            .unwrap();
        assert_eq!(
            without_info["channel_data"],
            r#"{"user_id":"10","user_info":true}"#
        );
    }

    #[test]
    fn users_are_authenticated() {
        let auth = pusher(Value::Null)
            .authenticate_user(
                "1234.1234",
                &json!({"id": "1", "user_info": {"name": "Taylor"}}),
            )
            .unwrap();
        assert_eq!(
            auth["user_data"],
            r#"{"id":"1","user_info":{"name":"Taylor"}}"#
        );
        assert_eq!(
            auth["auth"],
            "278d425bdf160c739803:65de85648f6ddcfa237c736bb038e86b82ef3e15b80fbf6e3ba7bedb36ff0414"
        );

        assert!(
            pusher(Value::Null)
                .authenticate_user("1234.1234", &json!({"name": "x"}))
                .is_err()
        );
    }

    #[test]
    fn input_is_validated() {
        let pusher = pusher(Value::Null);
        assert_eq!(
            pusher
                .authorize_channel("private-foobar", "1234", None)
                .unwrap_err()
                .to_string(),
            "Invalid socket ID 1234"
        );
        assert_eq!(
            pusher
                .authorize_channel("private foobar", "1.1", None)
                .unwrap_err()
                .to_string(),
            "Invalid channel name private foobar"
        );
        assert!(validate_channel("#server-to-user-1").is_ok());
        assert!(validate_channel("").is_err());
        assert!(validate_channel(&"a".repeat(201)).is_err());
        assert!(validate_channels(&vec!["a".to_string(); 101]).is_err());
        assert!(validate_socket_id("1.").is_err());
    }

    #[test]
    fn encrypted_channels_get_a_shared_secret() {
        let key = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
        let pusher = pusher(json!({"encryption_master_key_base64": key}));

        let auth = pusher
            .authorize_channel("private-encrypted-orders.1", "1234.1234", None)
            .unwrap();
        // base64(sha256("private-encrypted-orders.1" . key)), computed with PHP.
        assert_eq!(
            auth["shared_secret"],
            "XUyxZql99rulph8Jnn79rk5vgnJjAYyWGT1lVCstFJg="
        );

        let encrypted = pusher
            .encrypt_payload("private-encrypted-orders.1", r#"{"id":1}"#)
            .unwrap();
        let message: Value = serde_json::from_str(&encrypted).unwrap();
        assert_eq!(
            BASE64
                .decode(message["nonce"].as_str().unwrap())
                .unwrap()
                .len(),
            24
        );
        assert_eq!(
            pusher
                .decrypt_payload("private-encrypted-orders.1", &encrypted)
                .unwrap(),
            r#"{"id":1}"#
        );
        assert!(
            pusher
                .decrypt_payload("private-encrypted-orders.2", &encrypted)
                .is_err()
        );
        assert!(pusher.shared_secret("private-orders.1").is_err());

        let without_key = super::tests::pusher(Value::Null);
        assert!(
            without_key
                .authorize_channel("private-encrypted-orders.1", "1.1", None)
                .is_err()
        );

        assert!(
            Pusher::new(
                "k",
                "s",
                "1",
                &json!({"encryption_master_key_base64": "c2hvcnQ="})
            )
            .is_err()
        );
        assert!(
            Pusher::new(
                "k",
                "s",
                "1",
                &json!({"encryption_master_key_base64": "%%%"})
            )
            .is_err()
        );
    }
}
