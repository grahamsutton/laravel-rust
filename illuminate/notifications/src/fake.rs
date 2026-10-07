//! `Notification::fake()`: record notifications instead of sending them.

use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

use illuminate_support::class_basename;

use crate::manager::{Delivery, preferred_locale};
use crate::notifiable::{AnonymousNotifiable, Notifiable, NotifiableSnapshot};
use crate::notification::Notification;

#[derive(Clone)]
struct Record {
    notifiable: NotifiableSnapshot,
    notification: Arc<dyn Notification>,
    channels: Vec<String>,
    locale: Option<String>,
}

impl Record {
    fn is<N: Notification>(&self) -> bool {
        (*self.notification).as_any().is::<N>()
    }
}

/// A notification recorded by the fake. Dereferences to the notification.
pub struct FakeNotification<N> {
    record: Record,
    _marker: PhantomData<fn() -> N>,
}

impl<N> Clone for FakeNotification<N> {
    fn clone(&self) -> Self {
        Self {
            record: self.record.clone(),
            _marker: PhantomData,
        }
    }
}

impl<N: Notification> Deref for FakeNotification<N> {
    type Target = N;

    fn deref(&self) -> &N {
        self.notification()
    }
}

impl<N: Notification> FakeNotification<N> {
    /// The notification.
    pub fn notification(&self) -> &N {
        (*self.record.notification)
            .as_any()
            .downcast_ref::<N>()
            .expect("the fake only hands out notifications of the requested type")
    }

    /// The channels the notification was sent over.
    pub fn channels(&self) -> &[String] {
        &self.record.channels
    }

    /// The notifiable it was sent to.
    pub fn notifiable(&self) -> &NotifiableSnapshot {
        &self.record.notifiable
    }

    /// The on-demand notifiable it was sent to, for on-demand notifications.
    pub fn anonymous_notifiable(&self) -> Option<AnonymousNotifiable> {
        self.record.notifiable.anonymous()
    }

    /// The locale it was sent in.
    pub fn locale(&self) -> Option<&str> {
        self.record.locale.as_deref()
    }
}

fn times(count: usize) -> &'static str {
    if count == 1 { "time" } else { "times" }
}

/// The notification fake. Install it with
/// [`Notification::fake`](crate::facades::Notification::fake).
///
/// ```
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_notifications::facades::Notification;
/// use illuminate_notifications::{Notifiable, Notification as _};
/// use illuminate_support::{Value, json};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct User { id: u64, email: String }
///
/// impl Notifiable for User {
///     fn notifiable_key(&self) -> Value { json!(self.id) }
///     fn notifiable_type(&self) -> String { "App\\Models\\User".into() }
/// }
///
/// #[derive(Serialize)]
/// struct OrderShipped { order_id: u64 }
///
/// impl illuminate_notifications::Notification for OrderShipped {
///     fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
///         vec!["mail".into()]
///     }
/// }
///
/// let app = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(app);
///
/// Notification::fake();
///
/// let user = User { id: 1, email: "taylor@example.com".into() };
/// user.notify(OrderShipped { order_id: 1 }).await?;
///
/// Notification::assert_sent_to::<OrderShipped>(&user);
/// Notification::assert_sent_to_with::<OrderShipped>(&user, |notification, channels| {
///     notification.order_id == 1 && channels == ["mail"]
/// });
/// Notification::assert_count(1);
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Default)]
pub struct NotificationFake {
    records: Mutex<Vec<Record>>,
}

impl std::fmt::Debug for NotificationFake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationFake")
            .field("sent", &self.records.lock().unwrap().len())
            .finish()
    }
}

impl NotificationFake {
    /// Create an empty fake.
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn record(
        &self,
        notifiables: &[&dyn Notifiable],
        notification: &Arc<dyn Notification>,
        delivery: &Delivery,
    ) {
        for notifiable in notifiables {
            let channels: Vec<String> = delivery
                .channels
                .clone()
                .unwrap_or_else(|| notification.via(*notifiable))
                .into_iter()
                .filter(|channel| notification.should_send(*notifiable, channel))
                .collect();
            if channels.is_empty() {
                continue;
            }
            self.records.lock().unwrap().push(Record {
                notifiable: NotifiableSnapshot::of(*notifiable),
                notification: notification.clone(),
                channels,
                locale: preferred_locale(*notifiable, notification.as_ref(), delivery),
            });
        }
    }

