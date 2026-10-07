//! End-to-end tests: mailables sent through the `Mail` facade.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_events::Event;
use illuminate_mail::{
    Address, ArrayTransport, Attachment, Content, Envelope, Headers, Mail, MailManager,
    MailServiceProvider, Mailable, MailableBuilder, MessageSending, MessageSent, QueuedMessage,
    SentMessage, downcast_transport,
};
use illuminate_support::{Value, json};
use illuminate_view::{Factory, ViewValue};
use serde::Serialize;
use tempfile::TempDir;

struct App {
    container: Arc<Container>,
    views: TempDir,
    _guard: LocalInstanceGuard,
}

impl App {
    fn new(extra: Value) -> Self {
        let views = tempfile::tempdir().unwrap();
        let container = Arc::new(Container::new());
        let guard = Container::set_local_instance(container.clone());
        let mut config = json!({
            "app": {"name": "Laravel", "url": "https://laravel.test"},
            "view": {"paths": [views.path()]},
            "mail": {
                "default": "array",
                "mailers": {
                    "array": {"transport": "array"},
                    "second": {"transport": "array"},
                },
                "from": {"address": "hello@example.com", "name": "Example"},
            },
        });
        merge(&mut config, extra);
        container.instance(Repository::new(config));
        MailServiceProvider.register(&container);
        MailServiceProvider.boot(&container);
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

    fn sent(&self, mailer: &str) -> Vec<SentMessage> {
        let mailer = self
            .container
            .make::<MailManager>()
            .mailer(Some(mailer))
            .unwrap();
        downcast_transport::<ArrayTransport>(&mailer.transport())
            .unwrap()
            .messages()
    }
}

fn merge(target: &mut Value, extra: Value) {
    match (target, extra) {
        (Value::Object(target), Value::Object(extra)) => {
            for (key, value) in extra {
                merge(target.entry(key).or_insert(Value::Null), value);
            }
        }
        (target, extra) => *target = extra,
    }
}

#[derive(Serialize)]
struct Order {
    id: u64,
    total: String,
}

#[derive(Serialize)]
struct OrderShipped {
    order: Order,
}

impl Mailable for OrderShipped {
    fn envelope(&self) -> Envelope {
        Envelope::new()
            .subject(format!("Order #{} Shipped", self.order.id))
            .reply_to(Address::new("support@example.com", "Support"))
            .tag("shipment")
            .metadata("order_id", self.order.id)
    }

    fn content(&self) -> Content {
        Content::view("emails.orders.shipped")
            .with_text("emails.orders.shipped-text")
            .with(
                "url",
                format!("https://laravel.test/orders/{}", self.order.id),
            )
    }

    fn attachments(&self) -> Vec<Attachment> {
        vec![Attachment::from_data(|| "id,total\n1,10.00", "order.csv")]
    }

