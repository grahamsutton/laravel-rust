//! The Postmark API transport.

use async_trait::async_trait;
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Result, Value, ValueExt, json};

use super::api;
use super::{Transport, TransportException};
use crate::message::{Message, SentMessage};

/// Sends mail through [Postmark's](https://postmarkapp.com) email API (the
/// `postmark` mailer transport).
///
/// The server token comes from the mailer's `token` (or `key`), falling back
/// to `services.postmark.token` (or `services.postmark.key`). Set the
/// mailer's `message_stream_id` to send through a specific message stream:
///
/// ```
/// use illuminate_mail::{PostmarkTransport, Transport};
///
/// let transport = PostmarkTransport::new("server-token").with_message_stream("broadcasts");
///
/// assert_eq!(transport.name(), "postmark+api://api.postmarkapp.com?message_stream=broadcasts");
/// ```
///
/// Postmark accepts a single tag per message; metadata is sent as Postmark
/// metadata, and an `X-PM-Message-Stream` header picks the stream for a
/// single message. The `MessageID` Postmark returns becomes the sent
/// message's ID.
#[derive(Clone)]
pub struct PostmarkTransport {
    token: String,
    message_stream: Option<String>,
    client: Value,
}

impl std::fmt::Debug for PostmarkTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostmarkTransport")
            .field("message_stream", &self.message_stream)
            .finish_non_exhaustive()
    }
}

impl PostmarkTransport {
    /// The Postmark API host.
    pub const HOST: &'static str = "api.postmarkapp.com";

    /// Create a transport sending with the given server token.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            message_stream: None,
            client: Value::Null,
        }
    }

    /// Create the transport from a mailer's configuration and the
    /// `services.postmark` configuration.
    pub fn from_config(config: &Value, services: &Value) -> Result<Self> {
        let token = api::filled(config, "token")
            .or_else(|| api::filled(config, "key"))
            .or_else(|| api::filled(services, "token"))
            .or_else(|| api::filled(services, "key"))
            .ok_or_else(|| {
                InvalidArgumentException::new(
                    "The Postmark mailer requires a server token. Set the mailer's [token] or [services.postmark.token].",
                )
            })?;
        let transport = Self::new(token).with_client_options(config.get("client").cloned());
        Ok(match api::filled(config, "message_stream_id") {
            Some(stream) => transport.with_message_stream(stream),
            None => transport,
        })
    }

    /// Send through the given message stream.
    pub fn with_message_stream(mut self, stream: impl Into<String>) -> Self {
        self.message_stream = Some(stream.into());
        self
    }

    /// Set the HTTP client options (the mailer's `client`, like `{"timeout": 5}`).
    pub fn with_client_options(mut self, options: impl Into<Option<Value>>) -> Self {
        self.client = options.into().unwrap_or(Value::Null);
        self
    }

    /// The message stream messages are sent through.
    pub fn message_stream(&self) -> Option<&str> {
        self.message_stream.as_deref()
    }

    /// Build the API payload for a message.
    pub(crate) fn payload(&self, message: &Message) -> Result<Value> {
        let join = |addresses| api::mailboxes(addresses).join(",");
        let attachments: Vec<Value> = message
            .attachments
            .iter()
            .map(|attachment| {
                let mut item = json!({
                    "Name": attachment.filename,
                    "Content": api::base64(&attachment.data),
                    "ContentType": attachment.content_type,
                });
                if let Some(id) = &attachment.content_id {
                    item["ContentID"] = json!(format!("cid:{id}"));
                }
                item
            })
            .collect();

        let mut payload = json!({
            "From": api::mailbox(api::from(message)?),
            "To": join(&message.to),
            "Cc": join(&message.cc),
            "Bcc": join(&message.bcc),
            "ReplyTo": join(&message.reply_to),
            "Subject": message.subject,
            "TextBody": message.text,
            "HtmlBody": message.html,
            "Attachments": attachments,
        });

        let mut stream = self.message_stream.clone();
        let mut headers = Vec::new();
        for (name, value) in api::custom_headers(message) {
            if name.eq_ignore_ascii_case("X-PM-Message-Stream") {
                stream = Some(value);
            } else {
                headers.push(json!({"Name": name, "Value": value}));
            }
        }
        if !headers.is_empty() {
            payload["Headers"] = json!(headers);
        }
        match message.tags.as_slice() {
            [] => {}
            [tag] => payload["Tag"] = json!(tag),
            _ => {
                return Err(TransportException::new(
                    "Postmark only allows a single tag per email.",
                )
                .into());
            }
        }
        if !message.metadata.is_empty() {
            payload["Metadata"] = json!(message.metadata);
        }
        if let Some(stream) = stream {
            payload["MessageStream"] = json!(stream);
        }
        Ok(payload)
    }
}

