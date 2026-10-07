//! Mail transports: how messages actually leave your application.
//!
//! The built-in transports are `smtp` ([`SmtpTransport`]), `sendmail`
//! ([`SendmailTransport`]), `log` ([`LogTransport`]), `array`
//! ([`ArrayTransport`]), `failover` ([`FailoverTransport`]) and `roundrobin`
//! ([`RoundRobinTransport`]), plus the HTTP API transports: `postmark`
//! ([`PostmarkTransport`]), `resend` ([`ResendTransport`]), `mailgun`
//! ([`MailgunTransport`]) and `ses` / `ses-v2` ([`SesTransport`]).
//!
//! The API transports send their requests with the `Http` facade, so
//! `Http::fake()` intercepts them in your tests. When a provider rejects a
//! message, the error is a [`TransportException`] carrying the provider's
//! response.
//!
//! Register your own transports with [`Mail::extend`](crate::Mail::extend):
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_mail::{Message, SentMessage, Transport, async_trait};
//! use illuminate_support::Result;
//!
//! struct NullTransport;
//!
//! #[async_trait]
//! impl Transport for NullTransport {
//!     async fn send(&self, message: &Message) -> Result<SentMessage> {
//!         Ok(SentMessage::new(message.clone()))
//!     }
//!
//!     fn name(&self) -> String {
//!         "null".into()
//!     }
//! }
//! ```

mod api;
mod aws;
mod failover;
mod mailgun;
mod postmark;
mod resend;
mod sendmail;
mod ses;
mod smtp;

use std::any::Any;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use illuminate_support::Result;

use crate::message::{Message, SentMessage};

pub use aws::{AwsCredentials, SignatureV4};
pub use failover::{FailoverTransport, RoundRobinTransport};
pub use mailgun::MailgunTransport;
pub use postmark::PostmarkTransport;
pub use resend::ResendTransport;
pub use sendmail::{DEFAULT_SENDMAIL_COMMAND, SendmailTransport};
pub use ses::SesTransport;
pub use smtp::SmtpTransport;

/// Thrown when a transport is unable to send a message (Symfony's
/// `TransportException`).
///
/// The HTTP API transports include the provider's error code (or the HTTP
/// status) and the [`Response`](illuminate_http_client::Response), so you
/// may inspect exactly what the provider said:
///
/// ```no_run
/// # async fn example() -> illuminate_support::Result<()> {
/// use illuminate_mail::{Mail, TransportException};
///
/// if let Err(error) = Mail::raw("Hi!", |message| { message.to("taylor@example.com"); }).await {
///     if let Some(exception) = error.downcast_ref::<TransportException>() {
///         println!("{} ({})", exception.message, exception.code);
///     }
/// }
/// # Ok(()) }
/// ```
#[derive(Clone, Debug)]
pub struct TransportException {
    /// The exception message.
    pub message: String,
    /// The provider's error code, or the HTTP status of its response.
    pub code: i64,
    /// The provider's response, when it answered.
    pub response: Option<illuminate_http_client::Response>,
}

impl TransportException {
    /// Create a new transport exception.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 0,
            response: None,
        }
    }

    /// Set the error code.
    pub fn with_code(mut self, code: i64) -> Self {
        self.code = code;
        self
    }

    /// Attach the provider's response.
    pub fn with_response(mut self, response: illuminate_http_client::Response) -> Self {
        self.response = Some(response);
        self
    }
}

impl std::fmt::Display for TransportException {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TransportException {}

/// Lets transports be downcast to their concrete type. Implemented
/// automatically for every transport.
pub trait AsAnyTransport: Any + Send + Sync {
    /// Convert the shared transport into a shared [`Any`].
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;
}

impl<T: Any + Send + Sync> AsAnyTransport for T {
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

/// A mail transport (Symfony's `TransportInterface`).
#[async_trait]
pub trait Transport: AsAnyTransport {
    /// Send the message.
    async fn send(&self, message: &Message) -> Result<SentMessage>;

    /// The transport's name, like `smtp://127.0.0.1:2525` or `array`.
    fn name(&self) -> String;
}

/// Downcast a shared transport to its concrete type.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_mail::{ArrayTransport, Transport, downcast_transport};
///
/// let transport: Arc<dyn Transport> = Arc::new(ArrayTransport::new());
/// assert!(downcast_transport::<ArrayTransport>(&transport).is_some());
/// ```
pub fn downcast_transport<T: Transport>(transport: &Arc<dyn Transport>) -> Option<Arc<T>> {
    transport.clone().into_any().downcast::<T>().ok()
}

/// Keeps every message in memory — perfect for tests.
///
/// ```
/// use illuminate_mail::{ArrayTransport, Message, Transport};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let transport = ArrayTransport::new();
/// let mut message = Message::new();
/// message.from("a@example.com").to("b@example.com").subject("Hi");
///
/// transport.send(&message).await.unwrap();
///
/// assert_eq!(transport.messages().len(), 1);
/// assert_eq!(transport.messages()[0].message.subject.as_deref(), Some("Hi"));
/// # });
/// ```
#[derive(Debug, Default)]
pub struct ArrayTransport {
    messages: Mutex<Vec<SentMessage>>,
}

impl ArrayTransport {
    /// Create a new array transport.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get all of the messages sent so far.
    pub fn messages(&self) -> Vec<SentMessage> {
        self.messages.lock().unwrap().clone()
    }

    /// Clear all of the messages from the local collection.
    pub fn flush(&self) -> Vec<SentMessage> {
        std::mem::take(&mut *self.messages.lock().unwrap())
    }
}

#[async_trait]
impl Transport for ArrayTransport {
    async fn send(&self, message: &Message) -> Result<SentMessage> {
        let sent = SentMessage::new(message.clone());
        self.messages.lock().unwrap().push(sent.clone());
        Ok(sent)
    }

