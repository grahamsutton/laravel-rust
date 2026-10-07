//! # Illuminate Notifications
//!
//! Laravel's notifications: short, informational messages sent to your
//! users over a variety of delivery channels — email, the database and
//! [Slack](slack) are built in, and custom channels are a trait away.
//!
//! ## Writing notifications
//!
//! A notification is a serializable struct implementing [`Notification`]:
//!
//! ```
//! use illuminate_notifications::{MailMessage, Notifiable, Notification};
//! use illuminate_support::{Value, json};
//! use serde::Serialize;
//!
//! #[derive(Serialize)]
//! pub struct InvoicePaid {
//!     pub invoice_id: u64,
//! }
//!
//! impl Notification for InvoicePaid {
//!     fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
//!         vec!["mail".into(), "database".into()]
//!     }
//!
//!     fn to_mail(&self, _notifiable: &dyn Notifiable) -> Option<MailMessage> {
//!         Some(
//!             MailMessage::new()
//!                 .line("One of your invoices has been paid!")
//!                 .action("View Invoice", format!("https://example.com/invoices/{}", self.invoice_id))
//!                 .line("Thank you for using our application!"),
//!         )
//!     }
//!
//!     fn to_array(&self, _notifiable: &dyn Notifiable) -> Option<Value> {
//!         Some(json!({"invoice_id": self.invoice_id}))
//!     }
//! }
//! ```
//!
//! ## Sending notifications
//!
//! Anything implementing [`Notifiable`] (usually your `User` model) can be
//! notified, or use the facade:
//!
//! ```no_run
//! # use illuminate_notifications::{Notifiable, Notification as _};
//! # async fn example(user: &impl Notifiable, users: &Vec<impl Notifiable>, notification: impl illuminate_notifications::Notification + Clone) -> illuminate_support::Result<()> {
//! use illuminate_notifications::facades::Notification;
//!
//! user.notify(notification.clone()).await?;
//! Notification::send(users, notification.clone()).await?;
//! Notification::route("mail", "taylor@example.com").notify(notification).await?;
//! # Ok(()) }
//! ```
//!
//! Notifications returning `true` from
//! [`should_queue`](Notification::should_queue) are pre-rendered per
//! channel and queued (see [`queue`]).
//!
//! ## Database notifications
//!
//! The `database` channel stores notifications in the `notifications`
//! table (create it with [`CreateNotificationsTable`]); read them back with
//! [`HasDatabaseNotifications`] and [`DatabaseNotification`].
//!
//! ## Slack notifications
//!
//! The `slack` channel posts the notification's
//! [`to_slack`](Notification::to_slack) [`SlackMessage`] — Block Kit
//! blocks built with closures — through your Slack App's bot, or to an
//! incoming webhook. See the [`slack`] module.
//!
//! ## Testing
//!
//! [`facades::Notification::fake`] records notifications instead of
//! sending them; assert with `assert_sent_to`, `assert_sent_on_demand`,
//! `assert_nothing_sent`, `assert_count` and friends.

pub mod channels;
pub mod database;
pub mod events;
pub mod facades;
pub mod fake;
mod manager;
pub mod messages;
pub mod notifiable;
pub mod notification;
mod provider;
pub mod queue;
pub mod slack;

pub use channels::{Channel, DatabaseChannel, MailChannel, SlackChannel};
pub use database::{CreateNotificationsTable, DatabaseNotification, HasDatabaseNotifications};
pub use events::{NotificationFailed, NotificationSending, NotificationSent, NotificationSkipped};
pub use facades::PendingNotification;
pub use fake::{FakeNotification, NotificationFake};
pub use manager::ChannelManager;
pub use messages::{Action, DatabaseMessage, MailMessage};
pub use notifiable::{
    AnonymousNotifiable, IntoNotifiables, Notifiable, NotifiableData, NotifiableSnapshot,
};
pub use notification::{Notification, NotificationData};
pub use provider::NotificationServiceProvider;
pub use queue::{NotificationQueueHook, QueuedNotification, SendQueuedNotification};
pub use slack::{SlackMessage, SlackRoute};

/// Re-exported so custom channels don't need their own dependency.
pub use async_trait::async_trait;
