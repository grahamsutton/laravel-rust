//! The building blocks of a mailable: its [`Envelope`], [`Content`] and
//! [`Headers`].

use std::fmt;
use std::sync::Arc;

use illuminate_support::{Map, Value, to_value};
use indexmap::IndexMap;
use serde::Serialize;

use crate::address::{Address, IntoAddresses};
use crate::message::Message;

/// A callback that customizes the underlying message just before it is sent
/// (Laravel's `withSymfonyMessage` / `Envelope::using`).
pub type MessageCallback = Arc<dyn Fn(&mut Message) + Send + Sync>;

/// The mailable's envelope: who it's from, who it's to, and its subject.
///
/// ```
/// use illuminate_mail::{Address, Envelope};
///
/// let envelope = Envelope::new()
///     .from(Address::new("jeffrey@example.com", "Jeffrey Way"))
///     .reply_to(("taylor@example.com", "Taylor Otwell"))
///     .subject("Order Shipped")
///     .tag("shipment")
///     .metadata("order_id", 42);
///
/// assert!(envelope.is_from("jeffrey@example.com", Some("Jeffrey Way")));
/// assert!(envelope.has_reply_to("taylor@example.com", None));
/// assert!(envelope.has_subject("Order Shipped"));
/// assert!(envelope.has_tag("shipment"));
/// assert!(envelope.has_metadata("order_id", "42"));
/// ```
#[derive(Clone, Default)]
pub struct Envelope {
    /// The address sending the message.
    pub from: Option<Address>,
    /// The recipients of the message.
    pub to: Vec<Address>,
    /// The recipients receiving a copy of the message.
    pub cc: Vec<Address>,
    /// The recipients receiving a blind copy of the message.
    pub bcc: Vec<Address>,
    /// The "reply to" recipients of the message.
    pub reply_to: Vec<Address>,
    /// The subject of the message.
    pub subject: Option<String>,
    /// The message's tags.
    pub tags: Vec<String>,
    /// The message's meta data.
    pub metadata: IndexMap<String, String>,
    /// The message's message callbacks.
    pub using: Vec<MessageCallback>,
}

impl fmt::Debug for Envelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Envelope")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("cc", &self.cc)
            .field("bcc", &self.bcc)
            .field("reply_to", &self.reply_to)
            .field("subject", &self.subject)
            .field("tags", &self.tags)
            .field("metadata", &self.metadata)
            .field("using", &self.using.len())
            .finish()
    }
}

impl Envelope {
    /// Create a new, empty envelope.
    pub fn new() -> Self {
        Self::default()
    }

    /// Specify who the message will be "from".
    pub fn from(mut self, address: impl Into<Address>) -> Self {
        self.from = Some(address.into());
        self
    }

    /// Add a "to" recipient to the message envelope.
    pub fn to(mut self, addresses: impl IntoAddresses) -> Self {
        self.to.extend(addresses.into_addresses());
        self
    }

    /// Add a "cc" recipient to the message envelope.
    pub fn cc(mut self, addresses: impl IntoAddresses) -> Self {
        self.cc.extend(addresses.into_addresses());
        self
    }

    /// Add a "bcc" recipient to the message envelope.
    pub fn bcc(mut self, addresses: impl IntoAddresses) -> Self {
        self.bcc.extend(addresses.into_addresses());
        self
    }

    /// Add a "reply to" recipient to the message envelope.
    pub fn reply_to(mut self, addresses: impl IntoAddresses) -> Self {
        self.reply_to.extend(addresses.into_addresses());
        self
    }

    /// Set the subject of the message.
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Add "tags" to the message.
    pub fn tags<I: IntoIterator<Item = S>, S: Into<String>>(mut self, tags: I) -> Self {
        self.tags.extend(tags.into_iter().map(Into::into));
        self
    }

    /// Add a "tag" to the message.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Add metadata to the message.
    pub fn metadata(mut self, key: impl Into<String>, value: impl ToString) -> Self {
        self.metadata.insert(key.into(), value.to_string());
        self
    }

