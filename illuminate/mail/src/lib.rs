//! # Illuminate Mail
//!
//! Laravel's mail component: a clean, simple email API with drivers for
//! SMTP, `sendmail`, the log and an in-memory array, plus Markdown mail,
//! queued mail and first-class testing helpers.
//!
//! ## Writing mailables
//!
//! Each type of email your application sends is a [`Mailable`]: a
//! serializable struct whose fields are available to its view.
//!
//! ```
//! use illuminate_mail::{Address, Content, Envelope, Mailable};
//! use serde::Serialize;
//!
//! #[derive(Serialize)]
//! pub struct OrderShipped {
//!     pub order_id: u64,
//! }
//!
//! impl Mailable for OrderShipped {
//!     fn envelope(&self) -> Envelope {
//!         Envelope::new()
//!             .from(Address::new("jeffrey@example.com", "Jeffrey Way"))
//!             .subject("Order Shipped")
//!     }
//!
//!     fn content(&self) -> Content {
//!         Content::markdown("mail.orders.shipped")
//!     }
//! }
//! ```
//!
//! ## Sending mail
//!
//! ```no_run
//! # use illuminate_mail::Mailable;
//! # #[derive(serde::Serialize)] pub struct OrderShipped { pub order_id: u64 }
//! # impl Mailable for OrderShipped {}
//! # async fn example() -> illuminate_support::Result<()> {
//! use illuminate_mail::Mail;
//!
//! Mail::to("taylor@example.com")
//!     .cc(["abigail@example.com"])
//!     .send(OrderShipped { order_id: 1 })
//!     .await?;
//!
//! // Queue it instead (see the `queue` module for how queueing is wired up)...
//! Mail::to("taylor@example.com").queue(OrderShipped { order_id: 1 }).await?;
//! # Ok(()) }
//! ```
//!
//! ## Markdown mail
//!
//! `Content::markdown(...)` renders a Blade template using Laravel's mail
//! components (`<x-mail::message>`, `<x-mail::button>`, `<x-mail::panel>`,
//! `<x-mail::table>`, ...) into a styled HTML email — the theme's CSS is
//! inlined — plus a plain-text alternative. See [`markdown`].
//!
//! ## Configuration
//!
//! Mailers are configured in the `mail` configuration, exactly like
//! Laravel's `config/mail.php`: `mail.default`, `mail.mailers.*` (each with a
//! `transport` of `smtp`, `sendmail`, `log`, `array`, `failover` or
//! `roundrobin`), and the global `mail.from`, `mail.reply_to`, `mail.to` and
//! `mail.return_path` addresses. Register more transports with
//! [`Mail::extend`].
//!
//! ## Testing
//!
//! [`Mail::fake`] records mailables instead of sending them, so you can
//! assert on them with [`Mail::assert_sent`], [`Mail::assert_queued`],
//! [`Mail::assert_nothing_sent`] and friends. Mailables also have their own
//! assertions (`assert_has_subject`, `assert_see_in_html`, ...), and the
//! `array` transport keeps sent messages in memory.

pub mod address;
pub mod attachment;
pub mod events;
mod facade;
pub mod fake;
pub mod mailable;
pub mod mailables;
mod mailer;
mod manager;
pub mod markdown;
pub mod message;
pub(crate) mod mime;
mod pending;
mod provider;
pub mod queue;
pub mod transport;
mod view_message;

pub use address::{Address, IntoAddresses, MailRecipient};
pub use attachment::{Attachable, Attachment, MessageAttachment};
pub use events::{MessageSending, MessageSent};
pub use facade::Mail;
pub use fake::{FakeMailable, MailFake};
pub use mailable::{MailPreview, MailView, Mailable, MailableBuilder, MailableData, SendOptions};
pub use mailables::{Content, Envelope, Headers, MessageCallback};
pub use mailer::Mailer;
pub use manager::{MailManager, TransportCreator};
pub use markdown::Markdown;
pub use message::{Message, SentEnvelope, SentMessage};
pub use pending::PendingMail;
pub use provider::MailServiceProvider;
pub use queue::{QueueHook, QueuedMessage};
pub use transport::{
    ArrayTransport, FailoverTransport, LogTransport, RoundRobinTransport, SendmailTransport,
    SmtpTransport, Transport, downcast_transport,
};

/// Re-exported so custom transports don't need their own dependency.
pub use async_trait::async_trait;
