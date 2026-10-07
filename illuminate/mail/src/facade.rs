//! The `Mail` facade.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use illuminate_support::{Result, Value};
use serde::Serialize;

use crate::address::{Address, IntoAddresses};
use crate::fake::{FakeMailable, MailFake};
use crate::mailable::{MailView, Mailable};
use crate::mailer::Mailer;
use crate::manager::MailManager;
use crate::message::{Message, SentMessage};
use crate::pending::PendingMail;
use crate::queue::QueuedMessage;
use crate::transport::Transport;

/// The `Mail` facade: static access to the [`MailManager`] in the container.
///
/// ```no_run
/// # async fn example() -> illuminate_support::Result<()> {
/// # use illuminate_mail::Mailable;
/// # #[derive(serde::Serialize)] struct OrderShipped { order_id: u64 }
/// # impl Mailable for OrderShipped {}
/// use illuminate_mail::Mail;
///
/// Mail::to("taylor@example.com").send(OrderShipped { order_id: 1 }).await?;
///
/// Mail::mailer("postmark")?.to("taylor@example.com").send(OrderShipped { order_id: 2 }).await?;
///
/// Mail::raw("Hello world!", |message| {
///     message.to("taylor@example.com").subject("Hi");
/// })
/// .await?;
/// # Ok(()) }
/// ```
pub struct Mail;

impl Mail {
    /// Get the mail manager.
    pub fn manager() -> Arc<MailManager> {
        MailManager::resolve()
    }

    /// Get a mailer instance by name.
    pub fn mailer(name: &str) -> Result<Arc<Mailer>> {
        Self::manager().mailer(Some(name))
    }

    /// Get a mailer instance by name (alias of [`Mail::mailer`]).
    pub fn driver(name: &str) -> Result<Arc<Mailer>> {
        Self::mailer(name)
    }

    /// Get the default mailer.
    pub fn default_mailer() -> Result<Arc<Mailer>> {
        Self::manager().mailer(None)
    }

    /// Build a new, on-demand mailer from the given configuration.
    pub fn build(config: Value) -> Result<Arc<Mailer>> {
        Self::manager().build(config)
    }

    /// Register a custom transport creator.
    pub fn extend(
        driver: &str,
        creator: impl Fn(&Value) -> Result<Arc<dyn Transport>> + Send + Sync + 'static,
    ) {
        Self::manager().extend(driver, creator);
    }

    /// Forget a resolved mailer (the default when `None`).
    pub fn purge(name: Option<&str>) {
        Self::manager().purge(name);
    }

    // ------------------------------------------------------------------
    // Global addresses
    // ------------------------------------------------------------------

    /// Set the global "from" address of the default mailer.
    pub fn always_from(address: impl Into<Address>) -> Result<()> {
        Self::default_mailer()?.always_from(address);
        Ok(())
    }

    /// Set the global "reply to" address of the default mailer.
    pub fn always_reply_to(address: impl Into<Address>) -> Result<()> {
        Self::default_mailer()?.always_reply_to(address);
        Ok(())
    }

    /// Set the global "return path" address of the default mailer.
    pub fn always_return_path(address: impl Into<Address>) -> Result<()> {
        Self::default_mailer()?.always_return_path(address);
        Ok(())
    }

    /// Send every message from the default mailer to this address instead.
    pub fn always_to(address: impl Into<Address>) -> Result<()> {
        Self::default_mailer()?.always_to(address);
        Ok(())
    }

    // ------------------------------------------------------------------
    // Sending
    // ------------------------------------------------------------------

    /// Begin the process of mailing a mailable to the given recipients.
    pub fn to(users: impl IntoAddresses) -> PendingMail {
        PendingMail::new(None).to(users)
    }

    /// Begin the process of mailing a mailable, copying the given recipients.
    pub fn cc(users: impl IntoAddresses) -> PendingMail {
        PendingMail::new(None).cc(users)
    }

    /// Begin the process of mailing a mailable, blind copying the given recipients.
    pub fn bcc(users: impl IntoAddresses) -> PendingMail {
        PendingMail::new(None).bcc(users)
    }

