//! The `Notification` facade.

use std::future::Future;
use std::sync::Arc;

use illuminate_support::Result;
use serde::Serialize;

use crate::channels::Channel;
use crate::fake::{FakeNotification, NotificationFake};
use crate::manager::{ChannelManager, Delivery};
use crate::notifiable::{AnonymousNotifiable, IntoNotifiables, Notifiable};
use crate::notification::Notification as NotificationContract;
use crate::queue::QueuedNotification;

/// The `Notification` facade: static access to the [`ChannelManager`].
///
/// ```no_run
/// # async fn example(users: &Vec<impl illuminate_notifications::Notifiable>, invoice_paid: impl illuminate_notifications::Notification + Clone) -> illuminate_support::Result<()> {
/// use illuminate_notifications::facades::Notification;
///
/// Notification::send(users, invoice_paid.clone()).await?;
///
/// Notification::route("mail", "taylor@example.com")
///     .route("slack", "#invoices")
///     .notify(invoice_paid)
///     .await?;
/// # Ok(()) }
/// ```
pub struct Notification;

impl Notification {
    /// Get the channel manager.
    pub fn manager() -> Arc<ChannelManager> {
        ChannelManager::resolve()
    }

    /// Get a channel instance (the default channel when `None`).
    pub fn channel(name: Option<&str>) -> Result<Arc<dyn Channel>> {
        Self::manager().channel(name)
    }

    /// Register a custom channel.
    pub fn extend(name: &str, channel: impl Channel) {
        Self::manager().extend(name, channel);
    }

