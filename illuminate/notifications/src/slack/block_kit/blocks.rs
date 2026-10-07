//! Block Kit blocks: the visual components a Slack message is stacked from.

use illuminate_support::{Map, Result, Value};

use super::composites::{PlainTextOnlyTextObject, TextObject};
use super::elements::{ButtonElement, Element, ImageElement};
use super::{ensure_block_id, ensure_max_length, object};
use crate::slack::LogicException;

/// Any Block Kit block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// A row of interactive elements.
    Actions(ActionsBlock),
    /// Small, secondary text and images.
    Context(ContextBlock),
    /// A horizontal rule.
    Divider(DividerBlock),
    /// Large, bold text.
    Header(HeaderBlock),
    /// A standalone image.
    Image(ImageBlock),
    /// Text, fields and an optional accessory.
    Section(SectionBlock),
    /// A block straight from a Block Kit Builder template, sent as-is.
    Raw(Value),
}

impl Block {
    /// Get the Slack representation of the block.
    pub fn to_array(&self) -> Result<Value> {
        match self {
            Block::Actions(block) => block.to_array(),
            Block::Context(block) => block.to_array(),
            Block::Divider(block) => block.to_array(),
            Block::Header(block) => block.to_array(),
            Block::Image(block) => block.to_array(),
            Block::Section(block) => block.to_array(),
            Block::Raw(block) => Ok(block.clone()),
        }
    }
}

macro_rules! into_block {
    ($($variant:ident($block:ty)),* $(,)?) => {
        $(
            impl From<$block> for Block {
                fn from(block: $block) -> Self {
                    Block::$variant(block)
                }
            }
        )*
    };
}

into_block! {
    Actions(ActionsBlock),
    Context(ContextBlock),
    Divider(DividerBlock),
    Header(HeaderBlock),
    Image(ImageBlock),
    Section(SectionBlock),
    Raw(Value),
}

/// Start a block's JSON object with its type and (optional) ID.
fn block(kind: &str, block_id: Option<&str>) -> Result<Map<String, Value>> {
    ensure_block_id(block_id)?;
    let mut block = Map::new();
    object::put(&mut block, "type", kind);
    Ok(block)
}

/// Finish a block's JSON object with its ID.
fn finish(mut block: Map<String, Value>, block_id: Option<&str>) -> Value {
    object::put_some(&mut block, "block_id", block_id);
    Value::Object(block)
}

/// A row of interactive elements: up to 25 buttons.
///
/// ```
/// use illuminate_notifications::slack::ActionsBlock;
/// use illuminate_support::json;
///
/// let mut block = ActionsBlock::new();
/// block.button("Acknowledge Invoice").primary();
/// block.button("Deny").danger().id("deny_invoice");
///
/// assert_eq!(block.to_array()?, json!({
///     "type": "actions",
///     "elements": [
///         {"type": "button", "text": {"type": "plain_text", "text": "Acknowledge Invoice"}, "action_id": "button_acknowledge_invoice", "style": "primary"},
///         {"type": "button", "text": {"type": "plain_text", "text": "Deny"}, "action_id": "deny_invoice", "style": "danger"},
///     ],
/// }));
/// # Ok::<(), illuminate_support::Error>(())
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionsBlock {
    block_id: Option<String>,
    elements: Vec<ButtonElement>,
}

impl ActionsBlock {
    /// Create an empty actions block.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the block's ID (up to 255 characters).
    pub fn id(&mut self, id: impl Into<String>) -> &mut Self {
        self.block_id = Some(id.into());
        self
    }

    /// Add a button to the block.
    pub fn button(&mut self, text: impl Into<String>) -> &mut ButtonElement {
        self.elements.push(ButtonElement::new(text));
        self.elements.last_mut().expect("a button was just added")
    }

    /// Get the Slack representation of the block.
    pub fn to_array(&self) -> Result<Value> {
        let mut block = block("actions", self.block_id.as_deref())?;
        if self.elements.is_empty() {
            return Err(LogicException::new(
                "There must be at least one element in each actions block.",
            )
            .into());
        }
        if self.elements.len() > 25 {
            return Err(LogicException::new(
                "There is a maximum of 25 elements in each actions block.",
            )
            .into());
        }
        let elements = self
            .elements
            .iter()
            .map(ButtonElement::to_array)
            .collect::<Result<Vec<_>>>()?;
        object::put(&mut block, "elements", elements);
        Ok(finish(block, self.block_id.as_deref()))
    }
}