    fn headers(&self) -> Headers {
        Headers::new().text([("X-Order", self.order.id.to_string())])
    }
}

fn order_shipped() -> OrderShipped {
    OrderShipped {
        order: Order {
            id: 1,
            total: "10.00".into(),
        },
    }
}

fn order_views(app: &App) {
    app.view(
        "emails.orders.shipped",
        "<h1>Order {{ $order->id }} shipped</h1><p>Total: ${{ $order->total }}</p><a href=\"{{ $url }}\">View</a><small>{{ $mailer }}</small>",
    );
    app.view(
        "emails.orders.shipped-text",
        "Order {{ $order->id }} shipped: {{ $url }}",
    );
}

#[tokio::test]
async fn mailables_are_rendered_and_delivered_through_the_array_transport() {
    let app = App::new(json!({}));
    order_views(&app);

    let sent = Mail::to(("taylor@example.com", "Taylor"))
        .cc("abigail@example.com")
        .bcc(["james@example.com"])
        .send(order_shipped())
        .await
        .unwrap()
        .expect("the message was sent");
    assert!(!sent.message_id().is_empty());

    let messages = app.sent("array");
    assert_eq!(messages.len(), 1);
    let message = &messages[0].message;
    assert_eq!(
        message.from,
        vec![Address::new("hello@example.com", "Example")]
    );
    assert_eq!(
        message.to,
        vec![Address::new("taylor@example.com", "Taylor")]
    );
    assert!(message.has_cc("abigail@example.com"));
    assert!(message.has_bcc("james@example.com"));
    assert!(message.has_reply_to("support@example.com"));
    assert_eq!(message.subject.as_deref(), Some("Order #1 Shipped"));
    assert_eq!(
        message.html.as_deref(),
        Some(
            "<h1>Order 1 shipped</h1><p>Total: $10.00</p><a href=\"https://laravel.test/orders/1\">View</a><small>array</small>"
        )
    );
    assert_eq!(
        message.text.as_deref(),
        Some("Order 1 shipped: https://laravel.test/orders/1")
    );
    assert_eq!(message.tags, vec!["shipment".to_string()]);
    assert_eq!(message.metadata["order_id"], "1");
    assert_eq!(message.get_header("X-Order"), Some("1"));
    assert_eq!(message.attachments.len(), 1);
    assert_eq!(message.attachments[0].filename, "order.csv");
    assert_eq!(message.attachments[0].text(), "id,total\n1,10.00");

    let mime = messages[0].to_mime_string().unwrap();
    assert!(mime.contains("Subject: Order #1 Shipped"));
    assert!(mime.contains("X-Tag: shipment"));
    assert!(mime.contains("filename=\"order.csv\""));
}

#[tokio::test]
async fn mailers_can_be_chosen_and_global_addresses_applied() {
    let app = App::new(
        json!({"mail": {"mailers": {"second": {"transport": "array", "to": {"address": "dev@example.com", "name": "Dev"}}}}}),
    );
    order_views(&app);

    Mail::mailer("second")
        .unwrap()
        .to("taylor@example.com")
        .cc("abigail@example.com")
        .send(order_shipped())
        .await
        .unwrap();
    assert!(app.sent("array").is_empty());
    let message = &app.sent("second")[0].message;
    // The global "to" address replaces every recipient.
    assert_eq!(message.to, vec![Address::new("dev@example.com", "Dev")]);
    assert!(message.cc.is_empty());

    Mail::always_from(("noreply@example.com", "No Reply")).unwrap();
    Mail::always_reply_to("replies@example.com").unwrap();
    Mail::always_return_path("bounces@example.com").unwrap();
    Mail::raw("Plain text", |message| {
        message.to("taylor@example.com").subject("Raw");
    })
    .await
    .unwrap();
    let message = &app.sent("array")[0].message;
    assert_eq!(message.from[0].address, "noreply@example.com");
    assert_eq!(message.reply_to[0].address, "replies@example.com");
    assert_eq!(
        message.return_path.as_ref().unwrap().address,
        "bounces@example.com"
    );
    assert_eq!(message.text.as_deref(), Some("Plain text"));
    assert_eq!(message.html, None);

    Mail::always_to("everyone@example.com").unwrap();
    Mail::html("<p>Hi</p>", |message| {
        message.to("taylor@example.com");
    })
    .await
    .unwrap();
    let message = &app.sent("array")[1].message;
    assert_eq!(message.to, vec![Address::email("everyone@example.com")]);
    assert_eq!(message.html.as_deref(), Some("<p>Hi</p>"));
}

#[tokio::test]
async fn views_can_be_sent_without_mailables() {
    let app = App::new(json!({}));
    app.view(
        "emails.welcome",
        "<p>Welcome, {{ $name }}! ({{ $mailer }})</p>",
    );
    app.view("emails.welcome-text", "Welcome, {{ $name }}!");

    Mail::send_view("emails.welcome", json!({"name": "Taylor"}), |message| {
        message.to("taylor@example.com").subject("Welcome");
    })
    .await
    .unwrap();
    Mail::plain(
        "emails.welcome-text",
        json!({"name": "Abigail"}),
        |message| {
            message.to("abigail@example.com");
        },
    )
    .await
    .unwrap();
    Mail::send_view(
        illuminate_mail::MailView::Both {
            html: "emails.welcome".into(),
            text: "emails.welcome-text".into(),
        },
        json!({"name": "James"}),
        |message| {
            message.to("james@example.com");
        },
    )
    .await
    .unwrap();

    let sent = app.sent("array");
    assert_eq!(
        sent[0].message.html.as_deref(),
        Some("<p>Welcome, Taylor! (array)</p>")
    );
    assert_eq!(sent[1].message.text.as_deref(), Some("Welcome, Abigail!"));
    assert_eq!(sent[1].message.html, None);
    assert_eq!(
        sent[2].message.html.as_deref(),
        Some("<p>Welcome, James! (array)</p>")
    );
    assert_eq!(sent[2].message.text.as_deref(), Some("Welcome, James!"));
}

#[derive(Serialize)]
struct InvoicePaid {
    amount: u32,
}

impl Mailable for InvoicePaid {
    fn envelope(&self) -> Envelope {
        Envelope::new()
            .to("billing@example.com")
            .subject("Invoice Paid")
    }

