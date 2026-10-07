//! The channel manager: resolves channels and sends notifications
//! (Laravel's `ChannelManager` and `NotificationSender`).

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use illuminate_container::{Container, try_app};
use illuminate_events::Dispatcher;
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Error, Result, Str};
use illuminate_translation::with_locale_async;

use crate::channels::{Channel, DatabaseChannel, MailChannel, SlackChannel};
use crate::events::{
    NotificationFailed, NotificationSending, NotificationSent, NotificationSkipped,
};
use crate::fake::NotificationFake;
use crate::notifiable::{AnonymousNotifiable, IntoNotifiables, Notifiable, NotifiableSnapshot};
use crate::notification::Notification;
use crate::queue::{NotificationQueueHook, QueuedNotification, queue_hook};

/// How a batch of notifications should be sent.
#[derive(Clone, Debug, Default)]
pub(crate) struct Delivery {
    /// Send immediately, even if the notification should be queued.
    pub(crate) now: bool,
    /// Only use these channels (instead of the notification's `via`).
    pub(crate) channels: Option<Vec<String>>,
    /// The locale to send in (unless the notification has its own).
    pub(crate) locale: Option<String>,
}

/// The channel manager (the `Notification` facade's root).
///
/// ```
/// use illuminate_notifications::{ChannelManager, DatabaseChannel};
///
/// let manager = ChannelManager::new();
/// assert_eq!(manager.delivers_via(), "mail");
/// assert!(manager.channel(Some("database")).is_ok());
/// assert!(manager.channel(Some("pigeon")).is_err());
///
/// manager.extend("archive", DatabaseChannel::new().table("archived_notifications"));
/// assert!(manager.channel(Some("archive")).is_ok());
/// ```
pub struct ChannelManager {
    channels: RwLock<HashMap<String, Arc<dyn Channel>>>,
    default_channel: RwLock<String>,
    queue: RwLock<Option<NotificationQueueHook>>,
    fake: RwLock<Option<Arc<NotificationFake>>>,
}