/// Small, secondary text and images: up to 10 elements.
///
/// ```
/// use illuminate_notifications::slack::ContextBlock;
/// use illuminate_support::json;
///
/// let mut block = ContextBlock::new();
/// block.image("https://laravel.com/img/favicon/favicon-32x32.png", "Laravel");
/// block.text("*Customer* #1234").markdown();
///
/// assert_eq!(block.to_array()?, json!({
///     "type": "context",
///     "elements": [
///         {"type": "image", "image_url": "https://laravel.com/img/favicon/favicon-32x32.png", "alt_text": "Laravel"},
///         {"type": "mrkdwn", "text": "*Customer* #1234"},
///     ],
/// }));
/// # Ok::<(), illuminate_support::Error>(())
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextBlock {
    block_id: Option<String>,
    elements: Vec<Element>,
}

impl ContextBlock {
    /// Create an empty context block.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the block's ID (up to 255 characters).
    pub fn id(&mut self, id: impl Into<String>) -> &mut Self {
        self.block_id = Some(id.into());
        self
    }

    /// Add an image to the block.
    pub fn image(
        &mut self,
        url: impl Into<String>,
        alt_text: impl Into<String>,
    ) -> &mut ImageElement {
        self.elements
            .push(Element::Image(ImageElement::new(url, alt_text)));
        match self.elements.last_mut() {
            Some(Element::Image(image)) => image,
            _ => unreachable!("an image was just added"),
        }
    }

    /// Add text to the block (up to 3,000 characters).
    pub fn text(&mut self, text: impl Into<String>) -> &mut TextObject {
        self.elements.push(Element::Text(TextObject::new(text)));
        match self.elements.last_mut() {
            Some(Element::Text(text)) => text,
            _ => unreachable!("text was just added"),
        }
    }

    /// Get the Slack representation of the block.
    pub fn to_array(&self) -> Result<Value> {
        let mut block = block("context", self.block_id.as_deref())?;
        if self.elements.is_empty() {
            return Err(LogicException::new(
                "There must be at least one element in each context block.",
            )
            .into());
        }
        if self.elements.len() > 10 {
            return Err(LogicException::new(
                "There is a maximum of 10 elements in each context block.",
            )
            .into());
        }
        let elements = self
            .elements
            .iter()
            .map(Element::to_array)
            .collect::<Result<Vec<_>>>()?;
        object::put(&mut block, "elements", elements);
        Ok(finish(block, self.block_id.as_deref()))
    }
}

/// A horizontal rule, like `<hr>`.
///
/// ```
/// use illuminate_notifications::slack::DividerBlock;
/// use illuminate_support::json;
///
/// assert_eq!(DividerBlock::new().to_array()?, json!({"type": "divider"}));
/// # Ok::<(), illuminate_support::Error>(())
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DividerBlock {
    block_id: Option<String>,
}

impl DividerBlock {
    /// Create a divider.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the block's ID (up to 255 characters).
    pub fn id(&mut self, id: impl Into<String>) -> &mut Self {
        self.block_id = Some(id.into());
        self
    }

    /// Get the Slack representation of the block.
    pub fn to_array(&self) -> Result<Value> {
        let block = block("divider", self.block_id.as_deref())?;
        Ok(finish(block, self.block_id.as_deref()))
    }
}

/// Large, bold plain text (up to 150 characters).
///
/// ```
/// use illuminate_notifications::slack::HeaderBlock;
/// use illuminate_support::json;
///
/// let mut block = HeaderBlock::new("Invoice Paid :tada:");
/// block.emoji();
///
/// assert_eq!(block.to_array()?, json!({
///     "type": "header",
///     "text": {"type": "plain_text", "text": "Invoice Paid :tada:", "emoji": true},
/// }));
/// # Ok::<(), illuminate_support::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderBlock {
    block_id: Option<String>,
    text: PlainTextOnlyTextObject,
}

