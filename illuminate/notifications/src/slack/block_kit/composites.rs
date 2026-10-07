//! Block Kit composition objects: text objects and confirmation dialogs.

use illuminate_support::{Map, Result, Value};

use super::{fit, object};

/// A Block Kit text object: plain text, or Slack's `mrkdwn` once you call
/// [`markdown`](TextObject::markdown).
///
/// Text longer than the field allows is truncated with an ellipsis.
///
/// ```
/// use illuminate_notifications::slack::TextObject;
/// use illuminate_support::json;
///
/// let mut text = TextObject::new("*Invoice No:*\n1000");
/// text.markdown().verbatim();
///
/// assert_eq!(text.to_array()?, json!({"type": "mrkdwn", "text": "*Invoice No:*\n1000", "verbatim": true}));
/// # Ok::<(), illuminate_support::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextObject {
    text: String,
    max_length: usize,
    markdown: bool,
    emoji: Option<bool>,
    verbatim: Option<bool>,
}

impl TextObject {
    /// Create a plain text object (up to 3,000 characters).
    pub fn new(text: impl Into<String>) -> Self {
        Self::limited(text, 3000)
    }

    /// Create a plain text object holding up to `max_length` characters.
    pub(crate) fn limited(text: impl Into<String>, max_length: usize) -> Self {
        Self {
            text: text.into(),
            max_length,
            markdown: false,
            emoji: None,
            verbatim: None,
        }
    }

    /// Format the text with Slack's `mrkdwn` syntax.
    pub fn markdown(&mut self) -> &mut Self {
        self.markdown = true;
        self
    }

    /// Escape emoji into their colon-wrapped shortcodes (plain text only).
    pub fn emoji(&mut self) -> &mut Self {
        self.emoji = Some(true);
        self
    }

    /// Stop Slack from linking URLs, channel names and mentions (`mrkdwn` only).
    pub fn verbatim(&mut self) -> &mut Self {
        self.verbatim = Some(true);
        self
    }

    /// Get the Slack representation of the text object.
    pub fn to_array(&self) -> Result<Value> {
        let mut text = Map::new();
        let kind = if self.markdown {
            "mrkdwn"
        } else {
            "plain_text"
        };
        text.insert("type".into(), kind.into());
        text.insert("text".into(), fit(&self.text, self.max_length)?.into());
        match (self.markdown, self.emoji, self.verbatim) {
            (false, Some(emoji), _) => object::put(&mut text, "emoji", emoji),
            (true, _, Some(verbatim)) => object::put(&mut text, "verbatim", verbatim),
            _ => {}
        }
        Ok(Value::Object(text))
    }
}

/// A text object that may only ever be plain text: headers, button labels
/// and dialog titles.
///
/// ```
/// use illuminate_notifications::slack::PlainTextOnlyTextObject;
/// use illuminate_support::json;
///
/// let mut text = PlainTextOnlyTextObject::new("Budget Performance :tada:");
/// text.emoji();
///
/// assert_eq!(text.to_array()?, json!({"type": "plain_text", "text": "Budget Performance :tada:", "emoji": true}));
/// # Ok::<(), illuminate_support::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlainTextOnlyTextObject {
    text: String,
    max_length: usize,
    emoji: Option<bool>,
}

impl PlainTextOnlyTextObject {
    /// Create a plain text object (up to 3,000 characters).
    pub fn new(text: impl Into<String>) -> Self {
        Self::limited(text, 3000)
    }

    /// Create a plain text object holding up to `max_length` characters.
    pub(crate) fn limited(text: impl Into<String>, max_length: usize) -> Self {
        Self {
            text: text.into(),
            max_length,
            emoji: None,
        }
    }

    /// Escape emoji into their colon-wrapped shortcodes.
    pub fn emoji(&mut self) -> &mut Self {
        self.emoji = Some(true);
        self
    }

    /// The raw text, before it is truncated to fit.
    pub(crate) fn raw(&self) -> &str {
        &self.text
    }

    /// Get the Slack representation of the text object.
    pub fn to_array(&self) -> Result<Value> {
        let mut text = Map::new();
        text.insert("type".into(), "plain_text".into());
        text.insert("text".into(), fit(&self.text, self.max_length)?.into());
        object::put_some(&mut text, "emoji", self.emoji);
        Ok(Value::Object(text))
    }
}

/// The dialog Slack shows before a button's action is performed.
///
/// It asks "Are you sure?" with "Yes" and "No" buttons until you say
/// otherwise:
///
/// ```
/// use illuminate_notifications::slack::ConfirmObject;
/// use illuminate_support::json;
///
/// let mut dialog = ConfirmObject::new("Acknowledge the payment and send a thank you email?");
/// dialog.confirm("Send it");
/// dialog.deny("Not yet");
/// dialog.danger();
///
/// assert_eq!(dialog.to_array()?, json!({
///     "title": {"type": "plain_text", "text": "Are you sure?"},
///     "text": {"type": "plain_text", "text": "Acknowledge the payment and send a thank you email?"},
///     "confirm": {"type": "plain_text", "text": "Send it"},
///     "deny": {"type": "plain_text", "text": "Not yet"},
///     "style": "danger",
/// }));
/// # Ok::<(), illuminate_support::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmObject {
    title: PlainTextOnlyTextObject,
    text: TextObject,
    confirm: PlainTextOnlyTextObject,
    deny: PlainTextOnlyTextObject,
    style: Option<&'static str>,
}

