//! [`SlackMessage`]: a notification, as a Block Kit message.

use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Conditionable, Map, Result, Value, to_value};
use serde::Serialize;

use super::LogicException;
use super::block_kit::blocks::{
    ActionsBlock, Block, ContextBlock, DividerBlock, HeaderBlock, ImageBlock, SectionBlock,
};
use super::block_kit::composites::PlainTextOnlyTextObject;
use super::block_kit::object;

/// Structured data attached to a message, which your Slack App receives
/// in its events (Slack's message metadata).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventMetadata {
    /// The event's type, like `task_created`.
    pub event_type: String,
    /// The event's payload.
    pub event_payload: Value,
}

impl EventMetadata {
    /// Create event metadata.
    pub fn new(event_type: impl Into<String>, payload: impl Serialize) -> Self {
        let event_payload = match to_value(&payload) {
            Value::Null => Value::Object(Map::new()),
            payload => payload,
        };
        Self {
            event_type: event_type.into(),
            event_payload,
        }
    }

    /// Get the Slack representation of the metadata.
    pub fn to_array(&self) -> Value {
        let mut metadata = Map::new();
        object::put(&mut metadata, "event_type", self.event_type.as_str());
        object::put(&mut metadata, "event_payload", self.event_payload.clone());
        Value::Object(metadata)
    }
}

/// A Slack notification: plain text, Block Kit blocks, or both.
///
/// Build blocks with closures that receive each block as it is added:
///
/// ```
/// use illuminate_notifications::slack::SlackMessage;
/// use illuminate_support::json;
///
/// let message = SlackMessage::new()
///     .text("One of your invoices has been paid!")
///     .header_block("Invoice Paid")
///     .context_block(|block| {
///         block.text("Customer #1234");
///     })
///     .section_block(|block| {
///         block.text("An invoice has been paid.");
///         block.field("*Invoice No:*\n1000").markdown();
///         block.field("*Invoice Recipient:*\ntaylor@laravel.com").markdown();
///     })
///     .divider_block()
///     .section_block(|block| {
///         block.text("Congratulations!");
///     });
///
/// assert_eq!(message.to_array()?, json!({
///     "text": "One of your invoices has been paid!",
///     "blocks": [
///         {"type": "header", "text": {"type": "plain_text", "text": "Invoice Paid"}},
///         {"type": "context", "elements": [{"type": "plain_text", "text": "Customer #1234"}]},
///         {
///             "type": "section",
///             "text": {"type": "plain_text", "text": "An invoice has been paid."},
///             "fields": [
///                 {"type": "mrkdwn", "text": "*Invoice No:*\n1000"},
///                 {"type": "mrkdwn", "text": "*Invoice Recipient:*\ntaylor@laravel.com"},
///             ],
///         },
///         {"type": "divider"},
///         {"type": "section", "text": {"type": "plain_text", "text": "Congratulations!"}},
///     ],
/// }));
/// # Ok::<(), illuminate_support::Error>(())
/// ```
///
/// Interactive messages add buttons with
/// [`actions_block`](SlackMessage::actions_block). Slack posts a payload
/// to your App's "Request URL" when one is clicked:
///
/// ```
/// use illuminate_notifications::slack::SlackMessage;
///
/// let message = SlackMessage::new()
///     .text("One of your invoices has been paid!")
///     .actions_block(|block| {
///         // ID defaults to "button_acknowledge_invoice"...
///         block.button("Acknowledge Invoice").primary().confirm(
///             "Acknowledge the payment and send a thank you email?",
///             |dialog| {
///                 dialog.confirm("Yes");
///                 dialog.deny("No");
///             },
///         );
///
///         // Manually configure the ID...
///         block.button("Deny").danger().id("deny_invoice");
///     });
///
/// let blocks = &message.to_array()?["blocks"];
/// assert_eq!(blocks[0]["elements"][0]["action_id"], "button_acknowledge_invoice");
/// assert_eq!(blocks[0]["elements"][1]["action_id"], "deny_invoice");
/// # Ok::<(), illuminate_support::Error>(())
/// ```
///
/// Messages are [`Conditionable`], so `when` and `unless` work too.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[must_use = "Slack messages do nothing until they are returned from `to_slack`"]
pub struct SlackMessage {
    /// The channel to send the message to (when the notifiable doesn't route it).
    pub channel: Option<String>,
    /// The message's text: the whole message, or the notification's
    /// fallback text when it has blocks.
    pub text: Option<String>,
    /// The message's Block Kit blocks.
    pub blocks: Vec<Block>,
    /// The emoji to use as the bot's icon.
    pub icon: Option<String>,
    /// The image URL to use as the bot's icon.
    pub image: Option<String>,
    /// The message's event metadata.
    pub metadata: Option<EventMetadata>,
    /// Whether Slack should parse `mrkdwn` in the message's text.
    pub mrkdwn: Option<bool>,
    /// Whether Slack should unfurl text-based links.
    pub unfurl_links: Option<bool>,
    /// Whether Slack should unfurl media links.
    pub unfurl_media: Option<bool>,
    /// The timestamp of the message to reply to, in a thread.
    pub thread_ts: Option<String>,
    /// Whether a threaded reply should also be posted to the channel.
    pub broadcast_reply: Option<bool>,
    /// The bot's username.
    pub username: Option<String>,
    template_error: Option<String>,
}