impl HeaderBlock {
    /// Create a header.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            block_id: None,
            text: PlainTextOnlyTextObject::limited(text, 150),
        }
    }

    /// Set the block's ID (up to 255 characters).
    pub fn id(&mut self, id: impl Into<String>) -> &mut Self {
        self.block_id = Some(id.into());
        self
    }

    /// Escape emoji into their colon-wrapped shortcodes.
    pub fn emoji(&mut self) -> &mut Self {
        self.text.emoji();
        self
    }

    /// The header's text object.
    pub fn text_object(&mut self) -> &mut PlainTextOnlyTextObject {
        &mut self.text
    }

    /// Get the Slack representation of the block.
    pub fn to_array(&self) -> Result<Value> {
        let mut block = block("header", self.block_id.as_deref())?;
        object::put(&mut block, "text", self.text.to_array()?);
        Ok(finish(block, self.block_id.as_deref()))
    }
}

/// A standalone image, with an optional title.
///
/// ```
/// use illuminate_notifications::slack::ImageBlock;
/// use illuminate_support::json;
///
/// let mut block = ImageBlock::new("https://laravel.test/chart.png", "Revenue this month");
/// block.title("Revenue").emoji();
///
/// assert_eq!(block.to_array()?, json!({
///     "type": "image",
///     "image_url": "https://laravel.test/chart.png",
///     "alt_text": "Revenue this month",
///     "title": {"type": "plain_text", "text": "Revenue", "emoji": true},
/// }));
/// # Ok::<(), illuminate_support::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageBlock {
    block_id: Option<String>,
    url: String,
    alt_text: String,
    title: Option<PlainTextOnlyTextObject>,
}

impl ImageBlock {
    /// Create an image block. Slack requires the alternative text.
    pub fn new(url: impl Into<String>, alt_text: impl Into<String>) -> Self {
        Self {
            block_id: None,
            url: url.into(),
            alt_text: alt_text.into(),
            title: None,
        }
    }

    /// Set the block's ID (up to 255 characters).
    pub fn id(&mut self, id: impl Into<String>) -> &mut Self {
        self.block_id = Some(id.into());
        self
    }

    /// Set the image's alternative text (up to 2,000 characters).
    pub fn alt(&mut self, alt_text: impl Into<String>) -> &mut Self {
        self.alt_text = alt_text.into();
        self
    }

    /// Set the image's title (up to 2,000 characters).
    pub fn title(&mut self, title: impl Into<String>) -> &mut PlainTextOnlyTextObject {
        self.title
            .insert(PlainTextOnlyTextObject::limited(title, 2000))
    }

    /// Get the Slack representation of the block.
    pub fn to_array(&self) -> Result<Value> {
        let mut block = block("image", self.block_id.as_deref())?;
        ensure_max_length(Some(&self.url), 3000, "url")?;
        if self.alt_text.is_empty() {
            return Err(LogicException::new("Alt text is required for an image block.").into());
        }
        ensure_max_length(Some(&self.alt_text), 2000, "alt_text")?;
        ensure_max_length(
            self.title.as_ref().map(PlainTextOnlyTextObject::raw),
            2000,
            "title",
        )?;
        object::put(&mut block, "image_url", self.url.as_str());
        object::put(&mut block, "alt_text", self.alt_text.as_str());
        if let Some(title) = &self.title {
            object::put(&mut block, "title", title.to_array()?);
        }
        Ok(finish(block, self.block_id.as_deref()))
    }
}

/// Text, up to 10 fields shown in two columns, and an optional accessory.
///
/// ```
/// use illuminate_notifications::slack::SectionBlock;
/// use illuminate_support::json;
///
/// let mut block = SectionBlock::new();
/// block.text("An invoice has been paid.");
/// block.field("*Invoice No:*\n1000").markdown();
/// block.field("*Invoice Recipient:*\ntaylor@laravel.com").markdown();
///
/// assert_eq!(block.to_array()?, json!({
///     "type": "section",
///     "text": {"type": "plain_text", "text": "An invoice has been paid."},
///     "fields": [
///         {"type": "mrkdwn", "text": "*Invoice No:*\n1000"},
///         {"type": "mrkdwn", "text": "*Invoice Recipient:*\ntaylor@laravel.com"},
///     ],
/// }));
/// # Ok::<(), illuminate_support::Error>(())
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SectionBlock {
    block_id: Option<String>,
    text: Option<TextObject>,
    fields: Vec<TextObject>,
    accessory: Option<Box<Element>>,
}