impl Default for ConfirmObject {
    fn default() -> Self {
        Self::new("Please confirm this action.")
    }
}

impl ConfirmObject {
    /// Create a confirmation dialog explaining the action (up to 300 characters).
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            title: PlainTextOnlyTextObject::limited("Are you sure?", 100),
            text: TextObject::limited(text, 300),
            confirm: PlainTextOnlyTextObject::limited("Yes", 30),
            deny: PlainTextOnlyTextObject::limited("No", 30),
            style: None,
        }
    }

    /// Set the dialog's title (up to 100 characters).
    pub fn title(&mut self, title: impl Into<String>) -> &mut PlainTextOnlyTextObject {
        self.title = PlainTextOnlyTextObject::limited(title, 100);
        &mut self.title
    }

    /// Set the dialog's explanatory text (up to 300 characters).
    pub fn text(&mut self, text: impl Into<String>) -> &mut TextObject {
        self.text = TextObject::limited(text, 300);
        &mut self.text
    }

    /// Set the label of the button that confirms the action (up to 30 characters).
    pub fn confirm(&mut self, label: impl Into<String>) -> &mut PlainTextOnlyTextObject {
        self.confirm = PlainTextOnlyTextObject::limited(label, 30);
        &mut self.confirm
    }

    /// Set the label of the button that cancels the action (up to 30 characters).
    pub fn deny(&mut self, label: impl Into<String>) -> &mut PlainTextOnlyTextObject {
        self.deny = PlainTextOnlyTextObject::limited(label, 30);
        &mut self.deny
    }

    /// Give the confirm button a green, "primary" style.
    pub fn primary(&mut self) -> &mut Self {
        self.style = Some("primary");
        self
    }

    /// Give the confirm button a red, "danger" style.
    pub fn danger(&mut self) -> &mut Self {
        self.style = Some("danger");
        self
    }

    /// Get the Slack representation of the dialog.
    pub fn to_array(&self) -> Result<Value> {
        let mut dialog = Map::new();
        dialog.insert("title".into(), self.title.to_array()?);
        dialog.insert("text".into(), self.text.to_array()?);
        dialog.insert("confirm".into(), self.confirm.to_array()?);
        dialog.insert("deny".into(), self.deny.to_array()?);
        object::put_some(&mut dialog, "style", self.style);
        Ok(Value::Object(dialog))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn text_objects_are_plain_text_by_default() {
        let mut text = TextObject::new("Hello");
        assert_eq!(
            text.to_array().unwrap(),
            json!({"type": "plain_text", "text": "Hello"})
        );
        text.emoji().verbatim();
        // Verbatim only applies to mrkdwn...
        assert_eq!(
            text.to_array().unwrap(),
            json!({"type": "plain_text", "text": "Hello", "emoji": true})
        );
        // ...and emoji only to plain text.
        text.markdown();
        assert_eq!(
            text.to_array().unwrap(),
            json!({"type": "mrkdwn", "text": "Hello", "verbatim": true})
        );
    }

    #[test]
    fn text_is_truncated_to_fit_and_must_not_be_empty() {
        let text = TextObject::limited("a".repeat(31), 30);
        assert_eq!(
            text.to_array().unwrap()["text"],
            format!("{}...", "a".repeat(27))
        );
        let exact = TextObject::limited("a".repeat(30), 30);
        assert_eq!(exact.to_array().unwrap()["text"], "a".repeat(30));
        // Characters are counted, not bytes.
        let unicode = PlainTextOnlyTextObject::limited("é".repeat(30), 30);
        assert_eq!(unicode.to_array().unwrap()["text"], "é".repeat(30));

        let error = TextObject::new("").to_array().unwrap_err();
        assert_eq!(error.to_string(), "Text must be at least 1 character long.");
        assert!(PlainTextOnlyTextObject::new("").to_array().is_err());
    }

    #[test]
    fn confirm_objects_have_sensible_defaults() {
        let mut dialog = ConfirmObject::default();
        assert_eq!(
            dialog.to_array().unwrap(),
            json!({
                "title": {"type": "plain_text", "text": "Are you sure?"},
                "text": {"type": "plain_text", "text": "Please confirm this action."},
                "confirm": {"type": "plain_text", "text": "Yes"},
                "deny": {"type": "plain_text", "text": "No"},
            })
        );

        dialog.title("Really?").emoji();
        dialog.text("*This* cannot be undone.").markdown();
        dialog.confirm("Do it").emoji();
        dialog.deny("a".repeat(31));
        dialog.primary();
        let array = dialog.to_array().unwrap();
        assert_eq!(
            array["title"],
            json!({"type": "plain_text", "text": "Really?", "emoji": true})
        );
        assert_eq!(
            array["text"],
            json!({"type": "mrkdwn", "text": "*This* cannot be undone."})
        );
        assert_eq!(array["confirm"]["emoji"], true);
        assert_eq!(array["deny"]["text"], format!("{}...", "a".repeat(27)));
        assert_eq!(array["style"], "primary");

        let mut long = ConfirmObject::new("a".repeat(301));
        long.title("t".repeat(101));
        let array = long.to_array().unwrap();
        assert_eq!(array["text"]["text"].as_str().unwrap().chars().count(), 300);
        assert_eq!(
            array["title"]["text"].as_str().unwrap().chars().count(),
            100
        );
    }
}