    /// Add a message callback, run just before the message is sent.
    pub fn using(mut self, callback: impl Fn(&mut Message) + Send + Sync + 'static) -> Self {
        self.using.push(Arc::new(callback));
        self
    }

    /// Determine if the message is from the given address.
    pub fn is_from(&self, address: &str, name: Option<&str>) -> bool {
        self.from
            .as_ref()
            .is_some_and(|from| from.matches(address, name))
    }

    /// Determine if the message has the given address as a recipient.
    pub fn has_to(&self, address: &str, name: Option<&str>) -> bool {
        has_recipient(&self.to, address, name)
    }

    /// Determine if the message has the given address as a "cc" recipient.
    pub fn has_cc(&self, address: &str, name: Option<&str>) -> bool {
        has_recipient(&self.cc, address, name)
    }

    /// Determine if the message has the given address as a "bcc" recipient.
    pub fn has_bcc(&self, address: &str, name: Option<&str>) -> bool {
        has_recipient(&self.bcc, address, name)
    }

    /// Determine if the message has the given address as a "reply to" recipient.
    pub fn has_reply_to(&self, address: &str, name: Option<&str>) -> bool {
        has_recipient(&self.reply_to, address, name)
    }

    /// Determine if the message has the given subject.
    pub fn has_subject(&self, subject: &str) -> bool {
        self.subject.as_deref() == Some(subject)
    }

    /// Determine if the message has the given tag.
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// Determine if the message has the given metadata.
    pub fn has_metadata(&self, key: &str, value: &str) -> bool {
        self.metadata.get(key).is_some_and(|v| v == value)
    }
}

pub(crate) fn has_recipient(list: &[Address], address: &str, name: Option<&str>) -> bool {
    list.iter()
        .any(|recipient| recipient.matches(address, name))
}

/// The mailable's content: the view (or Markdown template) that renders it,
/// plus any extra data the view needs.
///
/// ```
/// use illuminate_mail::Content;
///
/// let content = Content::markdown("mail.orders.shipped")
///     .with("url", "https://example.com/orders/1");
///
/// assert_eq!(content.markdown.as_deref(), Some("mail.orders.shipped"));
/// assert_eq!(content.with["url"], "https://example.com/orders/1");
///
/// let content = Content::view("mail.orders.shipped").with_text("mail.orders.shipped-text");
/// assert_eq!(content.text.as_deref(), Some("mail.orders.shipped-text"));
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Content {
    /// The Blade view that should be rendered for the mailable.
    pub view: Option<String>,
    /// The Blade view that should be rendered for the mailable (alias of `view`).
    pub html: Option<String>,
    /// The Blade view that represents the text version of the message.
    pub text: Option<String>,
    /// The Blade view that represents the Markdown version of the message.
    pub markdown: Option<String>,
    /// The pre-rendered HTML of the message.
    pub html_string: Option<String>,
    /// The message's view data.
    pub with: Map<String, Value>,
}

impl Content {
    /// Create empty content.
    pub fn new() -> Self {
        Self::default()
    }

    /// Content rendered from a Blade view.
    pub fn view(view: impl Into<String>) -> Self {
        Self::new().with_view(view)
    }

    /// Content rendered from a Blade view (an alias of [`Content::view`]).
    pub fn html(view: impl Into<String>) -> Self {
        Self {
            html: Some(view.into()),
            ..Self::default()
        }
    }

    /// Plain-text content rendered from a Blade view.
    pub fn text(view: impl Into<String>) -> Self {
        Self::new().with_text(view)
    }

    /// Content rendered from a Markdown mail template.
    pub fn markdown(view: impl Into<String>) -> Self {
        Self::new().with_markdown(view)
    }

    /// Content from a pre-rendered HTML string.
    pub fn html_string(html: impl Into<String>) -> Self {
        Self {
            html_string: Some(html.into()),
            ..Self::default()
        }
    }

    /// Set the view for the message.
    pub fn with_view(mut self, view: impl Into<String>) -> Self {
        self.view = Some(view.into());
        self
    }

    /// Set the plain text view for the message.
    pub fn with_text(mut self, view: impl Into<String>) -> Self {
        self.text = Some(view.into());
        self
    }

