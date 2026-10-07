//! The Resend API transport.

use async_trait::async_trait;
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Map, Result, Value, ValueExt, json};

use super::Transport;
use super::api;
use crate::message::{Message, SentMessage};

/// Sends mail through [Resend's](https://resend.com) email API (the `resend`
/// mailer transport).
///
/// The API key comes from the mailer's `key`, falling back to
/// `services.resend.key`:
///
/// ```
/// use illuminate_mail::{ResendTransport, Transport};
///
/// let transport = ResendTransport::new("re_123456789");
///
/// assert_eq!(transport.name(), "resend");
/// ```
///
/// Metadata is sent as Resend tags (`{"name": key, "value": value}`), and
/// each of the message's tags as a tag with a `true` value. The ID Resend
/// returns becomes the sent message's ID, and is added to the sent message
/// as its `X-Resend-Email-ID` header.
#[derive(Clone)]
pub struct ResendTransport {
    key: String,
    client: Value,
}

impl std::fmt::Debug for ResendTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResendTransport").finish_non_exhaustive()
    }
}

impl ResendTransport {
    /// The Resend API endpoint for sending email.
    pub const ENDPOINT: &'static str = "https://api.resend.com/emails";

    /// Create a transport sending with the given API key.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            client: Value::Null,
        }
    }

    /// Create the transport from a mailer's configuration and the
    /// `services.resend` configuration.
    pub fn from_config(config: &Value, services: &Value) -> Result<Self> {
        let key = api::filled(config, "key")
            .or_else(|| api::filled(services, "key"))
            .ok_or_else(|| {
                InvalidArgumentException::new(
                    "The Resend mailer requires an API key. Set the mailer's [key] or [services.resend.key].",
                )
            })?;
        Ok(Self::new(key).with_client_options(config.get("client").cloned()))
    }

    /// Set the HTTP client options (the mailer's `client`, like `{"timeout": 5}`).
    pub fn with_client_options(mut self, options: impl Into<Option<Value>>) -> Self {
        self.client = options.into().unwrap_or(Value::Null);
        self
    }

    /// Build the API payload for a message.
    pub(crate) fn payload(&self, message: &Message) -> Result<Value> {
        let headers: Map<String, Value> = api::custom_headers(message)
            .into_iter()
            .map(|(name, value)| (name, Value::String(value)))
            .collect();

        let attachments: Vec<Value> = message
            .attachments
            .iter()
            .map(|attachment| {
                let content = if attachment.content_type == "text/calendar" {
                    attachment.text()
                } else {
                    api::base64(&attachment.data)
                };
                let mut item = json!({
                    "content_type": attachment.content_type,
                    "content": content,
                    "filename": attachment.filename,
                });
                if let Some(id) = &attachment.content_id {
                    item["content_id"] = Value::String(id.clone());
                }
                item
            })
            .collect();

        let mut payload = json!({
            "from": api::mailbox(api::from(message)?),
            "to": api::mailboxes(&message.to),
            "cc": api::mailboxes(&message.cc),
            "bcc": api::mailboxes(&message.bcc),
            "reply_to": api::mailboxes(&message.reply_to),
            "headers": headers,
            "subject": message.subject,
            "html": message.html,
            "text": message.text,
            "attachments": attachments,
        });

        let tags: Vec<Value> = message
            .tags
            .iter()
            .map(|tag| json!({"name": tag, "value": "true"}))
            .chain(
                message
                    .metadata
                    .iter()
                    .map(|(key, value)| json!({"name": key, "value": value})),
            )
            .collect();
        if !tags.is_empty() {
            payload["tags"] = Value::Array(tags);
        }
        Ok(payload)
    }
}

#[async_trait]
impl Transport for ResendTransport {
    async fn send(&self, message: &Message) -> Result<SentMessage> {
        let payload = self.payload(message)?;
        let response = api::request(&self.client)
            .with_token(&self.key, "Bearer")
            .post(Self::ENDPOINT, payload)
            .await
            .map_err(|e| {
                let reason = e.to_string();
                api::unreachable(
                    e,
                    format!("Request to Resend API failed. Reason: {reason}."),
                )
            })?;

        let status = response.status();
        let result = response.json();
        if !response.successful() {
            let reason = ["message", "error"]
                .iter()
                .find_map(|key| result.get(*key).filter(|reason| !reason.is_blank()))
                .map(ValueExt::to_string_lossy)
                .unwrap_or_else(|| response.body().into_owned());
            return Err(api::failed(
                format!("Request to Resend API failed. Reason: {reason}."),
                status.into(),
                response,
            ));
        }

        let id = result["id"].to_string_lossy();
        let mut sent = SentMessage::new(message.clone());
        sent.message.header("X-Resend-Email-ID", id.clone());
        sent.message_id = id;
        sent.debug = response.body().into_owned();
        Ok(sent)
    }

