//! The `log` and `null` broadcasters.

use async_trait::async_trait;
use serde::Serialize;

use illuminate_http::Request;
use illuminate_log::Log;
use illuminate_support::{Map, Result, Value};

use crate::channel::Channel;
use crate::contracts::Broadcaster;

/// Writes every broadcast to the application's log — wonderful for local
/// development and debugging.
///
/// ```text
/// [2024-01-01 12:00:00] local.INFO: Broadcasting [OrderShipped] on channels [private-orders.1] with payload:
/// {
///     "order_id": 1,
///     "socket": null
/// }
/// ```
///
/// Set a `channel` in the connection's configuration to log somewhere other
/// than your default log channel.
#[derive(Debug, Clone, Default)]
pub struct LogBroadcaster {
    channel: Option<String>,
}

impl LogBroadcaster {
    /// Create a broadcaster writing to the default log channel.
    pub fn new() -> Self {
        Self::default()
    }

    /// Write to the given log channel instead.
    pub fn on_channel(channel: impl Into<String>) -> Self {
        Self {
            channel: Some(channel.into()),
        }
    }
}

#[async_trait]
impl Broadcaster for LogBroadcaster {
    async fn auth(&self, _request: &Request) -> Result<Value> {
        Ok(Value::Null)
    }

    async fn valid_authentication_response(
        &self,
        _request: &Request,
        _result: Value,
    ) -> Result<Value> {
        Ok(Value::Null)
    }

    async fn broadcast(
        &self,
        channels: &[Channel],
        event: &str,
        payload: Map<String, Value>,
    ) -> Result<()> {
        let channels = channels
            .iter()
            .map(Channel::name)
            .collect::<Vec<_>>()
            .join(", ");
        let message = format!(
            "Broadcasting [{event}] on channels [{channels}] with payload:\n{}",
            pretty_json(&Value::Object(payload))?
        );
        match &self.channel {
            Some(channel) => Log::channel(channel).info(message),
            None => Log::info(message),
        }
        Ok(())
    }
}

/// Discards every broadcast: handy to disable broadcasting, in tests for
/// example.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullBroadcaster;

#[async_trait]
impl Broadcaster for NullBroadcaster {
    async fn auth(&self, _request: &Request) -> Result<Value> {
        Ok(Value::Null)
    }

    async fn valid_authentication_response(
        &self,
        _request: &Request,
        _result: Value,
    ) -> Result<Value> {
        Ok(Value::Null)
    }

    async fn broadcast(
        &self,
        _channels: &[Channel],
        _event: &str,
        _payload: Map<String, Value>,
    ) -> Result<()> {
        Ok(())
    }
}

/// JSON pretty printed with four spaces, like PHP's `JSON_PRETTY_PRINT`.
fn pretty_json(value: &Value) -> Result<String> {
    let mut buffer = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
    value.serialize(&mut serializer)?;
    Ok(String::from_utf8(buffer)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn payloads_are_pretty_printed_like_php() {
        assert_eq!(
            pretty_json(&json!({"id": 1, "tags": ["a"]})).unwrap(),
            "{\n    \"id\": 1,\n    \"tags\": [\n        \"a\"\n    ]\n}"
        );
    }

    #[tokio::test]
    async fn the_null_broadcaster_does_nothing() {
        let request = Request::create("/broadcasting/auth", "POST");
        assert_eq!(NullBroadcaster.auth(&request).await.unwrap(), Value::Null);
        assert_eq!(
            NullBroadcaster
                .valid_authentication_response(&request, json!(true))
                .await
                .unwrap(),
            Value::Null
        );
        NullBroadcaster
            .broadcast(&[Channel::new("orders")], "OrderShipped", Map::new())
            .await
            .unwrap();
        assert!(!NullBroadcaster.allows_jsonp());

        assert_eq!(
            LogBroadcaster::new().auth(&request).await.unwrap(),
            Value::Null
        );
        assert_eq!(
            LogBroadcaster::on_channel("broadcasts")
                .valid_authentication_response(&request, json!(true))
                .await
                .unwrap(),
            Value::Null
        );
    }
}
