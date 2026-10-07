//! The mailer: renders messages and hands them to a transport.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use illuminate_container::try_app;
use illuminate_events::Dispatcher;
use illuminate_support::{Map, Result, Value, to_value};
use serde::Serialize;

use crate::address::{Address, IntoAddresses};
use crate::events::{MessageSending, MessageSent};
use crate::fake::MailFake;
use crate::mailable::{
    MailView, Mailable, MailableBuilder, SendOptions, ViewSpec, mailable_view_data, render_views,
};
use crate::message::{Message, SentMessage};
use crate::pending::PendingMail;
use crate::queue::{QueueHook, QueuedMessage};
use crate::transport::Transport;

/// State shared by a mail manager and every mailer it creates: the fake
/// (when mail is faked) and the queue hook.
#[derive(Default)]
pub(crate) struct Shared {
    pub(crate) fake: RwLock<Option<Arc<MailFake>>>,
    pub(crate) queue: RwLock<Option<QueueHook>>,
}

impl Shared {
    pub(crate) fn fake(&self) -> Option<Arc<MailFake>> {
        self.fake.read().unwrap().clone()
    }

    pub(crate) fn queue_hook(&self) -> Option<QueueHook> {
        self.queue.read().unwrap().clone()
    }
}

/// A mailer: one configured way of sending mail (`mail.mailers.*`).
///
/// ```
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// use std::sync::Arc;
/// use illuminate_mail::{ArrayTransport, Mailer, downcast_transport};
///
/// let mailer = Mailer::new("array", Arc::new(ArrayTransport::new()));
/// mailer.always_from(("hello@example.com", "Example"));
///
/// mailer
///     .raw("Welcome to the app!", |message| {
///         message.to("taylor@example.com").subject("Welcome");
///     })
///     .await?;
///
/// let transport = downcast_transport::<ArrayTransport>(&mailer.transport()).unwrap();
/// let sent = &transport.messages()[0].message;
/// assert_eq!(sent.from[0].address, "hello@example.com");
/// assert_eq!(sent.text.as_deref(), Some("Welcome to the app!"));
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub struct Mailer {
    name: String,
    transport: RwLock<Arc<dyn Transport>>,
    from: RwLock<Option<Address>>,
    reply_to: RwLock<Option<Address>>,
    return_path: RwLock<Option<Address>>,
    to: RwLock<Option<Address>>,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Mailer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mailer")
            .field("name", &self.name)
            .field("transport", &self.transport().name())
            .finish_non_exhaustive()
    }
}

impl Mailer {
    /// Create a new mailer using the given transport.
    pub fn new(name: impl Into<String>, transport: Arc<dyn Transport>) -> Self {
        Self::with_shared(name, transport, Arc::new(Shared::default()))
    }

    pub(crate) fn with_shared(
        name: impl Into<String>,
        transport: Arc<dyn Transport>,
        shared: Arc<Shared>,
    ) -> Self {
        Self {
            name: name.into(),
            transport: RwLock::new(transport),
            from: RwLock::new(None),
            reply_to: RwLock::new(None),
            return_path: RwLock::new(None),
            to: RwLock::new(None),
            shared,
        }
    }

    /// The mailer's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Set the global from address and name.
    pub fn always_from(&self, address: impl Into<Address>) {
        *self.from.write().unwrap() = Some(address.into());
    }

    /// Set the global reply-to address and name.
    pub fn always_reply_to(&self, address: impl Into<Address>) {
        *self.reply_to.write().unwrap() = Some(address.into());
    }

    /// Set the global return path address.
    pub fn always_return_path(&self, address: impl Into<Address>) {
        *self.return_path.write().unwrap() = Some(address.into());
    }

    /// Set the global to address and name: every message goes there instead
    /// of to its recipients (handy during local development).
    pub fn always_to(&self, address: impl Into<Address>) {
        *self.to.write().unwrap() = Some(address.into());
    }

    /// Get the transport.
    pub fn transport(&self) -> Arc<dyn Transport> {
        self.transport.read().unwrap().clone()
    }

    /// Replace the transport.
    pub fn set_transport(&self, transport: Arc<dyn Transport>) {
        *self.transport.write().unwrap() = transport;
    }

    // ------------------------------------------------------------------
    // Pending mail
    // ------------------------------------------------------------------

    /// Begin the process of mailing a mailable to the given recipients.
    pub fn to(self: &Arc<Self>, users: impl IntoAddresses) -> PendingMail {
        PendingMail::new(Some(self.clone())).to(users)
    }

    /// Begin the process of mailing a mailable, copying the given recipients.
    pub fn cc(self: &Arc<Self>, users: impl IntoAddresses) -> PendingMail {
        PendingMail::new(Some(self.clone())).cc(users)
    }

    /// Begin the process of mailing a mailable, blind copying the given recipients.
    pub fn bcc(self: &Arc<Self>, users: impl IntoAddresses) -> PendingMail {
        PendingMail::new(Some(self.clone())).bcc(users)
    }