    /// Send a mailable (to the recipients in its envelope).
    pub async fn send<M: Mailable>(mailable: M) -> Result<Option<SentMessage>> {
        PendingMail::new(None).send(mailable).await
    }

    /// Send a mailable immediately, even if it should be queued.
    pub async fn send_now<M: Mailable>(mailable: M) -> Result<Option<SentMessage>> {
        PendingMail::new(None).send_now(mailable).await
    }

    /// Queue a mailable for sending.
    pub async fn queue<M: Mailable>(mailable: M) -> Result<()> {
        PendingMail::new(None).queue(mailable).await
    }

    /// Queue a mailable for sending after a delay.
    pub async fn later<M: Mailable>(delay: Duration, mailable: M) -> Result<()> {
        PendingMail::new(None).later(delay, mailable).await
    }

    /// Send a new message with only a raw text part.
    pub async fn raw(
        text: impl Into<String>,
        callback: impl FnOnce(&mut Message) + Send,
    ) -> Result<Option<SentMessage>> {
        Self::default_mailer()?.raw(text, callback).await
    }

    /// Send a new message with only an HTML part.
    pub async fn html(
        html: impl Into<String>,
        callback: impl FnOnce(&mut Message) + Send,
    ) -> Result<Option<SentMessage>> {
        Self::default_mailer()?.html(html, callback).await
    }

    /// Send a new message with only a plain part, rendered from a view.
    pub async fn plain(
        view: impl Into<String>,
        data: impl Serialize + Send,
        callback: impl FnOnce(&mut Message) + Send,
    ) -> Result<Option<SentMessage>> {
        Self::default_mailer()?.plain(view, data, callback).await
    }

    /// Send a new message using a view.
    pub async fn send_view(
        view: impl Into<MailView>,
        data: impl Serialize + Send,
        callback: impl FnOnce(&mut Message) + Send,
    ) -> Result<Option<SentMessage>> {
        Self::default_mailer()?
            .send_view(view, data, callback)
            .await
    }

    /// Render a mailable into HTML.
    pub fn render<M: Mailable>(mailable: &M) -> Result<String> {
        mailable.render()
    }

    // ------------------------------------------------------------------
    // Queueing
    // ------------------------------------------------------------------

