//! Block Kit elements: buttons and images.

use illuminate_support::{Map, Result, Str, Value};

use super::composites::{ConfirmObject, PlainTextOnlyTextObject, TextObject};
use super::{ensure_max_length, object};
use crate::slack::LogicException;

/// An interactive button. Clicking it sends your Slack App's "Request URL"
/// a payload naming the button's [`id`](ButtonElement::id).
///
/// The ID defaults to a slug of the button's text:
///
/// ```
/// use illuminate_notifications::slack::ButtonElement;
/// use illuminate_support::json;
///
/// let mut button = ButtonElement::new("Acknowledge Invoice");
/// button.primary().value("invoice_1000");
///
/// assert_eq!(button.to_array()?, json!({
///     "type": "button",
///     "text": {"type": "plain_text", "text": "Acknowledge Invoice"},
///     "action_id": "button_acknowledge_invoice",
///     "value": "invoice_1000",
///     "style": "primary",
/// }));
/// # Ok::<(), illuminate_support::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ButtonElement {
    text: PlainTextOnlyTextObject,
    action_id: String,
    url: Option<String>,
    value: Option<String>,
    style: Option<&'static str>,
    confirm: Option<Box<ConfirmObject>>,
    accessibility_label: Option<String>,
}

impl ButtonElement {
    /// Create a button with the given label (up to 75 characters).
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let slug: String = text.chars().take(248).collect();
        Self {
            action_id: format!("button_{}", Str::lower(&Str::slug_with(&slug, "_"))),
            text: PlainTextOnlyTextObject::limited(text, 75),
            url: None,
            value: None,
            style: None,
            confirm: None,
            accessibility_label: None,
        }
    }

    /// Open the given URL in the user's browser when the button is clicked.
    pub fn url(&mut self, url: impl Into<String>) -> &mut Self {
        self.url = Some(url.into());
        self
    }

    /// Set the button's action ID, which identifies it in interaction payloads.
    pub fn id(&mut self, id: impl Into<String>) -> &mut Self {
        self.action_id = id.into();
        self
    }

    /// Set the value sent along with interaction payloads.
    pub fn value(&mut self, value: impl Into<String>) -> &mut Self {
        self.value = Some(value.into());
        self
    }

    /// Make the button green: the primary call to action.
    pub fn primary(&mut self) -> &mut Self {
        self.style = Some("primary");
        self
    }

    /// Make the button red: a destructive action.
    pub fn danger(&mut self) -> &mut Self {
        self.style = Some("danger");
        self
    }

    /// Ask the user to confirm before the action is performed.
    ///
    /// ```
    /// use illuminate_notifications::slack::ButtonElement;
    ///
    /// let mut button = ButtonElement::new("Acknowledge Invoice");
    /// button.confirm("Acknowledge the payment and send a thank you email?", |dialog| {
    ///     dialog.confirm("Yes");
    ///     dialog.deny("No");
    /// });
    ///
    /// assert_eq!(button.to_array()?["confirm"]["deny"]["text"], "No");
    /// # Ok::<(), illuminate_support::Error>(())
    /// ```
    pub fn confirm(
        &mut self,
        text: impl Into<String>,
        callback: impl FnOnce(&mut ConfirmObject),
    ) -> &mut Self {
        let mut dialog = ConfirmObject::new(text);
        callback(&mut dialog);
        self.confirm = Some(Box::new(dialog));
        self
    }

    /// Set the label screen readers announce instead of the button's text.
    pub fn accessibility_label(&mut self, label: impl Into<String>) -> &mut Self {
        self.accessibility_label = Some(label.into());
        self
    }

    /// Get the Slack representation of the button.
    pub fn to_array(&self) -> Result<Value> {
        ensure_max_length(Some(&self.action_id), 255, "action_id")?;
        ensure_max_length(self.url.as_deref(), 3000, "url")?;
        ensure_max_length(self.value.as_deref(), 2000, "value")?;
        ensure_max_length(
            self.accessibility_label.as_deref(),
            75,
            "accessibility_label",
        )?;

        let mut button = Map::new();
        object::put(&mut button, "type", "button");
        object::put(&mut button, "text", self.text.to_array()?);
        object::put(&mut button, "action_id", self.action_id.as_str());
        object::put_some(&mut button, "url", self.url.as_deref());
        object::put_some(&mut button, "value", self.value.as_deref());
        object::put_some(&mut button, "style", self.style);
        if let Some(confirm) = &self.confirm {
            object::put(&mut button, "confirm", confirm.to_array()?);
        }
        object::put_some(
            &mut button,
            "accessibility_label",
            self.accessibility_label.as_deref(),
        );
        Ok(Value::Object(button))
    }
}

/// An image shown inside a context block (or as a section's accessory).
///
/// ```
/// use illuminate_notifications::slack::ImageElement;
/// use illuminate_support::json;
///
/// let image = ImageElement::new("https://laravel.com/img/logomark.min.svg", "Laravel");
///
/// assert_eq!(image.to_array()?, json!({
///     "type": "image",
///     "image_url": "https://laravel.com/img/logomark.min.svg",
///     "alt_text": "Laravel",
/// }));
/// # Ok::<(), illuminate_support::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageElement {
    url: String,
    alt_text: String,
}