    fn content(&self) -> Content {
        Content::markdown("mail.invoice.paid")
    }
}

#[tokio::test]
async fn markdown_mailables_have_styled_html_and_text_parts() {
    let app = App::new(json!({}));
    app.view(
        "mail.invoice.paid",
        "<x-mail::message>\n# Invoice Paid\n\nWe received **${{ $amount }}**.\n\n<x-mail::button :url=\"config('app.url')\" color=\"success\">\nView Invoice\n</x-mail::button>\n\nThanks,<br>\n{{ config('app.name') }}\n</x-mail::message>",
    );

    Mail::send(InvoicePaid { amount: 99 }).await.unwrap();
    let message = &app.sent("array")[0].message;
    assert!(message.has_to("billing@example.com"));
    let html = message.html.as_deref().unwrap();
    assert!(html.contains("<title>Laravel</title>"));
    assert!(html.contains(">Invoice Paid</h1>"));
    assert!(html.contains(">$99</strong>"));
    assert!(html.contains("href=\"https://laravel.test\" class=\"button button-success\""));
    assert!(html.contains("background-color: #16a34a;"));
    assert!(html.contains("Laravel. All rights reserved."));
    let text = message.text.as_deref().unwrap();
    assert!(text.starts_with("Laravel: https://laravel.test"));
    assert!(text.contains("We received **$99**."));
    assert!(text.contains("View Invoice: https://laravel.test"));
}

#[derive(Serialize)]
struct Greeting;

impl Mailable for Greeting {
    fn build(&self, mail: &mut MailableBuilder) {
        mail.to("taylor@example.com").view("emails.greeting");
    }
}

#[tokio::test]
async fn mailables_render_in_their_locale() {
    let app = App::new(json!({"app": {"locale": "en", "lang_path": app_lang_path()}}));
    illuminate_translation::TranslationServiceProvider.register(&app.container);
    Factory::resolve().blade().function("__", |args| {
        Ok(ViewValue::from(illuminate_translation::__(
            &args
                .first()
                .map(ViewValue::to_string_lossy)
                .unwrap_or_default(),
        )))
    });
    app.view("emails.greeting", "{{ __('Hello') }}");

    Mail::to("taylor@example.com").send(Greeting).await.unwrap();
    Mail::to("taylor@example.com")
        .locale("es")
        .send(Greeting)
        .await
        .unwrap();

    let sent = app.sent("array");
    assert_eq!(sent[0].message.html.as_deref(), Some("Hello"));
    assert_eq!(sent[1].message.html.as_deref(), Some("Hola"));
    // The subject defaults to the mailable's name.
    assert_eq!(sent[0].message.subject.as_deref(), Some("Greeting"));
}

fn app_lang_path() -> String {
    let dir = std::env::temp_dir().join(format!("illuminate-mail-lang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("es.json"), r#"{"Hello": "Hola"}"#).unwrap();
    dir.display().to_string()
}

#[tokio::test]
async fn message_sending_listeners_can_cancel_messages() {
    let app = App::new(json!({}));
    order_views(&app);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = seen.clone();
    Event::listen(move |event: Arc<MessageSent>| {
        let recorder = recorder.clone();
        async move {
            recorder
                .lock()
                .unwrap()
                .push(event.message().to[0].address.clone());
            Ok(())
        }
    });
    Event::listen(|event: Arc<MessageSending>| async move {
        assert_eq!(event.mailer, "array");
        !event.message.has_to("blocked@example.com")
    });

    let sent = Mail::to("blocked@example.com")
        .send(order_shipped())
        .await
        .unwrap();
    assert!(sent.is_none());
    Mail::to("taylor@example.com")
        .send(order_shipped())
        .await
        .unwrap();

    assert_eq!(app.sent("array").len(), 1);
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["taylor@example.com".to_string()]
    );
}

#[derive(Serialize)]
struct WeeklyDigest;

impl Mailable for WeeklyDigest {
    fn envelope(&self) -> Envelope {
        Envelope::new().subject("Your Weekly Digest")
    }

    fn content(&self) -> Content {
        Content::html_string("<p>Digest</p>")
    }

    fn should_queue(&self) -> bool {
        true
    }

    fn queue_name(&self) -> Option<String> {
        Some("emails".into())
    }
}

#[tokio::test]
async fn queued_mail_is_pre_rendered_and_handed_to_the_queue_hook() {
    let app = App::new(json!({}));
    let queued = Arc::new(Mutex::new(Vec::<QueuedMessage>::new()));

    // Without a hook, queued mail is sent right away (like the sync driver).
    Mail::to("taylor@example.com")
        .send(WeeklyDigest)
        .await
        .unwrap();
    assert_eq!(app.sent("array").len(), 1);

    let recorder = queued.clone();
    Mail::queue_using(move |message: QueuedMessage| {
        let recorder = recorder.clone();
        async move {
            recorder.lock().unwrap().push(message);
            Ok(())
        }
    });

    Mail::to("taylor@example.com")
        .send(WeeklyDigest)
        .await
        .unwrap();
    Mail::to("abigail@example.com")
        .later(Duration::from_secs(60), WeeklyDigest)
        .await
        .unwrap();
    assert_eq!(app.sent("array").len(), 1);

    let messages = queued.lock().unwrap().clone();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].mailer, "array");
    assert_eq!(messages[0].mailable, "WeeklyDigest");
    assert_eq!(messages[0].queue.as_deref(), Some("emails"));
    assert_eq!(messages[0].message.html.as_deref(), Some("<p>Digest</p>"));
    assert_eq!(messages[1].delay, Some(Duration::from_secs(60)));

