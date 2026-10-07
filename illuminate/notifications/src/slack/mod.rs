//! Slack notifications: [Block Kit](https://api.slack.com/block-kit)
//! messages posted by your Slack App's bot, or to an incoming webhook
//! (Laravel's `laravel/slack-notification-channel`).
//!
//! ## Configuration
//!
//! Create a [Slack App](https://api.slack.com/apps?new_app=1) with the
//! `chat:write`, `chat:write.public` and `chat:write.customize` scopes,
//! then place its "Bot User OAuth Token" in the `slack` section of your
//! `services` configuration:
//!
//! ```json
//! "slack": {
//!     "notifications": {
//!         "bot_user_oauth_token": "xoxb-...",
//!         "channel": "#general"
//!     }
//! }
//! ```
//!
//! ## Formatting Slack notifications
//!
//! Notifications sent over the `slack` channel return a [`SlackMessage`]
//! from [`to_slack`](crate::Notification::to_slack):
//!
//! ```
//! use illuminate_notifications::slack::SlackMessage;
//! use illuminate_notifications::{Notifiable, Notification};
//! use serde::Serialize;
//!
//! #[derive(Serialize)]
//! struct InvoicePaid {
//!     invoice_id: u64,
//! }
//!
//! impl Notification for InvoicePaid {
//!     fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
//!         vec!["slack".into()]
//!     }
//!
//!     fn to_slack(&self, _notifiable: &dyn Notifiable) -> Option<SlackMessage> {
//!         Some(
//!             SlackMessage::new()
//!                 .text("One of your invoices has been paid!")
//!                 .header_block("Invoice Paid")
//!                 .context_block(|block| {
//!                     block.text("Customer #1234");
//!                 })
//!                 .section_block(|block| {
//!                     block.text("An invoice has been paid.");
//!                     block.field(format!("*Invoice No:*\n{}", self.invoice_id)).markdown();
//!                     block.field("*Invoice Recipient:*\ntaylor@laravel.com").markdown();
//!                 })
//!                 .divider_block()
//!                 .section_block(|block| {
//!                     block.text("Congratulations!");
//!                 }),
//!         )
//!     }
//! }
//! ```
//!
//! ## Routing Slack notifications
//!
//! A notifiable's `slack` route (see
//! [`route_notification_for_slack`](crate::Notifiable::route_notification_for_slack))
//! may be:
//!
//! - nothing, which defers to the message's [`to`](SlackMessage::to)
//!   channel and then `services.slack.notifications.channel`;
//! - a channel name, like `"#support-channel"`, posted to with your
//!   application's bot token;
//! - a [`SlackRoute`], naming a channel *and* the token of an external
//!   workspace;
//! - an incoming webhook URL (`https://hooks.slack.com/...`), which the
//!   message is posted to directly.
//!
//! On-demand notifications route the same way:
//! `Notification::route("slack", "#general")`.

pub mod block_kit;
mod message;
mod route;

pub use block_kit::blocks::{
    ActionsBlock, Block, ContextBlock, DividerBlock, HeaderBlock, ImageBlock, SectionBlock,
};
pub use block_kit::composites::{ConfirmObject, PlainTextOnlyTextObject, TextObject};
pub use block_kit::elements::{ButtonElement, Element, ImageElement};
pub use message::{EventMetadata, SlackMessage};
pub use route::SlackRoute;
pub(crate) use route::{SlackDestination, filled};

/// Thrown when a Slack message breaks one of Slack's rules, or when a
/// notification can't be routed.
///
/// ```
/// use illuminate_notifications::slack::{LogicException, SlackMessage};
///
/// let error = SlackMessage::new().to_array().unwrap_err();
///
/// assert!(error.is::<LogicException>());
/// assert_eq!(error.to_string(), "Slack messages must contain at least a text message or block.");
/// ```
pub use illuminate_support::error::LogicException;