    /// Set the Markdown view for the message.
    pub fn with_markdown(mut self, view: impl Into<String>) -> Self {
        self.markdown = Some(view.into());
        self
    }

    /// Set the pre-rendered HTML for the message.
    pub fn with_html_string(mut self, html: impl Into<String>) -> Self {
        self.html_string = Some(html.into());
        self
    }

    /// Add a piece of view data to the message.
    pub fn with(mut self, key: impl Into<String>, value: impl Serialize) -> Self {
        self.with.insert(key.into(), to_value(&value));
        self
    }

    /// Add many pieces of view data (any serializable map or struct).
    pub fn with_data(mut self, data: impl Serialize) -> Self {
        if let Value::Object(map) = to_value(&data) {
            self.with.extend(map);
        }
        self
    }
}

/// Custom headers for the mailable.
///
/// ```
/// use illuminate_mail::Headers;
///
/// let headers = Headers::new()
///     .message_id("custom-message-id@example.com")
///     .references(["previous-message@example.com"])
///     .text([("X-Custom-Header", "Custom Value")]);
///
/// assert_eq!(headers.references_string(), "<previous-message@example.com>");
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Headers {
    /// The message's message ID.
    pub message_id: Option<String>,
    /// The message IDs that are referenced by the message.
    pub references: Vec<String>,
    /// The message's text headers.
    pub text: IndexMap<String, String>,
}

impl Headers {
    /// Create an empty set of headers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the message ID.
    pub fn message_id(mut self, message_id: impl Into<String>) -> Self {
        self.message_id = Some(message_id.into());
        self
    }

    /// Add references to the header.
    pub fn references<I: IntoIterator<Item = S>, S: Into<String>>(mut self, references: I) -> Self {
        self.references
            .extend(references.into_iter().map(Into::into));
        self
    }

    /// Add text headers.
    pub fn text<I, K, V>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.text
            .extend(headers.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// Get a string representation of the message IDs that are referenced.
    pub fn references_string(&self) -> String {
        self.references
            .iter()
            .map(|id| {
                let id = if id.starts_with('<') {
                    id.clone()
                } else {
                    format!("<{id}")
                };
                if id.ends_with('>') {
                    id
                } else {
                    format!("{id}>")
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_collect_recipients() {
        let envelope = Envelope::new()
            .to(["a@example.com", "b@example.com"])
            .cc(("c@example.com", "C"))
            .bcc("d@example.com")
            .tags(["one", "two"])
            .using(|message| {
                message.subject("changed");
            });
        assert!(envelope.has_to("A@example.com", None));
        assert!(envelope.has_to("b@example.com", None));
        assert!(envelope.has_cc("c@example.com", Some("C")));
        assert!(!envelope.has_cc("c@example.com", Some("X")));
        assert!(envelope.has_bcc("d@example.com", None));
        assert!(envelope.has_tag("two"));
        assert!(!envelope.is_from("x@y.com", None));
        assert_eq!(envelope.using.len(), 1);
        assert!(format!("{envelope:?}").contains("a@example.com"));
    }

    #[test]
    fn content_collects_view_data() {
        #[derive(Serialize)]
        struct Data {
            name: &'static str,
        }
        let content = Content::view("emails.welcome")
            .with("count", 3)
            .with_data(Data { name: "Taylor" });
        assert_eq!(content.view.as_deref(), Some("emails.welcome"));
        assert_eq!(content.with["count"], 3);
        assert_eq!(content.with["name"], "Taylor");
        assert_eq!(
            Content::html_string("<p>Hi</p>").html_string.as_deref(),
            Some("<p>Hi</p>")
        );
        assert_eq!(Content::text("plain").text.as_deref(), Some("plain"));
        assert_eq!(Content::html("emails.x").html.as_deref(), Some("emails.x"));
    }

    #[test]
    fn headers_format_references() {
        let headers = Headers::new().references(["<a@x>", "b@x"]);
        assert_eq!(headers.references_string(), "<a@x> <b@x>");
    }
}