    // The worker side: the payload survives serialization and is delivered.
    let payload = serde_json::to_string(&messages[1]).unwrap();
    Mail::send_queued(serde_json::from_str(&payload).unwrap())
        .await
        .unwrap();
    let sent = app.sent("array");
    assert_eq!(sent.len(), 2);
    assert!(sent[1].message.has_to("abigail@example.com"));
    assert_eq!(
        sent[1].message.subject.as_deref(),
        Some("Your Weekly Digest")
    );
}

#[tokio::test]
async fn queued_mail_goes_through_the_queue_component() {
    use illuminate_queue::{
        BusServiceProvider, Queue, QueueServiceProvider, Worker, WorkerOptions,
    };

    let views = tempfile::tempdir().unwrap();
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({
        "view": {"paths": [views.path()]},
        "mail": {"default": "array", "from": {"address": "hello@example.com", "name": null}},
        "queue": {"default": "array", "connections": {"array": {"driver": "array", "queue": "default"}}},
    })));
    QueueServiceProvider.register(&container);
    BusServiceProvider.register(&container);
    MailServiceProvider.register(&container);
    MailServiceProvider.boot(&container);

    Mail::to("taylor@example.com")
        .queue(WeeklyDigest)
        .await
        .unwrap();
    assert_eq!(Queue::size(Some("emails")).await.unwrap(), 1);
    let transport = container
        .make::<MailManager>()
        .mailer(None)
        .unwrap()
        .transport();
    let transport = downcast_transport::<ArrayTransport>(&transport).unwrap();
    assert!(transport.messages().is_empty());

    let worker = Worker::make();
    worker
        .daemon("array", "emails", &WorkerOptions::new().stop_when_empty())
        .await
        .unwrap();
    assert_eq!(worker.jobs_processed(), 1);
    let sent = transport.messages();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].message.has_to("taylor@example.com"));
    assert_eq!(sent[0].message.html.as_deref(), Some("<p>Digest</p>"));
}