    fn name(&self) -> String {
        "array".into()
    }
}

/// Writes every message to the log (at the `debug` level).
#[derive(Debug, Default)]
pub struct LogTransport {
    channel: Option<String>,
}

impl LogTransport {
    /// Log messages to the given channel (or the default channel).
    pub fn new(channel: Option<String>) -> Self {
        Self {
            channel: channel.filter(|c| !c.is_empty()),
        }
    }

    /// The log channel messages are written to.
    pub fn channel(&self) -> Option<&str> {
        self.channel.as_deref()
    }
}

#[async_trait]
impl Transport for LogTransport {
    async fn send(&self, message: &Message) -> Result<SentMessage> {
        let mime = String::from_utf8_lossy(&crate::mime::format(message)?).into_owned();
        let logger = illuminate_log::Log::manager().driver(self.channel.as_deref());
        logger.debug(decode_quoted_printable_parts(&mime));
        Ok(SentMessage::new(message.clone()))
    }

    fn name(&self) -> String {
        "log".into()
    }
}

/// Decode the quoted-printable parts of a MIME message so logged mail is
/// readable (like Laravel's `LogTransport`).
pub(crate) fn decode_quoted_printable_parts(mime: &str) -> String {
    const MARKER: &str = "Content-Transfer-Encoding: quoted-printable\r\n";
    let mut out = String::with_capacity(mime.len());
    let mut rest = mime;
    while let Some(index) = rest.find(MARKER) {
        let header_end = index + MARKER.len();
        let Some(body_start) = rest[header_end..]
            .find("\r\n\r\n")
            .map(|i| header_end + i + 4)
            .or_else(|| {
                rest[header_end..]
                    .starts_with("\r\n")
                    .then_some(header_end + 2)
            })
        else {
            break;
        };
        let body_end = rest[body_start..]
            .find("\r\n--")
            .map_or(rest.len(), |i| body_start + i);
        out.push_str(&rest[..body_start]);
        out.push_str(&decode_quoted_printable(&rest[body_start..body_end]));
        rest = &rest[body_end..];
    }
    out.push_str(rest);
    out
}

fn decode_quoted_printable(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            if bytes.get(i + 1) == Some(&b'\r') && bytes.get(i + 2) == Some(&b'\n') {
                i += 3;
                continue;
            }
            if bytes.get(i + 1) == Some(&b'\n') {
                i += 2;
                continue;
            }
            if let (Some(h), Some(l)) = (bytes.get(i + 1), bytes.get(i + 2))
                && let Ok(byte) = u8::from_str_radix(&format!("{}{}", *h as char, *l as char), 16)
            {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Fixtures shared by the transports' tests.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Arc;

    use illuminate_container::{Container, LocalInstanceGuard};
    use illuminate_http_client::{Http, Request};

    use crate::message::Message;

    /// A tiny PNG signature, standing in for an embedded logo.
    pub(crate) const PNG: &[u8] = &[137, 80, 78, 71];

    /// Install a fresh container, so each test gets its own `Http` fakes.
    pub(crate) fn container() -> LocalInstanceGuard {
        Container::set_local_instance(Arc::new(Container::new()))
    }

    /// The one request that was sent.
    pub(crate) fn sent_request() -> Request {
        let recorded = Http::recorded();
        assert_eq!(recorded.all().len(), 1, "Expected exactly one request.");
        recorded.all()[0].0.clone()
    }

    /// A message using everything an API transport needs to handle: names,
    /// cc, bcc, reply-to, both bodies, a priority, a custom header, a tag,
    /// metadata, an attachment and an inline image.
    pub(crate) fn message() -> Message {
        let mut message = Message::new();
        message
            .from(("hello@example.com", "Example"))
            .to(("taylor@example.com", "Taylor Otwell"))
            .to("abigail@example.com")
            .cc("james@example.com")
            .bcc("secret@example.com")
            .reply_to(("support@example.com", "Support"))
            .subject("Order Shipped")
            .text("Your order shipped.")
            .html("<p>Your order shipped.</p>")
            .priority(1)
            .header("X-Order", "1")
            .tag("shipment")
            .metadata("order_id", 1)
            .attach_data("id,total", "order.csv");
        message.embed_data(PNG.to_vec(), "logo.png", Some("image/png"));
        message.message_id = Some("abc@example.com".into());
        message
    }

    /// The content ID of the message's inline image.
    pub(crate) fn content_id(message: &Message) -> String {
        message
            .attachments
            .iter()
            .find_map(|attachment| attachment.content_id.clone())
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_printable_parts_are_decoded() {
        let mime = "Subject: Hi\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nGr=C3=BC=C3=9Fe, a very long line that was=\r\n wrapped\r\n--boundary--";
        let decoded = decode_quoted_printable_parts(mime);
        assert_eq!(
            decoded,
            "Subject: Hi\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nGrüße, a very long line that was wrapped\r\n--boundary--"
        );
        assert_eq!(decode_quoted_printable_parts("plain"), "plain");
    }

    #[tokio::test]
    async fn array_transports_can_be_flushed_and_downcast() {
        let transport: Arc<dyn Transport> = Arc::new(ArrayTransport::new());
        let mut message = Message::new();
        message.from("a@example.com").to("b@example.com");
        transport.send(&message).await.unwrap();
        let array = downcast_transport::<ArrayTransport>(&transport).unwrap();
        assert_eq!(array.flush().len(), 1);
        assert!(array.messages().is_empty());
        assert!(downcast_transport::<LogTransport>(&transport).is_none());
        assert_eq!(transport.name(), "array");
    }
}
