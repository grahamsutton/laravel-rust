//! The Mailgun API transport.

use async_trait::async_trait;
use illuminate_http_client::Part;
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Result, Value, ValueExt};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use super::Transport;
use super::api;
use crate::message::{Message, SentMessage};

/// What PHP's `urlencode` leaves alone in the domain.
const DOMAIN: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.');

/// Sends mail through [Mailgun's](https://www.mailgun.com) API (the
/// `mailgun` mailer transport), posting the full MIME message to
/// `https://{endpoint}/v3/{domain}/messages.mime`.
///
/// The `domain`, `secret` and `endpoint` come from the mailer's
/// configuration when it has a `secret`, and from `services.mailgun`
/// otherwise. Mailgun's EU region lives at the `api.eu.mailgun.net` endpoint:
///
/// ```
/// use illuminate_mail::{MailgunTransport, Transport};
///
/// let transport = MailgunTransport::new("key-secret", "mg.example.com")
///     .with_endpoint("api.eu.mailgun.net");
///
/// assert_eq!(transport.name(), "mailgun+https://api.eu.mailgun.net?domain=mg.example.com");
/// ```
///
/// Tags are sent as `o:tag` and metadata as `v:` variables; the ID Mailgun
/// returns becomes the sent message's ID.
#[derive(Clone)]
pub struct MailgunTransport {
    secret: String,
    domain: String,
    endpoint: String,
    client: Value,
}

impl std::fmt::Debug for MailgunTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailgunTransport")
            .field("domain", &self.domain)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl MailgunTransport {
    /// The default Mailgun API endpoint (the US region).
    pub const ENDPOINT: &'static str = "api.mailgun.net";

    /// Create a transport sending for the domain with the given API key.
    pub fn new(secret: impl Into<String>, domain: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            domain: domain.into(),
            endpoint: Self::ENDPOINT.to_string(),
            client: Value::Null,
        }
    }

    /// Create the transport from a mailer's configuration and the
    /// `services.mailgun` configuration.
    pub fn from_config(config: &Value, services: &Value) -> Result<Self> {
        let credentials = if api::filled(config, "secret").is_some() {
            config
        } else {
            services
        };
        let missing = |key: &str| {
            InvalidArgumentException::new(format!(
                "The Mailgun mailer requires a [{key}]. Set it on the mailer or in [services.mailgun.{key}]."
            ))
        };
        let secret = api::filled(credentials, "secret").ok_or_else(|| missing("secret"))?;
        let domain = api::filled(credentials, "domain").ok_or_else(|| missing("domain"))?;
        let transport =
            Self::new(secret, domain).with_client_options(config.get("client").cloned());
        Ok(match api::filled(credentials, "endpoint") {
            Some(endpoint) => transport.with_endpoint(endpoint),
            None => transport,
        })
    }

    /// Send through the given API endpoint, like `api.eu.mailgun.net`.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        let endpoint = endpoint.into();
        if !endpoint.is_empty() && endpoint != "default" {
            self.endpoint = endpoint.trim_end_matches('/').to_string();
        }
        self
    }

    /// Set the HTTP client options (the mailer's `client`, like `{"timeout": 5}`).
    pub fn with_client_options(mut self, options: impl Into<Option<Value>>) -> Self {
        self.client = options.into().unwrap_or(Value::Null);
        self
    }

    /// The domain messages are sent for.
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// The API endpoint.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The URL messages are posted to.
    pub fn url(&self) -> String {
        let base = if self.endpoint.contains("://") {
            self.endpoint.clone()
        } else {
            format!("https://{}", self.endpoint)
        };
        format!(
            "{base}/v3/{}/messages.mime",
            utf8_percent_encode(&self.domain, DOMAIN)
        )
    }
}

#[async_trait]
impl Transport for MailgunTransport {
    async fn send(&self, message: &Message) -> Result<SentMessage> {
        // Tags and metadata travel as API options rather than as headers.
        let mut mime = message.clone();
        mime.tags.clear();
        mime.metadata.clear();

        let mut request = api::request(&self.client)
            .with_basic_auth("api", &self.secret)
            .attach_part(Part::new("to", api::recipients(message).join(",")))
            .attach_part(
                Part::new("message", crate::mime::format(&mime)?)
                    .filename("message.mime")
                    .header("Content-Type", "application/octet-stream"),
            );
        for tag in &message.tags {
            request = request.attach_part(Part::new("o:tag", tag.clone()));
        }
        for (key, value) in &message.metadata {
            request = request.attach_part(Part::new(format!("v:{key}"), value.clone()));
        }

        let response = request
            .post(self.url(), ())
            .await
            .map_err(|e| api::unreachable(e, "Could not reach the remote Mailgun server."))?;

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
            return Err(api::failed(
                format!(
                    "Unable to send an email: {} (code {status}).",
                    result["message"].to_string_lossy()
                ),
                status.into(),
                response,
            ));
        }

