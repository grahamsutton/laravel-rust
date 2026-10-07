//! Mailables: each type of email your application sends is a type
//! implementing [`Mailable`].

use std::any::Any;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use illuminate_http::{IntoResponse, Response};
use illuminate_support::{Map, Result, Str, Value, class_basename, e, to_value};
use illuminate_view::{Factory, IntoViewData, ViewData};
use indexmap::IndexMap;
use serde::Serialize;

use crate::address::{Address, IntoAddresses};
use crate::attachment::Attachment;
use crate::mailables::{Content, Envelope, Headers, MessageCallback, has_recipient};
use crate::markdown::Markdown;
use crate::message::Message;
use crate::view_message::Embeds;

/// What every mailable gets for free from `#[derive(Serialize)]`: its
/// fields become view data (Laravel passes a mailable's public properties to
/// its view), and it can be identified and downcast in tests.
///
/// This is implemented automatically for every serializable type; mark
/// fields your views shouldn't see with `#[serde(skip)]`.
pub trait MailableData: Send + Sync + 'static {
    /// The mailable's fields, as view data.
    fn mailable_data(&self) -> Value;

    /// The mailable as [`Any`], for downcasting.
    fn as_any(&self) -> &dyn Any;

    /// The mailable's (short) type name, like `OrderShipped`.
    fn mailable_name(&self) -> String;
}

