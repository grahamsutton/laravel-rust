//! End-to-end tests: notifications over the mail and database channels.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_database::{DatabaseManager, Migration};
use illuminate_events::Event;
use illuminate_mail::{
    ArrayTransport, Content, Envelope, MailManager, MailServiceProvider, Mailable, SentMessage,
    downcast_transport,
};
use illuminate_notifications::facades::Notification;
use illuminate_notifications::{
    AnonymousNotifiable, Channel, CreateNotificationsTable, DatabaseNotification,
    HasDatabaseNotifications, MailMessage, Notifiable, Notification as NotificationContract,
    NotificationFailed, NotificationSending, NotificationSent, NotificationServiceProvider,
    QueuedNotification, async_trait,
};
use illuminate_support::{Result, Value, json};
use illuminate_view::{Factory, ViewValue};
use serde::Serialize;
use tempfile::TempDir;

struct App {
    container: Arc<Container>,
    views: TempDir,
    _guard: LocalInstanceGuard,
}

impl App {
    fn new() -> Self {
        Self::with_config(json!({}))
    }

    fn with_config(extra: Value) -> Self {
        let views = tempfile::tempdir().unwrap();
        let container = Arc::new(Container::new());
        let guard = Container::set_local_instance(container.clone());
        let mut config = json!({
            "app": {"name": "Laravel", "url": "https://laravel.test", "locale": "en"},
            "view": {"paths": [views.path()]},
            "mail": {
                "default": "array",
                "mailers": {"array": {"transport": "array"}},
                "from": {"address": "hello@example.com", "name": "Example"},
            },
        });
        if let (Value::Object(config), Value::Object(extra)) = (&mut config, extra) {
            config.extend(extra);
        }
        container.instance(Repository::new(config));
        MailServiceProvider.register(&container);
        NotificationServiceProvider.register(&container);
        MailServiceProvider.boot(&container);
        NotificationServiceProvider.boot(&container);
        Self {
            container,
            views,
            _guard: guard,
        }
    }

    fn view(&self, name: &str, contents: &str) {
        let path = self
            .views
            .path()
            .join(format!("{}.blade.html", name.replace('.', "/")));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn sent_mail(&self) -> Vec<SentMessage> {
        let mailer = self.container.make::<MailManager>().mailer(None).unwrap();
        downcast_transport::<ArrayTransport>(&mailer.transport())
            .unwrap()
            .messages()
    }

    async fn with_database(&self) {
        self.container.instance(DatabaseManager::from_config(json!({
            "default": "sqlite",
            "connections": {"sqlite": {"driver": "sqlite", "database": ":memory:"}},
        })));
        CreateNotificationsTable.up().await.unwrap();
    }
}

#[derive(Serialize)]
struct User {
    id: u64,
    name: String,
    email: String,
    #[serde(skip)]
    locale: Option<String>,
}

impl Notifiable for User {
    fn notifiable_key(&self) -> Value {
        json!(self.id)
    }

    fn notifiable_type(&self) -> String {
        "App\\Models\\User".into()
    }

    fn preferred_locale(&self) -> Option<String> {
        self.locale.clone()
    }
}

fn taylor() -> User {
    User {
        id: 1,
        name: "Taylor".into(),
        email: "taylor@example.com".into(),
        locale: None,
    }
}

fn abigail() -> User {
    User {
        id: 2,
        name: "Abigail".into(),
        email: "abigail@example.com".into(),
        locale: None,
    }
}

#[derive(Clone, Serialize)]
struct InvoicePaid {
    invoice_id: u64,
    amount: u32,
    channels: Vec<&'static str>,
    queued: bool,
}

impl InvoicePaid {
    fn new(channels: &[&'static str]) -> Self {
        Self {
            invoice_id: 7,
            amount: 99,
            channels: channels.to_vec(),
            queued: false,
        }
    }
}

impl NotificationContract for InvoicePaid {
    fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
        self.channels.iter().map(|c| c.to_string()).collect()
    }

    fn to_mail(&self, _notifiable: &dyn Notifiable) -> Option<MailMessage> {
        Some(
            MailMessage::new()
                .line("One of your invoices has been paid!")
                .action(
                    "View Invoice",
                    format!("https://laravel.test/invoices/{}", self.invoice_id),
                )
                .line("Thank you for using our application!")
                .tag("invoices"),
        )
    }