        let mut sent = SentMessage::new(message.clone());
        sent.message_id = result["id"]
            .to_string_lossy()
            .trim_matches(['<', '>'])
            .to_string();
        sent.debug = response.body().into_owned();
        Ok(sent)
    }

    fn name(&self) -> String {
        format!("mailgun+https://{}?domain={}", self.endpoint, self.domain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportException;
    use crate::transport::testing;
    use illuminate_http_client::Http;
    use illuminate_support::json;

    fn part<'a>(request: &'a illuminate_http_client::Request, name: &str) -> Vec<&'a Part> {
        request
            .parts()
            .iter()
            .filter(|part| part.name == name)
            .collect()
    }

    #[tokio::test]
    async fn messages_are_posted_as_mime_to_the_mailgun_api() {
        let _app = testing::container();
        Http::fake_using(|_| {
            Http::response(
                json!({"id": "<20111114174239.25659.5817@mg.example.com>", "message": "Queued. Thank you."}),
                200,
                &[],
            )
        });
        let message = testing::message();

        let sent = MailgunTransport::new("key-secret", "mg.example.com")
            .send(&message)
            .await
            .unwrap();

        assert_eq!(
            sent.message_id(),
            "20111114174239.25659.5817@mg.example.com"
        );

        let request = testing::sent_request();
        assert_eq!(request.method(), "POST");
        assert_eq!(
            request.url(),
            "https://api.mailgun.net/v3/mg.example.com/messages.mime"
        );
        assert!(request.has_header_value(
            "Authorization",
            &format!("Basic {}", api::base64(b"api:key-secret"))
        ));
        assert!(request.is_multipart());
        assert!(request.has_file_with(
            "to",
            Some("taylor@example.com,abigail@example.com,james@example.com,secret@example.com"),
            None
        ));
        assert_eq!(part(&request, "o:tag")[0].contents, "shipment");
        assert_eq!(part(&request, "v:order_id")[0].contents, "1");

        let mime = part(&request, "message")[0];
        assert_eq!(mime.filename.as_deref(), Some("message.mime"));
        let mime = String::from_utf8_lossy(&mime.contents);
        assert!(mime.contains("Subject: Order Shipped"));
        assert!(mime.contains("Message-ID: <abc@example.com>"));
        assert!(mime.contains("X-Order: 1"));
        assert!(mime.contains("filename=\"order.csv\""));
        assert!(mime.contains(&format!("Content-ID: <{}>", testing::content_id(&message))));
        assert!(
            !mime.contains("secret@example.com"),
            "Bcc must not be in the MIME"
        );
        assert!(!mime.contains("X-Tag") && !mime.contains("X-Metadata"));
    }

    #[tokio::test]
    async fn api_errors_become_transport_exceptions() {
        let _app = testing::container();
        Http::fake_sequence("api.eu.mailgun.net/*")
            .push(json!({"message": "'from' parameter is missing"}), 400)
            .push("Forbidden", 401)
            .push_failed_connection_with("Connection refused");
        let transport =
            MailgunTransport::new("key", "mg.example.com").with_endpoint("api.eu.mailgun.net");
        let message = testing::message();

        let error = transport.send(&message).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Unable to send an email: 'from' parameter is missing (code 400)."
        );
        let exception = error.downcast_ref::<TransportException>().unwrap();
        assert_eq!(exception.code, 400);
        assert!(exception.response.is_some());

        let error = transport.send(&message).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Unable to send an email: Forbidden (code 401)."
        );

        let error = transport.send(&message).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Could not reach the remote Mailgun server."
        );
        assert!(format!("{error:#}").contains("Connection refused"));
    }

    #[test]
    fn credentials_come_from_the_mailer_or_services() {
        let services = json!({
            "domain": "mg.example.com",
            "secret": "services-secret",
            "endpoint": "api.eu.mailgun.net",
            "scheme": "https",
        });

        let transport =
            MailgunTransport::from_config(&json!({"transport": "mailgun"}), &services).unwrap();
        assert_eq!(transport.secret, "services-secret");
        assert_eq!(transport.domain(), "mg.example.com");
        assert_eq!(transport.endpoint(), "api.eu.mailgun.net");
        assert_eq!(
            transport.url(),
            "https://api.eu.mailgun.net/v3/mg.example.com/messages.mime"
        );
        assert_eq!(
            transport.name(),
            "mailgun+https://api.eu.mailgun.net?domain=mg.example.com"
        );
        assert!(!format!("{transport:?}").contains("services-secret"));

        // A mailer with its own secret doesn't read the services configuration at all.
        let transport = MailgunTransport::from_config(
            &json!({"secret": "mailer-secret", "domain": "mail.example.com"}),
            &services,
        )
        .unwrap();
        assert_eq!(transport.secret, "mailer-secret");
        assert_eq!(transport.endpoint(), "api.mailgun.net");

        let error =
            MailgunTransport::from_config(&json!({"secret": "secret"}), &services).unwrap_err();
        assert!(error.to_string().contains("requires a [domain]"));
        let error = MailgunTransport::from_config(&json!({}), &Value::Null).unwrap_err();
        assert!(error.to_string().contains("requires a [secret]"));
    }

    #[test]
    fn endpoints_may_include_a_scheme() {
        let transport = MailgunTransport::new("key", "my domain.com")
            .with_endpoint("http://localhost:8025/")
            .with_endpoint("default");
        assert_eq!(
            transport.url(),
            "http://localhost:8025/v3/my%20domain.com/messages.mime"
        );
    }
}
