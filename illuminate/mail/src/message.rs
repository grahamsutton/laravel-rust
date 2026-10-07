//! The email message itself, and the receipt for a sent message.

use std::fmt;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::address::{Address, IntoAddresses};
use crate::attachment::{Attachment, MessageAttachment, guess_mime};
use crate::mailables::has_recipient;

/// An email message (Laravel's `Illuminate\Mail\Message`).
///
/// This is what `Mail::raw` callbacks, `Envelope::using` callbacks and
/// transports receive. Its setters return `&mut Self` so calls chain:
///
/// ```
/// use illuminate_mail::Message;
///
/// let mut message = Message::new();
/// message
///     .from(("hello@example.com", "Example"))
///     .to("taylor@example.com")
///     .cc(["abigail@example.com", "james@example.com"])
///     .subject("Welcome!")
///     .priority(1)
///     .text("Hello there.");
///
/// assert!(message.has_to("taylor@example.com"));
/// assert_eq!(message.cc.len(), 2);
/// assert_eq!(message.subject.as_deref(), Some("Welcome!"));
/// ```
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Message {
    /// The "from" addresses.
    pub from: Vec<Address>,
    /// The "sender" address.
    pub sender: Option<Address>,
    /// The "return path" (bounce) address.
    pub return_path: Option<Address>,
    /// The "reply to" addresses.
    pub reply_to: Vec<Address>,
    /// The recipients.
    pub to: Vec<Address>,
    /// The "cc" recipients.
    pub cc: Vec<Address>,
    /// The "bcc" recipients.
    pub bcc: Vec<Address>,
    /// The subject.
    pub subject: Option<String>,
    /// The priority, from 1 (highest) to 5 (lowest).
    pub priority: Option<u8>,
    /// The HTML body.
    pub html: Option<String>,
    /// The plain-text body.
    pub text: Option<String>,
    /// The attachments (including inline, embedded files).
    pub attachments: Vec<MessageAttachment>,
    /// Additional text headers.
    pub headers: Vec<(String, String)>,
    /// Tags (sent as `X-Tag` headers).
    pub tags: Vec<String>,
    /// Metadata (sent as `X-Metadata-*` headers).
    pub metadata: IndexMap<String, String>,
    /// The message ID (without angle brackets).
    pub message_id: Option<String>,
    /// Attachments that are read when the message is sent.
    #[serde(skip)]
    pub(crate) pending: Vec<Attachment>,
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Message")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("cc", &self.cc)
            .field("bcc", &self.bcc)
            .field("reply_to", &self.reply_to)
            .field("subject", &self.subject)
            .field("attachments", &self.attachments)
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

impl Message {
    /// Create an empty message.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the "from" address of the message (replacing any previous one).
    pub fn from(&mut self, address: impl Into<Address>) -> &mut Self {
        self.from = vec![address.into()];
        self
    }

    /// Set the "sender" of the message.
    pub fn sender(&mut self, address: impl Into<Address>) -> &mut Self {
        self.sender = Some(address.into());
        self
    }

    /// Set the "return path" of the message.
    pub fn return_path(&mut self, address: impl Into<Address>) -> &mut Self {
        self.return_path = Some(address.into());
        self
    }

    /// Add recipients to the message.
    pub fn to(&mut self, addresses: impl IntoAddresses) -> &mut Self {
        self.to.extend(addresses.into_addresses());
        self
    }

    /// Remove all "to" addresses from the message.
    pub fn forget_to(&mut self) -> &mut Self {
        self.to.clear();
        self
    }

    /// Add carbon copies to the message.
    pub fn cc(&mut self, addresses: impl IntoAddresses) -> &mut Self {
        self.cc.extend(addresses.into_addresses());
        self
    }

    /// Remove all carbon copy addresses from the message.
    pub fn forget_cc(&mut self) -> &mut Self {
        self.cc.clear();
        self
    }

    /// Add blind carbon copies to the message.
    pub fn bcc(&mut self, addresses: impl IntoAddresses) -> &mut Self {
        self.bcc.extend(addresses.into_addresses());
        self
    }

    /// Remove all of the blind carbon copy addresses from the message.
    pub fn forget_bcc(&mut self) -> &mut Self {
        self.bcc.clear();
        self
    }

    /// Add "reply to" addresses to the message.
    pub fn reply_to(&mut self, addresses: impl IntoAddresses) -> &mut Self {
        self.reply_to.extend(addresses.into_addresses());
        self
    }

    /// Set the subject of the message.
    pub fn subject(&mut self, subject: impl Into<String>) -> &mut Self {
        self.subject = Some(subject.into());
        self
    }

    /// Set the message priority level (1 is the highest, 5 the lowest).
    pub fn priority(&mut self, level: u8) -> &mut Self {
        self.priority = Some(level.clamp(1, 5));
        self
    }

    /// Set the HTML body of the message.
    pub fn html(&mut self, html: impl Into<String>) -> &mut Self {
        self.html = Some(html.into());
        self
    }

    /// Set the plain-text body of the message.
    pub fn text(&mut self, text: impl Into<String>) -> &mut Self {
        self.text = Some(text.into());
        self
    }

    /// Add a text header to the message.
    pub fn header(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Get the first value of a text header.
    pub fn get_header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Add a tag to the message.
    pub fn tag(&mut self, tag: impl Into<String>) -> &mut Self {
        self.tags.push(tag.into());
        self
    }

    /// Add metadata to the message.
    pub fn metadata(&mut self, key: impl Into<String>, value: impl ToString) -> &mut Self {
        self.metadata.insert(key.into(), value.to_string());
        self
    }

    /// Attach a file to the message: a path, or any [`Attachment`].
    /// Files are read when the message is sent.
    pub fn attach(&mut self, attachment: impl Into<Attachment>) -> &mut Self {
        self.pending.push(attachment.into());
        self
    }

    /// Attach in-memory data as a document to the message.
    pub fn attach_data(&mut self, data: impl Into<Vec<u8>>, name: impl Into<String>) -> &mut Self {
        let name = name.into();
        self.attachments.push(MessageAttachment {
            content_type: guess_mime(&name),
            filename: name,
            data: data.into(),
            content_id: None,
        });
        self
    }

    /// Embed a file in the message and get the CID (`cid:...`) to use as
    /// an image source.
    pub fn embed(&mut self, attachment: impl Into<Attachment>) -> String {
        let id = content_id();
        self.pending.push(attachment.into().inline(id.clone()));
        format!("cid:{id}")
    }

    /// Embed in-memory data in the message and get the CID.
    pub fn embed_data(
        &mut self,
        data: impl Into<Vec<u8>>,
        name: impl Into<String>,
        content_type: Option<&str>,
    ) -> String {
        let id = content_id();
        let name = name.into();
        self.attachments.push(MessageAttachment {
            content_type: content_type.map_or_else(|| guess_mime(&name), str::to_string),
            filename: name,
            data: data.into(),
            content_id: Some(id.clone()),
        });
        format!("cid:{id}")
    }

    /// The attachments still waiting to be read (from paths and storage).
    pub fn pending_attachments(&self) -> &[Attachment] {
        &self.pending
    }

    /// Read every pending attachment into the message.
    pub async fn resolve_attachments(&mut self) -> illuminate_support::Result<()> {
        for attachment in std::mem::take(&mut self.pending) {
            self.attachments.push(attachment.resolve().await?);
        }
        Ok(())
    }

    /// Determine if the message is from the given address.
    pub fn has_from(&self, address: &str) -> bool {
        has_recipient(&self.from, address, None)
    }

    /// Determine if the message has the given recipient.
    pub fn has_to(&self, address: &str) -> bool {
        has_recipient(&self.to, address, None)
    }

    /// Determine if the message has the given "cc" recipient.
    pub fn has_cc(&self, address: &str) -> bool {
        has_recipient(&self.cc, address, None)
    }

    /// Determine if the message has the given "bcc" recipient.
    pub fn has_bcc(&self, address: &str) -> bool {
        has_recipient(&self.bcc, address, None)
    }

    /// Determine if the message has the given "reply to" address.
    pub fn has_reply_to(&self, address: &str) -> bool {
        has_recipient(&self.reply_to, address, None)
    }

    /// Every recipient of the message (to, cc and bcc).
    pub fn recipients(&self) -> Vec<Address> {
        self.to
            .iter()
            .chain(&self.cc)
            .chain(&self.bcc)
            .cloned()
            .collect()
    }

    /// The address bounces go to: the return path, the sender, or the first "from".
    pub fn envelope_sender(&self) -> Option<&Address> {
        self.return_path
            .as_ref()
            .or(self.sender.as_ref())
            .or(self.from.first())
    }

    /// Format the message as an RFC 5322 (MIME) string.
    pub fn to_mime_string(&self) -> illuminate_support::Result<String> {
        Ok(String::from_utf8_lossy(&crate::mime::format(self)?).into_owned())
    }
}

/// A random content ID for embedded attachments.
fn content_id() -> String {
    format!("{}@laravel", crate::mime::random_hex(16))
}

/// The "envelope" a message was actually delivered with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentEnvelope {
    /// The address bounces are sent to.
    pub sender: Address,
    /// Everyone the message was delivered to.
    pub recipients: Vec<Address>,
}

/// A message that was sent, as returned by the transport.
///
/// ```no_run
/// # async fn example() -> illuminate_support::Result<()> {
/// use illuminate_mail::Mail;
///
/// let sent = Mail::raw("Hi!", |message| {
///     message.to("taylor@example.com").subject("Hello");
/// })
/// .await?;
///
/// if let Some(sent) = sent {
///     println!("Sent {}", sent.message_id());
/// }
/// # Ok(()) }
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SentMessage {
    /// The message that was sent.
    pub message: Message,
    /// The message ID.
    pub message_id: String,
    /// The delivery envelope.
    pub envelope: SentEnvelope,
    /// Debug output from the transport (the SMTP conversation, say).
    pub debug: String,
}

impl SentMessage {
    /// Create a sent message for the given message.
    pub fn new(message: Message) -> Self {
        let message_id = message.message_id.clone().unwrap_or_default();
        let envelope = SentEnvelope {
            sender: message.envelope_sender().cloned().unwrap_or_default(),
            recipients: message.recipients(),
        };
        Self {
            message,
            message_id,
            envelope,
            debug: String::new(),
        }
    }

    /// Get the message ID.
    pub fn message_id(&self) -> &str {
        &self.message_id
    }

    /// Get the original message.
    pub fn original_message(&self) -> &Message {
        &self.message
    }

    /// Get the transport's debug output.
    pub fn debug(&self) -> &str {
        &self.debug
    }

    /// The message as an RFC 5322 (MIME) string.
    pub fn to_mime_string(&self) -> illuminate_support::Result<String> {
        self.message.to_mime_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_collect_addresses_and_headers() {
        let mut message = Message::new();
        message
            .from("a@example.com")
            .from(("b@example.com", "B"))
            .return_path("bounce@example.com")
            .to(["x@example.com", "y@example.com"])
            .bcc("z@example.com")
            .reply_to("r@example.com")
            .header("X-Mailer", "Laravel")
            .tag("welcome")
            .metadata("user_id", 1)
            .priority(9);
        assert_eq!(message.from, vec![Address::new("b@example.com", "B")]);
        assert_eq!(message.priority, Some(5));
        assert_eq!(message.get_header("x-mailer"), Some("Laravel"));
        assert_eq!(message.recipients().len(), 3);
        assert_eq!(
            message.envelope_sender().unwrap().address,
            "bounce@example.com"
        );
        assert!(message.has_reply_to("r@example.com"));
        assert!(message.has_from("b@example.com"));
        assert!(message.has_bcc("z@example.com"));
        message.forget_to().forget_bcc().forget_cc();
        assert!(message.recipients().is_empty());
    }

    #[tokio::test]
    async fn attachments_and_embeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logo.png");
        std::fs::write(&path, [137, 80, 78, 71]).unwrap();

        let mut message = Message::new();
        let cid = message.embed(path.as_path());
        let data_cid = message.embed_data(b"<svg/>".to_vec(), "logo.svg", Some("image/svg+xml"));
        message
            .attach(path.as_path())
            .attach_data("a,b", "data.csv");
        assert!(cid.starts_with("cid:"));
        assert_ne!(cid, data_cid);
        assert_eq!(message.pending_attachments().len(), 2);

        message.resolve_attachments().await.unwrap();
        assert!(message.pending_attachments().is_empty());
        assert_eq!(message.attachments.len(), 4);
        let inline = message.attachments.iter().filter(|a| a.is_inline()).count();
        assert_eq!(inline, 2);
        assert!(
            message
                .attachments
                .iter()
                .any(|a| a.filename == "data.csv" && a.content_type == "text/csv")
        );
    }

    #[test]
    fn sent_messages_know_their_envelope() {
        let mut message = Message::new();
        message
            .from("from@example.com")
            .to("to@example.com")
            .cc("cc@example.com");
        message.message_id = Some("abc@example.com".into());
        let sent = SentMessage::new(message);
        assert_eq!(sent.message_id(), "abc@example.com");
        assert_eq!(sent.envelope.sender.address, "from@example.com");
        assert_eq!(sent.envelope.recipients.len(), 2);
        assert_eq!(sent.original_message().to[0].address, "to@example.com");
    }
}