impl Conditionable for SlackMessage {}

impl SlackMessage {
    /// Create a new Slack message.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the channel the message should be sent to, like `#general`.
    pub fn to(mut self, channel: impl Into<String>) -> Self {
        self.channel = Some(channel.into());
        self
    }

    /// Set the message's text. When the message has blocks, this is the
    /// fallback shown in notifications.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Add an actions block: a row of buttons.
    pub fn actions_block(self, callback: impl FnOnce(&mut ActionsBlock)) -> Self {
        let mut block = ActionsBlock::new();
        callback(&mut block);
        self.push(block)
    }

    /// Add a context block: small, secondary text and images.
    pub fn context_block(self, callback: impl FnOnce(&mut ContextBlock)) -> Self {
        let mut block = ContextBlock::new();
        callback(&mut block);
        self.push(block)
    }

    /// Add a divider.
    pub fn divider_block(self) -> Self {
        self.push(DividerBlock::new())
    }

    /// Add a header: large, bold text (up to 150 characters).
    pub fn header_block(self, text: impl Into<String>) -> Self {
        self.push(HeaderBlock::new(text))
    }

    /// Add a header, configuring its text with the callback.
    ///
    /// ```
    /// use illuminate_notifications::slack::SlackMessage;
    ///
    /// let message = SlackMessage::new().header_block_with("Budget Performance :rocket:", |text| {
    ///     text.emoji();
    /// });
    ///
    /// assert_eq!(message.to_array()?["blocks"][0]["text"]["emoji"], true);
    /// # Ok::<(), illuminate_support::Error>(())
    /// ```
    pub fn header_block_with(
        self,
        text: impl Into<String>,
        callback: impl FnOnce(&mut PlainTextOnlyTextObject),
    ) -> Self {
        let mut block = HeaderBlock::new(text);
        callback(block.text_object());
        self.push(block)
    }

    /// Add an image block. Slack requires the alternative text.
    pub fn image_block(self, url: impl Into<String>, alt_text: impl Into<String>) -> Self {
        self.push(ImageBlock::new(url, alt_text))
    }

    /// Add an image block, configuring it with the callback.
    ///
    /// ```
    /// use illuminate_notifications::slack::SlackMessage;
    ///
    /// let message = SlackMessage::new().image_block_with("https://laravel.test/chart.png", "Revenue chart", |image| {
    ///     image.title("Revenue");
    ///     image.id("revenue_chart");
    /// });
    ///
    /// assert_eq!(message.to_array()?["blocks"][0]["title"]["text"], "Revenue");
    /// # Ok::<(), illuminate_support::Error>(())
    /// ```
    pub fn image_block_with(
        self,
        url: impl Into<String>,
        alt_text: impl Into<String>,
        callback: impl FnOnce(&mut ImageBlock),
    ) -> Self {
        let mut block = ImageBlock::new(url, alt_text);
        callback(&mut block);
        self.push(block)
    }

    /// Add a section block: text, fields and an optional accessory.
    pub fn section_block(self, callback: impl FnOnce(&mut SectionBlock)) -> Self {
        let mut block = SectionBlock::new();
        callback(&mut block);
        self.push(block)
    }

    /// Add a block you built yourself.
    pub fn block(self, block: impl Into<Block>) -> Self {
        self.push(block)
    }

    fn push(mut self, block: impl Into<Block>) -> Self {
        self.blocks.push(block.into());
        self
    }