    /// Send the given notification to the given notifiable entities.
    pub async fn send<'a, N: NotificationContract>(
        notifiables: impl IntoNotifiables<'a> + Send,
        notification: N,
    ) -> Result<()> {
        Self::manager().send(notifiables, notification).await
    }

    /// Send the given notification immediately.
    pub async fn send_now<'a, N: NotificationContract>(
        notifiables: impl IntoNotifiables<'a> + Send,
        notification: N,
    ) -> Result<()> {
        Self::manager().send_now(notifiables, notification).await
    }

    /// Send the given notification immediately over the given channels only.
    pub async fn send_now_via<'a, N: NotificationContract>(
        notifiables: impl IntoNotifiables<'a> + Send,
        notification: N,
        channels: &[&str],
    ) -> Result<()> {
        let channels = channels.iter().map(|c| c.to_string()).collect();
        Self::manager()
            .send_now_via(notifiables, notification, channels)
            .await
    }

    /// Begin sending a notification to an anonymous notifiable.
    pub fn route(channel: &str, route: impl Serialize) -> AnonymousNotifiable {
        AnonymousNotifiable::new().route(channel, route)
    }

    /// Send notifications in the given locale.
    pub fn locale(locale: impl Into<String>) -> PendingNotification {
        PendingNotification {
            locale: Some(locale.into()),
        }
    }

    /// Install the hook that pushes queued notifications onto the queue.
    pub fn queue_using<F, Fut>(hook: F)
    where
        F: Fn(QueuedNotification) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self::manager().queue_using(hook);
    }

    /// Deliver a notification taken off the queue (what the worker calls).
    pub async fn send_queued(queued: QueuedNotification) -> Result<()> {
        Self::manager().send_queued(queued).await
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    /// Replace sending with a fake that records notifications, for the
    /// current container only.
    pub fn fake() -> Arc<NotificationFake> {
        Self::manager().fake()
    }

    /// Determine if notifications are being faked.
    pub fn is_fake() -> bool {
        Self::manager().is_fake()
    }

    fn the_fake() -> Arc<NotificationFake> {
        Self::manager()
            .get_fake()
            .expect("Notifications are not faked. Call Notification::fake() first.")
    }

    /// Get the notifications of type `N` sent to the given notifiable.
    pub fn sent<N: NotificationContract>(notifiable: &dyn Notifiable) -> Vec<FakeNotification<N>> {
        Self::the_fake().sent(notifiable)
    }

    /// Determine if a notification of type `N` was sent to the notifiable.
    pub fn has_sent<N: NotificationContract>(notifiable: &dyn Notifiable) -> bool {
        Self::the_fake().has_sent::<N>(notifiable)
    }

    /// Assert that a notification of type `N` was sent to the notifiable.
    pub fn assert_sent_to<N: NotificationContract>(notifiable: &dyn Notifiable) {
        Self::the_fake().assert_sent_to::<N>(notifiable);
    }

    /// Assert that a notification of type `N` passing the truth test (which
    /// receives the notification and its channels) was sent to the notifiable.
    pub fn assert_sent_to_with<N: NotificationContract>(
        notifiable: &dyn Notifiable,
        callback: impl Fn(&N, &[String]) -> bool,
    ) {
        Self::the_fake().assert_sent_to_with(notifiable, callback);
    }

    /// Assert that a notification of type `N` was sent to the notifiable a number of times.
    pub fn assert_sent_to_times<N: NotificationContract>(
        notifiable: &dyn Notifiable,
        times: usize,
    ) {
        Self::the_fake().assert_sent_to_times::<N>(notifiable, times);
    }

    /// Assert that a notification of type `N` was sent to the notifiable exactly once.
    pub fn assert_sent_to_once<N: NotificationContract>(notifiable: &dyn Notifiable) {
        Self::the_fake().assert_sent_to_once::<N>(notifiable);
    }

    /// Assert that no notification of type `N` was sent to the notifiable.
    pub fn assert_not_sent_to<N: NotificationContract>(notifiable: &dyn Notifiable) {
        Self::the_fake().assert_not_sent_to::<N>(notifiable);
    }

    /// Assert that no notification of type `N` passing the truth test was
    /// sent to the notifiable.
    pub fn assert_not_sent_to_with<N: NotificationContract>(
        notifiable: &dyn Notifiable,
        callback: impl Fn(&N, &[String]) -> bool,
    ) {
        Self::the_fake().assert_not_sent_to_with(notifiable, callback);
    }

    /// Assert that no notifications were sent.
    pub fn assert_nothing_sent() {
        Self::the_fake().assert_nothing_sent();
    }

    /// Assert that no notifications were sent to the notifiable.
    pub fn assert_nothing_sent_to(notifiable: &dyn Notifiable) {
        Self::the_fake().assert_nothing_sent_to(notifiable);
    }

    /// Assert that a notification of type `N` was sent a number of times.
    pub fn assert_sent_times<N: NotificationContract>(times: usize) {
        Self::the_fake().assert_sent_times::<N>(times);
    }

    /// Assert the total number of notifications that were sent.
    pub fn assert_count(count: usize) {
        Self::the_fake().assert_count(count);
    }

    /// Assert that a notification of type `N` was sent on-demand.
    pub fn assert_sent_on_demand<N: NotificationContract>() {
        Self::the_fake().assert_sent_on_demand::<N>();
    }

    /// Assert that a notification of type `N` passing the truth test was
    /// sent on-demand.
    pub fn assert_sent_on_demand_with<N: NotificationContract>(
        callback: impl Fn(&N, &[String], &AnonymousNotifiable) -> bool,
    ) {
        Self::the_fake().assert_sent_on_demand_with(callback);
    }

    /// Assert that a notification of type `N` was sent on-demand a number of times.
    pub fn assert_sent_on_demand_times<N: NotificationContract>(times: usize) {
        Self::the_fake().assert_sent_on_demand_times::<N>(times);
    }
}

/// Notifications being sent in a particular locale
/// (`Notification::locale("es").send(...)`).
#[derive(Clone, Debug)]
#[must_use = "pending notifications do nothing until they are sent"]
pub struct PendingNotification {
    locale: Option<String>,
}

impl PendingNotification {
    /// Send the notification (queueing it when it should be queued).
    pub async fn send<'a, N: NotificationContract>(
        self,
        notifiables: impl IntoNotifiables<'a> + Send,
        notification: N,
    ) -> Result<()> {
        let delivery = Delivery {
            locale: self.locale,
            ..Delivery::default()
        };
        ChannelManager::resolve()
            .deliver(
                notifiables.into_notifiables(),
                Arc::new(notification),
                delivery,
            )
            .await
    }

    /// Send the notification immediately.
    pub async fn send_now<'a, N: NotificationContract>(
        self,
        notifiables: impl IntoNotifiables<'a> + Send,
        notification: N,
    ) -> Result<()> {
        let delivery = Delivery {
            locale: self.locale,
            now: true,
            ..Delivery::default()
        };
        ChannelManager::resolve()
            .deliver(
                notifiables.into_notifiables(),
                Arc::new(notification),
                delivery,
            )
            .await
    }
}