impl SectionBlock {
    /// Create an empty section.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the block's ID (up to 255 characters).
    pub fn id(&mut self, id: impl Into<String>) -> &mut Self {
        self.block_id = Some(id.into());
        self
    }

    /// Set the section's text (up to 3,000 characters).
    pub fn text(&mut self, text: impl Into<String>) -> &mut TextObject {
        self.text.insert(TextObject::limited(text, 3000))
    }

    /// Add a field to the section (up to 2,000 characters).
    pub fn field(&mut self, text: impl Into<String>) -> &mut TextObject {
        self.fields.push(TextObject::limited(text, 2000));
        self.fields.last_mut().expect("a field was just added")
    }

    /// Show an element (a button or an image) beside the section's text.
    ///
    /// ```
    /// use illuminate_notifications::slack::{ButtonElement, SectionBlock};
    ///
    /// let mut button = ButtonElement::new("View Invoice");
    /// button.url("https://laravel.test/invoices/1000");
    ///
    /// let mut block = SectionBlock::new();
    /// block.text("An invoice has been paid.");
    /// block.accessory(button);
    ///
    /// assert_eq!(block.to_array()?["accessory"]["url"], "https://laravel.test/invoices/1000");
    /// # Ok::<(), illuminate_support::Error>(())
    /// ```
    pub fn accessory(&mut self, element: impl Into<Element>) -> &mut Self {
        self.accessory = Some(Box::new(element.into()));
        self
    }

