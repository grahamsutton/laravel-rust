//! The SMTP transport.

use std::time::Duration;

use async_trait::async_trait;
use illuminate_support::error::RuntimeException;
use illuminate_support::{Result, Value, ValueExt};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use super::Transport;
use crate::message::{Message, SentMessage};

/// Sends mail through an SMTP server (using `lettre`).
///
/// Configured like Laravel's `smtp` mailer:
///
/// ```
/// use illuminate_mail::{SmtpTransport, Transport};
/// use illuminate_support::json;
///
/// let transport = SmtpTransport::from_config(&json!({
///     "transport": "smtp",
///     "host": "127.0.0.1",
///     "port": 2525,
///     "username": null,
///     "password": null,
///     "timeout": null,
///     "local_domain": "example.com",
/// })).unwrap();
///
/// assert_eq!(transport.name(), "smtp://127.0.0.1:2525");
/// ```
///
/// The `smtps` scheme (or port 465) uses implicit TLS; `smtp` upgrades the
/// connection with `STARTTLS` when the server supports it (set
/// `"auto_tls": false` to disable that, or `"require_tls": true` to
/// insist on it).
pub struct SmtpTransport {
    inner: AsyncSmtpTransport<Tokio1Executor>,
    scheme: String,
    host: String,
    port: u16,
}

impl std::fmt::Debug for SmtpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpTransport")
            .field("scheme", &self.scheme)
            .field("host", &self.host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

fn string(config: &Value, key: &str) -> Option<String> {
    config
        .get(key)
        .filter(|v| !v.is_null())
        .map(ValueExt::to_string_lossy)
        .filter(|v| !v.is_empty())
}

fn flag(config: &Value, key: &str, default: bool) -> bool {
    match config.get(key) {
        None | Some(Value::Null) => default,
        Some(Value::String(s)) => {
            matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes")
        }
        Some(other) => other.truthy(),
    }
}

impl SmtpTransport {
    /// Create the transport from a mailer configuration.
    pub fn from_config(config: &Value) -> Result<Self> {
        let host = string(config, "host").unwrap_or_else(|| "127.0.0.1".to_string());
        let port = config
            .get("port")
            .and_then(ValueExt::to_i64_lossy)
            .filter(|port| *port > 0);
        let scheme = string(config, "scheme")
            .unwrap_or_else(|| if port == Some(465) { "smtps" } else { "smtp" }.to_string());
        let port = port.unwrap_or(if scheme == "smtps" { 465 } else { 25 }) as u16;

        let mut builder = match scheme.as_str() {
            "smtps" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host)
                .map_err(|e| RuntimeException::new(format!("Invalid SMTP host [{host}]: {e}")))?,
            "smtp" => {
                let builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host.clone());
                if flag(config, "auto_tls", true) {
                    let parameters = TlsParameters::new(host.clone())
                        .map_err(|e| RuntimeException::new(format!("Invalid SMTP host [{host}]: {e}")))?;
                    builder.tls(if flag(config, "require_tls", false) {
                        Tls::Required(parameters)
                    } else {
                        Tls::Opportunistic(parameters)
                    })
                } else {
                    builder
                }
            }
            other => {
                return Err(RuntimeException::new(format!(
                    "The \"{other}\" scheme is not supported; supported schemes for mailer \"smtp\" are: \"smtp\", \"smtps\"."
                ))
                .into());
            }
        }
        .port(port);

        if let Some(username) = string(config, "username") {
            builder = builder.credentials(Credentials::new(
                username,
                string(config, "password").unwrap_or_default(),
            ));
        }
        if let Some(timeout) = config
            .get("timeout")
            .and_then(ValueExt::to_f64_lossy)
            .filter(|t| *t > 0.0)
        {
            builder = builder.timeout(Some(Duration::from_secs_f64(timeout)));
        }
        if let Some(domain) = string(config, "local_domain") {
            builder = builder.hello_name(ClientId::Domain(domain));
        }

        Ok(Self {
            inner: builder.build(),
            scheme,
            host,
            port,
        })
    }

    /// Test the connection to the SMTP server.
    pub async fn test_connection(&self) -> Result<bool> {
        Ok(self.inner.test_connection().await?)
    }
}

#[async_trait]
impl Transport for SmtpTransport {
    async fn send(&self, message: &Message) -> Result<SentMessage> {
        let envelope = crate::mime::envelope(message)?;
        let bytes = crate::mime::format(message)?;
        let response = self.inner.send_raw(&envelope, &bytes).await.map_err(|e| {
            RuntimeException::new(format!("Failed to send the message via SMTP: {e}"))
        })?;
        let mut sent = SentMessage::new(message.clone());
        sent.debug = format!(
            "{} {}",
            response.code(),
            response.message().collect::<Vec<_>>().join(" ")
        );
        Ok(sent)
    }