    /// Use the blocks of a template designed in Slack's
    /// [Block Kit Builder](https://app.slack.com/block-kit-builder) (its
    /// `{"blocks": [...]}` payload, or just the array of blocks).
    ///
    /// ```
    /// use illuminate_notifications::slack::SlackMessage;
    ///
    /// let message = SlackMessage::new().using_block_kit_template(r#"{
    ///     "blocks": [
    ///         {"type": "header", "text": {"type": "plain_text", "text": "Team Announcement"}},
    ///         {"type": "section", "text": {"type": "plain_text", "text": "We are hiring!"}}
    ///     ]
    /// }"#);
    ///
    /// assert_eq!(message.to_array()?["blocks"][1]["text"]["text"], "We are hiring!");
    /// # Ok::<(), illuminate_support::Error>(())
    /// ```
    ///
    /// Templates that aren't valid JSON fail when the message is sent.
    pub fn using_block_kit_template(mut self, template: impl AsRef<str>) -> Self {
        let blocks = match serde_json::from_str::<Value>(template.as_ref()) {
            Ok(Value::Object(mut template)) => template.remove("blocks"),
            Ok(blocks) => Some(blocks),
            Err(error) => {
                self.template_error = Some(format!(
                    "The Block Kit template is not valid JSON: {error}."
                ));
                return self;
            }
        };
        match blocks {
            Some(Value::Array(blocks)) => self.blocks.extend(blocks.into_iter().map(Block::Raw)),
            _ => {
                self.template_error =
                    Some("The Block Kit template must contain an array of blocks.".into());
            }
        }
        self
    }

    /// Set the bot's username.
    pub fn username(mut self, username: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self
    }

    /// Use an emoji, like `:ghost:`, as the bot's icon.
    pub fn emoji(mut self, emoji: impl Into<String>) -> Self {
        self.image = None;
        self.icon = Some(emoji.into());
        self
    }

    /// Use an image URL as the bot's icon.
    pub fn image(mut self, image: impl Into<String>) -> Self {
        self.icon = None;
        self.image = Some(image.into());
        self
    }

    /// Attach event metadata to the message.
    ///
    /// ```
    /// use illuminate_notifications::slack::SlackMessage;
    /// use illuminate_support::json;
    ///
    /// let message = SlackMessage::new()
    ///     .text("Task created")
    ///     .metadata("task_created", json!({"id": "11223", "title": "Redesign Homepage"}));
    ///
    /// assert_eq!(message.to_array()?["metadata"], json!({
    ///     "event_type": "task_created",
    ///     "event_payload": {"id": "11223", "title": "Redesign Homepage"},
    /// }));
    /// # Ok::<(), illuminate_support::Error>(())
    /// ```
    pub fn metadata(mut self, event_type: impl Into<String>, payload: impl Serialize) -> Self {
        self.metadata = Some(EventMetadata::new(event_type, payload));
        self
    }

    /// Stop Slack from parsing `mrkdwn` in the message's text.
    pub fn disable_markdown_parsing(mut self) -> Self {
        self.mrkdwn = Some(false);
        self
    }

    /// Choose whether Slack unfurls text-based links.
    pub fn unfurl_links(mut self, unfurl: bool) -> Self {
        self.unfurl_links = Some(unfurl);
        self
    }

    /// Choose whether Slack unfurls media links.
    pub fn unfurl_media(mut self, unfurl: bool) -> Self {
        self.unfurl_media = Some(unfurl);
        self
    }

    /// Send the message as a reply in the thread of the given message
    /// timestamp.
    pub fn thread_timestamp(mut self, timestamp: impl Into<String>) -> Self {
        self.thread_ts = Some(timestamp.into());
        self
    }

    /// Choose whether a threaded reply is also posted to the channel.
    pub fn broadcast_reply(mut self, broadcast: bool) -> Self {
        self.broadcast_reply = Some(broadcast);
        self
    }