impl ImageElement {
    /// Create an image element. Slack requires the alternative text.
    pub fn new(url: impl Into<String>, alt_text: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            alt_text: alt_text.into(),
        }
    }

    /// Set the image's alternative text.
    pub fn alt(&mut self, alt_text: impl Into<String>) -> &mut Self {
        self.alt_text = alt_text.into();
        self
    }

    /// Get the Slack representation of the image.
    pub fn to_array(&self) -> Result<Value> {
        if self.alt_text.is_empty() {
            return Err(LogicException::new("Alt text is required for an image element.").into());
        }
        let mut image = Map::new();
        object::put(&mut image, "type", "image");
        object::put(&mut image, "image_url", self.url.as_str());
        object::put(&mut image, "alt_text", self.alt_text.as_str());
        Ok(Value::Object(image))
    }
}

/// Any Block Kit element: what context blocks hold, and what a section
/// may show as its [`accessory`](super::blocks::SectionBlock::accessory).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Element {
    /// A button.
    Button(ButtonElement),
    /// An image.
    Image(ImageElement),
    /// A text object.
    Text(TextObject),
}

impl Element {
    /// Get the Slack representation of the element.
    pub fn to_array(&self) -> Result<Value> {
        match self {
            Element::Button(button) => button.to_array(),
            Element::Image(image) => image.to_array(),
            Element::Text(text) => text.to_array(),
        }
    }
}

impl From<ButtonElement> for Element {
    fn from(button: ButtonElement) -> Self {
        Element::Button(button)
    }
}

impl From<ImageElement> for Element {
    fn from(image: ImageElement) -> Self {
        Element::Image(image)
    }
}

impl From<TextObject> for Element {
    fn from(text: TextObject) -> Self {
        Element::Text(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn buttons_have_every_option() {
        let mut button = ButtonElement::new("Deny");
        button
            .danger()
            .id("deny_invoice")
            .url("https://laravel.test/invoices/1")
            .value("1")
            .accessibility_label("Deny the invoice")
            .confirm("Deny the invoice?", |dialog| {
                dialog.danger();
            });
        assert_eq!(
            button.to_array().unwrap(),
            json!({
                "type": "button",
                "text": {"type": "plain_text", "text": "Deny"},
                "action_id": "deny_invoice",
                "url": "https://laravel.test/invoices/1",
                "value": "1",
                "style": "danger",
                "confirm": {
                    "title": {"type": "plain_text", "text": "Are you sure?"},
                    "text": {"type": "plain_text", "text": "Deny the invoice?"},
                    "confirm": {"type": "plain_text", "text": "Yes"},
                    "deny": {"type": "plain_text", "text": "No"},
                    "style": "danger",
                },
                "accessibility_label": "Deny the invoice",
            })
        );
    }

    #[test]
    fn button_text_is_truncated_and_ids_are_slugged() {
        let button = ButtonElement::new("a".repeat(80));
        let array = button.to_array().unwrap();
        assert_eq!(array["text"]["text"], format!("{}...", "a".repeat(72)));
        assert_eq!(array["action_id"], format!("button_{}", "a".repeat(80)));
        assert_eq!(
            ButtonElement::new("Pay $10 @ Laravel!").to_array().unwrap()["action_id"],
            "button_pay_10_at_laravel"
        );
        // Long labels make IDs that still fit Slack's limit.
        let long = ButtonElement::new("b".repeat(300)).to_array().unwrap();
        assert_eq!(long["action_id"].as_str().unwrap().len(), 255);
    }

    #[test]
    fn button_limits_are_validated() {
        type Configure = fn(&mut ButtonElement);
        let cases: [(Configure, &str); 4] = [
            (
                |button| {
                    button.id("a".repeat(256));
                },
                "Maximum length for the action_id field is 255 characters.",
            ),
            (
                |button| {
                    button.url("a".repeat(3001));
                },
                "Maximum length for the url field is 3000 characters.",
            ),
            (
                |button| {
                    button.value("a".repeat(2001));
                },
                "Maximum length for the value field is 2000 characters.",
            ),
            (
                |button| {
                    button.accessibility_label("a".repeat(76));
                },
                "Maximum length for the accessibility_label field is 75 characters.",
            ),
        ];
        for (configure, message) in cases {
            let mut button = ButtonElement::new("Click Me");
            configure(&mut button);
            assert_eq!(button.to_array().unwrap_err().to_string(), message);
        }
        assert_eq!(
            ButtonElement::new("").to_array().unwrap_err().to_string(),
            "Text must be at least 1 character long."
        );
    }

    #[test]
    fn images_require_alt_text() {
        let mut image = ImageElement::new("https://laravel.test/logo.png", "");
        let error = image.to_array().unwrap_err();
        assert!(error.is::<LogicException>());
        assert_eq!(
            error.to_string(),
            "Alt text is required for an image element."
        );
        image.alt("Logo");
        assert_eq!(image.to_array().unwrap()["alt_text"], "Logo");
    }

    #[test]
    fn elements_wrap_anything() {
        let elements: Vec<Element> = vec![
            ButtonElement::new("Go").into(),
            ImageElement::new("https://laravel.test/a.png", "A").into(),
            TextObject::new("Hi").into(),
        ];
        let types: Vec<Value> = elements
            .iter()
            .map(|element| element.to_array().unwrap()["type"].clone())
            .collect();
        assert_eq!(
            types,
            vec![json!("button"), json!("image"), json!("plain_text")]
        );
    }
}