#[tokio::test]
async fn mail_can_be_faked_per_container() {
    let app = App::new(json!({}));
    let fake = Mail::fake();
    assert!(Mail::is_fake());

    Mail::to(("taylor@example.com", "Taylor"))
        .cc("abigail@example.com")
        .send(order_shipped())
        .await
        .unwrap();
    Mail::mailer("postmark")
        .unwrap()
        .to("james@example.com")
        .send(order_shipped())
        .await
        .unwrap();
    Mail::to("taylor@example.com")
        .send(WeeklyDigest)
        .await
        .unwrap();
    Mail::raw("ignored", |message| {
        message.to("x@example.com");
    })
    .await
    .unwrap();

    Mail::assert_sent::<OrderShipped>();
    Mail::assert_sent_times::<OrderShipped>(2);
    Mail::assert_sent_to::<OrderShipped>("taylor@example.com");
    Mail::assert_sent_with::<OrderShipped>(|mail| {
        mail.order.id == 1
            && mail.has_cc("abigail@example.com")
            && mail.has_subject("Order #1 Shipped")
    });
    Mail::assert_sent_with::<OrderShipped>(|mail| {
        mail.uses_mailer("postmark") && mail.has_to("james@example.com")
    });
    Mail::assert_not_sent_with::<OrderShipped>(|mail| mail.has_to("nobody@example.com"));
    Mail::assert_not_sent_to::<OrderShipped>("nobody@example.com");
    Mail::assert_not_sent::<InvoicePaid>();
    Mail::assert_sent_count(2);
    Mail::assert_queued::<WeeklyDigest>();
    Mail::assert_queued_once::<WeeklyDigest>();
    Mail::assert_queued_to::<WeeklyDigest>("taylor@example.com");
    Mail::assert_queued_with::<WeeklyDigest>(|mail| mail.has_subject("Your Weekly Digest"));
    Mail::assert_not_queued::<OrderShipped>();
    Mail::assert_queued_count(1);
    Mail::assert_outgoing_count(3);
    Mail::assert_not_outgoing::<InvoicePaid>();
    assert_eq!(Mail::sent::<OrderShipped>(|_| true).len(), 2);
    assert!(Mail::has_queued::<WeeklyDigest>());
    assert!(fake.has_sent::<OrderShipped>());
    assert!(app.sent("array").is_empty(), "nothing is really sent");

    // Another container isn't faked.
    {
        let other = App::new(json!({}));
        order_views(&other);
        assert!(!Mail::is_fake());
        Mail::to("taylor@example.com")
            .send(order_shipped())
            .await
            .unwrap();
        assert_eq!(other.sent("array").len(), 1);
    }
    assert!(Mail::is_fake());
}

#[tokio::test]
async fn fake_assertions_explain_failures() {
    let _app = App::new(json!({}));
    Mail::fake();
    Mail::assert_nothing_sent();
    Mail::assert_nothing_queued();
    Mail::assert_nothing_outgoing();
    Mail::to("taylor@example.com")
        .send(WeeklyDigest)
        .await
        .unwrap();

    let message = |f: fn()| {
        let error = std::panic::catch_unwind(f).unwrap_err();
        error
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| error.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap()
    };
    assert_eq!(
        message(Mail::assert_sent::<WeeklyDigest>),
        "The expected [WeeklyDigest] mailable was not sent. Did you mean to use assert_queued() instead?"
    );
    assert_eq!(
        message(|| Mail::assert_queued_times::<WeeklyDigest>(2)),
        "The expected [WeeklyDigest] mailable was queued 1 time instead of 2 times."
    );
    assert_eq!(
        message(Mail::assert_nothing_queued),
        "The following mailables were queued unexpectedly:\n\n- WeeklyDigest\n"
    );
    assert_eq!(
        message(|| Mail::assert_queued_count(3)),
        "The total number of mailables queued was 1 instead of 3."
    );
    assert_eq!(
        message(Mail::assert_not_queued::<WeeklyDigest>),
        "The unexpected [WeeklyDigest] mailable was queued."
    );
}

#[tokio::test]
#[should_panic(expected = "Mail is not faked. Call Mail::fake() first.")]
async fn assertions_require_the_fake() {
    let _app = App::new(json!({}));
    Mail::assert_nothing_sent();
}

#[derive(Serialize)]
struct WithFiles {
    logo: String,
}

impl Mailable for WithFiles {
    fn envelope(&self) -> Envelope {
        Envelope::new().to("taylor@example.com").using(|message| {
            message.priority(1);
        })
    }

    fn content(&self) -> Content {
        Content::view("emails.files")
    }