    fn to_array(&self, _notifiable: &dyn Notifiable) -> Option<Value> {
        Some(json!({"invoice_id": self.invoice_id, "amount": self.amount}))
    }

    fn to_channel(&self, channel: &str, _notifiable: &dyn Notifiable) -> Option<Value> {
        (channel == "sms").then(|| json!(format!("Invoice {} paid", self.invoice_id)))
    }

    fn should_queue(&self) -> bool {
        self.queued
    }

    fn queue_name(&self, channel: &str) -> Option<String> {
        (channel == "mail").then(|| "mail".to_string())
    }

    fn queue_delay(&self, _notifiable: &dyn Notifiable, channel: &str) -> Option<Duration> {
        (channel == "database").then_some(Duration::from_secs(30))
    }
}

#[tokio::test]
async fn mail_notifications_use_the_default_template() {
    let app = App::new();
    taylor().notify(InvoicePaid::new(&["mail"])).await.unwrap();

    let sent = app.sent_mail();
    assert_eq!(sent.len(), 1);
    let message = &sent[0].message;
    assert!(message.has_to("taylor@example.com"));
    assert!(message.has_from("hello@example.com"));
    assert_eq!(message.subject.as_deref(), Some("Invoice Paid"));
    assert_eq!(message.tags, vec!["invoices".to_string()]);

    let html = message.html.as_deref().unwrap();
    assert!(html.contains(">Hello!</h1>"), "{html}");
    assert!(html.contains(">One of your invoices has been paid!</p>"));
    assert!(
        html.contains("href=\"https://laravel.test/invoices/7\" class=\"button button-primary\"")
    );
    assert!(html.contains(">Thank you for using our application!</p>"));
    assert!(html.contains("Regards,<br>"));
    assert!(html.contains("If you're having trouble clicking the &quot;View Invoice&quot; button"));
    assert!(html.contains("<a href=\"https://laravel.test/invoices/7\""));

    let text = message.text.as_deref().unwrap();
    assert!(text.contains("# Hello!"));
    assert!(text.contains("View Invoice: https://laravel.test/invoices/7"));
    assert!(text.contains("Regards,\nLaravel"), "{text}");
}

#[derive(Serialize)]
struct PaymentFailed;

impl NotificationContract for PaymentFailed {
    fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
        vec!["mail".into()]
    }

    fn to_mail(&self, _notifiable: &dyn Notifiable) -> Option<MailMessage> {
        Some(
            MailMessage::new()
                .error()
                .subject("Payment Failed")
                .line("We could not charge your card.")
                .action("Update Card", "https://laravel.test/billing")
                .salutation("The Billing Team")
                .cc("billing@example.com")
                .from(("billing@example.com", "Billing"))
                .attach_data("receipt", "receipt.txt"),
        )
    }
}

#[tokio::test]
async fn error_mail_notifications_and_message_options() {
    let app = App::new();
    Notification::send(&taylor(), PaymentFailed).await.unwrap();

    let message = &app.sent_mail()[0].message;
    assert_eq!(message.subject.as_deref(), Some("Payment Failed"));
    assert_eq!(message.from[0].name.as_deref(), Some("Billing"));
    assert!(message.has_cc("billing@example.com"));
    assert_eq!(message.attachments[0].filename, "receipt.txt");
    let html = message.html.as_deref().unwrap();
    assert!(html.contains(">Whoops!</h1>"));
    assert!(html.contains("class=\"button button-error\""));
    assert!(html.contains("The Billing Team"));
    assert!(!html.contains("Regards,"));
}

#[derive(Serialize)]
struct Welcome;

impl NotificationContract for Welcome {
    fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
        vec!["mail".into()]
    }

    fn to_mail(&self, notifiable: &dyn Notifiable) -> Option<MailMessage> {
        let name = notifiable.notifiable_attributes()["name"].clone();
        Some(
            MailMessage::new()
                .view("emails.welcome")
                .text("emails.welcome-text")
                .with_data(json!({"name": name})),
        )
    }
}

#[tokio::test]
async fn mail_notifications_can_use_custom_views() {
    let app = App::new();
    app.view(
        "emails.welcome",
        "<p>Welcome, {{ $name }}! ({{ $__laravel_notification_id }})</p>",
    );
    app.view("emails.welcome-text", "Welcome, {{ $name }}!");
    abigail().notify_now(Welcome).await.unwrap();
    let message = &app.sent_mail()[0].message;
    assert!(
        message
            .html
            .as_deref()
            .unwrap()
            .starts_with("<p>Welcome, Abigail! (")
    );
    assert_eq!(message.text.as_deref(), Some("Welcome, Abigail!"));
    assert_eq!(message.subject.as_deref(), Some("Welcome"));
}

