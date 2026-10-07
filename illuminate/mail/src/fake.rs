//! `Mail::fake()`: record mailables instead of sending them, then assert
//! on what was sent.

use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use illuminate_support::class_basename;

use crate::mailable::{Mailable, MailableBuilder, SendOptions};
use crate::mailables::has_recipient;

#[derive(Clone)]
pub(crate) struct Record {
    mailable: Arc<dyn Mailable>,
    options: SendOptions,
    delay: Option<Duration>,
}

impl Record {
    fn is<M: Mailable>(&self) -> bool {
        (*self.mailable).as_any().is::<M>()
    }
}

/// A mailable recorded by the fake, with the recipients and options it was
/// sent with. Dereferences to the mailable itself, so its fields are right
/// there:
///
/// ```ignore
/// Mail::assert_sent_with::<OrderShipped>(|mail| {
///     mail.order.id == order.id && mail.has_to(&user.email)
/// });
/// ```
pub struct FakeMailable<M> {
    record: Record,
    _marker: PhantomData<fn() -> M>,
}

impl<M> Clone for FakeMailable<M> {
    fn clone(&self) -> Self {
        Self {
            record: self.record.clone(),
            _marker: PhantomData,
        }
    }
}

impl<M: Mailable> Deref for FakeMailable<M> {
    type Target = M;

    fn deref(&self) -> &M {
        self.mailable()
    }
}

impl<M: Mailable> FakeMailable<M> {
    /// The mailable.
    pub fn mailable(&self) -> &M {
        (*self.record.mailable)
            .as_any()
            .downcast_ref::<M>()
            .expect("the fake only hands out mailables of the requested type")
    }

    /// The options (`Mail::to(...)` recipients, locale, mailer) it was sent with.
    pub fn options(&self) -> &SendOptions {
        &self.record.options
    }

    /// The mailable's message definition, including the recipients given
    /// when it was sent.
    pub fn builder(&self) -> MailableBuilder {
        MailableBuilder::for_mailable(self.record.mailable.as_ref(), &self.record.options)
    }

    /// Determine if the mailable was sent to the given address.
    pub fn has_to(&self, address: &str) -> bool {
        has_recipient(&self.builder().to, address, None)
    }

    /// Determine if the mailable was copied to the given address.
    pub fn has_cc(&self, address: &str) -> bool {
        has_recipient(&self.builder().cc, address, None)
    }

    /// Determine if the mailable was blind copied to the given address.
    pub fn has_bcc(&self, address: &str) -> bool {
        has_recipient(&self.builder().bcc, address, None)
    }

    /// Determine if the mailable has the given "reply to" address.
    pub fn has_reply_to(&self, address: &str) -> bool {
        has_recipient(&self.builder().reply_to, address, None)
    }

    /// Determine if the mailable is from the given address.
    pub fn has_from(&self, address: &str) -> bool {
        has_recipient(&self.builder().from, address, None)
    }

    /// Determine if the mailable has the given subject.
    pub fn has_subject(&self, subject: &str) -> bool {
        self.builder()
            .subject_or_default(&self.record.mailable.mailable_name())
            == subject
    }

    /// Determine if the mailable has the given tag.
    pub fn has_tag(&self, tag: &str) -> bool {
        self.builder().tags.iter().any(|t| t == tag)
    }

    /// Determine if the mailable has the given metadata.
    pub fn has_metadata(&self, key: &str, value: &str) -> bool {
        self.builder().metadata.get(key).is_some_and(|v| v == value)
    }

    /// Determine if the mailable was sent by the given mailer.
    pub fn uses_mailer(&self, mailer: &str) -> bool {
        self.mailer() == Some(mailer)
    }

    /// The mailer the mailable was sent with.
    pub fn mailer(&self) -> Option<&str> {
        self.record.options.mailer.as_deref()
    }

    /// The locale the mailable was sent in.
    pub fn locale(&self) -> Option<String> {
        self.record
            .options
            .locale
            .clone()
            .or_else(|| self.record.mailable.locale())
    }

    /// The delay a queued mailable was queued with.
    pub fn delay(&self) -> Option<Duration> {
        self.record.delay
    }
}

fn times(count: usize) -> &'static str {
    if count == 1 { "time" } else { "times" }
}

/// The mail fake. Install it with [`Mail::fake`](crate::Mail::fake).
///
/// ```
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_mail::{Mail, Mailable};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct OrderShipped { order_id: u64 }
///
/// impl Mailable for OrderShipped {}
///
/// let app = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(app);
///
/// Mail::fake();
///
/// Mail::to("taylor@example.com").send(OrderShipped { order_id: 1 }).await?;
///
/// Mail::assert_sent::<OrderShipped>();
/// Mail::assert_sent_to::<OrderShipped>("taylor@example.com");
/// Mail::assert_sent_with::<OrderShipped>(|mail| mail.order_id == 1);
/// Mail::assert_sent_count(1);
/// Mail::assert_nothing_queued();
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Default)]
pub struct MailFake {
    sent: Mutex<Vec<Record>>,
    queued: Mutex<Vec<Record>>,
}