#[async_trait]
impl Transport for PostmarkTransport {
    async fn send(&self, message: &Message) -> Result<SentMessage> {
        let payload = self.payload(message)?;
        let response = api::request(&self.client)
            .with_headers([
                ("Accept", "application/json"),
                ("X-Postmark-Server-Token", self.token.as_str()),
            ])
            .post(format!("https://{}/email", Self::HOST), payload)
            .await
            .map_err(|e| api::unreachable(e, "Could not reach the remote Postmark server."))?;

        let status = response.status();
        let result = response.json();
        if !result.is_object() {
            let body = response.body().into_owned();
            return Err(api::failed(
                format!("Unable to send an email: {body} (code {status})."),
                status.into(),
                response,
            ));
        }
        if status != 200 {
            let code = result["ErrorCode"].to_i64_lossy().unwrap_or_default();
            return Err(api::failed(
                format!(
                    "Unable to send an email: {} (code {code}).",
                    result["Message"].to_string_lossy()
                ),
                code,
                response,
            ));
        }

        let mut sent = SentMessage::new(message.clone());
        sent.message_id = result["MessageID"].to_string_lossy();
        sent.debug = response.body().into_owned();
        Ok(sent)
    }

    fn name(&self) -> String {
        match &self.message_stream {
            Some(stream) => format!("postmark+api://{}?message_stream={stream}", Self::HOST),
            None => format!("postmark+api://{}", Self::HOST),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::testing::{self, PNG};
    use illuminate_http_client::Http;

    fn accepted() -> Value {
        json!({
            "To": "taylor@example.com",
            "SubmittedAt": "2026-10-07T12:00:00.0000000Z",
            "MessageID": "b7bc2f4a-e38e-4336-af7d-e6c392c2f817",
            "ErrorCode": 0,
            "Message": "OK",
        })
    }

    #[tokio::test]
    async fn messages_are_sent_through_the_postmark_api() {
        let _app = testing::container();
        Http::fake_using(|_| Http::response(accepted(), 200, &[]));
        let message = testing::message();

        let sent = PostmarkTransport::new("server-token")
            .with_message_stream("outbound")
            .send(&message)
            .await
            .unwrap();

        assert_eq!(sent.message_id(), "b7bc2f4a-e38e-4336-af7d-e6c392c2f817");
        assert_eq!(
            sent.original_message().subject.as_deref(),
            Some("Order Shipped")
        );
        assert!(sent.debug().contains("\"Message\":\"OK\""));

        let request = testing::sent_request();
        assert_eq!(request.method(), "POST");
        assert_eq!(request.url(), "https://api.postmarkapp.com/email");
        assert!(request.has_header_value("X-Postmark-Server-Token", "server-token"));
        assert!(request.has_header_value("Accept", "application/json"));
        assert!(request.is_json());
        assert_eq!(
            request.data(),
            &json!({
                "From": "\"Example\" <hello@example.com>",
                "To": "\"Taylor Otwell\" <taylor@example.com>,abigail@example.com",
                "Cc": "james@example.com",
                "Bcc": "secret@example.com",
                "ReplyTo": "\"Support\" <support@example.com>",
                "Subject": "Order Shipped",
                "TextBody": "Your order shipped.",
                "HtmlBody": "<p>Your order shipped.</p>",
                "Attachments": [
                    {"Name": "order.csv", "Content": api::base64(b"id,total"), "ContentType": "text/csv"},
                    {
                        "Name": "logo.png",
                        "Content": api::base64(PNG),
                        "ContentType": "image/png",
                        "ContentID": format!("cid:{}", testing::content_id(&message)),
                    },
                ],
                "Headers": [
                    {"Name": "X-Priority", "Value": "1 (Highest)"},
                    {"Name": "X-Order", "Value": "1"},
                ],
                "Tag": "shipment",
                "Metadata": {"order_id": "1"},
                "MessageStream": "outbound",
            })
        );
    }

    #[tokio::test]
    async fn optional_fields_are_left_out_and_headers_may_pick_the_stream() {
        let _app = testing::container();
        Http::fake_using(|_| Http::response(accepted(), 200, &[]));
        let mut message = Message::new();
        message
            .from("hello@example.com")
            .to("taylor@example.com")
            .text("Hi");

        PostmarkTransport::new("token")
            .send(&message)
            .await
            .unwrap();
        let data = testing::sent_request().data().clone();
        assert_eq!(data["Cc"], "");
        assert!(data["Subject"].is_null() && data["HtmlBody"].is_null());
        assert_eq!(data["Attachments"], json!([]));
        for key in ["Headers", "Tag", "Metadata", "MessageStream"] {
            assert!(data.get(key).is_none(), "{key} should not be sent");
        }

        let _app = testing::container();
        Http::fake_using(|_| Http::response(accepted(), 200, &[]));
        message.header("X-PM-Message-Stream", "broadcasts");
        PostmarkTransport::new("token")
            .with_message_stream("outbound")
            .send(&message)
            .await
            .unwrap();
        let data = testing::sent_request().data().clone();
        assert_eq!(data["MessageStream"], "broadcasts");
        assert!(data.get("Headers").is_none());
    }

    #[tokio::test]
    async fn postmark_only_allows_a_single_tag() {
        let _app = testing::container();
        Http::fake();
        let mut message = testing::message();
        message.tag("another");

        let error = PostmarkTransport::new("token")
            .send(&message)
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Postmark only allows a single tag per email."
        );
        assert!(error.downcast_ref::<TransportException>().is_some());
        Http::assert_nothing_sent();
    }

    #[tokio::test]
    async fn api_errors_become_transport_exceptions() {
        let _app = testing::container();
        Http::fake_sequence("api.postmarkapp.com/*")
            .push(
                json!({"ErrorCode": 300, "Message": "Invalid 'To' address: 'nope'."}),
                422,
            )
            .push("<html>Bad Gateway</html>", 502)
            .push_failed_connection_with("Connection refused");
        let transport = PostmarkTransport::new("token");
        let message = testing::message();

        let error = transport.send(&message).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Unable to send an email: Invalid 'To' address: 'nope'. (code 300)."
        );
        let exception = error.downcast_ref::<TransportException>().unwrap();
        assert_eq!(exception.code, 300);
        assert_eq!(exception.response.as_ref().unwrap().status(), 422);

        let error = transport.send(&message).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Unable to send an email: <html>Bad Gateway</html> (code 502)."
        );
        assert_eq!(
            error.downcast_ref::<TransportException>().unwrap().code,
            502
        );