#[derive(Serialize)]
struct ReceiptMailable {
    total: u32,
}

impl Mailable for ReceiptMailable {
    fn envelope(&self) -> Envelope {
        Envelope::new()
            .to("receipts@example.com")
            .subject("Receipt")
    }

    fn content(&self) -> Content {
        Content::html_string(format!("<p>Total: {}</p>", self.total))
    }
}

#[derive(Serialize)]
struct SendReceipt;

impl NotificationContract for SendReceipt {
    fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
        vec!["mail".into()]
    }

    fn to_mailable(&self, _notifiable: &dyn Notifiable) -> Option<Box<dyn Mailable>> {
        Some(Box::new(ReceiptMailable { total: 42 }))
    }
}

#[tokio::test]
async fn mail_notifications_can_send_mailables() {
    let app = App::new();
    taylor().notify(SendReceipt).await.unwrap();
    let message = &app.sent_mail()[0].message;
    assert!(message.has_to("receipts@example.com"));
    assert_eq!(message.html.as_deref(), Some("<p>Total: 42</p>"));
}

#[tokio::test]
async fn on_demand_notifications_use_their_routes() {
    let app = App::new();
    app.with_database().await;
    Notification::route("mail", json!({"taylor@example.com": "Taylor"}))
        .notify(InvoicePaid::new(&["mail", "database"]))
        .await
        .unwrap();
    let message = &app.sent_mail()[0].message;
    assert_eq!(message.to[0].name.as_deref(), Some("Taylor"));
    // The database channel is skipped for on-demand notifications.
    assert_eq!(DatabaseNotification::query().count().await.unwrap(), 0);

    // Notifiables without an email address are skipped by the mail channel.
    let nobody = User {
        id: 3,
        name: "Nobody".into(),
        email: String::new(),
        locale: None,
    };
    nobody.notify(InvoicePaid::new(&["mail"])).await.unwrap();
    assert_eq!(app.sent_mail().len(), 1);
}

#[tokio::test]
async fn database_notifications_are_stored_and_read_back() {
    let app = App::new();
    app.with_database().await;
    let taylor = taylor();
    let abigail = abigail();

    Notification::send(
        &vec![taylor.clone_user(), abigail.clone_user()],
        InvoicePaid::new(&["database"]),
    )
    .await
    .unwrap();
    taylor
        .notify(InvoicePaid::new(&["database"]))
        .await
        .unwrap();

    let notifications = taylor.notifications().await.unwrap();
    assert_eq!(notifications.len(), 2);
    let first = &notifications[0];
    assert_eq!(first.notifiable_type, "App\\Models\\User");
    assert!(first.notification_type.ends_with("InvoicePaid"));
    assert_eq!(first.data, json!({"invoice_id": 7, "amount": 99}));
    assert!(first.unread());
    assert_eq!(first.id.len(), 36);
    assert_eq!(abigail.notifications().await.unwrap().len(), 1);

    let mut unread = taylor.unread_notifications().await.unwrap();
    assert_eq!(unread.len(), 2);
    unread[0].mark_as_read().await.unwrap();
    assert!(unread[0].read());
    assert_eq!(taylor.unread_notifications().await.unwrap().len(), 1);
    assert_eq!(taylor.read_notifications().await.unwrap().len(), 1);

    let mut found = DatabaseNotification::find(&unread[0].id)
        .await
        .unwrap()
        .unwrap();
    assert!(found.read());
    found.mark_as_unread().await.unwrap();
    assert_eq!(taylor.unread_notifications().await.unwrap().len(), 2);

    assert_eq!(taylor.mark_notifications_as_read().await.unwrap(), 2);
    assert!(taylor.unread_notifications().await.unwrap().is_empty());
    let mut all = taylor.notifications().await.unwrap();
    DatabaseNotification::mark_all_as_unread(&mut all)
        .await
        .unwrap();
    assert_eq!(taylor.unread_notifications().await.unwrap().len(), 2);
    DatabaseNotification::mark_all_as_read(&mut all)
        .await
        .unwrap();
    assert!(all.iter().all(DatabaseNotification::read));

    all[0].delete().await.unwrap();
    assert_eq!(taylor.notifications().await.unwrap().len(), 1);
    assert!(
        DatabaseNotification::find("missing")
            .await
            .unwrap()
            .is_none()
    );
}

