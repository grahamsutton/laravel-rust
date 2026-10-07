//! Turning a [`Message`] into an RFC 5322 / MIME document (using `lettre`).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use illuminate_support::Result;
use illuminate_support::error::RuntimeException;
use lettre::message::header::ContentType;
use lettre::message::{Attachment as LettreAttachment, Mailbox, MultiPart, SinglePart};

use crate::address::Address;
use crate::message::Message;

/// Random lowercase hexadecimal characters (`bytes` random bytes).
pub(crate) fn random_hex(bytes: usize) -> String {
    (0..bytes)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect()
}

/// Generate a message ID for a message (like Symfony: random bytes plus
/// the domain of the sender).
pub(crate) fn generate_message_id(message: &Message) -> String {
    let domain = message
        .envelope_sender()
        .map(Address::domain)
        .filter(|domain| !domain.is_empty())
        .unwrap_or("localhost");
    format!("{}@{domain}", random_hex(16))
}

fn lettre_address(address: &Address) -> Result<lettre::Address> {
    address.address.parse::<lettre::Address>().map_err(|e| {
        RuntimeException::new(format!(
            "Email \"{}\" does not comply with addr-spec of RFC 2822: {e}",
            address.address
        ))
        .into()
    })
}

fn mailbox(address: &Address) -> Result<Mailbox> {
    Ok(Mailbox::new(address.name.clone(), lettre_address(address)?))
}

/// The SMTP envelope for a message.
pub(crate) fn envelope(message: &Message) -> Result<lettre::address::Envelope> {
    let sender = message.envelope_sender().ok_or_else(|| {
        RuntimeException::new("An email must have a \"From\" or a \"Sender\" header.")
    })?;
    let recipients = message
        .recipients()
        .iter()
        .map(lettre_address)
        .collect::<Result<Vec<_>>>()?;
    if recipients.is_empty() {
        return Err(RuntimeException::new(
            "An email must have a \"To\", \"Cc\", or \"Bcc\" header.",
        )
        .into());
    }
    Ok(lettre::address::Envelope::new(
        Some(lettre_address(sender)?),
        recipients,
    )?)
}

enum Part {
    Single(SinglePart),
    Multi(MultiPart),
}

impl Part {
    fn add_to(self, builder: lettre::message::MultiPartBuilder) -> MultiPart {
        match self {
            Part::Single(part) => builder.singlepart(part),
            Part::Multi(part) => builder.multipart(part),
        }
    }

    fn push(self, multipart: MultiPart) -> MultiPart {
        match self {
            Part::Single(part) => multipart.singlepart(part),
            Part::Multi(part) => multipart.multipart(part),
        }
    }
}

fn content_type(mime: &str) -> ContentType {
    ContentType::parse(mime).unwrap_or_else(|_| {
        ContentType::parse("application/octet-stream").expect("a valid content type")
    })
}

/// Build the `lettre` message.
pub(crate) fn build(message: &Message) -> Result<lettre::Message> {
    if message.from.is_empty() && message.sender.is_none() {
        return Err(
            RuntimeException::new("An email must have a \"From\" or a \"Sender\" header.").into(),
        );
    }
    let mut builder = lettre::Message::builder();
    for from in &message.from {
        builder = builder.from(mailbox(from)?);
    }
    if let Some(sender) = &message.sender {
        builder = builder.sender(mailbox(sender)?);
    } else if message.from.len() > 1 {
        builder = builder.sender(mailbox(&message.from[0])?);
    }
    for address in &message.reply_to {
        builder = builder.reply_to(mailbox(address)?);
    }
    for address in &message.to {
        builder = builder.to(mailbox(address)?);
    }
    for address in &message.cc {
        builder = builder.cc(mailbox(address)?);
    }
    for address in &message.bcc {
        builder = builder.bcc(mailbox(address)?);
    }
    builder = builder.subject(message.subject.clone().unwrap_or_default());
    let id = message
        .message_id
        .clone()
        .unwrap_or_else(|| generate_message_id(message));
    builder = builder.message_id(Some(format!("<{}>", id.trim_matches(['<', '>']))));
    builder = builder.envelope(envelope(message)?);

    let body = match (&message.text, &message.html) {
        (Some(text), Some(html)) => Part::Multi(MultiPart::alternative_plain_html(
            text.clone(),
            html.clone(),
        )),
        (Some(text), None) => Part::Single(SinglePart::plain(text.clone())),
        (None, Some(html)) => Part::Single(SinglePart::html(html.clone())),
        (None, None) => Part::Single(SinglePart::plain(String::new())),
    };

    let (inline, attached): (Vec<_>, Vec<_>) =
        message.attachments.iter().partition(|a| a.is_inline());

    let body = if inline.is_empty() {
        body
    } else {
        let mut related = body.add_to(MultiPart::related());
        for attachment in inline {
            let part = LettreAttachment::new_inline_with_name(
                attachment.content_id.clone().unwrap_or_default(),
                attachment.filename.clone(),
            )
            .body(
                attachment.data.clone(),
                content_type(&attachment.content_type),
            );
            related = related.singlepart(part);
        }
        Part::Multi(related)
    };

    let body = if attached.is_empty() {
        body
    } else {
        let mut mixed = body.add_to(MultiPart::mixed());
        for attachment in attached {
            let part = LettreAttachment::new(attachment.filename.clone()).body(
                attachment.data.clone(),
                content_type(&attachment.content_type),
            );
            mixed = Part::Single(part).push(mixed);
        }
        Part::Multi(mixed)
    };

    let built = match body {
        Part::Single(part) => builder.singlepart(part),
        Part::Multi(part) => builder.multipart(part),
    };
    built.map_err(|e| RuntimeException::new(format!("Unable to build the email: {e}")).into())
}