    /// Get the Slack representation of the block.
    pub fn to_array(&self) -> Result<Value> {
        let mut block = block("section", self.block_id.as_deref())?;
        if self.text.is_none() && self.fields.is_empty() {
            return Err(LogicException::new(
                "A section requires at least one block, or the text to be set.",
            )
            .into());
        }
        if self.fields.len() > 10 {
            return Err(LogicException::new(
                "There is a maximum of 10 fields in each section block.",
            )
            .into());
        }
        if let Some(text) = &self.text {
            object::put(&mut block, "text", text.to_array()?);
        }
        object::put_some(&mut block, "block_id", self.block_id.as_deref());
        if !self.fields.is_empty() {
            let fields = self
                .fields
                .iter()
                .map(TextObject::to_array)
                .collect::<Result<Vec<_>>>()?;
            object::put(&mut block, "fields", fields);
        }
        if let Some(accessory) = &self.accessory {
            object::put(&mut block, "accessory", accessory.to_array()?);
        }
        Ok(Value::Object(block))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn error(block: impl Into<Block>) -> String {
        block.into().to_array().unwrap_err().to_string()
    }

    #[test]
    fn every_block_may_have_an_id() {
        let mut actions = ActionsBlock::new();
        actions.id("actions_1").button("Go");
        let mut context = ContextBlock::new();
        context.id("context_1").text("Hi");
        let mut divider = DividerBlock::new();
        divider.id("divider_1");
        let mut header = HeaderBlock::new("Hello");
        header.id("header_1");
        let mut image = ImageBlock::new("https://laravel.test/a.png", "A");
        image.id("image_1");
        let mut section = SectionBlock::new();
        section.id("section_1").text("Hi");

        let blocks: Vec<Block> = vec![
            actions.into(),
            context.into(),
            divider.into(),
            header.into(),
            image.into(),
            section.into(),
        ];
        for (block, kind) in blocks.iter().zip([
            "actions", "context", "divider", "header", "image", "section",
        ]) {
            let array = block.to_array().unwrap();
            assert_eq!(array["type"], kind);
            assert_eq!(array["block_id"], format!("{kind}_1"));
        }
    }

    #[test]
    fn block_ids_are_limited_to_255_characters() {
        let id = "a".repeat(256);
        let message = "Maximum length for the block_id field is 255 characters.";
        let mut actions = ActionsBlock::new();
        actions.id(&id).button("Go");
        assert_eq!(error(actions), message);
        let mut context = ContextBlock::new();
        context.id(&id).text("Hi");
        assert_eq!(error(context), message);
        let mut divider = DividerBlock::new();
        divider.id(&id);
        assert_eq!(error(divider), message);
        let mut header = HeaderBlock::new("Hi");
        header.id(&id);
        assert_eq!(error(header), message);
        let mut image = ImageBlock::new("https://laravel.test/a.png", "A");
        image.id(&id);
        assert_eq!(error(image), message);
        let mut section = SectionBlock::new();
        section.id(&id).text("Hi");
        assert_eq!(error(section), message);
    }

    #[test]
    fn actions_blocks_hold_one_to_twenty_five_buttons() {
        assert_eq!(
            error(ActionsBlock::new()),
            "There must be at least one element in each actions block."
        );
        let mut block = ActionsBlock::new();
        for i in 0..26 {
            block.button(format!("Button {i}"));
        }
        assert_eq!(
            error(block.clone()),
            "There is a maximum of 25 elements in each actions block."
        );
        let mut block = ActionsBlock::new();
        for i in 0..25 {
            block.button(format!("Button {i}"));
        }
        assert_eq!(
            block.to_array().unwrap()["elements"]
                .as_array()
                .unwrap()
                .len(),
            25
        );
    }

    #[test]
    fn context_blocks_hold_one_to_ten_elements() {
        assert_eq!(
            error(ContextBlock::new()),
            "There must be at least one element in each context block."
        );
        let mut block = ContextBlock::new();
        for i in 0..11 {
            block.text(format!("Text {i}"));
        }
        assert_eq!(
            error(block),
            "There is a maximum of 10 elements in each context block."
        );
        let mut block = ContextBlock::new();
        block.image("https://laravel.test/a.png", "");
        assert_eq!(error(block), "Alt text is required for an image element.");
    }

    #[test]
    fn headers_are_truncated_to_150_characters() {
        let mut header = HeaderBlock::new("a".repeat(151));
        let array = header.to_array().unwrap();
        assert_eq!(array["text"]["text"], format!("{}...", "a".repeat(147)));
        header.text_object().emoji();
        assert_eq!(header.to_array().unwrap()["text"]["emoji"], true);
        assert_eq!(
            error(HeaderBlock::new("")),
            "Text must be at least 1 character long."
        );
    }

    #[test]
    fn image_blocks_validate_their_fields() {
        assert_eq!(
            error(ImageBlock::new("https://laravel.test/a.png", "")),
            "Alt text is required for an image block."
        );
        assert_eq!(
            error(ImageBlock::new("a".repeat(3001), "A")),
            "Maximum length for the url field is 3000 characters."
        );
        assert_eq!(
            error(ImageBlock::new(
                "https://laravel.test/a.png",
                "a".repeat(2001)
            )),
            "Maximum length for the alt_text field is 2000 characters."
        );
        let mut image = ImageBlock::new("https://laravel.test/a.png", "");
        image.alt("A").title("t".repeat(2001));
        assert_eq!(
            error(image),
            "Maximum length for the title field is 2000 characters."
        );
    }

    #[test]
    fn sections_need_text_or_fields() {
        assert_eq!(
            error(SectionBlock::new()),
            "A section requires at least one block, or the text to be set."
        );
        let mut block = SectionBlock::new();
        for i in 0..11 {
            block.field(format!("Field {i}"));
        }
        assert_eq!(
            error(block),
            "There is a maximum of 10 fields in each section block."
        );

        let mut block = SectionBlock::new();
        block.field("Only a field");
        block.accessory(ImageElement::new("https://laravel.test/a.png", "A"));
        assert_eq!(
            block.to_array().unwrap(),
            json!({
                "type": "section",
                "fields": [{"type": "plain_text", "text": "Only a field"}],
                "accessory": {"type": "image", "image_url": "https://laravel.test/a.png", "alt_text": "A"},
            })
        );

        let mut block = SectionBlock::new();
        block.text("a".repeat(3001));
        block.field("b".repeat(2001));
        let array = block.to_array().unwrap();
        assert_eq!(array["text"]["text"].as_str().unwrap().len(), 3000);
        assert_eq!(array["fields"][0]["text"].as_str().unwrap().len(), 2000);
    }

    #[test]
    fn raw_blocks_are_sent_as_is() {
        let raw = json!({"type": "rich_text", "elements": []});
        assert_eq!(Block::from(raw.clone()).to_array().unwrap(), raw);
    }
}