impl<T: Serialize + Send + Sync + 'static> MailableData for T {
    fn mailable_data(&self) -> Value {
        to_value(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn mailable_name(&self) -> String {
        class_basename::<T>()
    }
}

/// An email your application sends.
///
/// Describe the message with [`envelope`](Mailable::envelope),
/// [`content`](Mailable::content), [`attachments`](Mailable::attachments)
/// and [`headers`](Mailable::headers). The mailable's serialized fields are
/// available to its view, so `{{ $order['id'] }}` (or `{{ $order->id }}`)
/// just works:
///
/// ```
/// use illuminate_mail::{Address, Attachment, Content, Envelope, Mailable};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct Order { id: u64, total: f64 }
///
/// #[derive(Serialize)]
/// struct OrderShipped { order: Order }
///
/// impl Mailable for OrderShipped {
///     fn envelope(&self) -> Envelope {
///         Envelope::new()
///             .from(Address::new("jeffrey@example.com", "Jeffrey Way"))
///             .subject("Order Shipped")
///     }
///
///     fn content(&self) -> Content {
///         Content::markdown("mail.orders.shipped").with("url", format!("https://example.com/orders/{}", self.order.id))
///     }
///
///     fn attachments(&self) -> Vec<Attachment> {
///         vec![Attachment::from_data(|| "id,total\n1,10.00", "order.csv")]
///     }
/// }
///
/// let mailable = OrderShipped { order: Order { id: 1, total: 10.0 } };
/// mailable.assert_from("jeffrey@example.com");
/// mailable.assert_has_subject("Order Shipped");
/// mailable.assert_has_attached_data("id,total\n1,10.00", "order.csv");
/// ```
///
/// Mailables that should be queued return `true` from
/// [`should_queue`](Mailable::should_queue).
pub trait Mailable: MailableData {
    /// Get the message envelope.
    fn envelope(&self) -> Envelope {
        Envelope::new()
    }

    /// Get the message content definition.
    fn content(&self) -> Content {
        Content::new()
    }

    /// Get the attachments for the message.
    fn attachments(&self) -> Vec<Attachment> {
        Vec::new()
    }

    /// Get the message headers.
    fn headers(&self) -> Headers {
        Headers::new()
    }

    /// Build the message fluently (Laravel's `build` method), the
    /// alternative to `envelope` and `content`.
    fn build(&self, _mail: &mut MailableBuilder) {}

    /// Whether the mailable should be queued instead of sent right away
    /// (Laravel's `ShouldQueue` interface).
    fn should_queue(&self) -> bool {
        false
    }

    /// The locale the mailable should be rendered in.
    fn locale(&self) -> Option<String> {
        None
    }

    /// The name of the mailer that should send the mailable.
    fn mailer(&self) -> Option<String> {
        None
    }

    /// The Markdown theme to render the mailable with.
    fn theme(&self) -> Option<String> {
        None
    }

    /// The queue connection a queued mailable should use.
    fn queue_connection(&self) -> Option<String> {
        None
    }

    /// The queue a queued mailable should be pushed onto.
    fn queue_name(&self) -> Option<String> {
        None
    }

    /// How long a queued mailable should wait before being sent.
    fn queue_delay(&self) -> Option<Duration> {
        None
    }

    // ------------------------------------------------------------------
    // Rendering & previewing
    // ------------------------------------------------------------------

    /// Render the mailable into HTML (or plain text, for text-only mail).
    fn render(&self) -> Result<String>
    where
        Self: Sized,
    {
        render_mailable(self, &SendOptions::default()).map(|rendered| rendered.preview())
    }

    /// Render the plain-text version of the mailable.
    fn render_text(&self) -> Result<String>
    where
        Self: Sized,
    {
        render_mailable(self, &SendOptions::default())
            .map(|rendered| rendered.text.unwrap_or_default())
    }

    /// Preview the mailable in the browser: return it from a route.
    ///
    /// ```ignore
    /// Route::get("/mailable", || async {
    ///     let invoice = Invoice::find(1).await?;
    ///     Ok(InvoicePaid { invoice }.preview())
    /// });
    /// ```
    fn preview(&self) -> MailPreview
    where
        Self: Sized,
    {
        MailPreview(self.render())
    }

    /// Build the mailable's message definition (envelope, content,
    /// attachments and headers, merged).
    fn prepare(&self) -> MailableBuilder
    where
        Self: Sized,
    {
        MailableBuilder::for_mailable(self, &SendOptions::default())
    }

    // ------------------------------------------------------------------
    // Inspecting
    // ------------------------------------------------------------------

    /// Determine if the mailable is from the given address.
    fn has_from(&self, address: &str) -> bool
    where
        Self: Sized,
    {
        has_recipient(&self.prepare().from, address, None)
    }

    /// Determine if the mailable has the given recipient.
    fn has_to(&self, address: &str) -> bool
    where
        Self: Sized,
    {
        has_recipient(&self.prepare().to, address, None)
    }

    /// Determine if the mailable has the given "cc" recipient.
    fn has_cc(&self, address: &str) -> bool
    where
        Self: Sized,
    {
        has_recipient(&self.prepare().cc, address, None)
    }

    /// Determine if the mailable has the given "bcc" recipient.
    fn has_bcc(&self, address: &str) -> bool
    where
        Self: Sized,
    {
        has_recipient(&self.prepare().bcc, address, None)
    }

    /// Determine if the mailable has the given "reply to" address.
    fn has_reply_to(&self, address: &str) -> bool
    where
        Self: Sized,
    {
        has_recipient(&self.prepare().reply_to, address, None)
    }

    /// Determine if the mailable has the given subject.
    fn has_subject(&self, subject: &str) -> bool
    where
        Self: Sized,
    {
        self.prepare().subject_or_default(&self.mailable_name()) == subject
    }

    /// Determine if the mailable has the given tag.
    fn has_tag(&self, tag: &str) -> bool
    where
        Self: Sized,
    {
        self.prepare().tags.iter().any(|t| t == tag)
    }

    /// Determine if the mailable has the given metadata.
    fn has_metadata(&self, key: &str, value: &str) -> bool
    where
        Self: Sized,
    {
        self.prepare().metadata.get(key).is_some_and(|v| v == value)
    }

    /// Determine if the mailable has the given attachment.
    fn has_attachment(&self, attachment: impl Into<Attachment>) -> bool
    where
        Self: Sized,
    {
        let expected = attachment.into();
        self.prepare()
            .attachments
            .iter()
            .any(|actual| actual.is_equivalent(&expected))
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    /// Assert that the mailable is from the given address.
    fn assert_from(&self, address: &str)
    where
        Self: Sized,
    {
        let actual = format_addresses(&self.prepare().from);
        assert!(
            self.has_from(address),
            "Email was not from expected address.\nExpected: [{address}]\nActual: [{actual}]"
        );
    }

    /// Assert that the mailable has the given recipient.
    fn assert_has_to(&self, address: &str)
    where
        Self: Sized,
    {
        let actual = format_addresses(&self.prepare().to);
        assert!(
            self.has_to(address),
            "Did not see expected recipient in email 'to' recipients.\nExpected: [{address}]\nActual: [{actual}]"
        );
    }

    /// Assert that the mailable has the given recipient (alias of
    /// [`assert_has_to`](Mailable::assert_has_to)).
    fn assert_to(&self, address: &str)
    where
        Self: Sized,
    {
        self.assert_has_to(address);
    }

    /// Assert that the mailable has the given "cc" recipient.
    fn assert_has_cc(&self, address: &str)
    where
        Self: Sized,
    {
        let actual = format_addresses(&self.prepare().cc);
        assert!(
            self.has_cc(address),
            "Did not see expected recipient in email 'cc' recipients.\nExpected: [{address}]\nActual: [{actual}]"
        );
    }

    /// Assert that the mailable has the given "bcc" recipient.
    fn assert_has_bcc(&self, address: &str)
    where
        Self: Sized,
    {
        let actual = format_addresses(&self.prepare().bcc);
        assert!(
            self.has_bcc(address),
            "Did not see expected recipient in email 'bcc' recipients.\nExpected: [{address}]\nActual: [{actual}]"
        );
    }

    /// Assert that the mailable has the given "reply to" address.
    fn assert_has_reply_to(&self, address: &str)
    where
        Self: Sized,
    {
        let actual = format_addresses(&self.prepare().reply_to);
        assert!(
            self.has_reply_to(address),
            "Did not see expected address as email 'reply to' recipient.\nExpected: [{address}]\nActual: [{actual}]"
        );
    }

    /// Assert that the mailable has the given subject.
    fn assert_has_subject(&self, subject: &str)
    where
        Self: Sized,
    {
        let actual = self.prepare().subject_or_default(&self.mailable_name());
        assert!(
            actual == subject,
            "Email subject does not match expected value.\nExpected: [{subject}]\nActual: [{actual}]"
        );
    }

    /// Assert that the given text is present in the HTML email body.
    fn assert_see_in_html(&self, text: &str)
    where
        Self: Sized,
    {
        let expected = e(text);
        let html = self.render().expect("The mailable could not be rendered");
        assert!(
            html.contains(&expected),
            "Did not see expected text [{expected}] within email body."
        );
    }

    /// Assert that the given text is not present in the HTML email body.
    fn assert_dont_see_in_html(&self, text: &str)
    where
        Self: Sized,
    {
        let unexpected = e(text);
        let html = self.render().expect("The mailable could not be rendered");
        assert!(
            !html.contains(&unexpected),
            "Saw unexpected text [{unexpected}] within email body."
        );
    }

    /// Assert that the given strings are present, in order, in the HTML email body.
    fn assert_see_in_order_in_html(&self, texts: &[&str])
    where
        Self: Sized,
    {
        let html = self.render().expect("The mailable could not be rendered");
        let escaped: Vec<String> = texts.iter().map(e).collect();
        assert_see_in_order(&html, &escaped);
    }

    /// Assert that the given text is present in the plain-text email body.
    fn assert_see_in_text(&self, text: &str)
    where
        Self: Sized,
    {
        let body = self
            .render_text()
            .expect("The mailable could not be rendered");
        assert!(
            body.contains(text),
            "Did not see expected text [{text}] within text email body."
        );
    }

    /// Assert that the given text is not present in the plain-text email body.
    fn assert_dont_see_in_text(&self, text: &str)
    where
        Self: Sized,
    {
        let body = self
            .render_text()
            .expect("The mailable could not be rendered");
        assert!(
            !body.contains(text),
            "Saw unexpected text [{text}] within text email body."
        );
    }

    /// Assert that the given strings are present, in order, in the plain-text email body.
    fn assert_see_in_order_in_text(&self, texts: &[&str])
    where
        Self: Sized,
    {
        let body = self
            .render_text()
            .expect("The mailable could not be rendered");
        let texts: Vec<String> = texts.iter().map(|t| t.to_string()).collect();
        assert_see_in_order(&body, &texts);
    }

    /// Assert that the mailable has no attachments.
    fn assert_has_no_attachments(&self)
    where
        Self: Sized,
    {
        let count = self.prepare().attachments.len();
        assert!(
            count == 0,
            "Expected no attachments, but found [{count}] attachment(s)."
        );
    }

    /// Assert that the mailable has the given attachment.
    fn assert_has_attachment(&self, attachment: impl Into<Attachment>)
    where
        Self: Sized,
    {
        assert!(
            self.has_attachment(attachment),
            "Did not find the expected attachment."
        );
    }

    /// Assert that the mailable has the given data as an attachment.
    fn assert_has_attached_data(&self, data: impl Into<Vec<u8>>, name: &str)
    where
        Self: Sized,
    {
        assert!(
            self.has_attachment(Attachment::from_bytes(data, name)),
            "Did not find the expected attachment."
        );
    }

    /// Assert that the mailable has the given attachment from the default storage disk.
    fn assert_has_attachment_from_storage(&self, path: &str)
    where
        Self: Sized,
    {
        let found = self
            .prepare()
            .attachments
            .iter()
            .any(|a| a.storage() == Some((None, path)));
        assert!(found, "Did not find the expected attachment.");
    }

    /// Assert that the mailable has the given attachment from a specific storage disk.
    fn assert_has_attachment_from_storage_disk(&self, disk: &str, path: &str)
    where
        Self: Sized,
    {
        let found = self
            .prepare()
            .attachments
            .iter()
            .any(|a| a.storage() == Some((Some(disk), path)));
        assert!(found, "Did not find the expected attachment.");
    }

    /// Assert that the mailable has the given tag.
    fn assert_has_tag(&self, tag: &str)
    where
        Self: Sized,
    {
        let tags = self.prepare().tags;
        let actual = if tags.is_empty() {
            "none".to_string()
        } else {
            tags.join(", ")
        };
        assert!(
            self.has_tag(tag),
            "Did not see expected tag in email tags.\nExpected: [{tag}]\nActual: [{actual}]"
        );
    }

    /// Assert that the mailable has the given metadata.
    fn assert_has_metadata(&self, key: &str, value: &str)
    where
        Self: Sized,
    {
        let actual = match self.prepare().metadata.get(key) {
            Some(actual) => format!("[{key}] => [{actual}]"),
            None => format!("key [{key}] not found"),
        };
        assert!(
            self.has_metadata(key, value),
            "Email metadata does not match expected value.\nExpected: [{key}] => [{value}]\nActual: {actual}"
        );
    }
}

fn format_addresses(addresses: &[Address]) -> String {
    if addresses.is_empty() {
        return "none".to_string();
    }
    addresses
        .iter()
        .map(|a| match &a.name {
            Some(name) => format!("{} ({name})", a.address),
            None => a.address.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn assert_see_in_order(haystack: &str, needles: &[String]) {
    let mut position = 0;
    for needle in needles {
        match haystack[position..].find(needle.as_str()) {
            Some(found) => position += found + needle.len(),
            None => panic!(
                "Failed asserting that [{needle}] appears in the expected order within the email body."
            ),
        }
    }
}

/// Recipients and options added when the mailable is sent (with
/// `Mail::to(...)`, `->locale(...)`, `Mail::mailer(...)`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SendOptions {
    /// Additional recipients.
    pub to: Vec<Address>,
    /// Additional "cc" recipients.
    pub cc: Vec<Address>,
    /// Additional "bcc" recipients.
    pub bcc: Vec<Address>,
    /// The locale to render in.
    pub locale: Option<String>,
    /// The mailer to send with.
    pub mailer: Option<String>,
}

/// A mailable's message as it is being built: Laravel's fluent
/// `$this->from(...)->subject(...)->view(...)` calls.
///
/// Envelopes, content, attachments and headers are merged into a builder
/// before the message is rendered. You may also use it directly from
/// [`Mailable::build`]:
///
/// ```
/// use illuminate_mail::{Mailable, MailableBuilder};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct Welcome;
///
/// impl Mailable for Welcome {
///     fn build(&self, mail: &mut MailableBuilder) {
///         mail.from(("hello@example.com", "Example"))
///             .subject("Welcome aboard!")
///             .view("emails.welcome")
///             .with("cta", "Get started")
///             .tag("onboarding");
///     }
/// }
///
/// Welcome.assert_has_subject("Welcome aboard!");
/// Welcome.assert_has_tag("onboarding");
/// assert_eq!(Welcome.prepare().view.as_deref(), Some("emails.welcome"));
/// ```
#[derive(Clone, Default)]
pub struct MailableBuilder {
    /// The person the message is from.
    pub from: Vec<Address>,
    /// The "to" recipients of the message.
    pub to: Vec<Address>,
    /// The "cc" recipients of the message.
    pub cc: Vec<Address>,
    /// The "bcc" recipients of the message.
    pub bcc: Vec<Address>,
    /// The "reply to" recipients of the message.
    pub reply_to: Vec<Address>,
    /// The subject of the message.
    pub subject: Option<String>,
    /// The Blade view to use for the message.
    pub view: Option<String>,
    /// The plain text view to use for the message.
    pub text_view: Option<String>,
    /// The Markdown template for the message.
    pub markdown: Option<String>,
    /// The HTML to use for the message.
    pub html: Option<String>,
    /// The view data for the message.
    pub view_data: Map<String, Value>,
    /// The attachments for the message.
    pub attachments: Vec<Attachment>,
    /// The tags for the message.
    pub tags: Vec<String>,
    /// The metadata for the message.
    pub metadata: IndexMap<String, String>,
    /// The custom headers for the message.
    pub headers: Headers,
    /// The priority of the message (1 is the highest).
    pub priority: Option<u8>,
    /// The callbacks for the message.
    pub callbacks: Vec<MessageCallback>,
    /// The locale of the message.
    pub locale: Option<String>,
    /// The name of the mailer that should send the message.
    pub mailer: Option<String>,
    /// The name of the theme that should be used when formatting the message.
    pub theme: Option<String>,
}

impl fmt::Debug for MailableBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MailableBuilder")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("cc", &self.cc)
            .field("bcc", &self.bcc)
            .field("reply_to", &self.reply_to)
            .field("subject", &self.subject)
            .field("view", &self.view)
            .field("text_view", &self.text_view)
            .field("markdown", &self.markdown)
            .field("view_data", &self.view_data)
            .field("attachments", &self.attachments)
            .field("tags", &self.tags)
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

impl MailableBuilder {
    /// Prepare a mailable for delivery: run its `build` method and hydrate
    /// its headers, envelope, content and attachments.
    pub fn for_mailable(mailable: &dyn Mailable, options: &SendOptions) -> Self {
        let mut builder = MailableBuilder {
            to: options.to.clone(),
            cc: options.cc.clone(),
            bcc: options.bcc.clone(),
            locale: options.locale.clone().or_else(|| mailable.locale()),
            mailer: options.mailer.clone().or_else(|| mailable.mailer()),
            theme: mailable.theme(),
            ..Default::default()
        };

        mailable.build(&mut builder);

        let headers = mailable.headers();
        if headers.message_id.is_some() {
            builder.headers.message_id = headers.message_id;
        }
        builder.headers.references.extend(headers.references);
        builder.headers.text.extend(headers.text);

        let envelope = mailable.envelope();
        if let Some(from) = envelope.from {
            builder.from.push(from);
        }
        builder.to.extend(envelope.to);
        builder.cc.extend(envelope.cc);
        builder.bcc.extend(envelope.bcc);
        builder.reply_to.extend(envelope.reply_to);
        if envelope.subject.is_some() {
            builder.subject = envelope.subject;
        }
        builder.tags.extend(envelope.tags);
        builder.metadata.extend(envelope.metadata);
        builder.callbacks.extend(envelope.using);

        let content = mailable.content();
        if let Some(view) = content.view.or(content.html) {
            builder.view = Some(view);
        }
        if content.text.is_some() {
            builder.text_view = content.text;
        }
        if content.markdown.is_some() {
            builder.markdown = content.markdown;
        }
        if content.html_string.is_some() {
            builder.html = content.html_string;
        }
        builder.view_data.extend(content.with);

        builder.attachments.extend(mailable.attachments());
        builder
    }

    /// Set the sender of the message.
    pub fn from(&mut self, address: impl Into<Address>) -> &mut Self {
        self.from.push(address.into());
        self
    }

    /// Set the recipients of the message.
    pub fn to(&mut self, addresses: impl IntoAddresses) -> &mut Self {
        self.to.extend(addresses.into_addresses());
        self
    }

    /// Set the "cc" recipients of the message.
    pub fn cc(&mut self, addresses: impl IntoAddresses) -> &mut Self {
        self.cc.extend(addresses.into_addresses());
        self
    }

    /// Set the "bcc" recipients of the message.
    pub fn bcc(&mut self, addresses: impl IntoAddresses) -> &mut Self {
        self.bcc.extend(addresses.into_addresses());
        self
    }

    /// Set the "reply to" address of the message.
    pub fn reply_to(&mut self, addresses: impl IntoAddresses) -> &mut Self {
        self.reply_to.extend(addresses.into_addresses());
        self
    }

    /// Set the subject of the message.
    pub fn subject(&mut self, subject: impl Into<String>) -> &mut Self {
        self.subject = Some(subject.into());
        self
    }

    /// Set the Blade view for the message.
    pub fn view(&mut self, view: impl Into<String>) -> &mut Self {
        self.view = Some(view.into());
        self
    }

    /// Set the plain text view for the message.
    pub fn text(&mut self, view: impl Into<String>) -> &mut Self {
        self.text_view = Some(view.into());
        self
    }

    /// Set the Markdown template for the message.
    pub fn markdown(&mut self, view: impl Into<String>) -> &mut Self {
        self.markdown = Some(view.into());
        self
    }

    /// Set the rendered HTML content for the message.
    pub fn html(&mut self, html: impl Into<String>) -> &mut Self {
        self.html = Some(html.into());
        self
    }

    /// Set the view data for the message.
    pub fn with(&mut self, key: impl Into<String>, value: impl Serialize) -> &mut Self {
        self.view_data.insert(key.into(), to_value(&value));
        self
    }

    /// Attach a file to the message.
    pub fn attach(&mut self, attachment: impl Into<Attachment>) -> &mut Self {
        self.attachments.push(attachment.into());
        self
    }

    /// Attach in-memory data as an attachment.
    pub fn attach_data(&mut self, data: impl Into<Vec<u8>>, name: impl Into<String>) -> &mut Self {
        self.attachments.push(Attachment::from_bytes(data, name));
        self
    }

    /// Attach a file to the message from the default storage disk.
    pub fn attach_from_storage(&mut self, path: impl Into<String>) -> &mut Self {
        self.attachments.push(Attachment::from_storage(path));
        self
    }

    /// Attach a file to the message from a storage disk.
    pub fn attach_from_storage_disk(
        &mut self,
        disk: impl Into<String>,
        path: impl Into<String>,
    ) -> &mut Self {
        self.attachments
            .push(Attachment::from_storage_disk(disk, path));
        self
    }

    /// Add a tag header to the message.
    pub fn tag(&mut self, tag: impl Into<String>) -> &mut Self {
        self.tags.push(tag.into());
        self
    }

    /// Add a metadata header to the message.
    pub fn metadata(&mut self, key: impl Into<String>, value: impl ToString) -> &mut Self {
        self.metadata.insert(key.into(), value.to_string());
        self
    }

    /// Set the priority of this message (1 is the highest, 5 the lowest).
    pub fn priority(&mut self, level: u8) -> &mut Self {
        self.priority = Some(level);
        self
    }

    /// Register a callback to be called with the message just before it is
    /// sent (Laravel's `withSymfonyMessage`).
    pub fn with_message(
        &mut self,
        callback: impl Fn(&mut Message) + Send + Sync + 'static,
    ) -> &mut Self {
        self.callbacks.push(Arc::new(callback));
        self
    }

    /// Set the locale of the message.
    pub fn locale(&mut self, locale: impl Into<String>) -> &mut Self {
        self.locale = Some(locale.into());
        self
    }

    /// Set the name of the mailer that should send the message.
    pub fn mailer(&mut self, mailer: impl Into<String>) -> &mut Self {
        self.mailer = Some(mailer.into());
        self
    }

    /// Set the theme used to format the message.
    pub fn theme(&mut self, theme: impl Into<String>) -> &mut Self {
        self.theme = Some(theme.into());
        self
    }

    /// The subject, defaulting to the mailable's name in title case
    /// (`OrderShipped` becomes "Order Shipped").
    pub fn subject_or_default(&self, mailable_name: &str) -> String {
        self.subject
            .clone()
            .unwrap_or_else(|| Str::title(&Str::snake_with(mailable_name, " ")))
    }

    pub(crate) fn view_spec(&self) -> ViewSpec {
        ViewSpec {
            view: self.view.clone(),
            text_view: self.text_view.clone(),
            markdown: self.markdown.clone(),
            html: self.html.clone(),
            raw: None,
            markdown_template: None,
        }
    }

    /// Apply the builder's addresses, subject, tags, metadata, headers and
    /// callbacks to a message (Laravel's `buildFrom`, `buildRecipients`, ...).
    pub(crate) fn apply_to(&self, message: &mut Message, mailable_name: &str) {
        if let Some(from) = self.from.first() {
            message.from(from.clone());
        }
        message.to.extend(self.to.iter().cloned());
        message.cc.extend(self.cc.iter().cloned());
        message.bcc.extend(self.bcc.iter().cloned());
        message.reply_to.extend(self.reply_to.iter().cloned());
        message.subject(self.subject_or_default(mailable_name));
        message.tags.extend(self.tags.iter().cloned());
        message
            .metadata
            .extend(self.metadata.iter().map(|(k, v)| (k.clone(), v.clone())));
        if let Some(priority) = self.priority {
            message.priority(priority);
        }
        if let Some(id) = &self.headers.message_id {
            message.message_id = Some(id.trim_matches(['<', '>']).to_string());
        }
        if !self.headers.references.is_empty() {
            message.header("References", self.headers.references_string());
        }
        for (name, value) in &self.headers.text {
            message.header(name.clone(), value.clone());
        }
        for callback in &self.callbacks {
            callback(message);
        }
        message.pending.extend(self.attachments.iter().cloned());
    }
}

/// The views a message is rendered from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ViewSpec {
    pub(crate) view: Option<String>,
    pub(crate) text_view: Option<String>,
    pub(crate) markdown: Option<String>,
    pub(crate) html: Option<String>,
    pub(crate) raw: Option<String>,
    pub(crate) markdown_template: Option<String>,
}

/// The view(s) for a message sent without a mailable (`Mail::send_view`).
///
/// ```
/// use illuminate_mail::MailView;
///
/// let view: MailView = "emails.welcome".into();
/// assert_eq!(view, MailView::View("emails.welcome".into()));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MailView {
    /// An HTML Blade view.
    View(String),
    /// A plain-text Blade view.
    Text(String),
    /// An HTML view and its plain-text counterpart.
    Both {
        /// The HTML view.
        html: String,
        /// The plain-text view.
        text: String,
    },
    /// A Markdown mail view.
    Markdown(String),
    /// A Markdown mail template, given as Blade source.
    MarkdownTemplate(String),
    /// A string of HTML.
    Html(String),
    /// Raw plain text.
    Raw(String),
}

impl From<&str> for MailView {
    fn from(view: &str) -> Self {
        MailView::View(view.to_string())
    }
}

impl From<String> for MailView {
    fn from(view: String) -> Self {
        MailView::View(view)
    }
}

impl From<MailView> for ViewSpec {
    fn from(view: MailView) -> Self {
        match view {
            MailView::View(view) => ViewSpec {
                view: Some(view),
                ..Default::default()
            },
            MailView::Text(view) => ViewSpec {
                text_view: Some(view),
                ..Default::default()
            },
            MailView::Both { html, text } => ViewSpec {
                view: Some(html),
                text_view: Some(text),
                ..Default::default()
            },
            MailView::Markdown(view) => ViewSpec {
                markdown: Some(view),
                ..Default::default()
            },
            MailView::MarkdownTemplate(template) => ViewSpec {
                markdown_template: Some(template),
                ..Default::default()
            },
            MailView::Html(html) => ViewSpec {
                html: Some(html),
                ..Default::default()
            },
            MailView::Raw(text) => ViewSpec {
                raw: Some(text),
                ..Default::default()
            },
        }
    }
}

/// The rendered bodies of a message.
#[derive(Clone, Debug, Default)]
pub(crate) struct Rendered {
    pub(crate) html: Option<String>,
    pub(crate) text: Option<String>,
    pub(crate) embeds: Vec<crate::attachment::MessageAttachment>,
}

impl Rendered {
    /// The HTML (or the text, for text-only mail), with embedded images
    /// inlined as data URIs so the preview displays them.
    pub(crate) fn preview(&self) -> String {
        let mut body = self
            .html
            .clone()
            .or_else(|| self.text.clone())
            .unwrap_or_default();
        for embed in &self.embeds {
            if let Some(id) = &embed.content_id {
                use base64::Engine;
                let data = base64::engine::general_purpose::STANDARD.encode(&embed.data);
                body = body.replace(
                    &format!("cid:{id}"),
                    &format!("data:{};base64,{data}", embed.content_type),
                );
            }
        }
        body
    }
}

/// Render the views of a message.
pub(crate) fn render_views(
    spec: &ViewSpec,
    data: Map<String, Value>,
    theme: Option<&str>,
) -> Result<Rendered> {
    let factory = Factory::resolve();
    let embeds = Embeds::default();
    let mut html_data: ViewData = Value::Object(data.clone()).into_view_data();
    html_data.insert("message".to_string(), embeds.html_object());
    let mut text_data: ViewData = Value::Object(data).into_view_data();
    text_data.insert("message".to_string(), embeds.text_object());

    let markdown = || {
        let markdown = Markdown::resolve();
        match theme {
            Some(theme) => markdown.theme(theme),
            None => markdown,
        }
    };

    let html = if let Some(html) = &spec.html {
        Some(html.clone())
    } else if let Some(view) = &spec.markdown {
        Some(markdown().render(view, html_data)?)
    } else if let Some(template) = &spec.markdown_template {
        Some(markdown().render_template(template, html_data)?)
    } else if let Some(view) = &spec.view {
        Some(factory.make(view, html_data).render()?)
    } else {
        None
    };

    let text = if let Some(view) = &spec.text_view {
        Some(factory.make(view, text_data).render()?)
    } else if let (Some(view), None) = (&spec.markdown, &spec.html) {
        Some(markdown().render_text(view, text_data)?)
    } else if let (Some(template), None) = (&spec.markdown_template, &spec.html) {
        Some(markdown().render_text_template(template, text_data)?)
    } else {
        spec.raw.clone()
    };

    // Like Laravel, never send an empty body.
    let blank = |body: String| {
        if body.is_empty() {
            " ".to_string()
        } else {
            body
        }
    };
    Ok(Rendered {
        html: html.map(blank),
        text: text.map(|text| {
            if spec.raw.is_some() {
                text
            } else {
                blank(text)
            }
        }),
        embeds: embeds.take(),
    })
}

/// The view data for a mailable: its `with` data, overridden by its fields.
pub(crate) fn mailable_view_data(
    mailable: &dyn Mailable,
    builder: &MailableBuilder,
    mailer: Option<&str>,
) -> Map<String, Value> {
    let mut data = builder.view_data.clone();
    if let Value::Object(fields) = mailable.mailable_data() {
        data.extend(fields);
    }
    data.insert(
        "__laravel_mailable".to_string(),
        Value::String(mailable.mailable_name()),
    );
    if let Some(mailer) = mailer {
        data.insert("mailer".to_string(), Value::String(mailer.to_string()));
    }
    data
}

/// Render a mailable (in its locale).
pub(crate) fn render_mailable(mailable: &dyn Mailable, options: &SendOptions) -> Result<Rendered> {
    let builder = MailableBuilder::for_mailable(mailable, options);
    let render = || {
        let data = mailable_view_data(mailable, &builder, None);
        render_views(&builder.view_spec(), data, builder.theme.as_deref())
    };
    match &builder.locale {
        Some(locale) => illuminate_translation::with_locale(locale, render),
        None => render(),
    }
}

/// A rendered mailable, ready to be previewed in the browser.
#[derive(Debug)]
pub struct MailPreview(Result<String>);

impl MailPreview {
    /// The rendered HTML, or the error that prevented rendering.
    pub fn into_result(self) -> Result<String> {
        self.0
    }
}

impl IntoResponse for MailPreview {
    fn into_response(self) -> Response {
        self.0.map(Response::new).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc as StdArc;

    use illuminate_config::Repository;
    use illuminate_container::Container;
    use illuminate_support::json;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Order {
        id: u64,
        name: String,
    }

    #[derive(Serialize)]
    struct OrderShipped {
        order: Order,
        #[serde(skip)]
        secret: String,
    }

    impl Mailable for OrderShipped {
        fn envelope(&self) -> Envelope {
            Envelope::new()
                .from(("jeffrey@example.com", "Jeffrey Way"))
                .to("taylor@example.com")
                .cc("abigail@example.com")
                .bcc("james@example.com")
                .reply_to("support@example.com")
                .tag("shipment")
                .metadata("order_id", self.order.id)
        }

        fn content(&self) -> Content {
            Content::view("emails.orders.shipped")
                .with_text("emails.orders.shipped-text")
                .with("url", "https://example.com/orders/1?a=1&b=2")
        }

        fn attachments(&self) -> Vec<Attachment> {
            vec![
                Attachment::from_path("/invoices/1.pdf").as_("Invoice.pdf"),
                Attachment::from_storage("receipts/1.pdf"),
                Attachment::from_storage_disk("s3", "photos/1.jpg"),
                Attachment::from_bytes("a,b", "data.csv"),
            ]
        }

        fn headers(&self) -> Headers {
            Headers::new()
                .message_id("custom@example.com")
                .text([("X-Custom", "yes")])
        }
    }

    fn order_shipped() -> OrderShipped {
        OrderShipped {
            order: Order {
                id: 1,
                name: "Tom & Jerry".into(),
            },
            secret: "hidden".into(),
        }
    }

    fn app(
        views: &std::path::Path,
    ) -> (StdArc<Container>, illuminate_container::LocalInstanceGuard) {
        let app = StdArc::new(Container::new());
        let guard = Container::set_local_instance(app.clone());
        app.instance(Repository::new(
            json!({"view": {"paths": [views]}, "app": {"name": "Laravel"}}),
        ));
        (app, guard)
    }

    #[test]
    fn mailables_are_prepared_from_their_envelope_and_content() {
        let mailable = order_shipped();
        let builder = mailable.prepare();
        assert_eq!(builder.view.as_deref(), Some("emails.orders.shipped"));
        assert_eq!(
            builder.text_view.as_deref(),
            Some("emails.orders.shipped-text")
        );
        assert_eq!(
            builder.headers.message_id.as_deref(),
            Some("custom@example.com")
        );
        assert_eq!(
            builder.subject_or_default(&mailable.mailable_name()),
            "Order Shipped"
        );
        assert_eq!(mailable.mailable_name(), "OrderShipped");
        assert_eq!(mailable.mailable_data()["order"]["id"], 1);
        assert!(mailable.mailable_data().get("secret").is_none());
        assert_eq!(mailable.secret, "hidden");

        mailable.assert_from("jeffrey@example.com");
        mailable.assert_has_to("taylor@example.com");
        mailable.assert_to("TAYLOR@example.com");
        mailable.assert_has_cc("abigail@example.com");
        mailable.assert_has_bcc("james@example.com");
        mailable.assert_has_reply_to("support@example.com");
        mailable.assert_has_subject("Order Shipped");
        mailable.assert_has_tag("shipment");
        mailable.assert_has_metadata("order_id", "1");
        mailable.assert_has_attachment(Attachment::from_path("/invoices/1.pdf").as_("Invoice.pdf"));
        mailable.assert_has_attached_data("a,b", "data.csv");
        mailable.assert_has_attachment_from_storage("receipts/1.pdf");
        mailable.assert_has_attachment_from_storage_disk("s3", "photos/1.jpg");
        assert!(!mailable.has_attachment("/invoices/1.pdf"));
        assert!(!mailable.has_to("nobody@example.com"));
    }

    #[test]
    #[should_panic(
        expected = "Did not see expected recipient in email 'to' recipients.\nExpected: [nobody@example.com]\nActual: [taylor@example.com]"
    )]
    fn failed_recipient_assertions_explain_themselves() {
        order_shipped().assert_has_to("nobody@example.com");
    }

    #[test]
    #[should_panic(expected = "Email subject does not match expected value.")]
    fn failed_subject_assertions_explain_themselves() {
        order_shipped().assert_has_subject("Wrong");
    }

    #[test]
    #[should_panic(expected = "Expected no attachments, but found [4] attachment(s).")]
    fn attachment_count_assertions() {
        order_shipped().assert_has_no_attachments();
    }

    #[test]
    fn mailables_render_their_views_with_their_data() {
        let views = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(views.path().join("emails/orders")).unwrap();
        std::fs::write(
            views.path().join("emails/orders/shipped.blade.html"),
            "<h1>Order {{ $order->id }}</h1><p>{{ $order['name'] }}</p><a href=\"{{ $url }}\">View</a>{{ $__laravel_mailable }}",
        )
        .unwrap();
        std::fs::write(
            views.path().join("emails/orders/shipped-text.blade.html"),
            "Order {{ $order->id }} shipped. {{ $url }}",
        )
        .unwrap();
        let (_app, _guard) = app(views.path());

        let mailable = order_shipped();
        let html = mailable.render().unwrap();
        assert_eq!(
            html,
            "<h1>Order 1</h1><p>Tom &amp; Jerry</p><a href=\"https://example.com/orders/1?a=1&amp;b=2\">View</a>OrderShipped"
        );
        mailable.assert_see_in_html("Tom & Jerry");
        mailable.assert_dont_see_in_html("Abigail");
        mailable.assert_see_in_order_in_html(&["Order 1", "Tom & Jerry", "View"]);
        mailable.assert_see_in_text("Order 1 shipped.");
        mailable.assert_dont_see_in_text("<h1>");
        mailable.assert_see_in_order_in_text(&["Order", "shipped"]);

        let response = mailable.preview().into_response();
        assert_eq!(response.status().as_u16(), 200);
        assert!(response.content_string().contains("<h1>Order 1</h1>"));
    }

    #[test]
    fn missing_views_fail_to_render() {
        let views = tempfile::tempdir().unwrap();
        let (_app, _guard) = app(views.path());
        assert!(order_shipped().render().is_err());
        assert!(order_shipped().preview().into_result().is_err());
    }

    #[derive(Serialize)]
    struct Welcome {
        name: String,
    }

    impl Mailable for Welcome {
        fn build(&self, mail: &mut MailableBuilder) {
            mail.from("hello@example.com")
                .to(("user@example.com", "User"))
                .subject(format!("Welcome, {}!", self.name))
                .markdown("mail.welcome")
                .with("cta", "Get started")
                .priority(2)
                .attach_data("x", "x.txt")
                .attach_from_storage_disk("s3", "guide.pdf")
                .theme("default")
                .locale("en")
                .mailer("array")
                .with_message(|message| {
                    message.header("X-Welcome", "1");
                });
        }
    }

    #[test]
    fn markdown_mailables_render_html_and_text() {
        let views = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(views.path().join("mail")).unwrap();
        std::fs::write(
            views.path().join("mail/welcome.blade.html"),
            "<x-mail::message>\n# Welcome, {{ $name }}!\n\n<x-mail::button url=\"https://example.com\">\n{{ $cta }}\n</x-mail::button>\n</x-mail::message>",
        )
        .unwrap();
        let (_app, _guard) = app(views.path());

        let mailable = Welcome {
            name: "Taylor".into(),
        };
        mailable.assert_has_subject("Welcome, Taylor!");
        mailable.assert_has_to("user@example.com");
        let builder = mailable.prepare();
        assert_eq!(builder.mailer.as_deref(), Some("array"));
        assert_eq!(builder.locale.as_deref(), Some("en"));

        let html = mailable.render().unwrap();
        assert!(html.contains(">Welcome, Taylor!</h1>"));
        assert!(html.contains("Get started</a>"));
        let text = mailable.render_text().unwrap();
        assert!(text.contains("# Welcome, Taylor!"));
        assert!(text.contains("Get started: https://example.com"));
        assert!(
            text.starts_with("Laravel:"),
            "the header shows the app name: {text}"
        );
    }

    #[test]
    fn builders_apply_to_messages() {
        let builder = Welcome {
            name: "Taylor".into(),
        }
        .prepare();
        let mut message = Message::new();
        message.from("global@example.com");
        builder.apply_to(&mut message, "Welcome");
        assert!(message.has_from("hello@example.com"));
        assert!(message.has_to("user@example.com"));
        assert_eq!(message.subject.as_deref(), Some("Welcome, Taylor!"));
        assert_eq!(message.priority, Some(2));
        assert_eq!(message.get_header("X-Welcome"), Some("1"));
        assert_eq!(message.pending_attachments().len(), 2);

        let builder = order_shipped().prepare();
        let mut message = Message::new();
        builder.apply_to(&mut message, "OrderShipped");
        assert_eq!(message.message_id.as_deref(), Some("custom@example.com"));
        assert_eq!(message.get_header("X-Custom"), Some("yes"));
        assert_eq!(message.tags, vec!["shipment".to_string()]);
        assert_eq!(message.metadata["order_id"], "1");
    }

    #[test]
    fn html_strings_and_raw_text_need_no_views() {
        let views = tempfile::tempdir().unwrap();
        let (_app, _guard) = app(views.path());
        let rendered =
            render_views(&MailView::Html("<p>Hi</p>".into()).into(), Map::new(), None).unwrap();
        assert_eq!(rendered.html.as_deref(), Some("<p>Hi</p>"));
        assert_eq!(rendered.text, None);

        let rendered =
            render_views(&MailView::Raw("plain".into()).into(), Map::new(), None).unwrap();
        assert_eq!(rendered.html, None);
        assert_eq!(rendered.text.as_deref(), Some("plain"));
        assert_eq!(rendered.preview(), "plain");
    }
}
