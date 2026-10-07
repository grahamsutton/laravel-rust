//! Pending mail: `Mail::to($user)->cc(...)->send(...)`.

use std::sync::Arc;
use std::time::Duration;

use illuminate_support::Result;

use crate::address::{Address, IntoAddresses};
use crate::mailable::{Mailable, SendOptions};
use crate::mailer::Mailer;
use crate::manager::MailManager;
use crate::message::SentMessage;

/// A message being addressed, waiting for its mailable.
///
/// ```no_run
/// # async fn example() -> illuminate_support::Result<()> {
/// # use illuminate_mail::Mailable;
/// # #[derive(serde::Serialize)] struct OrderShipped;
/// # impl Mailable for OrderShipped {}
/// use illuminate_mail::Mail;
///
/// Mail::to("taylor@example.com")
///     .cc(["abigail@example.com"])
///     .bcc(("james@example.com", "James"))
///     .locale("es")
///     .send(OrderShipped)
///     .await?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone)]
#[must_use = "pending mail does nothing until it is sent"]
pub struct PendingMail {
    mailer: Option<Arc<Mailer>>,
    to: Vec<Address>,
    cc: Vec<Address>,
    bcc: Vec<Address>,
    locale: Option<String>,
}

impl PendingMail {
    /// Create pending mail for the given mailer (the default mailer when `None`).
    pub fn new(mailer: Option<Arc<Mailer>>) -> Self {
        Self {
            mailer,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            locale: None,
        }
    }

    /// Set the locale of the message.
    pub fn locale(mut self, locale: impl Into<String>) -> Self {
        self.locale = Some(locale.into());
        self
    }

    /// Set the recipients of the message. A recipient with a preferred
    /// locale sets the message's locale (unless one was given).
    pub fn to(mut self, users: impl IntoAddresses) -> Self {
        if self.locale.is_none() {
            self.locale = users.preferred_locale();
        }
        self.to = users.into_addresses();
        self
    }

    /// Set the recipients of the message.
    pub fn cc(mut self, users: impl IntoAddresses) -> Self {
        self.cc = users.into_addresses();
        self
    }

    /// Set the recipients of the message.
    pub fn bcc(mut self, users: impl IntoAddresses) -> Self {
        self.bcc = users.into_addresses();
        self
    }

    fn split(self, mailable: &dyn Mailable) -> Result<(Arc<Mailer>, SendOptions)> {
        let mailer = match self.mailer {
            Some(mailer) => mailer,
            None => MailManager::resolve().mailer_for(mailable)?,
        };
        let options = SendOptions {
            to: self.to,
            cc: self.cc,
            bcc: self.bcc,
            locale: self.locale,
            mailer: Some(mailer.name().to_string()),
        };
        Ok((mailer, options))
    }

    /// Send a new mailable message instance (queueing it if it should be queued).
    pub async fn send<M: Mailable>(self, mailable: M) -> Result<Option<SentMessage>> {
        let (mailer, options) = self.split(&mailable)?;
        mailer.send_mailable(Arc::new(mailable), options).await
    }

    /// Send a mailable message immediately, even if it should be queued.
    pub async fn send_now<M: Mailable>(self, mailable: M) -> Result<Option<SentMessage>> {
        let (mailer, options) = self.split(&mailable)?;
        mailer.send_mailable_now(Arc::new(mailable), options).await
    }

    /// Push the given mailable onto the queue.
    pub async fn queue<M: Mailable>(self, mailable: M) -> Result<()> {
        let (mailer, options) = self.split(&mailable)?;
        mailer
            .queue_mailable(Arc::new(mailable), options, None)
            .await
    }

    /// Deliver the queued message after the given delay.
    pub async fn later<M: Mailable>(self, delay: Duration, mailable: M) -> Result<()> {
        let (mailer, options) = self.split(&mailable)?;
        mailer
            .queue_mailable(Arc::new(mailable), options, Some(delay))
            .await
    }
}