        let error = transport.send(&message).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Could not reach the remote Postmark server."
        );
        assert!(error.downcast_ref::<TransportException>().is_some());
        assert!(format!("{error:#}").contains("Connection refused"));
    }

    #[test]
    fn tokens_are_resolved_like_laravel() {
        let services = json!({"token": "services-token", "key": "services-key"});

        let transport = PostmarkTransport::from_config(
            &json!({"token": "mailer-token", "key": "mailer-key", "message_stream_id": "outbound"}),
            &services,
        )
        .unwrap();
        assert_eq!(transport.token, "mailer-token");
        assert_eq!(transport.message_stream(), Some("outbound"));
        assert_eq!(
            transport.name(),
            "postmark+api://api.postmarkapp.com?message_stream=outbound"
        );

        let transport =
            PostmarkTransport::from_config(&json!({"key": "mailer-key"}), &services).unwrap();
        assert_eq!(transport.token, "mailer-key");

        let transport = PostmarkTransport::from_config(&json!({"token": ""}), &services).unwrap();
        assert_eq!(transport.token, "services-token");
        assert_eq!(transport.name(), "postmark+api://api.postmarkapp.com");

        let transport =
            PostmarkTransport::from_config(&json!({}), &json!({"key": "services-key"})).unwrap();
        assert_eq!(transport.token, "services-key");

        let error = PostmarkTransport::from_config(&json!({}), &Value::Null).unwrap_err();
        assert!(error.to_string().contains("requires a server token"));
        assert!(!format!("{transport:?}").contains("services-key"));
    }

    #[tokio::test]
    async fn client_options_are_applied_to_requests() {
        let _app = testing::container();
        Http::fake_using(|_| Http::response(accepted(), 200, &[]));
        let transport = PostmarkTransport::from_config(
            &json!({"token": "token", "client": {"timeout": 5, "headers": {"X-Client": "laravel"}}}),
            &Value::Null,
        )
        .unwrap();

        transport.send(&testing::message()).await.unwrap();

        assert!(testing::sent_request().has_header_value("X-Client", "laravel"));
    }
}
