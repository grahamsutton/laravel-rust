//! The Ably broadcaster (publishing through Ably's REST API).

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use illuminate_http::Request;
use illuminate_http_client::Http;
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Map, Result, Value, ValueExt, json};

use crate::authorization::{ChannelRegistry, access_denied};
use crate::broadcasters::pusher::hmac_sha256;
use crate::broadcasters::url_encode;
use crate::channel::Channel;
use crate::contracts::Broadcaster;
use crate::exceptions::BroadcastException;

/// Broadcasts events with [Ably](https://ably.com), and authorizes channels
/// for Ably's Pusher protocol adapter.
///
/// The connection needs your Ably API `key` (`app.key:secret`). The REST
/// `host` (default `rest.ably.io`), `port` and `tls` may be overridden.
pub struct AblyBroadcaster {
    key: String,
    host: String,
    port: Option<u16>,
    tls: bool,
    channels: Arc<ChannelRegistry>,
}

impl AblyBroadcaster {
    /// Create a new broadcaster instance for the given API key.
    pub fn new(key: impl Into<String>, channels: Arc<ChannelRegistry>) -> Self {
        Self {
            key: key.into(),
            host: "rest.ably.io".to_string(),
            port: None,
            tls: true,
            channels,
        }
    }

    /// Create a broadcaster from a broadcasting connection's configuration.
    pub fn from_config(config: &Value, channels: Arc<ChannelRegistry>) -> Result<Self> {
        let key = config
            .get("key")
            .filter(|key| !key.is_blank())
            .map(ValueExt::to_string_lossy)
            .ok_or_else(|| {
                InvalidArgumentException::new("The Ably [key] has not been configured.")
            })?;
        if !key.contains(':') {
            return Err(InvalidArgumentException::new(
                "The Ably [key] must be of the form \"app.key:secret\".",
            )
            .into());
        }

        let mut broadcaster = Self::new(key, channels);
        if let Some(host) = ["host", "restHost"]
            .iter()
            .find_map(|option| config.get(*option).filter(|host| !host.is_blank()))
        {
            broadcaster.host = host.to_string_lossy();
        }
        broadcaster.port = config
            .get("port")
            .and_then(ValueExt::to_i64_lossy)
            .and_then(|port| u16::try_from(port).ok());
        if let Some(tls) = config.get("tls") {
            broadcaster.tls = tls.truthy();
        }
        Ok(broadcaster)
    }

    /// The public part of the API key (before the `:`).
    pub fn public_token(&self) -> &str {
        self.key
            .split_once(':')
            .map_or(self.key.as_str(), |(public, _)| public)
    }

    /// The private part of the API key (after the `:`).
    fn private_token(&self) -> &str {
        self.key.split_once(':').map_or("", |(_, private)| private)
    }

    /// The signature for an authorization response: the HMAC-SHA256 of
    /// `"{socket_id}:{channel}[:{user_data}]"` keyed by the private token.
    pub fn generate_ably_signature(
        &self,
        channel_name: &str,
        socket_id: &str,
        user_data: Option<&str>,
    ) -> String {
        let message = match user_data {
            Some(data) => format!("{socket_id}:{channel_name}:{data}"),
            None => format!("{socket_id}:{channel_name}"),
        };
        hmac_sha256(self.private_token(), &message)
    }

    /// Determine if the channel is protected by authentication.
    pub fn is_guarded_channel(channel: &str) -> bool {
        channel.starts_with("private-") || channel.starts_with("presence-")
    }

    /// Remove the `private-` or `presence-` prefix from a channel name.
    pub fn normalize_channel_name(channel: &str) -> String {
        channel
            .strip_prefix("private-")
            .or_else(|| channel.strip_prefix("presence-"))
            .unwrap_or(channel)
            .to_string()
    }

    /// Ably's channel names: `private:…`, `presence:…` and `public:…`.
    ///
    /// ```
    /// use illuminate_broadcasting::{AblyBroadcaster, Channel, PresenceChannel, PrivateChannel};
    ///
    /// assert_eq!(
    ///     AblyBroadcaster::format_channels(&[PrivateChannel::new("a"), PresenceChannel::new("b"), Channel::new("c")]),
    ///     vec!["private:a", "presence:b", "public:c"]
    /// );
    /// ```
    pub fn format_channels(channels: &[Channel]) -> Vec<String> {
        channels
            .iter()
            .map(|channel| {
                let name = channel.name();
                if let Some(rest) = name.strip_prefix("private-") {
                    format!("private:{rest}")
                } else if let Some(rest) = name.strip_prefix("presence-") {
                    format!("presence:{rest}")
                } else {
                    format!("public:{name}")
                }
            })
            .collect()
    }

    fn base_url(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        match self.port {
            Some(port) => format!("{scheme}://{}:{port}", self.host),
            None => format!("{scheme}://{}", self.host),
        }
    }
}