    /// Get the message's `chat.postMessage` payload.
    ///
    /// Fails when the message breaks one of Slack's rules: it must have
    /// text or blocks, at most 50 blocks, and every block must be valid.
    pub fn to_array(&self) -> Result<Value> {
        if let Some(error) = &self.template_error {
            return Err(InvalidArgumentException::new(error.clone()).into());
        }
        if self.blocks.is_empty() && self.text.is_none() {
            return Err(LogicException::new(
                "Slack messages must contain at least a text message or block.",
            )
            .into());
        }
        if self.blocks.len() > 50 {
            return Err(
                LogicException::new("Slack messages can only contain up to 50 blocks.").into(),
            );
        }

        let mut payload = Map::new();
        object::put_some(&mut payload, "channel", self.channel.as_deref());
        object::put_some(&mut payload, "text", self.text.as_deref());
        if !self.blocks.is_empty() {
            let blocks = self
                .blocks
                .iter()
                .map(Block::to_array)
                .collect::<Result<Vec<_>>>()?;
            object::put(&mut payload, "blocks", blocks);
        }
        object::put_some(&mut payload, "icon_emoji", self.icon.as_deref());
        object::put_some(&mut payload, "icon_url", self.image.as_deref());
        object::put_some(
            &mut payload,
            "metadata",
            self.metadata.as_ref().map(EventMetadata::to_array),
        );
        object::put_some(&mut payload, "mrkdwn", self.mrkdwn);
        object::put_some(&mut payload, "thread_ts", self.thread_ts.as_deref());
        object::put_some(&mut payload, "reply_broadcast", self.broadcast_reply);
        object::put_some(&mut payload, "unfurl_links", self.unfurl_links);
        object::put_some(&mut payload, "unfurl_media", self.unfurl_media);
        object::put_some(&mut payload, "username", self.username.as_deref());
        Ok(Value::Object(payload))
    }

    /// Get a link that previews the message's blocks in Slack's Block Kit
    /// Builder.
    ///
    /// ```
    /// use illuminate_notifications::slack::SlackMessage;
    ///
    /// let url = SlackMessage::new().header_block("Invoice Paid").block_kit_builder_url()?;
    ///
    /// assert!(url.starts_with("https://app.slack.com/block-kit-builder#%7B%22blocks%22"));
    /// # Ok::<(), illuminate_support::Error>(())
    /// ```
    pub fn block_kit_builder_url(&self) -> Result<String> {
        let mut payload = self.to_array()?;
        if let Value::Object(fields) = &mut payload {
            fields.retain(|key, _| !matches!(key.as_str(), "username" | "text" | "channel"));
        }
        Ok(format!(
            "https://app.slack.com/block-kit-builder#{}",
            raw_url_encode(&serde_json::to_string(&payload)?)
        ))
    }

    /// Dump a Block Kit Builder link previewing the message, then end the
    /// process.
    pub fn dd(&self) -> ! {
        match self.block_kit_builder_url() {
            Ok(url) => println!("{url}"),
            Err(error) => println!("{error}"),
        }
        std::process::exit(1)
    }

    /// Dump the message's raw payload, then end the process.
    pub fn dd_raw(&self) -> ! {
        match self.to_array() {
            Ok(payload) => println!("{payload:#}"),
            Err(error) => println!("{error}"),
        }
        std::process::exit(1)
    }
}