    /// Install the hook that pushes queued mail (as a pre-rendered
    /// [`QueuedMessage`]) onto the queue. See [the queue module](crate::queue).
    pub fn queue_using<F, Fut>(hook: F)
    where
        F: Fn(QueuedMessage) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self::manager().queue_using(hook);
    }

    /// Deliver a message taken off the queue (what the queue worker calls).
    pub async fn send_queued(queued: QueuedMessage) -> Result<Option<SentMessage>> {
        Self::manager().send_queued(queued).await
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    /// Replace sending with a fake that records mailables, for the current
    /// container only.
    pub fn fake() -> Arc<MailFake> {
        Self::manager().fake()
    }

    /// Determine if mail is being faked.
    pub fn is_fake() -> bool {
        Self::manager().is_fake()
    }

    fn the_fake() -> Arc<MailFake> {
        Self::manager()
            .get_fake()
            .expect("Mail is not faked. Call Mail::fake() first.")
    }

    /// Get the sent mailables of type `M` matching the callback.
    pub fn sent<M: Mailable>(callback: impl Fn(&FakeMailable<M>) -> bool) -> Vec<FakeMailable<M>> {
        Self::the_fake().sent(callback)
    }

    /// Get the queued mailables of type `M` matching the callback.
    pub fn queued<M: Mailable>(
        callback: impl Fn(&FakeMailable<M>) -> bool,
    ) -> Vec<FakeMailable<M>> {
        Self::the_fake().queued(callback)
    }

    /// Determine if a mailable of type `M` was sent.
    pub fn has_sent<M: Mailable>() -> bool {
        Self::the_fake().has_sent::<M>()
    }

    /// Determine if a mailable of type `M` was queued.
    pub fn has_queued<M: Mailable>() -> bool {
        Self::the_fake().has_queued::<M>()
    }

    /// Assert that a mailable of type `M` was sent.
    pub fn assert_sent<M: Mailable>() {
        Self::the_fake().assert_sent::<M>();
    }

    /// Assert that a mailable of type `M` passing the truth test was sent.
    pub fn assert_sent_with<M: Mailable>(callback: impl Fn(&FakeMailable<M>) -> bool) {
        Self::the_fake().assert_sent_with(callback);
    }

    /// Assert that a mailable of type `M` was sent to the given address.
    pub fn assert_sent_to<M: Mailable>(address: &str) {
        Self::the_fake().assert_sent_to::<M>(address);
    }

    /// Assert that a mailable of type `M` was sent a number of times.
    pub fn assert_sent_times<M: Mailable>(times: usize) {
        Self::the_fake().assert_sent_times::<M>(times);
    }

    /// Assert that a mailable of type `M` was sent exactly once.
    pub fn assert_sent_once<M: Mailable>() {
        Self::the_fake().assert_sent_once::<M>();
    }

    /// Assert that no mailable of type `M` was sent.
    pub fn assert_not_sent<M: Mailable>() {
        Self::the_fake().assert_not_sent::<M>();
    }

    /// Assert that no mailable of type `M` passing the truth test was sent.
    pub fn assert_not_sent_with<M: Mailable>(callback: impl Fn(&FakeMailable<M>) -> bool) {
        Self::the_fake().assert_not_sent_with(callback);
    }

    /// Assert that no mailable of type `M` was sent to the given address.
    pub fn assert_not_sent_to<M: Mailable>(address: &str) {
        Self::the_fake().assert_not_sent_to::<M>(address);
    }

    /// Assert that no mailables were sent.
    pub fn assert_nothing_sent() {
        Self::the_fake().assert_nothing_sent();
    }

    /// Assert the total number of mailables that were sent.
    pub fn assert_sent_count(count: usize) {
        Self::the_fake().assert_sent_count(count);
    }

    /// Assert that a mailable of type `M` was queued.
    pub fn assert_queued<M: Mailable>() {
        Self::the_fake().assert_queued::<M>();
    }

    /// Assert that a mailable of type `M` passing the truth test was queued.
    pub fn assert_queued_with<M: Mailable>(callback: impl Fn(&FakeMailable<M>) -> bool) {
        Self::the_fake().assert_queued_with(callback);
    }

    /// Assert that a mailable of type `M` was queued to the given address.
    pub fn assert_queued_to<M: Mailable>(address: &str) {
        Self::the_fake().assert_queued_to::<M>(address);
    }

    /// Assert that a mailable of type `M` was queued a number of times.
    pub fn assert_queued_times<M: Mailable>(times: usize) {
        Self::the_fake().assert_queued_times::<M>(times);
    }

    /// Assert that a mailable of type `M` was queued exactly once.
    pub fn assert_queued_once<M: Mailable>() {
        Self::the_fake().assert_queued_once::<M>();
    }

    /// Assert that no mailable of type `M` was queued.
    pub fn assert_not_queued<M: Mailable>() {
        Self::the_fake().assert_not_queued::<M>();
    }

    /// Assert that no mailable of type `M` passing the truth test was queued.
    pub fn assert_not_queued_with<M: Mailable>(callback: impl Fn(&FakeMailable<M>) -> bool) {
        Self::the_fake().assert_not_queued_with(callback);
    }

    /// Assert that no mailables were queued.
    pub fn assert_nothing_queued() {
        Self::the_fake().assert_nothing_queued();
    }

    /// Assert the total number of mailables that were queued.
    pub fn assert_queued_count(count: usize) {
        Self::the_fake().assert_queued_count(count);
    }

    /// Assert that no mailable of type `M` was sent or queued.
    pub fn assert_not_outgoing<M: Mailable>() {
        Self::the_fake().assert_not_outgoing::<M>();
    }

    /// Assert that no mailables were sent or queued.
    pub fn assert_nothing_outgoing() {
        Self::the_fake().assert_nothing_outgoing();
    }

    /// Assert the total number of mailables that were sent or queued.
    pub fn assert_outgoing_count(count: usize) {
        Self::the_fake().assert_outgoing_count(count);
    }
}
