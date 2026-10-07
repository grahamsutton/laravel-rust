//! Mail notifications: [`MailMessage`].

use std::fmt;
use std::sync::Arc;

use illuminate_mail::{Address, Attachment, IntoAddresses, Markdown, Message, MessageCallback};
use illuminate_support::{Map, Result, Value, json, to_value};
use illuminate_view::Factory;
use indexmap::IndexMap;
use serde::Serialize;

/// The built-in Markdown template for mail notifications
/// (`notifications::email`).
pub const EMAIL_TEMPLATE: &str = include_str!("../../resources/views/email.blade.html");

/// The default notification template's name.
pub const DEFAULT_TEMPLATE: &str = "notifications::email";

/// A call to action: a button in a mail notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Action {
    /// The action text.
    pub text: String,
    /// The action URL.
    pub url: String,
}

impl Action {
    /// Create a new action.
    pub fn new(text: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            url: url.into(),
        }
    }
}

/// A mail notification: greeting, lines of text, a call to action, and a
/// salutation, rendered with Laravel's notification email template.
///
/// ```
/// use illuminate_notifications::MailMessage;
///
/// let message = MailMessage::new()
///     .subject("Invoice Paid")
///     .greeting("Hello!")
///     .line("One of your invoices has been paid!")
///     .action("View Invoice", "https://example.com/invoice/1")
///     .line("Thank you for using our application!");
///
/// assert_eq!(message.intro_lines, vec!["One of your invoices has been paid!"]);
/// assert_eq!(message.outro_lines, vec!["Thank you for using our application!"]);
/// assert_eq!(message.to_array()["actionUrl"], "https://example.com/invoice/1");
/// ```
#[derive(Clone)]
pub struct MailMessage {
    /// The "level" of the notification (`info`, `success`, `error`).
    pub level: String,
    /// The subject of the notification.
    pub subject: Option<String>,
    /// The notification's greeting.
    pub greeting: Option<String>,
    /// The notification's salutation.
    pub salutation: Option<String>,
    /// The "intro" lines of the notification.
    pub intro_lines: Vec<String>,
    /// The "outro" lines of the notification.
    pub outro_lines: Vec<String>,
    /// The text / label for the action.
    pub action_text: Option<String>,
    /// The action URL.
    pub action_url: Option<String>,
    /// The name of the mailer that should send the notification.
    pub mailer: Option<String>,
    /// The Blade view that should be rendered for the mail message.
    pub view: Option<String>,
    /// The plain-text view for the mail message.
    pub text_view: Option<String>,
    /// The view data for the message.
    pub view_data: Map<String, Value>,
    /// The Markdown template to render (`notifications::email` by default).
    pub markdown: Option<String>,
    /// The current theme being used when generating emails.
    pub theme: Option<String>,
    /// The "from" information for the message.
    pub from: Option<Address>,
    /// The "reply to" information for the message.
    pub reply_to: Vec<Address>,
    /// The "cc" information for the message.
    pub cc: Vec<Address>,
    /// The "bcc" information for the message.
    pub bcc: Vec<Address>,
    /// The attachments for the message.
    pub attachments: Vec<Attachment>,
    /// The tags for the message.
    pub tags: Vec<String>,
    /// The metadata for the message.
    pub metadata: IndexMap<String, String>,
    /// Priority level of the message.
    pub priority: Option<u8>,
    /// The callbacks for the message.
    pub callbacks: Vec<MessageCallback>,
}

impl Default for MailMessage {
    fn default() -> Self {
        Self {
            level: "info".into(),
            subject: None,
            greeting: None,
            salutation: None,
            intro_lines: Vec::new(),
            outro_lines: Vec::new(),
            action_text: None,
            action_url: None,
            mailer: None,
            view: None,
            text_view: None,
            view_data: Map::new(),
            markdown: Some(DEFAULT_TEMPLATE.into()),
            theme: None,
            from: None,
            reply_to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            attachments: Vec::new(),
            tags: Vec::new(),
            metadata: IndexMap::new(),
            priority: None,
            callbacks: Vec::new(),
        }
    }
}

impl fmt::Debug for MailMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MailMessage")
            .field("level", &self.level)
            .field("subject", &self.subject)
            .field("greeting", &self.greeting)
            .field("intro_lines", &self.intro_lines)
            .field("action_text", &self.action_text)
            .field("action_url", &self.action_url)
            .field("outro_lines", &self.outro_lines)
            .field("view", &self.view)
            .field("markdown", &self.markdown)
            .finish_non_exhaustive()
    }
}