    fn name(&self) -> String {
        "resend".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportException;
    use crate::transport::testing::{self, PNG};
    use illuminate_http_client::Http;

    #[tokio::test]
    async fn messages_are_sent_through_the_resend_api() {
        let _app = testing::container();
        Http::fake_using(|_| {
            Http::response(
                json!({"id": "49a3999c-0ce1-4ea6-ab68-afcd6dc2e794"}),
                200,
                &[],
            )
        });
        let message = testing::message();

        let sent = ResendTransport::new("re_123").send(&message).await.unwrap();

        assert_eq!(sent.message_id(), "49a3999c-0ce1-4ea6-ab68-afcd6dc2e794");
        assert_eq!(
            sent.message.get_header("X-Resend-Email-ID"),
            Some("49a3999c-0ce1-4ea6-ab68-afcd6dc2e794")
        );

        let request = testing::sent_request();
        assert_eq!(request.method(), "POST");
        assert_eq!(request.url(), "https://api.resend.com/emails");
        assert!(request.has_header_value("Authorization", "Bearer re_123"));
        assert!(request.is_json());
        assert_eq!(
            request.data(),
            &json!({
                "from": "\"Example\" <hello@example.com>",
                "to": ["\"Taylor Otwell\" <taylor@example.com>", "abigail@example.com"],
                "cc": ["james@example.com"],
                "bcc": ["secret@example.com"],
                "reply_to": ["\"Support\" <support@example.com>"],
                "headers": {"X-Priority": "1 (Highest)", "X-Order": "1"},
                "subject": "Order Shipped",
                "html": "<p>Your order shipped.</p>",
                "text": "Your order shipped.",
                "attachments": [
                    {"content_type": "text/csv", "content": api::base64(b"id,total"), "filename": "order.csv"},
                    {
                        "content_type": "image/png",
                        "content": api::base64(PNG),
                        "filename": "logo.png",
                        "content_id": testing::content_id(&message),
                    },
                ],
                "tags": [
                    {"name": "shipment", "value": "true"},
                    {"name": "order_id", "value": "1"},
                ],
            })
        );
    }

    #[test]
    fn calendar_invites_are_sent_as_text_and_tags_are_optional() {
        let mut message = Message::new();
        message
            .from("hello@example.com")
            .to("taylor@example.com")
            .attach_data("BEGIN:VCALENDAR", "invite.ics");
        message.attachments[0].content_type = "text/calendar".into();

        let payload = ResendTransport::new("key").payload(&message).unwrap();

        assert_eq!(payload["attachments"][0]["content"], "BEGIN:VCALENDAR");
        assert_eq!(payload["headers"], json!({}));
        assert!(payload["subject"].is_null());
        assert!(payload.get("tags").is_none());
    }

    #[tokio::test]
    async fn api_errors_are_reported_like_laravel() {
        let _app = testing::container();
        Http::fake_sequence("api.resend.com/*")
            .push(
                json!({"statusCode": 422, "message": "Invalid `to` field.", "name": "validation_error"}),
                422,
            )
            .push("Internal Server Error", 500)
            .push_failed_connection_with("Connection refused");
        let transport = ResendTransport::new("key");
        let message = testing::message();

        let error = transport.send(&message).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Request to Resend API failed. Reason: Invalid `to` field.."
        );
        let exception = error.downcast_ref::<TransportException>().unwrap();
        assert_eq!(exception.code, 422);
        assert_eq!(
            exception.response.as_ref().unwrap()["name"],
            "validation_error"
        );

        let error = transport.send(&message).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Request to Resend API failed. Reason: Internal Server Error."
        );

        let error = transport.send(&message).await.unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Request to Resend API failed. Reason: ")
        );
        assert!(error.to_string().contains("Connection refused"));
        assert!(error.downcast_ref::<TransportException>().is_some());
    }

    #[test]
    fn keys_are_resolved_from_the_mailer_or_services() {
        let services = json!({"key": "services-key"});
        let transport =
            ResendTransport::from_config(&json!({"key": "mailer-key"}), &services).unwrap();
        assert_eq!(transport.key, "mailer-key");
        let transport =
            ResendTransport::from_config(&json!({"transport": "resend"}), &services).unwrap();
        assert_eq!(transport.key, "services-key");
        assert!(!format!("{transport:?}").contains("services-key"));

        let error = ResendTransport::from_config(&json!({}), &json!({"key": null})).unwrap_err();
        assert!(error.to_string().contains("requires an API key"));
    }
}
