//! Mail transports: how messages actually leave your application.
//!
//! The built-in transports are `smtp` ([`SmtpTransport`]), `sendmail`
//! ([`SendmailTransport`]), `log` ([`LogTransport`]), `array`
//! ([`ArrayTransport`]), `failover` ([`FailoverTransport`]) and `roundrobin`
//! ([`RoundRobinTransport`]). Register your own with
//! [`Mail::extend`](crate::Mail::extend):
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

mod failover;
mod sendmail;
mod smtp;

use std::any::Any;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use illuminate_support::Result;

use crate::message::{Message, SentMessage};

pub use failover::{FailoverTransport, RoundRobinTransport};
pub use sendmail::{DEFAULT_SENDMAIL_COMMAND, SendmailTransport};
pub use smtp::SmtpTransport;

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