/// The headers `lettre` doesn't model directly (tags, metadata, priority,
/// custom text headers), in order.
fn extra_headers(message: &Message) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    if let Some(return_path) = &message.return_path {
        headers.push((
            "Return-Path".to_string(),
            format!("<{}>", return_path.address),
        ));
    }
    if let Some(priority) = message.priority {
        let label = match priority {
            1 => "Highest",
            2 => "High",
            3 => "Normal",
            4 => "Low",
            _ => "Lowest",
        };
        headers.push(("X-Priority".to_string(), format!("{priority} ({label})")));
    }
    for tag in &message.tags {
        headers.push(("X-Tag".to_string(), tag.clone()));
    }
    for (key, value) in &message.metadata {
        headers.push((format!("X-Metadata-{key}"), value.clone()));
    }
    headers.extend(message.headers.iter().cloned());
    headers
}

/// Encode a header value, using RFC 2047 when it isn't plain ASCII.
fn encode_header_value(value: &str) -> String {
    let value = value.replace(['\r', '\n'], " ");
    if value.is_ascii() {
        value
    } else {
        format!("=?utf-8?b?{}?=", STANDARD.encode(value.as_bytes()))
    }
}

fn is_valid_header_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_graphic() && b != b':')
}

/// Format the message as RFC 5322 bytes.
pub(crate) fn format(message: &Message) -> Result<Vec<u8>> {
    let built = build(message)?;
    let mut out = Vec::new();
    for (name, value) in extra_headers(message) {
        if !is_valid_header_name(&name) {
            return Err(RuntimeException::new(format!("Invalid header name [{name}].")).into());
        }
        out.extend_from_slice(format!("{name}: {}\r\n", encode_header_value(&value)).as_bytes());
    }
    out.extend_from_slice(&built.formatted());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachment::MessageAttachment;

    fn message() -> Message {
        let mut message = Message::new();
        message
            .from(("hello@example.com", "Example"))
            .to("taylor@example.com")
            .bcc("secret@example.com")
            .subject("Hello");
        message
    }

    #[test]
    fn plain_messages_are_formatted() {
        let mut message = message();
        message
            .text("Hi Taylor")
            .tag("welcome")
            .metadata("user_id", 1)
            .priority(1);
        message.header("X-Custom", "Grüße");
        let mime = String::from_utf8(format(&message).unwrap()).unwrap();
        assert!(mime.contains("From: Example <hello@example.com>"));
        assert!(mime.contains("To: taylor@example.com"));
        assert!(
            !mime.contains("secret@example.com"),
            "bcc must not be in the headers"
        );
        assert!(mime.contains("Subject: Hello"));
        assert!(mime.contains("X-Tag: welcome\r\n"));
        assert!(mime.contains("X-Metadata-user_id: 1\r\n"));
        assert!(mime.contains("X-Priority: 1 (Highest)\r\n"));
        assert!(mime.contains("X-Custom: =?utf-8?b?"));
        assert!(mime.contains("Message-ID: <"));
        assert!(mime.contains("Hi Taylor"));

        let envelope = envelope(&message).unwrap();
        assert_eq!(envelope.to().len(), 2);
    }

    #[test]
    fn multipart_messages_include_every_part() {
        let mut message = message();
        message.text("plain").html("<p>html</p>");
        message.message_id = Some("fixed@example.com".into());
        message.attachments.push(MessageAttachment {
            filename: "report.csv".into(),
            content_type: "text/csv".into(),
            data: b"a,b".to_vec(),
            content_id: None,
        });
        message.attachments.push(MessageAttachment {
            filename: "logo.png".into(),
            content_type: "image/png".into(),
            data: vec![1, 2, 3],
            content_id: Some("logo@laravel".into()),
        });
        let mime = String::from_utf8(format(&message).unwrap()).unwrap();
        assert!(mime.contains("Message-ID: <fixed@example.com>"));
        assert!(mime.contains("multipart/mixed"));
        assert!(mime.contains("multipart/related"));
        assert!(mime.contains("multipart/alternative"));
        assert!(mime.contains("filename=\"report.csv\""));
        assert!(mime.contains("Content-ID: <logo@laravel>"));
    }

    #[test]
    fn invalid_messages_are_rejected() {
        let mut message = Message::new();
        message.to("taylor@example.com");
        assert!(
            format(&message)
                .unwrap_err()
                .to_string()
                .contains("\"From\"")
        );

        let mut message = Message::new();
        message.from("hello@example.com");
        assert!(format(&message).unwrap_err().to_string().contains("\"To\""));

        let mut message = Message::new();
        message.from("not an address").to("taylor@example.com");
        assert!(
            format(&message)
                .unwrap_err()
                .to_string()
                .contains("addr-spec")
        );

        let mut message = self::message();
        message.header("Bad Name", "x");
        assert!(
            format(&message)
                .unwrap_err()
                .to_string()
                .contains("Invalid header")
        );
    }

    #[test]
    fn message_ids_use_the_sender_domain() {
        let id = generate_message_id(&message());
        assert!(id.ends_with("@example.com"));
        assert_eq!(id.len(), 32 + "@example.com".len());
    }
}