impl std::fmt::Debug for MailFake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailFake")
            .field("sent", &self.sent.lock().unwrap().len())
            .field("queued", &self.queued.lock().unwrap().len())
            .finish()
    }
}

impl MailFake {
    /// Create an empty fake.
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn record_sent(&self, mailable: Arc<dyn Mailable>, options: SendOptions) {
        self.sent.lock().unwrap().push(Record {
            mailable,
            options,
            delay: None,
        });
    }

    pub(crate) fn record_queued(
        &self,
        mailable: Arc<dyn Mailable>,
        options: SendOptions,
        delay: Option<Duration>,
    ) {
        self.queued.lock().unwrap().push(Record {
            mailable,
            options,
            delay,
        });
    }

    fn matching<M: Mailable>(
        records: &Mutex<Vec<Record>>,
        callback: impl Fn(&FakeMailable<M>) -> bool,
    ) -> Vec<FakeMailable<M>> {
        records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| record.is::<M>())
            .map(|record| FakeMailable {
                record: record.clone(),
                _marker: PhantomData,
            })
            .filter(|mail| callback(mail))
            .collect()
    }

    /// Get all of the sent mailables of type `M` matching the callback.
    pub fn sent<M: Mailable>(
        &self,
        callback: impl Fn(&FakeMailable<M>) -> bool,
    ) -> Vec<FakeMailable<M>> {
        Self::matching(&self.sent, callback)
    }

    /// Get all of the queued mailables of type `M` matching the callback.
    pub fn queued<M: Mailable>(
        &self,
        callback: impl Fn(&FakeMailable<M>) -> bool,
    ) -> Vec<FakeMailable<M>> {
        Self::matching(&self.queued, callback)
    }

    /// Determine if a mailable of type `M` was sent.
    pub fn has_sent<M: Mailable>(&self) -> bool {
        !self.sent::<M>(|_| true).is_empty()
    }

    /// Determine if a mailable of type `M` was queued.
    pub fn has_queued<M: Mailable>(&self) -> bool {
        !self.queued::<M>(|_| true).is_empty()
    }

    fn suggestion(&self) -> &'static str {
        if self.queued.lock().unwrap().is_empty() {
            ""
        } else {
            " Did you mean to use assert_queued() instead?"
        }
    }

    // ------------------------------------------------------------------
    // Sent
    // ------------------------------------------------------------------

    /// Assert that a mailable of type `M` was sent.
    pub fn assert_sent<M: Mailable>(&self) {
        assert!(
            self.has_sent::<M>(),
            "The expected [{}] mailable was not sent.{}",
            class_basename::<M>(),
            self.suggestion()
        );
    }

    /// Assert that a mailable of type `M` passing the truth test was sent.
    pub fn assert_sent_with<M: Mailable>(&self, callback: impl Fn(&FakeMailable<M>) -> bool) {
        assert!(
            !self.sent::<M>(callback).is_empty(),
            "The expected [{}] mailable was not sent.{}",
            class_basename::<M>(),
            self.suggestion()
        );
    }

    /// Assert that a mailable of type `M` was sent to the given address.
    pub fn assert_sent_to<M: Mailable>(&self, address: &str) {
        assert!(
            !self.sent::<M>(|mail| mail.has_to(address)).is_empty(),
            "The expected [{}] mailable was not sent to address [{address}].{}",
            class_basename::<M>(),
            self.suggestion()
        );
    }

    /// Assert that a mailable of type `M` was sent a number of times.
    pub fn assert_sent_times<M: Mailable>(&self, expected: usize) {
        let count = self.sent::<M>(|_| true).len();
        assert!(
            count == expected,
            "The expected [{}] mailable was sent {count} {} instead of {expected} {}.",
            class_basename::<M>(),
            times(count),
            times(expected)
        );
    }

    /// Assert that a mailable of type `M` was sent exactly once.
    pub fn assert_sent_once<M: Mailable>(&self) {
        self.assert_sent_times::<M>(1);
    }

    /// Assert that no mailable of type `M` was sent.
    pub fn assert_not_sent<M: Mailable>(&self) {
        assert!(
            !self.has_sent::<M>(),
            "The unexpected [{}] mailable was sent.",
            class_basename::<M>()
        );
    }

    /// Assert that no mailable of type `M` passing the truth test was sent.
    pub fn assert_not_sent_with<M: Mailable>(&self, callback: impl Fn(&FakeMailable<M>) -> bool) {
        assert!(
            self.sent::<M>(callback).is_empty(),
            "The unexpected [{}] mailable was sent.",
            class_basename::<M>()
        );
    }

    /// Assert that no mailable of type `M` was sent to the given address.
    pub fn assert_not_sent_to<M: Mailable>(&self, address: &str) {
        assert!(
            self.sent::<M>(|mail| mail.has_to(address)).is_empty(),
            "The unexpected [{}] mailable was sent to address [{address}].",
            class_basename::<M>()
        );
    }

    /// Assert that no mailables were sent.
    pub fn assert_nothing_sent(&self) {
        let names = Self::names(&self.sent);
        assert!(
            names.is_empty(),
            "The following mailables were sent unexpectedly:\n\n- {}\n",
            names.join("\n- ")
        );
    }

    /// Assert the total number of mailables that were sent.
    pub fn assert_sent_count(&self, expected: usize) {
        let total = self.sent.lock().unwrap().len();
        assert!(
            total == expected,
            "The total number of mailables sent was {total} instead of {expected}."
        );
    }

    // ------------------------------------------------------------------
    // Queued
    // ------------------------------------------------------------------

    /// Assert that a mailable of type `M` was queued.
    pub fn assert_queued<M: Mailable>(&self) {
        assert!(
            self.has_queued::<M>(),
            "The expected [{}] mailable was not queued.",
            class_basename::<M>()
        );
    }

    /// Assert that a mailable of type `M` passing the truth test was queued.
    pub fn assert_queued_with<M: Mailable>(&self, callback: impl Fn(&FakeMailable<M>) -> bool) {
        assert!(
            !self.queued::<M>(callback).is_empty(),
            "The expected [{}] mailable was not queued.",
            class_basename::<M>()
        );
    }

    /// Assert that a mailable of type `M` was queued to the given address.
    pub fn assert_queued_to<M: Mailable>(&self, address: &str) {
        assert!(
            !self.queued::<M>(|mail| mail.has_to(address)).is_empty(),
            "The expected [{}] mailable was not queued to address [{address}].",
            class_basename::<M>()
        );
    }

    /// Assert that a mailable of type `M` was queued a number of times.
    pub fn assert_queued_times<M: Mailable>(&self, expected: usize) {
        let count = self.queued::<M>(|_| true).len();
        assert!(
            count == expected,
            "The expected [{}] mailable was queued {count} {} instead of {expected} {}.",
            class_basename::<M>(),
            times(count),
            times(expected)
        );
    }

    /// Assert that a mailable of type `M` was queued exactly once.
    pub fn assert_queued_once<M: Mailable>(&self) {
        self.assert_queued_times::<M>(1);
    }

    /// Assert that no mailable of type `M` was queued.
    pub fn assert_not_queued<M: Mailable>(&self) {
        assert!(
            !self.has_queued::<M>(),
            "The unexpected [{}] mailable was queued.",
            class_basename::<M>()
        );
    }

    /// Assert that no mailable of type `M` passing the truth test was queued.
    pub fn assert_not_queued_with<M: Mailable>(&self, callback: impl Fn(&FakeMailable<M>) -> bool) {
        assert!(
            self.queued::<M>(callback).is_empty(),
            "The unexpected [{}] mailable was queued.",
            class_basename::<M>()
        );
    }

    /// Assert that no mailables were queued.
    pub fn assert_nothing_queued(&self) {
        let names = Self::names(&self.queued);
        assert!(
            names.is_empty(),
            "The following mailables were queued unexpectedly:\n\n- {}\n",
            names.join("\n- ")
        );
    }

    /// Assert the total number of mailables that were queued.
    pub fn assert_queued_count(&self, expected: usize) {
        let total = self.queued.lock().unwrap().len();
        assert!(
            total == expected,
            "The total number of mailables queued was {total} instead of {expected}."
        );
    }

    // ------------------------------------------------------------------
    // Outgoing (sent or queued)
    // ------------------------------------------------------------------

    /// Assert that no mailable of type `M` was sent or queued.
    pub fn assert_not_outgoing<M: Mailable>(&self) {
        self.assert_not_sent::<M>();
        self.assert_not_queued::<M>();
    }

    /// Assert that no mailables were sent or queued.
    pub fn assert_nothing_outgoing(&self) {
        self.assert_nothing_sent();
        self.assert_nothing_queued();
    }

    /// Assert the total number of mailables that were sent or queued.
    pub fn assert_outgoing_count(&self, expected: usize) {
        let total = self.sent.lock().unwrap().len() + self.queued.lock().unwrap().len();
        assert!(
            total == expected,
            "The total number of outgoing mailables was {total} instead of {expected}."
        );
    }

    fn names(records: &Mutex<Vec<Record>>) -> Vec<String> {
        records
            .lock()
            .unwrap()
            .iter()
            .map(|record| record.mailable.mailable_name())
            .collect()
    }
}