    // ------------------------------------------------------------------
    // Mailables
    // ------------------------------------------------------------------

    /// Send a mailable (queueing it when it [should be queued](Mailable::should_queue)).
    pub async fn send<M: Mailable>(&self, mailable: M) -> Result<Option<SentMessage>> {
        self.send_mailable(Arc::new(mailable), SendOptions::default())
            .await
    }

    /// Send a mailable immediately, even if it should be queued.
    pub async fn send_now<M: Mailable>(&self, mailable: M) -> Result<Option<SentMessage>> {
        self.send_mailable_now(Arc::new(mailable), SendOptions::default())
            .await
    }

    /// Queue a mailable for sending.
    pub async fn queue<M: Mailable>(&self, mailable: M) -> Result<()> {
        self.queue_mailable(Arc::new(mailable), SendOptions::default(), None)
            .await
    }

    /// Queue a mailable for sending after the given delay.
    pub async fn later<M: Mailable>(&self, delay: Duration, mailable: M) -> Result<()> {
        self.queue_mailable(Arc::new(mailable), SendOptions::default(), Some(delay))
            .await
    }

    /// Send a shared mailable with the given options, queueing it when it
    /// should be queued.
    pub async fn send_mailable(
        &self,
        mailable: Arc<dyn Mailable>,
        options: SendOptions,
    ) -> Result<Option<SentMessage>> {
        if mailable.should_queue() {
            self.queue_mailable(mailable, options, None).await?;
            return Ok(None);
        }
        self.send_mailable_now(mailable, options).await
    }

    fn options_for(&self, mut options: SendOptions) -> SendOptions {
        if options.mailer.is_none() {
            options.mailer = Some(self.name.clone());
        }
        options
    }

    /// Send a shared mailable immediately.
    pub async fn send_mailable_now(
        &self,
        mailable: Arc<dyn Mailable>,
        options: SendOptions,
    ) -> Result<Option<SentMessage>> {
        let options = self.options_for(options);
        if let Some(fake) = self.shared.fake() {
            fake.record_sent(mailable, options);
            return Ok(None);
        }
        let builder = MailableBuilder::for_mailable(mailable.as_ref(), &options);
        let locale = builder.locale.clone();
        let send = async {
            let (message, data) = self
                .build_mailable_message(mailable.as_ref(), &builder)
                .await?;
            self.send_message(message, data).await
        };
        match locale {
            Some(locale) => illuminate_translation::with_locale_async(&locale, send).await,
            None => send.await,
        }
    }

    /// Queue a shared mailable: render it, then hand it to the queue hook
    /// (or send it right away when no hook is installed).
    pub async fn queue_mailable(
        &self,
        mailable: Arc<dyn Mailable>,
        options: SendOptions,
        delay: Option<Duration>,
    ) -> Result<()> {
        let options = self.options_for(options);
        let delay = delay.or_else(|| mailable.queue_delay());
        if let Some(fake) = self.shared.fake() {
            fake.record_queued(mailable, options, delay);
            return Ok(());
        }
        let builder = MailableBuilder::for_mailable(mailable.as_ref(), &options);
        let locale = builder.locale.clone();
        let build = self.build_mailable_message(mailable.as_ref(), &builder);
        let (message, data) = match locale {
            Some(locale) => illuminate_translation::with_locale_async(&locale, build).await?,
            None => build.await?,
        };
        let queued = QueuedMessage {
            mailer: self.name.clone(),
            mailable: mailable.mailable_name(),
            message,
            data,
            connection: mailable.queue_connection(),
            queue: mailable.queue_name(),
            delay,
        };
        match self.shared.queue_hook() {
            Some(hook) => hook(queued).await,
            None => self
                .send_message(queued.message, queued.data)
                .await
                .map(|_| ()),
        }
    }

    /// Render a mailable and build its message.
    async fn build_mailable_message(
        &self,
        mailable: &dyn Mailable,
        builder: &MailableBuilder,
    ) -> Result<(Message, Value)> {
        let data = mailable_view_data(mailable, builder, Some(&self.name));
        let rendered = render_views(&builder.view_spec(), data.clone(), builder.theme.as_deref())?;
        let mut message = self.create_message();
        message.html = rendered.html;
        message.text = rendered.text;
        builder.apply_to(&mut message, &mailable.mailable_name());
        message.attachments.extend(rendered.embeds);
        message.resolve_attachments().await?;
        Ok((message, Value::Object(data)))
    }

    /// Render a mailable into HTML.
    pub fn render<M: Mailable>(&self, mailable: &M) -> Result<String> {
        mailable.render()
    }

    // ------------------------------------------------------------------
    // Views & raw messages
    // ------------------------------------------------------------------

    /// Send a new message with only a raw text part.
    pub async fn raw(
        &self,
        text: impl Into<String>,
        callback: impl FnOnce(&mut Message) + Send,
    ) -> Result<Option<SentMessage>> {
        self.send_view(MailView::Raw(text.into()), (), callback)
            .await
    }

