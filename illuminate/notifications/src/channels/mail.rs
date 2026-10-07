//! The `mail` channel.

use std::sync::Arc;

use async_trait::async_trait;
use illuminate_mail::{
    Address, MailManager, MailView, Mailable, Mailer, Message, QueuedMessage, SendOptions,
    SentMessage,
};
use illuminate_support::error::RuntimeException;
use illuminate_support::{Result, Str, Value, ValueExt, json};
use illuminate_view::Factory;

use super::Channel;
use crate::messages::{DEFAULT_TEMPLATE, EMAIL_TEMPLATE, MailMessage};
use crate::notifiable::{Notifiable, mail_route_addresses};
use crate::notification::Notification;

/// Sends notifications as email, through the mail component.
///
/// The notification's [`to_mail`](Notification::to_mail) message is
/// rendered with Laravel's notification email template (or the message's
/// own view) and sent to the notifiable's `mail` route. A notification may
/// instead return a mailable from [`to_mailable`](Notification::to_mailable).
#[derive(Debug, Default, Clone, Copy)]
pub struct MailChannel;

fn sent_value(sent: Option<SentMessage>) -> Value {
    match sent {
        Some(sent) => json!({"message_id": sent.message_id}),
        None => Value::Null,
    }
}

impl MailChannel {
    /// Build the mail message for a notification, without sending it.
    async fn compose(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
        id: &str,
        queued: bool,
    ) -> Result<Option<(Arc<Mailer>, Message, Value)>> {
        let Some(route) = notifiable
            .route_notification_for("mail", notification)
            .filter(|route| !route.is_blank())
        else {
            return Ok(None);
        };
        let mail = notification.to_mail(notifiable).ok_or_else(|| {
            RuntimeException::new(format!(
                "Notification [{}] is missing a to_mail method.",
                notification.notification_name()
            ))
        })?;

        let mailer = MailManager::resolve().mailer(mail.mailer.as_deref())?;
        let factory = Factory::resolve();
        let view = match (&mail.view, &mail.text_view) {
            (Some(html), Some(text)) => MailView::Both {
                html: html.clone(),
                text: text.clone(),
            },
            (Some(html), None) => MailView::View(html.clone()),
            (None, Some(text)) => MailView::Text(text.clone()),
            (None, None) if mail.uses_default_template(&factory) => {
                MailView::MarkdownTemplate(EMAIL_TEMPLATE.to_string())
            }
            (None, None) => MailView::Markdown(
                mail.markdown
                    .clone()
                    .unwrap_or_else(|| DEFAULT_TEMPLATE.to_string()),
            ),
        };

        let mut data = mail.data();
        data.insert(
            "__laravel_notification_id".into(),
            Value::String(id.to_string()),
        );
        data.insert(
            "__laravel_notification".into(),
            Value::String(notification.notification_type().to_string()),
        );
        data.insert("__laravel_notification_queued".into(), Value::Bool(queued));

        let recipients = mail_route_addresses(&route);
        let subject = mail.subject.clone().unwrap_or_else(|| {
            Str::title(&Str::snake_with(&notification.notification_name(), " "))
        });
        let theme = mail.theme.clone();
        let (message, data) = mailer
            .compose(view, Value::Object(data), theme, |message| {
                build_message(message, &mail, recipients, subject);
            })
            .await?;
        Ok(Some((mailer, message, data)))
    }
}

/// Apply a mail notification's addresses, subject, attachments and
/// headers to the message.
fn build_message(
    message: &mut Message,
    mail: &MailMessage,
    recipients: Vec<Address>,
    subject: String,
) {
    if let Some(from) = &mail.from {
        message.from(from.clone());
    }
    message.reply_to(mail.reply_to.clone());
    message.to(recipients);
    message.cc(mail.cc.clone());
    message.bcc(mail.bcc.clone());
    message.subject(subject);
    for attachment in &mail.attachments {
        message.attach(attachment.clone());
    }
    if let Some(priority) = mail.priority {
        message.priority(priority);
    }
    for tag in &mail.tags {
        message.tag(tag.clone());
    }
    for (key, value) in &mail.metadata {
        message.metadata(key.clone(), value);
    }
    for callback in &mail.callbacks {
        callback(message);
    }
}

#[async_trait]
impl Channel for MailChannel {
    async fn send(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
        id: &str,
    ) -> Result<Value> {
        if let Some(mailable) = notification.to_mailable(notifiable) {
            let mailable: Arc<dyn Mailable> = Arc::from(mailable);
            let mailer = MailManager::resolve().mailer_for(mailable.as_ref())?;
            let sent = mailer
                .send_mailable_now(mailable, SendOptions::default())
                .await?;
            return Ok(sent_value(sent));
        }
        match self.compose(notifiable, notification, id, false).await? {
            Some((mailer, message, data)) => {
                Ok(sent_value(mailer.send_message(message, data).await?))
            }
            None => Ok(Value::Null),
        }
    }

    async fn prepare(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
        id: &str,
    ) -> Result<Option<Value>> {
        let (mailer, message, data) = if let Some(mailable) = notification.to_mailable(notifiable) {
            let mailer = MailManager::resolve().mailer_for(mailable.as_ref())?;
            let (message, data) = mailer
                .compose_mailable(mailable.as_ref(), SendOptions::default())
                .await?;
            (mailer, message, data)
        } else {
            match self.compose(notifiable, notification, id, true).await? {
                Some(composed) => composed,
                None => return Ok(None),
            }
        };
        let queued = QueuedMessage {
            mailer: mailer.name().to_string(),
            mailable: notification.notification_name(),
            message,
            data,
            connection: None,
            queue: None,
            delay: None,
        };
        Ok(Some(serde_json::to_value(queued)?))
    }

    fn supports_queueing(&self) -> bool {
        true
    }

    async fn deliver(&self, payload: Value) -> Result<Value> {
        let queued: QueuedMessage = serde_json::from_value(payload)?;
        Ok(sent_value(
            MailManager::resolve().send_queued(queued).await?,
        ))
    }
}