impl User {
    fn clone_user(&self) -> User {
        User {
            id: self.id,
            name: self.name.clone(),
            email: self.email.clone(),
            locale: self.locale.clone(),
        }
    }
}

#[tokio::test]
async fn notifications_can_be_faked() {
    let _app = App::new();
    let fake = Notification::fake();
    let (taylor, abigail) = (taylor(), abigail());

    taylor
        .notify(InvoicePaid::new(&["mail", "database"]))
        .await
        .unwrap();
    Notification::send(
        &vec![taylor.clone_user(), abigail.clone_user()],
        PaymentFailed,
    )
    .await
    .unwrap();
    Notification::route("mail", "james@example.com")
        .notify(Welcome)
        .await
        .unwrap();
    Notification::locale("es")
        .send(&abigail, Welcome)
        .await
        .unwrap();

    Notification::assert_sent_to::<InvoicePaid>(&taylor);
    Notification::assert_sent_to_with::<InvoicePaid>(&taylor, |notification, channels| {
        notification.invoice_id == 7 && channels == ["mail", "database"]
    });
    Notification::assert_not_sent_to::<InvoicePaid>(&abigail);
    Notification::assert_not_sent_to_with::<PaymentFailed>(&abigail, |_, channels| {
        channels.is_empty()
    });
    Notification::assert_sent_to_times::<PaymentFailed>(&taylor, 1);
    Notification::assert_sent_to_once::<PaymentFailed>(&abigail);
    Notification::assert_sent_times::<PaymentFailed>(2);
    Notification::assert_sent_on_demand::<Welcome>();
    Notification::assert_sent_on_demand_times::<Welcome>(1);
    Notification::assert_sent_on_demand_with::<Welcome>(|_, channels, notifiable| {
        channels == ["mail"] && notifiable.routes["mail"] == "james@example.com"
    });
    Notification::assert_count(5);
    assert!(Notification::has_sent::<Welcome>(&abigail));
    assert_eq!(
        Notification::sent::<Welcome>(&abigail)[0].locale(),
        Some("es")
    );
    assert_eq!(fake.count(), 5);

    let message = |f: fn()| {
        let error = std::panic::catch_unwind(f).unwrap_err();
        error
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| error.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap()
    };
    assert_eq!(
        message(Notification::assert_nothing_sent),
        "Notifications were sent unexpectedly."
    );
    assert_eq!(
        message(|| Notification::assert_count(1)),
        "Expected 1 notifications to be sent, but 5 were sent."
    );
    assert_eq!(
        message(|| Notification::assert_sent_times::<InvoicePaid>(3)),
        "Expected [InvoicePaid] to be sent 3 times, but was sent 1 time."
    );
}

#[tokio::test]
async fn faked_notifications_respect_should_send_and_isolation() {
    let _app = App::new();
    Notification::fake();
    Notification::assert_nothing_sent();
    let james = User {
        id: 9,
        name: "James".into(),
        email: "james@example.com".into(),
        locale: Some("fr".into()),
    };
    Notification::assert_nothing_sent_to(&james);
    james.notify(Skippable).await.unwrap();
    Notification::assert_nothing_sent();
    james.notify(InvoicePaid::new(&["mail"])).await.unwrap();
    assert_eq!(
        Notification::sent::<InvoicePaid>(&james)[0].locale(),
        Some("fr")
    );

    {
        let other = App::new();
        assert!(!Notification::is_fake());
        james.notify(InvoicePaid::new(&["mail"])).await.unwrap();
        assert_eq!(other.sent_mail().len(), 1);
    }
    Notification::assert_count(1);
}

#[derive(Serialize)]
struct Skippable;

impl NotificationContract for Skippable {
    fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
        vec!["mail".into()]
    }

    fn to_mail(&self, _notifiable: &dyn Notifiable) -> Option<MailMessage> {
        Some(MailMessage::new().line("skipped"))
    }

    fn should_send(&self, _notifiable: &dyn Notifiable, channel: &str) -> bool {
        channel != "mail"
    }
}

#[tokio::test]
#[should_panic(expected = "Notifications are not faked. Call Notification::fake() first.")]
async fn assertions_require_the_fake() {
    let _app = App::new();
    Notification::assert_nothing_sent();
}