impl Default for ChannelManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ChannelManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelManager")
            .field("default_channel", &self.delivers_via())
            .field(
                "channels",
                &self.channels.read().unwrap().keys().collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl ChannelManager {
    /// Create a new channel manager.
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
            default_channel: RwLock::new("mail".into()),
            queue: RwLock::new(None),
            fake: RwLock::new(None),
        }
    }

    /// Resolve the manager from the container, registering one if needed.
    pub fn resolve() -> Arc<ChannelManager> {
        if let Some(manager) = try_app::<ChannelManager>() {
            return manager;
        }
        let container = Container::get_instance();
        container.singleton_if::<ChannelManager>(|_| Arc::new(ChannelManager::new()));
        container.make::<ChannelManager>()
    }

    /// Get a channel instance (the default channel when `None`).
    pub fn channel(&self, name: Option<&str>) -> Result<Arc<dyn Channel>> {
        let name = name.map_or_else(|| self.delivers_via(), str::to_string);
        if let Some(channel) = self.channels.read().unwrap().get(&name) {
            return Ok(channel.clone());
        }
        let channel: Arc<dyn Channel> = match name.as_str() {
            "mail" => Arc::new(MailChannel),
            "database" => Arc::new(DatabaseChannel::new()),
            "slack" => Arc::new(SlackChannel),
            _ => {
                return Err(InvalidArgumentException::new(format!(
                    "Driver [{name}] not supported."
                ))
                .into());
            }
        };
        self.channels.write().unwrap().insert(name, channel.clone());
        Ok(channel)
    }

    /// Register a custom channel (or replace a built-in one).
    pub fn extend(&self, name: &str, channel: impl Channel) -> &Self {
        self.channels
            .write()
            .unwrap()
            .insert(name.to_string(), Arc::new(channel));
        self
    }

    /// Get the default channel name.
    pub fn delivers_via(&self) -> String {
        self.default_channel.read().unwrap().clone()
    }

    /// Set the default channel name.
    pub fn deliver_via(&self, channel: &str) {
        *self.default_channel.write().unwrap() = channel.to_string();
    }

    // ------------------------------------------------------------------
    // Sending
    // ------------------------------------------------------------------

    /// Send the given notification to the given notifiable entities
    /// (queueing it when it should be queued).
    pub async fn send<'a, N: Notification>(
        &self,
        notifiables: impl IntoNotifiables<'a> + Send,
        notification: N,
    ) -> Result<()> {
        self.deliver(
            notifiables.into_notifiables(),
            Arc::new(notification),
            Delivery::default(),
        )
        .await
    }

    /// Send the given notification immediately.
    pub async fn send_now<'a, N: Notification>(
        &self,
        notifiables: impl IntoNotifiables<'a> + Send,
        notification: N,
    ) -> Result<()> {
        let delivery = Delivery {
            now: true,
            ..Delivery::default()
        };
        self.deliver(
            notifiables.into_notifiables(),
            Arc::new(notification),
            delivery,
        )
        .await
    }

    /// Send the given notification immediately, over the given channels only.
    pub async fn send_now_via<'a, N: Notification>(
        &self,
        notifiables: impl IntoNotifiables<'a> + Send,
        notification: N,
        channels: Vec<String>,
    ) -> Result<()> {
        let delivery = Delivery {
            now: true,
            channels: Some(channels),
            ..Delivery::default()
        };
        self.deliver(
            notifiables.into_notifiables(),
            Arc::new(notification),
            delivery,
        )
        .await
    }

    /// Send a shared notification.
    pub(crate) async fn deliver(
        &self,
        notifiables: Vec<&dyn Notifiable>,
        notification: Arc<dyn Notification>,
        delivery: Delivery,
    ) -> Result<()> {
        if let Some(fake) = self.get_fake() {
            fake.record(&notifiables, &notification, &delivery);
            return Ok(());
        }
        if !delivery.now && notification.should_queue() {
            return self
                .queue_notification(notifiables, notification, delivery)
                .await;
        }
        for notifiable in notifiables {
            let channels = delivery
                .channels
                .clone()
                .unwrap_or_else(|| notification.via(notifiable));
            if channels.is_empty() {
                continue;
            }
            let locale = preferred_locale(notifiable, notification.as_ref(), &delivery);
            let id = Str::uuid().to_string();
            let send = async {
                for channel in &channels {
                    if is_anonymous(notifiable) && channel == "database" {
                        continue;
                    }
                    self.send_to_notifiable(notifiable, &id, &notification, channel)
                        .await?;
                }
                Ok::<(), Error>(())
            };
            match &locale {
                Some(locale) => with_locale_async(locale, send).await?,
                None => send.await?,
            }
        }
        Ok(())
    }

    /// Determine if the notification should be sent over the channel:
    /// `should_send`, then the cancellable `NotificationSending` event.
    async fn should_send(
        &self,
        notifiable: &dyn Notifiable,
        id: &str,
        notification: &Arc<dyn Notification>,
        channel: &str,
    ) -> Result<bool> {
        let events = try_app::<Dispatcher>();
        let mut send = notification.should_send(notifiable, channel);
        if send && let Some(events) = &events {
            send = events
                .until(NotificationSending {
                    notifiable: NotifiableSnapshot::of(notifiable),
                    notification: notification.clone(),
                    id: id.to_string(),
                    channel: channel.to_string(),
                })
                .await?;
        }
        if !send && let Some(events) = &events {
            events
                .dispatch(NotificationSkipped {
                    notifiable: NotifiableSnapshot::of(notifiable),
                    notification: notification.clone(),
                    id: id.to_string(),
                    channel: channel.to_string(),
                })
                .await?;
        }
        Ok(send)
    }

    async fn send_to_notifiable(
        &self,
        notifiable: &dyn Notifiable,
        id: &str,
        notification: &Arc<dyn Notification>,
        channel: &str,
    ) -> Result<()> {
        if !self
            .should_send(notifiable, id, notification, channel)
            .await?
        {
            return Ok(());
        }
        let driver = self.channel(Some(channel))?;
        let result = driver.send(notifiable, notification.as_ref(), id).await;
        let events = try_app::<Dispatcher>();
        match result {
            Ok(response) => {
                notification.after_sending(notifiable, channel, &response);
                if let Some(events) = events {
                    events
                        .dispatch(NotificationSent {
                            notifiable: NotifiableSnapshot::of(notifiable),
                            notification: Some(notification.clone()),
                            notification_type: notification.notification_type().to_string(),
                            id: id.to_string(),
                            channel: channel.to_string(),
                            response,
                        })
                        .await?;
                }
                Ok(())
            }
            Err(error) => {
                if let Some(events) = events {
                    events
                        .dispatch(NotificationFailed {
                            notifiable: NotifiableSnapshot::of(notifiable),
                            notification: Some(notification.clone()),
                            notification_type: notification.notification_type().to_string(),
                            id: id.to_string(),
                            channel: channel.to_string(),
                            error: error.to_string(),
                        })
                        .await?;
                }
                Err(error)
            }
        }
    }

    // ------------------------------------------------------------------
    // Queueing
    // ------------------------------------------------------------------

    async fn queue_notification(
        &self,
        notifiables: Vec<&dyn Notifiable>,
        notification: Arc<dyn Notification>,
        delivery: Delivery,
    ) -> Result<()> {
        for notifiable in notifiables {
            let id = Str::uuid().to_string();
            let locale = preferred_locale(notifiable, notification.as_ref(), &delivery);
            let channels = delivery
                .channels
                .clone()
                .unwrap_or_else(|| notification.via(notifiable));
            for channel in channels {
                if is_anonymous(notifiable) && channel == "database" {
                    continue;
                }
                let driver = self.channel(Some(&channel))?;
                if !driver.supports_queueing() {
                    // The channel can't pre-render notifications: send it now.
                    let send = self.send_to_notifiable(notifiable, &id, &notification, &channel);
                    match &locale {
                        Some(locale) => with_locale_async(locale, send).await?,
                        None => send.await?,
                    }
                    continue;
                }
                if !self
                    .should_send(notifiable, &id, &notification, &channel)
                    .await?
                {
                    continue;
                }
                let prepare = driver.prepare(notifiable, notification.as_ref(), &id);
                let payload = match &locale {
                    Some(locale) => with_locale_async(locale, prepare).await?,
                    None => prepare.await?,
                };
                let Some(payload) = payload else {
                    continue;
                };
                let queued = QueuedNotification {
                    id: id.clone(),
                    notification: notification.notification_type().to_string(),
                    payload,
                    notifiable: NotifiableSnapshot::of(notifiable),
                    locale: locale.clone(),
                    connection: notification.queue_connection(&channel),
                    queue: notification.queue_name(&channel),
                    delay: notification.queue_delay(notifiable, &channel),
                    channel,
                };
                match self.queue_hook() {
                    Some(hook) => hook(queued).await?,
                    None => self.send_queued(queued).await?,
                }
            }
        }
        Ok(())
    }

    /// Install the hook that pushes queued notifications onto the queue.
    pub fn queue_using<F, Fut>(&self, hook: F)
    where
        F: Fn(QueuedNotification) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        *self.queue.write().unwrap() = Some(queue_hook(hook));
    }

    /// Remove the queue hook (queued notifications will be sent immediately).
    pub fn forget_queue_hook(&self) {
        *self.queue.write().unwrap() = None;
    }

    /// Determine if a queue hook is installed.
    pub fn has_queue_hook(&self) -> bool {
        self.queue.read().unwrap().is_some()
    }

    fn queue_hook(&self) -> Option<NotificationQueueHook> {
        self.queue.read().unwrap().clone()
    }

    /// Deliver a notification taken off the queue (what the worker calls).
    pub async fn send_queued(&self, queued: QueuedNotification) -> Result<()> {
        let driver = self.channel(Some(&queued.channel))?;
        let deliver = driver.deliver(queued.payload.clone());
        let result = match &queued.locale {
            Some(locale) => with_locale_async(locale, deliver).await,
            None => deliver.await,
        };
        let events = try_app::<Dispatcher>();
        match result {
            Ok(response) => {
                if let Some(events) = events {
                    events
                        .dispatch(NotificationSent {
                            notifiable: queued.notifiable,
                            notification: None,
                            notification_type: queued.notification,
                            id: queued.id,
                            channel: queued.channel,
                            response,
                        })
                        .await?;
                }
                Ok(())
            }
            Err(error) => {
                if let Some(events) = events {
                    events
                        .dispatch(NotificationFailed {
                            notifiable: queued.notifiable,
                            notification: None,
                            notification_type: queued.notification,
                            id: queued.id,
                            channel: queued.channel,
                            error: error.to_string(),
                        })
                        .await?;
                }
                Err(error)
            }
        }
    }

    // ------------------------------------------------------------------
    // Faking
    // ------------------------------------------------------------------

    /// Replace sending with a [`NotificationFake`] that records notifications.
    pub fn fake(&self) -> Arc<NotificationFake> {
        let fake = Arc::new(NotificationFake::new());
        *self.fake.write().unwrap() = Some(fake.clone());
        fake
    }

    /// Stop faking notifications.
    pub fn unfake(&self) {
        *self.fake.write().unwrap() = None;
    }

    /// Determine if notifications are being faked.
    pub fn is_fake(&self) -> bool {
        self.fake.read().unwrap().is_some()
    }

    /// The fake, when notifications are being faked.
    pub fn get_fake(&self) -> Option<Arc<NotificationFake>> {
        self.fake.read().unwrap().clone()
    }
}