    fn matching<N: Notification>(
        &self,
        filter: impl Fn(&Record) -> bool,
    ) -> Vec<FakeNotification<N>> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| record.is::<N>() && filter(record))
            .map(|record| FakeNotification {
                record: record.clone(),
                _marker: PhantomData,
            })
            .collect()
    }

    /// Get the notifications of type `N` sent to the given notifiable.
    pub fn sent<N: Notification>(&self, notifiable: &dyn Notifiable) -> Vec<FakeNotification<N>> {
        self.matching::<N>(|record| record.notifiable.is(notifiable))
    }

    /// Get the notifications of type `N` sent on-demand.
    pub fn sent_on_demand<N: Notification>(&self) -> Vec<FakeNotification<N>> {
        self.matching::<N>(|record| record.notifiable.is_anonymous())
    }

    /// Determine if a notification of type `N` was sent to the notifiable.
    pub fn has_sent<N: Notification>(&self, notifiable: &dyn Notifiable) -> bool {
        !self.sent::<N>(notifiable).is_empty()
    }

    /// The total number of notifications sent.
    pub fn count(&self) -> usize {
        self.records.lock().unwrap().len()
    }

    /// Assert that a notification of type `N` was sent to the notifiable.
    pub fn assert_sent_to<N: Notification>(&self, notifiable: &dyn Notifiable) {
        assert!(
            self.has_sent::<N>(notifiable),
            "The expected [{}] notification was not sent.",
            class_basename::<N>()
        );
    }

    /// Assert that a notification of type `N`, sent over the given
    /// channels, passing the truth test was sent to the notifiable.
    pub fn assert_sent_to_with<N: Notification>(
        &self,
        notifiable: &dyn Notifiable,
        callback: impl Fn(&N, &[String]) -> bool,
    ) {
        let found = self
            .sent::<N>(notifiable)
            .iter()
            .any(|sent| callback(sent.notification(), sent.channels()));
        assert!(
            found,
            "The expected [{}] notification was not sent.",
            class_basename::<N>()
        );
    }

    /// Assert that a notification of type `N` was sent to the notifiable a number of times.
    pub fn assert_sent_to_times<N: Notification>(
        &self,
        notifiable: &dyn Notifiable,
        expected: usize,
    ) {
        let count = self.sent::<N>(notifiable).len();
        assert!(
            count == expected,
            "Expected [{}] to be sent {expected} {}, but was sent {count} {}.",
            class_basename::<N>(),
            times(expected),
            times(count)
        );
    }

    /// Assert that a notification of type `N` was sent to the notifiable exactly once.
    pub fn assert_sent_to_once<N: Notification>(&self, notifiable: &dyn Notifiable) {
        self.assert_sent_to_times::<N>(notifiable, 1);
    }

    /// Assert that no notification of type `N` was sent to the notifiable.
    pub fn assert_not_sent_to<N: Notification>(&self, notifiable: &dyn Notifiable) {
        assert!(
            !self.has_sent::<N>(notifiable),
            "The unexpected [{}] notification was sent.",
            class_basename::<N>()
        );
    }

    /// Assert that no notification of type `N` passing the truth test was
    /// sent to the notifiable.
    pub fn assert_not_sent_to_with<N: Notification>(
        &self,
        notifiable: &dyn Notifiable,
        callback: impl Fn(&N, &[String]) -> bool,
    ) {
        let found = self
            .sent::<N>(notifiable)
            .iter()
            .any(|sent| callback(sent.notification(), sent.channels()));
        assert!(
            !found,
            "The unexpected [{}] notification was sent.",
            class_basename::<N>()
        );
    }

    /// Assert that no notifications were sent.
    pub fn assert_nothing_sent(&self) {
        assert!(self.count() == 0, "Notifications were sent unexpectedly.");
    }

    /// Assert that no notifications were sent to the notifiable.
    pub fn assert_nothing_sent_to(&self, notifiable: &dyn Notifiable) {
        let sent = self
            .records
            .lock()
            .unwrap()
            .iter()
            .any(|record| record.notifiable.is(notifiable));
        assert!(!sent, "Notifications were sent unexpectedly.");
    }

    /// Assert that a notification of type `N` was sent a number of times
    /// (to any notifiable).
    pub fn assert_sent_times<N: Notification>(&self, expected: usize) {
        let count = self.matching::<N>(|_| true).len();
        assert!(
            count == expected,
            "Expected [{}] to be sent {expected} {}, but was sent {count} {}.",
            class_basename::<N>(),
            times(expected),
            times(count)
        );
    }

    /// Assert the total number of notifications that were sent.
    pub fn assert_count(&self, expected: usize) {
        let count = self.count();
        assert!(
            count == expected,
            "Expected {expected} notifications to be sent, but {count} were sent."
        );
    }

    /// Assert that a notification of type `N` was sent on-demand
    /// (`Notification::route(...)`).
    pub fn assert_sent_on_demand<N: Notification>(&self) {
        assert!(
            !self.sent_on_demand::<N>().is_empty(),
            "The expected [{}] notification was not sent.",
            class_basename::<N>()
        );
    }

    /// Assert that a notification of type `N` passing the truth test was
    /// sent on-demand. The callback also receives the anonymous notifiable,
    /// so routes can be checked.
    pub fn assert_sent_on_demand_with<N: Notification>(
        &self,
        callback: impl Fn(&N, &[String], &AnonymousNotifiable) -> bool,
    ) {
        let found = self.sent_on_demand::<N>().iter().any(|sent| {
            let notifiable = sent.anonymous_notifiable().unwrap_or_default();
            callback(sent.notification(), sent.channels(), &notifiable)
        });
        assert!(
            found,
            "The expected [{}] notification was not sent.",
            class_basename::<N>()
        );
    }

    /// Assert that a notification of type `N` was sent on-demand a number of times.
    pub fn assert_sent_on_demand_times<N: Notification>(&self, expected: usize) {
        let count = self.sent_on_demand::<N>().len();
        assert!(
            count == expected,
            "Expected [{}] to be sent {expected} {}, but was sent {count} {}.",
            class_basename::<N>(),
            times(expected),
            times(count)
        );
    }
}