struct SmsChannel {
    sent: Arc<Mutex<Vec<(Value, Value)>>>,
}

#[async_trait]
impl Channel for SmsChannel {
    async fn send(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn NotificationContract,
        _id: &str,
    ) -> Result<Value> {
        let route = notifiable
            .route_notification_for("sms", notification)
            .unwrap_or(Value::Null);
        let text = notification
            .to_channel("sms", notifiable)
            .unwrap_or_default();
        self.sent.lock().unwrap().push((route, text));
        Ok(json!("delivered"))
    }
}

struct BrokenChannel;

#[async_trait]
impl Channel for BrokenChannel {
    async fn send(
        &self,
        _: &dyn Notifiable,
        _: &dyn NotificationContract,
        _: &str,
    ) -> Result<Value> {
        Err(illuminate_support::error::RuntimeException::new("gateway timeout").into())
    }
}

#[tokio::test]
async fn custom_channels_events_and_failures() {
    let app = App::new();
    let sent = Arc::new(Mutex::new(Vec::new()));
    Notification::extend("sms", SmsChannel { sent: sent.clone() });
    Notification::extend("broken", BrokenChannel);
    assert!(Notification::channel(Some("pigeon")).is_err());

    let events = Arc::new(Mutex::new(Vec::new()));
    let recorder = events.clone();
    Event::listen(move |event: Arc<NotificationSent>| {
        let recorder = recorder.clone();
        async move {
            assert!(event.notification::<InvoicePaid>().is_some());
            recorder
                .lock()
                .unwrap()
                .push(format!("sent:{}:{}", event.channel, event.response));
            Ok(())
        }
    });
    let recorder = events.clone();
    Event::listen(move |event: Arc<NotificationFailed>| {
        let recorder = recorder.clone();
        async move {
            recorder
                .lock()
                .unwrap()
                .push(format!("failed:{}:{}", event.channel, event.error));
            Ok(())
        }
    });
    Event::listen(|event: Arc<NotificationSending>| async move {
        assert!(event.notification::<InvoicePaid>().is_some());
        event.channel != "mail"
    });

    Notification::route("sms", "+15555550100")
        .route("mail", "taylor@example.com")
        .notify(InvoicePaid::new(&["mail", "sms"]))
        .await
        .unwrap();
    assert!(
        app.sent_mail().is_empty(),
        "the mail channel was cancelled by the listener"
    );
    assert_eq!(
        *sent.lock().unwrap(),
        vec![(json!("+15555550100"), json!("Invoice 7 paid"))]
    );

    let error = Notification::send_now_via(&taylor(), InvoicePaid::new(&["mail"]), &["broken"])
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "gateway timeout");
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "sent:sms:\"delivered\"".to_string(),
            "failed:broken:gateway timeout".to_string()
        ]
    );
}

#[tokio::test]
async fn notifications_render_in_the_notifiables_locale() {
    let lang = tempfile::tempdir().unwrap();
    std::fs::write(
        lang.path().join("es.json"),
        r#"{"Hello!": "¡Hola!", "Regards,": "Saludos,"}"#,
    )
    .unwrap();
    let app = App::with_config(json!({
        "app": {"name": "Laravel", "url": "https://laravel.test", "locale": "en", "lang_path": lang.path()},
    }));
    illuminate_translation::TranslationServiceProvider.register(&app.container);
    Factory::resolve().blade().function("__", |args| {
        let key = args
            .first()
            .map(ViewValue::to_string_lossy)
            .unwrap_or_default();
        let replace = args.get(1).map(ViewValue::to_json).unwrap_or(Value::Null);
        Ok(ViewValue::from(illuminate_translation::__with(
            &key, replace,
        )))
    });

    let mut maria = taylor();
    maria.locale = Some("es".into());
    maria.notify(InvoicePaid::new(&["mail"])).await.unwrap();
    taylor().notify(InvoicePaid::new(&["mail"])).await.unwrap();

    let sent = app.sent_mail();
    let spanish = sent[0].message.text.as_deref().unwrap();
    assert!(spanish.contains("# ¡Hola!"), "{spanish}");
    assert!(spanish.contains("Saludos,"));
    let english = sent[1].message.text.as_deref().unwrap();
    assert!(english.contains("# Hello!"));
}