/// Collapse a (possibly multi-line) line into one trimmed line.
fn format_line(line: &str) -> String {
    line.lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

impl MailMessage {
    /// Create a new mail message.
    pub fn new() -> Self {
        Self::default()
    }

    /// Indicate that the notification gives information about a successful operation.
    pub fn success(self) -> Self {
        self.level("success")
    }

    /// Indicate that the notification gives information about an error.
    pub fn error(self) -> Self {
        self.level("error")
    }

    /// Set the "level" of the notification (success, error, etc.).
    pub fn level(mut self, level: impl Into<String>) -> Self {
        self.level = level.into();
        self
    }

    /// Set the subject of the notification.
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Set the greeting of the notification.
    pub fn greeting(mut self, greeting: impl Into<String>) -> Self {
        self.greeting = Some(greeting.into());
        self
    }

    /// Set the salutation of the notification.
    pub fn salutation(mut self, salutation: impl Into<String>) -> Self {
        self.salutation = Some(salutation.into());
        self
    }

    /// Add a line of text to the notification: before the action, it's an
    /// "intro" line; after it, an "outro" line.
    pub fn line(mut self, line: impl AsRef<str>) -> Self {
        let line = format_line(line.as_ref());
        if self.action_text.is_none() {
            self.intro_lines.push(line);
        } else {
            self.outro_lines.push(line);
        }
        self
    }

    /// Add a line of text if the given condition is true.
    pub fn line_if(self, condition: bool, line: impl AsRef<str>) -> Self {
        if condition { self.line(line) } else { self }
    }

    /// Add lines of text to the notification.
    pub fn lines<I: IntoIterator<Item = S>, S: AsRef<str>>(self, lines: I) -> Self {
        lines
            .into_iter()
            .fold(self, |message, line| message.line(line))
    }

    /// Add lines of text if the given condition is true.
    pub fn lines_if<I: IntoIterator<Item = S>, S: AsRef<str>>(
        self,
        condition: bool,
        lines: I,
    ) -> Self {
        if condition { self.lines(lines) } else { self }
    }

    /// Configure the "call to action" button.
    pub fn action(mut self, text: impl Into<String>, url: impl Into<String>) -> Self {
        self.action_text = Some(text.into());
        self.action_url = Some(url.into());
        self
    }

    /// Configure the "call to action" button from an [`Action`].
    pub fn with_action(self, action: Action) -> Self {
        self.action(action.text, action.url)
    }

    /// Set the name of the mailer that should send the notification.
    pub fn mailer(mut self, mailer: impl Into<String>) -> Self {
        self.mailer = Some(mailer.into());
        self
    }

    /// Set the view for the mail message.
    pub fn view(mut self, view: impl Into<String>) -> Self {
        self.view = Some(view.into());
        self.markdown = None;
        self
    }

    /// Set the plain text view for the mail message.
    pub fn text(mut self, view: impl Into<String>) -> Self {
        self.text_view = Some(view.into());
        self.markdown = None;
        self
    }

    /// Set the Markdown template for the notification.
    pub fn markdown(mut self, view: impl Into<String>) -> Self {
        self.markdown = Some(view.into());
        self.view = None;
        self.text_view = None;
        self
    }

    /// Set the default Markdown template.
    pub fn template(mut self, template: impl Into<String>) -> Self {
        self.markdown = Some(template.into());
        self
    }

    /// Add view data for the view or Markdown template.
    pub fn with_data(mut self, data: impl Serialize) -> Self {
        if let Value::Object(map) = to_value(&data) {
            self.view_data.extend(map);
        }
        self
    }

    /// Set the theme to use with the Markdown template.
    pub fn theme(mut self, theme: impl Into<String>) -> Self {
        self.theme = Some(theme.into());
        self
    }

    /// Set the from address for the mail message.
    pub fn from(mut self, address: impl Into<Address>) -> Self {
        self.from = Some(address.into());
        self
    }

    /// Set the "reply to" address of the message.
    pub fn reply_to(mut self, addresses: impl IntoAddresses) -> Self {
        self.reply_to.extend(addresses.into_addresses());
        self
    }

    /// Set the cc address for the mail message.
    pub fn cc(mut self, addresses: impl IntoAddresses) -> Self {
        self.cc.extend(addresses.into_addresses());
        self
    }

    /// Set the bcc address for the mail message.
    pub fn bcc(mut self, addresses: impl IntoAddresses) -> Self {
        self.bcc.extend(addresses.into_addresses());
        self
    }

    /// Attach a file to the message.
    pub fn attach(mut self, attachment: impl Into<Attachment>) -> Self {
        self.attachments.push(attachment.into());
        self
    }

    /// Attach in-memory data as an attachment.
    pub fn attach_data(mut self, data: impl Into<Vec<u8>>, name: impl Into<String>) -> Self {
        self.attachments.push(Attachment::from_bytes(data, name));
        self
    }

    /// Attach a file to the message from the default storage disk.
    pub fn attach_from_storage(mut self, path: impl Into<String>) -> Self {
        self.attachments.push(Attachment::from_storage(path));
        self
    }

    /// Attach a file to the message from a storage disk.
    pub fn attach_from_storage_disk(
        mut self,
        disk: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        self.attachments
            .push(Attachment::from_storage_disk(disk, path));
        self
    }

    /// Add a tag header to the message.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Add a metadata header to the message.
    pub fn metadata(mut self, key: impl Into<String>, value: impl ToString) -> Self {
        self.metadata.insert(key.into(), value.to_string());
        self
    }

    /// Set the priority of this message (1 is the highest, 5 the lowest).
    pub fn priority(mut self, level: u8) -> Self {
        self.priority = Some(level);
        self
    }

    /// Register a callback to be called with the message just before it
    /// is sent (Laravel's `withSymfonyMessage`).
    pub fn with_message(mut self, callback: impl Fn(&mut Message) + Send + Sync + 'static) -> Self {
        self.callbacks.push(Arc::new(callback));
        self
    }

    /// Get an array representation of the message (the template's data).
    pub fn to_array(&self) -> Value {
        let displayable = self
            .action_url
            .as_deref()
            .unwrap_or_default()
            .replace("mailto:", "")
            .replace("tel:", "");
        json!({
            "level": self.level,
            "subject": self.subject,
            "greeting": self.greeting,
            "salutation": self.salutation,
            "introLines": self.intro_lines,
            "outroLines": self.outro_lines,
            "actionText": self.action_text,
            "actionUrl": self.action_url,
            "displayableActionUrl": displayable,
        })
    }

    /// Get the data array for the mail message.
    pub fn data(&self) -> Map<String, Value> {
        let mut data = match self.to_array() {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        data.extend(self.view_data.clone());
        data
    }

    /// Whether the message renders the built-in notification template.
    pub(crate) fn uses_default_template(&self, factory: &Factory) -> bool {
        self.view.is_none()
            && self.text_view.is_none()
            && self
                .markdown
                .as_deref()
                .is_none_or(|m| m == DEFAULT_TEMPLATE)
            && !factory.exists(DEFAULT_TEMPLATE)
    }

    /// Render the mail notification message into HTML (for previews).
    pub fn render(&self) -> Result<String> {
        let factory = Factory::resolve();
        if let Some(view) = self.view.as_deref() {
            return factory.make(view, Value::Object(self.data())).render();
        }
        let markdown = Markdown::resolve();
        let markdown = match &self.theme {
            Some(theme) => markdown.theme(theme),
            None => markdown,
        };
        if self.uses_default_template(&factory) {
            return markdown.render_template(EMAIL_TEMPLATE, Value::Object(self.data()));
        }
        let view = self.markdown.as_deref().unwrap_or(DEFAULT_TEMPLATE);
        markdown.render(view, Value::Object(self.data()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_go_before_and_after_the_action() {
        let message = MailMessage::new()
            .line("First   \n   line")
            .line_if(false, "skipped")
            .lines(["a", "b"])
            .action("Go", "mailto:taylor@example.com")
            .lines_if(true, ["after"])
            .success();
        assert_eq!(message.intro_lines, vec!["First line", "a", "b"]);
        assert_eq!(message.outro_lines, vec!["after"]);
        assert_eq!(message.level, "success");
        assert_eq!(
            message.to_array()["displayableActionUrl"],
            "taylor@example.com"
        );
        assert_eq!(message.to_array()["introLines"][0], "First line");
    }

    #[test]
    fn views_and_markdown_are_exclusive() {
        let message = MailMessage::new()
            .view("emails.custom")
            .with_data(json!({"x": 1}));
        assert_eq!(message.markdown, None);
        assert_eq!(message.data()["x"], 1);
        let message = message.markdown("mail.custom");
        assert_eq!(message.view, None);
        assert_eq!(message.markdown.as_deref(), Some("mail.custom"));
        let message = MailMessage::new()
            .error()
            .with_action(Action::new("Retry", "/retry"));
        assert_eq!(message.level, "error");
        assert_eq!(message.action_text.as_deref(), Some("Retry"));
        assert!(format!("{message:?}").contains("Retry"));
    }

    #[test]
    fn messages_collect_mail_options() {
        let message = MailMessage::new()
            .from(("billing@example.com", "Billing"))
            .reply_to("support@example.com")
            .cc(["a@example.com"])
            .bcc("b@example.com")
            .attach_data("x", "x.txt")
            .attach_from_storage("invoices/1.pdf")
            .attach_from_storage_disk("s3", "logos/1.png")
            .tag("invoice")
            .metadata("invoice_id", 1)
            .priority(2)
            .mailer("postmark")
            .theme("brand")
            .with_message(|message| {
                message.header("X-Test", "1");
            });
        assert_eq!(
            message.from.as_ref().unwrap().name.as_deref(),
            Some("Billing")
        );
        assert_eq!(message.attachments.len(), 3);
        assert_eq!(message.callbacks.len(), 1);
        assert_eq!(message.metadata["invoice_id"], "1");
    }
}