    fn attachments(&self) -> Vec<Attachment> {
        vec![
            Attachment::from_storage("reports/weekly.pdf"),
            Attachment::from_storage_disk("public", "photos/1.jpg").as_("photo.jpg"),
        ]
    }
}

#[tokio::test]
async fn storage_attachments_and_embedded_images() {
    let disk = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(disk.path().join("reports")).unwrap();
    std::fs::create_dir_all(disk.path().join("photos")).unwrap();
    std::fs::write(disk.path().join("reports/weekly.pdf"), "%PDF-1.4").unwrap();
    std::fs::write(disk.path().join("photos/1.jpg"), [0xFF, 0xD8, 0xFF]).unwrap();
    let logo = disk.path().join("logo.png");
    std::fs::write(&logo, [0x89, b'P', b'N', b'G']).unwrap();

    let app = App::new(json!({
        "filesystems": {
            "default": "local",
            "disks": {
                "local": {"driver": "local", "root": disk.path()},
                "public": {"driver": "local", "root": disk.path()},
            },
        },
    }));
    app.view("emails.files", "<img src=\"{{ $message->embed($logo) }}\">");

    let mailable = WithFiles {
        logo: logo.display().to_string(),
    };
    let preview = mailable.render().unwrap();
    assert!(preview.starts_with("<img src=\"data:image/png;base64,"));

    Mail::send(mailable).await.unwrap();
    let message = &app.sent("array")[0].message;
    assert_eq!(message.priority, Some(1));
    let html = message.html.as_deref().unwrap();
    let cid = html
        .trim_start_matches("<img src=\"cid:")
        .trim_end_matches("\">");
    let names: Vec<&str> = message
        .attachments
        .iter()
        .map(|a| a.filename.as_str())
        .collect();
    assert_eq!(names, vec!["logo.png", "weekly.pdf", "photo.jpg"]);
    assert_eq!(message.attachments[0].content_id.as_deref(), Some(cid));
    assert_eq!(message.attachments[1].content_type, "application/pdf");
    assert_eq!(message.attachments[1].text(), "%PDF-1.4");
    assert_eq!(message.attachments[2].content_type, "image/jpeg");
}

#[tokio::test]
async fn the_log_transport_writes_messages_to_the_log() {
    let logs = tempfile::tempdir().unwrap();
    let log_file = logs.path().join("mail.log");
    let app = App::new(json!({
        "mail": {"default": "log", "mailers": {"log": {"transport": "log", "channel": "mail"}}},
        "logging": {
            "default": "mail",
            "channels": {"mail": {"driver": "single", "path": log_file, "level": "debug"}},
        },
    }));
    order_views(&app);

    Mail::to("taylor@example.com")
        .send(order_shipped())
        .await
        .unwrap();

    let logged = std::fs::read_to_string(&log_file).unwrap();
    assert!(logged.contains("DEBUG"), "{logged}");
    assert!(logged.contains("Subject: Order #1 Shipped"));
    assert!(logged.contains("To: taylor@example.com"));
    assert!(logged.contains("Order 1 shipped"));
}

#[tokio::test]
async fn custom_transports_can_be_registered() {
    let app = App::new(json!({"mail": {"mailers": {"custom": {"transport": "memory"}}}}));
    let shared = Arc::new(ArrayTransport::new());
    let transport = shared.clone();
    Mail::extend("memory", move |config| {
        assert_eq!(config["transport"], "memory");
        Ok(transport.clone() as Arc<dyn illuminate_mail::Transport>)
    });
    order_views(&app);
    Mail::mailer("custom")
        .unwrap()
        .to("taylor@example.com")
        .send(order_shipped())
        .await
        .unwrap();
    assert_eq!(shared.messages().len(), 1);
    assert!(Mail::mailer("undefined").is_err());
    let on_demand = Mail::build(json!({"transport": "array"})).unwrap();
    assert_eq!(on_demand.name(), "ondemand");
}

#[tokio::test]
async fn rendering_errors_are_reported() {
    let _app = App::new(json!({}));
    let error = Mail::to("taylor@example.com")
        .send(order_shipped())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("emails.orders.shipped"),
        "{error}"
    );
    let error = Mail::raw("x", |_| {}).await.unwrap_err();
    assert!(error.to_string().contains("\"To\""), "{error}");
}