fn is_anonymous(notifiable: &dyn Notifiable) -> bool {
    notifiable.notifiable_type() == AnonymousNotifiable::TYPE
}

/// The notification's locale, the sender's locale, or the notifiable's
/// preferred locale.
pub(crate) fn preferred_locale(
    notifiable: &dyn Notifiable,
    notification: &dyn Notification,
    delivery: &Delivery,
) -> Option<String> {
    notification
        .locale()
        .or_else(|| delivery.locale.clone())
        .or_else(|| notifiable.preferred_locale())
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::{Value, json};
    use serde::Serialize;

    #[derive(Serialize)]
    struct User {
        id: u64,
    }

    impl Notifiable for User {
        fn notifiable_key(&self) -> Value {
            json!(self.id)
        }

        fn notifiable_type(&self) -> String {
            "User".into()
        }

        fn preferred_locale(&self) -> Option<String> {
            Some("fr".into())
        }
    }

    #[derive(Serialize)]
    struct Silent {
        locale: Option<String>,
    }

    impl Notification for Silent {
        fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
            Vec::new()
        }

        fn locale(&self) -> Option<String> {
            self.locale.clone()
        }
    }

    #[test]
    fn channels_are_resolved_and_cached() {
        let manager = ChannelManager::new();
        let mail = manager.channel(None).unwrap();
        assert!(Arc::ptr_eq(&mail, &manager.channel(Some("mail")).unwrap()));
        manager.deliver_via("database");
        assert_eq!(manager.delivers_via(), "database");
        assert!(manager.channel(None).unwrap().supports_queueing());
        let error = manager.channel(Some("pigeon")).err().unwrap();
        assert_eq!(error.to_string(), "Driver [pigeon] not supported.");
        assert!(format!("{manager:?}").contains("database"));
    }

    #[test]
    fn locales_are_chosen_like_laravel() {
        let user = User { id: 1 };
        let delivery = Delivery::default();
        assert_eq!(
            preferred_locale(&user, &Silent { locale: None }, &delivery).as_deref(),
            Some("fr")
        );
        let delivery = Delivery {
            locale: Some("es".into()),
            ..Delivery::default()
        };
        assert_eq!(
            preferred_locale(&user, &Silent { locale: None }, &delivery).as_deref(),
            Some("es")
        );
        assert_eq!(
            preferred_locale(
                &user,
                &Silent {
                    locale: Some("de".into())
                },
                &delivery
            )
            .as_deref(),
            Some("de")
        );
    }

    #[tokio::test]
    async fn notifications_without_channels_do_nothing() {
        let manager = ChannelManager::new();
        manager
            .send(&User { id: 1 }, Silent { locale: None })
            .await
            .unwrap();
        assert!(!manager.has_queue_hook());
        manager.queue_using(|_| async { Ok(()) });
        assert!(manager.has_queue_hook());
        manager.forget_queue_hook();
        assert!(!manager.has_queue_hook());
        let fake = manager.fake();
        assert!(manager.is_fake());
        manager
            .send_now(&User { id: 1 }, Silent { locale: None })
            .await
            .unwrap();
        assert_eq!(fake.count(), 0);
        manager.unfake();
        assert!(!manager.is_fake());
    }
}