impl fmt::Debug for AblyBroadcaster {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AblyBroadcaster")
            .field("key", &self.public_token())
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Broadcaster for AblyBroadcaster {
    async fn auth(&self, request: &Request) -> Result<Value> {
        let channel_name = request.string("channel_name");
        let normalized = Self::normalize_channel_name(&channel_name);

        if channel_name.is_empty()
            || (Self::is_guarded_channel(&channel_name)
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
            let signature = self.generate_ably_signature(&channel_name, &socket_id, None);
            return Ok(json!({"auth": format!("{}:{signature}", self.public_token())}));
        }

        let normalized = Self::normalize_channel_name(&channel_name);
        let user = self
            .channels
            .retrieve_user(request, &normalized)
            .await
            .ok_or_else(access_denied)?;

        let mut user_data = Map::new();
        let user_id = user.auth_identifier().to_string_lossy();
        if !user_id.is_empty() && user_id != "0" {
            user_data.insert("user_id".into(), Value::from(user_id));
        }
        if result.truthy() {
            user_data.insert("user_info".into(), result);
        }
        let channel_data = serde_json::to_string(&user_data)?;
        let signature =
            self.generate_ably_signature(&channel_name, &socket_id, Some(&channel_data));

        Ok(json!({
            "auth": format!("{}:{signature}", self.public_token()),
            "channel_data": channel_data,
        }))
    }

    async fn broadcast(
        &self,
        channels: &[Channel],
        event: &str,
        payload: Map<String, Value>,
    ) -> Result<()> {
        let socket = payload
            .get("socket")
            .filter(|socket| socket.truthy())
            .map(ValueExt::to_string_lossy);

        let mut message = Map::new();
        if !event.is_empty() {
            message.insert("name".into(), Value::from(event));
        }
        if let Some(socket) = socket {
            message.insert("connectionKey".into(), Value::from(socket));
        }
        message.insert("data".into(), Value::from(serde_json::to_string(&payload)?));
        message.insert("encoding".into(), Value::from("json"));

        let (key_name, key_secret) = self.key.split_once(':').unwrap_or((&self.key, ""));

        for channel in Self::format_channels(channels) {
            let url = format!(
                "{}/channels/{}/messages",
                self.base_url(),
                url_encode(&channel)
            );
            let response = Http::as_json()
                .accept_json()
                .with_basic_auth(key_name, key_secret)
                .post(url, &message)
                .await
                .map_err(|error| BroadcastException::new(format!("Ably error: {error}")))?;

            if !response.successful() {
                let body = response.json();
                let reason = body
                    .dot("error.message")
                    .map(ValueExt::to_string_lossy)
                    .unwrap_or_else(|| response.body().to_string());
                return Err(BroadcastException::new(format!("Ably error: {reason}")).into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn broadcaster() -> AblyBroadcaster {
        AblyBroadcaster::from_config(
            &json!({"key": "abcd:efgh"}),
            Arc::new(ChannelRegistry::new()),
        )
        .unwrap()
    }

    #[test]
    fn keys_are_split_into_tokens() {
        let ably = broadcaster();
        assert_eq!(ably.public_token(), "abcd");
        assert_eq!(ably.private_token(), "efgh");
        assert_eq!(ably.base_url(), "https://rest.ably.io");
        assert!(format!("{ably:?}").contains("abcd"));
        assert!(!format!("{ably:?}").contains("efgh"));

        let custom = AblyBroadcaster::from_config(
            &json!({"key": "a:b", "host": "localhost", "port": 8080, "tls": false}),
            Arc::new(ChannelRegistry::new()),
        )
        .unwrap();
        assert_eq!(custom.base_url(), "http://localhost:8080");

        assert!(
            AblyBroadcaster::from_config(&json!({}), Arc::new(ChannelRegistry::new())).is_err()
        );
        assert!(
            AblyBroadcaster::from_config(&json!({"key": "nope"}), Arc::new(ChannelRegistry::new()))
                .is_err()
        );
    }

    #[test]
    fn signatures_are_hmacs_of_the_socket_and_channel() {
        let ably = broadcaster();
        assert_eq!(
            ably.generate_ably_signature("private-test", "1.1", None),
            hmac_sha256("efgh", "1.1:private-test")
        );
        assert_eq!(
            ably.generate_ably_signature("presence-test", "1.1", Some(r#"{"user_id":"1"}"#)),
            hmac_sha256("efgh", r#"1.1:presence-test:{"user_id":"1"}"#)
        );
    }

    #[test]
    fn channel_names_follow_ably_conventions() {
        assert_eq!(
            AblyBroadcaster::normalize_channel_name("private-encrypted-a"),
            "encrypted-a"
        );
        assert_eq!(AblyBroadcaster::normalize_channel_name("presence-a"), "a");
        assert_eq!(AblyBroadcaster::normalize_channel_name("a"), "a");
        assert!(AblyBroadcaster::is_guarded_channel("presence-a"));
        assert!(!AblyBroadcaster::is_guarded_channel("a"));
    }
}