    fn name(&self) -> String {
        format!("{}://{}:{}", self.scheme, self.host, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn smtp_transports_are_configured() {
        let transport =
            SmtpTransport::from_config(&json!({"host": "mail.example.com", "port": 465})).unwrap();
        assert_eq!(transport.name(), "smtps://mail.example.com:465");

        let transport = SmtpTransport::from_config(&json!({
            "host": "mail.example.com",
            "port": "587",
            "username": "user",
            "password": "secret",
            "timeout": 5,
            "auto_tls": false,
        }))
        .unwrap();
        assert_eq!(transport.name(), "smtp://mail.example.com:587");

        let transport = SmtpTransport::from_config(&json!({})).unwrap();
        assert_eq!(transport.name(), "smtp://127.0.0.1:25");

        let error = SmtpTransport::from_config(&json!({"scheme": "pop3"})).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("\"pop3\" scheme is not supported")
        );
        assert!(format!("{transport:?}").contains("127.0.0.1"));
    }

    #[tokio::test]
    async fn unreachable_servers_fail_to_send() {
        // Nothing listens on port 1 of the loopback interface.
        let transport = SmtpTransport::from_config(&json!({
            "host": "127.0.0.1",
            "port": 1,
            "auto_tls": false,
            "timeout": 2,
        }))
        .unwrap();
        let mut message = Message::new();
        message.from("a@example.com").to("b@example.com").text("Hi");
        let error = transport.send(&message).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Failed to send the message via SMTP")
        );
        assert!(!matches!(transport.test_connection().await, Ok(true)));
    }

    /// A tiny SMTP server that accepts one message and returns the DATA it received.
    async fn fake_smtp_server() -> (u16, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let mut transcript = Vec::new();
            let mut in_data = false;
            writer.write_all(b"220 localhost ESMTP\r\n").await.unwrap();
            while let Ok(Some(line)) = lines.next_line().await {
                if in_data {
                    if line == "." {
                        in_data = false;
                        writer
                            .write_all(b"250 2.0.0 Ok: queued as 12345\r\n")
                            .await
                            .unwrap();
                    } else {
                        transcript.push(line);
                    }
                    continue;
                }
                let command = line.to_ascii_uppercase();
                transcript.push(line.clone());
                let reply: &[u8] = if command.starts_with("EHLO") {
                    b"250-localhost\r\n250 8BITMIME\r\n"
                } else if command.starts_with("DATA") {
                    in_data = true;
                    b"354 End data with <CR><LF>.<CR><LF>\r\n"
                } else if command.starts_with("QUIT") {
                    writer.write_all(b"221 Bye\r\n").await.unwrap();
                    break;
                } else {
                    b"250 Ok\r\n"
                };
                writer.write_all(reply).await.unwrap();
            }
            transcript
        });
        (port, handle)
    }

    #[tokio::test]
    async fn messages_are_delivered_over_smtp() {
        let (port, server) = fake_smtp_server().await;
        let transport = SmtpTransport::from_config(&json!({
            "host": "127.0.0.1",
            "port": port,
            "local_domain": "example.com",
            "timeout": 5,
        }))
        .unwrap();
        let mut message = Message::new();
        message
            .from(("hello@example.com", "Example"))
            .to("taylor@example.com")
            .bcc("secret@example.com")
            .subject("Hello")
            .text("Hello over SMTP");
        message.message_id = Some("smtp-test@example.com".into());

        let sent = transport.send(&message).await.unwrap();
        assert_eq!(sent.message_id(), "smtp-test@example.com");
        assert!(sent.debug().starts_with("250"), "{}", sent.debug());
        drop(transport);

        let transcript = server.await.unwrap();
        assert!(transcript.iter().any(|l| l == "EHLO example.com"));
        assert!(
            transcript
                .iter()
                .any(|l| l.starts_with("MAIL FROM:<hello@example.com>"))
        );
        assert!(
            transcript
                .iter()
                .any(|l| l.starts_with("RCPT TO:<taylor@example.com>"))
        );
        assert!(
            transcript
                .iter()
                .any(|l| l.starts_with("RCPT TO:<secret@example.com>"))
        );
        assert!(transcript.iter().any(|l| l == "Subject: Hello"));
        assert!(transcript.iter().any(|l| l == "Hello over SMTP"));
        assert!(!transcript.iter().any(|l| l.starts_with("Bcc:")));
    }
}