/// Percent-encode a string like PHP's `rawurlencode`.
fn raw_url_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn simple_messages_only_need_text() {
        let message = SlackMessage::new().text("This is a simple Web API text message.");
        assert_eq!(
            message.to_array().unwrap(),
            json!({"text": "This is a simple Web API text message."})
        );
    }

    #[test]
    fn messages_need_text_or_blocks() {
        let error = SlackMessage::new().to_array().unwrap_err();
        assert!(error.is::<LogicException>());
        assert_eq!(
            error.to_string(),
            "Slack messages must contain at least a text message or block."
        );
        assert!(SlackMessage::new().divider_block().to_array().is_ok());
    }

    #[test]
    fn messages_hold_at_most_fifty_blocks() {
        let fifty = (0..50).fold(SlackMessage::new(), |message, _| message.divider_block());
        assert!(fifty.to_array().is_ok());
        let error = fifty.divider_block().to_array().unwrap_err();
        assert_eq!(
            error.to_string(),
            "Slack messages can only contain up to 50 blocks."
        );
    }

    #[test]
    fn invalid_blocks_make_the_message_invalid() {
        let error = SlackMessage::new()
            .text("Hello")
            .section_block(|_| {})
            .to_array()
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "A section requires at least one block, or the text to be set."
        );
    }

    #[test]
    fn every_option_is_sent() {
        let message = SlackMessage::new()
            .to("#ghost-talk")
            .text("Boo!")
            .username("larabot")
            .emoji(":ghost:")
            .metadata("task_created", json!({"id": "11223"}))
            .disable_markdown_parsing()
            .unfurl_links(true)
            .unfurl_media(false)
            .thread_timestamp("123456.7890")
            .broadcast_reply(true);
        assert_eq!(
            message.to_array().unwrap(),
            json!({
                "channel": "#ghost-talk",
                "text": "Boo!",
                "icon_emoji": ":ghost:",
                "metadata": {"event_type": "task_created", "event_payload": {"id": "11223"}},
                "mrkdwn": false,
                "thread_ts": "123456.7890",
                "reply_broadcast": true,
                "unfurl_links": true,
                "unfurl_media": false,
                "username": "larabot",
            })
        );
        let array = message.to_array().unwrap();
        let keys: Vec<&str> = array
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .take(2)
            .collect();
        assert_eq!(keys, ["channel", "text"]);
    }

    #[test]
    fn icons_are_an_emoji_or_an_image() {
        let message = SlackMessage::new()
            .text("Hi")
            .emoji(":ghost:")
            .image("https://laravel.test/bot.png");
        let array = message.to_array().unwrap();
        assert_eq!(array["icon_url"], "https://laravel.test/bot.png");
        assert!(array.get("icon_emoji").is_none());

        let array = message.emoji(":tada:").to_array().unwrap();
        assert_eq!(array["icon_emoji"], ":tada:");
        assert!(array.get("icon_url").is_none());
    }

    #[test]
    fn metadata_payloads_default_to_an_empty_object() {
        let message = SlackMessage::new().text("Hi").metadata("ping", ());
        assert_eq!(
            message.to_array().unwrap()["metadata"],
            json!({"event_type": "ping", "event_payload": {}})
        );
    }

    #[test]
    fn every_block_type_can_be_added() {
        let mut button = crate::slack::ButtonElement::new("View");
        button.url("https://laravel.test");
        let message = SlackMessage::new()
            .header_block("Header")
            .header_block_with("Emoji :tada:", |text| {
                text.emoji();
            })
            .context_block(|block| {
                block.image("https://laravel.test/a.png", "A");
                block.text("_Context_").markdown();
            })
            .section_block(|block| {
                block.text("Section").markdown();
                block.accessory(button);
            })
            .divider_block()
            .image_block("https://laravel.test/b.png", "B")
            .image_block_with("https://laravel.test/c.png", "C", |image| {
                image.title("Title");
            })
            .actions_block(|block| {
                block.button("Approve").primary();
            })
            .block(crate::slack::DividerBlock::new());
        let blocks = message.to_array().unwrap()["blocks"].clone();
        let types: Vec<&str> = blocks
            .as_array()
            .unwrap()
            .iter()
            .map(|block| block["type"].as_str().unwrap())
            .collect();
        assert_eq!(
            types,
            [
                "header", "header", "context", "section", "divider", "image", "image", "actions",
                "divider"
            ]
        );
        assert_eq!(blocks[1]["text"]["emoji"], true);
        assert_eq!(blocks[2]["elements"][1]["type"], "mrkdwn");
        assert_eq!(blocks[3]["accessory"]["url"], "https://laravel.test");
        assert_eq!(blocks[6]["title"]["text"], "Title");
        assert_eq!(blocks[7]["elements"][0]["style"], "primary");
    }

    #[test]
    fn block_kit_templates_are_merged_into_the_blocks() {
        let message = SlackMessage::new()
            .header_block("Before")
            .using_block_kit_template(r#"[{"type": "divider"}]"#)
            .using_block_kit_template(
                r#"{"blocks": [{"type": "section", "text": {"type": "mrkdwn", "text": "*Hi*"}}]}"#,
            );
        assert_eq!(
            message.to_array().unwrap()["blocks"],
            json!([
                {"type": "header", "text": {"type": "plain_text", "text": "Before"}},
                {"type": "divider"},
                {"type": "section", "text": {"type": "mrkdwn", "text": "*Hi*"}},
            ])
        );

        let error = SlackMessage::new()
            .using_block_kit_template("{nope")
            .to_array()
            .unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("The Block Kit template is not valid JSON:")
        );
        let error = SlackMessage::new()
            .using_block_kit_template(r#"{"text": "no blocks"}"#)
            .to_array()
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "The Block Kit template must contain an array of blocks."
        );
    }

    #[test]
    fn messages_are_conditionable() {
        let message = SlackMessage::new()
            .text("Hi")
            .when(true, |message| message.to("#general"))
            .unless(true, |message| message.username("never"));
        assert_eq!(message.channel.as_deref(), Some("#general"));
        assert!(message.username.is_none());
    }

    #[test]
    fn block_kit_builder_links_preview_the_blocks() {
        let message = SlackMessage::new()
            .to("#general")
            .username("larabot")
            .text("Fallback")
            .divider_block();
        assert_eq!(
            message.block_kit_builder_url().unwrap(),
            "https://app.slack.com/block-kit-builder#%7B%22blocks%22%3A%5B%7B%22type%22%3A%22divider%22%7D%5D%7D"
        );
        assert!(SlackMessage::new().block_kit_builder_url().is_err());
    }
}
