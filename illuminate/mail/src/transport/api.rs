//! Helpers shared by the HTTP API transports (Postmark, Resend, Mailgun
//! and Amazon SES).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use illuminate_http_client::{Http, PendingRequest, Response};
use illuminate_support::{Error, Result, Value, ValueExt};

use super::TransportException;
use crate::address::Address;
use crate::message::Message;

/// Headers the APIs model themselves, so they're never sent as custom headers.
const HEADERS_TO_BYPASS: &[&str] = &[
    "from",
    "to",
    "cc",
    "bcc",
    "subject",
    "content-type",
    "sender",
    "reply-to",
];

/// A non-blank configuration value, as a string.
pub(crate) fn filled(config: &Value, key: &str) -> Option<String> {
    config
        .get(key)
        .filter(|value| !value.is_blank())
        .map(ValueExt::to_string_lossy)
}

/// Begin a request (through the `Http` facade, so it may be faked) with
/// the mailer's `client` options, like `{"timeout": 5}`.
pub(crate) fn request(client: &Value) -> PendingRequest {
    match client {
        Value::Object(_) => Http::with_options(client.clone()),
        _ => Http::new_request(),
    }
}

/// Format an address for an API payload, like Symfony: `"Name" <email>`.
pub(crate) fn mailbox(address: &Address) -> String {
    match address.name.as_deref().filter(|name| !name.is_empty()) {
        Some(name) => format!("\"{}\" <{}>", name.replace('"', "\\\""), address.address),
        None => address.address.clone(),
    }
}

/// Format addresses for an API payload.
pub(crate) fn mailboxes(addresses: &[Address]) -> Vec<String> {
    addresses.iter().map(mailbox).collect()
}

/// The address the message is from.
pub(crate) fn from(message: &Message) -> Result<&Address> {
    message
        .from
        .first()
        .or(message.sender.as_ref())
        .ok_or_else(|| {
            TransportException::new("An email must have a \"From\" or a \"Sender\" header.").into()
        })
}

/// The plain email addresses of every recipient (to, cc and bcc).
pub(crate) fn recipients(message: &Message) -> Vec<String> {
    message
        .recipients()
        .into_iter()
        .map(|address| address.address)
        .collect()
}

/// The message's own headers, as an API receives them: the priority and
/// any custom headers, minus the ones the API models itself.
pub(crate) fn custom_headers(message: &Message) -> Vec<(String, String)> {
    message
        .priority
        .map(|priority| {
            (
                "X-Priority".to_string(),
                crate::mime::priority_header(priority),
            )
        })
        .into_iter()
        .chain(message.headers.iter().cloned())
        .filter(|(name, _)| !HEADERS_TO_BYPASS.contains(&name.to_ascii_lowercase().as_str()))
        .collect()
}

/// Base64 encode attachment contents.
pub(crate) fn base64(data: &[u8]) -> String {
    STANDARD.encode(data)
}

/// The error for a provider that couldn't be reached at all.
pub(crate) fn unreachable(error: Error, message: impl Into<String>) -> Error {
    error.context(TransportException::new(message))
}

/// The error for a provider that answered with a failure.
pub(crate) fn failed(message: impl Into<String>, code: i64, response: Response) -> Error {
    TransportException::new(message)
        .with_code(code)
        .with_response(response)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn addresses_are_formatted_like_symfony() {
        assert_eq!(
            mailbox(&Address::new("taylor@example.com", "Taylor \"T\" Otwell")),
            "\"Taylor \\\"T\\\" Otwell\" <taylor@example.com>"
        );
        assert_eq!(mailbox(&Address::new("a@example.com", "")), "a@example.com");
        assert_eq!(mailbox(&Address::email("a@example.com")), "a@example.com");
    }

    #[test]
    fn custom_headers_skip_the_ones_apis_model() {
        let mut message = Message::new();
        message
            .priority(2)
            .header("X-Custom", "yes")
            .header("Reply-To", "ignored@example.com")
            .header("content-type", "text/plain");
        assert_eq!(
            custom_headers(&message),
            vec![
                ("X-Priority".to_string(), "2 (High)".to_string()),
                ("X-Custom".to_string(), "yes".to_string()),
            ]
        );
    }

    #[test]
    fn configuration_values_must_be_filled() {
        let config = json!({"token": "abc", "blank": "", "null": null, "port": 25});
        assert_eq!(filled(&config, "token").as_deref(), Some("abc"));
        assert_eq!(filled(&config, "port").as_deref(), Some("25"));
        assert!(filled(&config, "blank").is_none());
        assert!(filled(&config, "null").is_none());
        assert!(filled(&config, "missing").is_none());
        assert!(from(&Message::new()).is_err());
    }
}