#[tokio::test]
async fn queued_notifications_are_pre_rendered_per_channel() {
    let app = App::new();
    app.with_database().await;
    let queued = Arc::new(Mutex::new(Vec::<QueuedNotification>::new()));
    let recorder = queued.clone();
    Notification::queue_using(move |notification: QueuedNotification| {
        let recorder = recorder.clone();
        async move {
            recorder.lock().unwrap().push(notification);
            Ok(())
        }
    });

    let mut notification = InvoicePaid::new(&["mail", "database"]);
    notification.queued = true;
    taylor().notify(notification).await.unwrap();

    assert!(app.sent_mail().is_empty());
    assert_eq!(DatabaseNotification::query().count().await.unwrap(), 0);
    let jobs = queued.lock().unwrap().clone();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].channel, "mail");
    assert_eq!(jobs[0].queue.as_deref(), Some("mail"));
    assert_eq!(jobs[1].channel, "database");
    assert_eq!(jobs[1].delay, Some(Duration::from_secs(30)));
    assert_eq!(jobs[0].id, jobs[1].id);
    assert_eq!(jobs[0].notifiable.key, json!(1));
    assert!(
        jobs[0].payload["message"]["html"]
            .as_str()
            .unwrap()
            .contains("One of your invoices has been paid!")
    );

    // The worker side: payloads survive serialization and are delivered.
    for job in jobs {
        let payload = serde_json::to_string(&job).unwrap();
        Notification::send_queued(serde_json::from_str(&payload).unwrap())
            .await
            .unwrap();
    }
    assert_eq!(app.sent_mail().len(), 1);
    assert!(app.sent_mail()[0].message.has_to("taylor@example.com"));
    let stored = taylor().notifications().await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].data["invoice_id"], 7);
}

#[tokio::test]
async fn queued_notifications_without_a_hook_are_sent_right_away() {
    let app = App::new();
    let mut notification = InvoicePaid::new(&["mail"]);
    notification.queued = true;
    taylor().notify(notification.clone()).await.unwrap();
    assert_eq!(app.sent_mail().len(), 1);

    // `notify_now` never queues.
    let hooked = Arc::new(Mutex::new(0));
    let counter = hooked.clone();
    Notification::queue_using(move |_| {
        let counter = counter.clone();
        async move {
            *counter.lock().unwrap() += 1;
            Ok(())
        }
    });
    taylor().notify_now(notification.clone()).await.unwrap();
    taylor()
        .notify_now_via(notification, &["mail"])
        .await
        .unwrap();
    assert_eq!(*hooked.lock().unwrap(), 0);
    assert_eq!(app.sent_mail().len(), 3);
}

#[tokio::test]
async fn queued_notifications_go_through_the_queue_component() {
    use illuminate_queue::{
        BusServiceProvider, Queue, QueueServiceProvider, Worker, WorkerOptions,
    };

    let views = tempfile::tempdir().unwrap();
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({
        "app": {"name": "Laravel"},
        "view": {"paths": [views.path()]},
        "mail": {"default": "array", "from": {"address": "hello@example.com"}},
        "queue": {"default": "array", "connections": {"array": {"driver": "array", "queue": "default"}}},
    })));
    QueueServiceProvider.register(&container);
    BusServiceProvider.register(&container);
    MailServiceProvider.register(&container);
    NotificationServiceProvider.register(&container);
    MailServiceProvider.boot(&container);
    NotificationServiceProvider.boot(&container);

    let mut notification = InvoicePaid::new(&["mail"]);
    notification.queued = true;
    taylor().notify(notification).await.unwrap();
    assert_eq!(Queue::size(Some("mail")).await.unwrap(), 1);

    let worker = Worker::make();
    worker
        .daemon("array", "mail", &WorkerOptions::new().stop_when_empty())
        .await
        .unwrap();
    assert_eq!(worker.jobs_processed(), 1);
    let mailer = container.make::<MailManager>().mailer(None).unwrap();
    let sent = downcast_transport::<ArrayTransport>(&mailer.transport())
        .unwrap()
        .messages();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].message.has_to("taylor@example.com"));
}

#[tokio::test]
async fn mail_notifications_render_for_previews() {
    let _app = App::new();
    let html = InvoicePaid::new(&["mail"])
        .to_mail(&taylor())
        .unwrap()
        .render()
        .unwrap();
    assert!(html.contains("One of your invoices has been paid!"));
    let anonymous = AnonymousNotifiable::new();
    assert!(anonymous.notifiable_key().is_null());
}
