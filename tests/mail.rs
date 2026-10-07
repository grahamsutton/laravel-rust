//! Mail and notifications, end to end: Eloquent users notified over mail
//! and the database, Markdown mail delivered to the array mailer, and the
//! fakes.

use laravel::facades::Notification as Notifications;
use laravel::mail::{ArrayTransport, MailManager, downcast_transport};
use laravel::notifications::HasDatabaseNotifications;
use laravel::prelude::*;
use laravel::testing::TestApp;

#[derive(Debug, Clone, Default, Model)]
#[fillable(name, email)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
}

impl Notifiable for User {
    fn notifiable_key(&self) -> Value {
        self.get_key()
    }

    fn notifiable_type(&self) -> String {
        "User".into()
    }
}

#[derive(Serialize)]
pub struct OrderShipped {
    pub order_id: u64,
}

impl Mailable for OrderShipped {
    fn envelope(&self) -> Envelope {
        Envelope::new()
            .from(Address::new("orders@laravel.com", "Laravel"))
            .subject("Order Shipped")
    }

    fn content(&self) -> Content {
        Content::markdown("mail.orders.shipped")
    }
}

#[derive(Clone, Serialize)]
pub struct InvoicePaid {
    pub invoice_id: u64,
}

impl Notification for InvoicePaid {
    fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
        vec!["mail".into(), "database".into()]
    }

    fn to_mail(&self, _notifiable: &dyn Notifiable) -> Option<MailMessage> {
        Some(
            MailMessage::new()
                .subject("Invoice Paid")
                .line("One of your invoices has been paid!")
                .action("View Invoice", format!("https://laravel.com/invoices/{}", self.invoice_id))
                .line("Thank you for using our application!"),
        )
    }

    fn to_array(&self, _notifiable: &dyn Notifiable) -> Option<Value> {
        Some(json!({"invoice_id": self.invoice_id}))
    }
}

struct CreateTables;

#[async_trait]
impl Migration for CreateTables {
    async fn up(&self) -> Result<()> {
        Schema::create("users", |table| {
            table.id();
            table.string("name");
            table.string("email");
        })
        .await?;
        laravel::notifications::CreateNotificationsTable.up().await
    }
}

async fn app() -> (TestApp, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap().keep();
    std::fs::create_dir_all(dir.join("resources/views/mail/orders")).unwrap();
    std::fs::write(
        dir.join("resources/views/mail/orders/shipped.blade.html"),
        "<x-mail::message>\n# Order Shipped\n\nYour order #{{ $order_id }} has shipped!\n\n<x-mail::button :url=\"'https://laravel.com/orders'\">\nView Order\n</x-mail::button>\n\nThanks,<br>\n{{ config('app.name') }}\n</x-mail::message>\n",
    )
    .unwrap();

    let mut app = TestApp::new(
        Application::configure_detached(&dir)
            .with_routing(|routing| {
                routing.web(|| {
                    Route::post("/orders/{id}/ship", |Path(id): Path<u64>| async move {
                        Mail::to("taylor@laravel.com").send(OrderShipped { order_id: id }).await?;
                        Ok::<_, Error>("Shipped")
                    });
                });
            })
            .with_migrations(laravel::database::migrations![
                "0001_01_01_000000_create_tables" => CreateTables,
            ]),
    );
    app.app().override_config("mail.default", "array");
    app.app().override_config("mail.mailers.array", json!({"transport": "array"}));
    app.refresh_database().await;
    (app, dir)
}

fn sent() -> Vec<laravel::mail::SentMessage> {
    let mailer = laravel::container::app::<MailManager>().mailer(Some("array")).unwrap();
    downcast_transport::<ArrayTransport>(&mailer.transport()).unwrap().messages()
}

#[tokio::test]
async fn markdown_mail_is_delivered() {
    let (mut app, _dir) = app().await;

    app.post("/orders/42/ship", json!({})).await.assert_ok().assert_see("Shipped");

    let sent = sent();
    assert_eq!(sent.len(), 1);
    let message = &sent[0].message;
    assert_eq!(message.subject.as_deref(), Some("Order Shipped"));
    assert_eq!(message.to[0].address, "taylor@laravel.com");
    let html = message.html.as_deref().unwrap();
    assert!(html.contains("Your order #42 has shipped!"));
    assert!(html.contains("https://laravel.com/orders"));
    assert!(html.contains("style=\""), "the theme's CSS is inlined");
    assert!(message.text.as_deref().unwrap().contains("Your order #42 has shipped!"));
}

#[tokio::test]
async fn mail_can_be_faked() {
    let (mut app, _dir) = app().await;
    Mail::fake();

    app.post("/orders/7/ship", json!({})).await.assert_ok();

    Mail::assert_sent_with::<OrderShipped>(|mail| mail.order_id == 7 && mail.has_to("taylor@laravel.com"));
    assert!(sent().is_empty());
}

#[tokio::test]
async fn users_are_notified_by_mail_and_in_the_database() {
    let (_app, _dir) = app().await;
    let taylor = User::create(json!({"name": "Taylor", "email": "taylor@laravel.com"})).await.unwrap();

    taylor.notify(InvoicePaid { invoice_id: 3 }).await.unwrap();

    let sent = sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].message.subject.as_deref(), Some("Invoice Paid"));
    assert!(sent[0].message.html.as_deref().unwrap().contains("One of your invoices has been paid!"));

    let notifications = taylor.unread_notifications().await.unwrap();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].data["invoice_id"], 3);
}

#[tokio::test]
async fn notifications_can_be_faked() {
    let (_app, _dir) = app().await;
    Notifications::fake();
    let taylor = User::create(json!({"name": "Taylor", "email": "taylor@laravel.com"})).await.unwrap();

    Notifications::send(&taylor, InvoicePaid { invoice_id: 9 }).await.unwrap();

    Notifications::assert_sent_to::<InvoicePaid>(&taylor);
    assert!(sent().is_empty());
}