    /// Send a new message with only an HTML part.
    pub async fn html(
        &self,
        html: impl Into<String>,
        callback: impl FnOnce(&mut Message) + Send,
    ) -> Result<Option<SentMessage>> {
        self.send_view(MailView::Html(html.into()), (), callback)
            .await
    }

    /// Send a new message with only a plain part, rendered from a view.
    pub async fn plain(
        &self,
        view: impl Into<String>,
        data: impl Serialize + Send,
        callback: impl FnOnce(&mut Message) + Send,
    ) -> Result<Option<SentMessage>> {
        self.send_view(MailView::Text(view.into()), data, callback)
            .await
    }

    /// Send a new message using a view (Laravel's `Mail::send($view, $data, $callback)`).
    ///
    /// When mail is faked, messages that aren't mailables are not recorded
    /// (just like Laravel's fake) and nothing is sent.
    pub async fn send_view(
        &self,
        view: impl Into<MailView>,
        data: impl Serialize + Send,
        callback: impl FnOnce(&mut Message) + Send,
    ) -> Result<Option<SentMessage>> {
        if self.shared.fake().is_some() {
            return Ok(None);
        }
        let (message, data) = self.compose(view, data, None, callback).await?;
        self.send_message(message, data).await
    }

    /// Render a message from views and build it, without sending it: the
    /// global addresses are applied, the callback runs and attachments are
    /// read. Returns the message and the data it was rendered with, ready
    /// for [`Mailer::send_message`] (or for the queue).
    pub async fn compose(
        &self,
        view: impl Into<MailView>,
        data: impl Serialize + Send,
        theme: Option<String>,
        callback: impl FnOnce(&mut Message) + Send,
    ) -> Result<(Message, Value)> {
        let mut data = match to_value(&data) {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        data.insert("mailer".into(), Value::String(self.name.clone()));
        let spec: ViewSpec = view.into().into();
        let rendered = render_views(&spec, data.clone(), theme.as_deref())?;
        let mut message = self.create_message();
        message.html = rendered.html;
        message.text = rendered.text;
        message.attachments.extend(rendered.embeds);
        callback(&mut message);
        message.resolve_attachments().await?;
        Ok((message, Value::Object(data)))
    }

    /// Render a mailable and build its message, without sending it.
    pub async fn compose_mailable(
        &self,
        mailable: &dyn Mailable,
        options: SendOptions,
    ) -> Result<(Message, Value)> {
        let options = self.options_for(options);
        let builder = MailableBuilder::for_mailable(mailable, &options);
        match builder.locale.clone() {
            Some(locale) => {
                illuminate_translation::with_locale_async(
                    &locale,
                    self.build_mailable_message(mailable, &builder),
                )
                .await
            }
            None => self.build_mailable_message(mailable, &builder).await,
        }
    }

    /// Create a new message with the global "from", "reply to" and "return
    /// path" addresses.
    pub fn create_message(&self) -> Message {
        let mut message = Message::new();
        if let Some(from) = self.from.read().unwrap().clone() {
            message.from(from);
        }
        if let Some(reply_to) = self.reply_to.read().unwrap().clone() {
            message.reply_to(reply_to);
        }
        if let Some(return_path) = self.return_path.read().unwrap().clone() {
            message.return_path(return_path);
        }
        message
    }

    // ------------------------------------------------------------------
    // Sending
    // ------------------------------------------------------------------

    /// Send a built message: apply the global "to" address, fire the
    /// [`MessageSending`] event (which may cancel the message), hand it to
    /// the transport and fire [`MessageSent`].
    ///
    /// While mail is faked, messages are dropped (only mailables are recorded).
    pub async fn send_message(
        &self,
        mut message: Message,
        data: Value,
    ) -> Result<Option<SentMessage>> {
        if self.shared.fake().is_some() {
            return Ok(None);
        }
        if let Some(to) = self.to.read().unwrap().clone() {
            message.forget_to().forget_cc().forget_bcc();
            message.to(to);
        }
        message.resolve_attachments().await?;
        if message.message_id.is_none() {
            message.message_id = Some(crate::mime::generate_message_id(&message));
        }

        let events = try_app::<Dispatcher>();
        if let Some(events) = &events {
            let sending = MessageSending {
                message: message.clone(),
                data: data.clone(),
                mailer: self.name.clone(),
            };
            if !events.until(sending).await? {
                return Ok(None);
            }
        }

        // Like Symfony, refuse to send messages without a sender or recipients.
        crate::mime::envelope(&message)?;
        let sent = self.transport().send(&message).await?;

        if let Some(events) = &events {
            events
                .dispatch(MessageSent {
                    sent: sent.clone(),
                    data,
                    mailer: self.name.clone(),
                })
                .await?;
        }
        Ok(Some(sent))
    }

    /// Deliver a message taken off the queue.
    pub async fn send_queued(&self, queued: QueuedMessage) -> Result<Option<SentMessage>> {
        self.send_message(queued.message, queued.data).await
    }
}
