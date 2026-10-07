//! End-to-end tests: mailables sent through the HTTP API transports, with
//! the `Http` facade faked.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_http_client::{Http, Request};
use illuminate_mail::{
    Address, Attachment, Content, Envelope, Headers, Mail, MailManager, MailServiceProvider,
    Mailable, SentMessage, TransportException,
};
use illuminate_support::{Value, json};
use serde::Serialize;
use tempfile::TempDir;

const PNG: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10];

struct App {
    container: Arc<Container>,
    _views: TempDir,
    _guard: LocalInstanceGuard,
}

impl App {
    /// Boot the mail component with every API mailer configured the way the
    /// Laravel skeleton does: mailers in `mail.mailers`, credentials in
    /// `services`.
    fn new() -> Self {
        let views = tempfile::tempdir().unwrap();
        let write = |name: &str, contents: &str| {
            let path = views.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        };
        write(
            "emails/orders/shipped.blade.html",
            "<p>Order #{{ $id }} shipped!</p><img src=\"{{ $message->embed($logo) }}\">",
        );
        write(
            "emails/orders/shipped-text.blade.html",
            "Order #{{ $id }} shipped!",
        );
        std::fs::write(views.path().join("logo.png"), PNG).unwrap();

        let container = Arc::new(Container::new());
        let guard = Container::set_local_instance(container.clone());
        container.instance(Repository::new(json!({
            "app": {"name": "Laravel", "url": "https://laravel.test"},
            "view": {"paths": [views.path()]},
            "mail": {
                "default": "log",
                "mailers": {
                    "postmark": {"transport": "postmark", "message_stream_id": "outbound"},
                    "resend": {"transport": "resend"},
                    "mailgun": {"transport": "mailgun"},
                    "ses": {"transport": "ses", "options": {"ConfigurationSetName": "Orders"}},
                    "ses-v2": {"transport": "ses-v2"},
                },
                "from": {"address": "hello@example.com", "name": "Example"},
            },
            "services": {
                "postmark": {"key": "postmark-token"},
                "resend": {"key": "re_123"},
                "mailgun": {"domain": "mg.example.com", "secret": "key-secret", "endpoint": "api.mailgun.net", "scheme": "https"},
                "ses": {"key": "AKIDEXAMPLE", "secret": "secret", "region": "eu-west-1", "token": "session-token"},
            },
        })));
        MailServiceProvider.register(&container);
        MailServiceProvider.boot(&container);
        Self {
            container,
            _views: views,
            _guard: guard,
        }
    }

    /// Make the given mailer the default one.
    fn using(self, mailer: &str) -> Self {
        self.container
            .make::<MailManager>()
            .set_default_driver(mailer);
        self
    }

    fn logo(&self) -> String {
        self._views.path().join("logo.png").display().to_string()
    }
}

#[derive(Serialize)]
struct OrderShipped {
    id: u64,
    logo: String,
}

impl Mailable for OrderShipped {
    fn envelope(&self) -> Envelope {
        Envelope::new()
            .subject(format!("Order #{} Shipped", self.id))
            .reply_to(Address::new("support@example.com", "Support"))
            .tag("shipment")
            .metadata("order_id", self.id)
    }

    fn content(&self) -> Content {
        Content::view("emails.orders.shipped").with_text("emails.orders.shipped-text")
    }

    fn attachments(&self) -> Vec<Attachment> {
        vec![Attachment::from_data(|| "id,total\n1,10.00", "order.csv")]
    }

    fn headers(&self) -> Headers {
        Headers::new().text([("X-Order", self.id.to_string())])
    }
}

/// Send the order shipped mail through the default mailer.
async fn send_order_shipped(app: &App) -> illuminate_support::Result<SentMessage> {
    let sent = Mail::to(("taylor@example.com", "Taylor Otwell"))
        .cc(["abigail@example.com"])
        .bcc(["secret@example.com"])
        .send(OrderShipped {
            id: 1,
            logo: app.logo(),
        })
        .await?;
    Ok(sent.expect("the message was sent"))
}

fn sent_request() -> Request {
    let recorded = Http::recorded();
    assert_eq!(recorded.all().len(), 1, "Expected exactly one request.");
    recorded.all()[0].0.clone()
}

fn decode(data: &Value) -> Vec<u8> {
    STANDARD.decode(data.as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn mailables_are_sent_through_postmark() {
    let app = App::new().using("postmark");
    Http::fake_urls([(
        "api.postmarkapp.com/*",
        Http::response(
            json!({"MessageID": "pm-123", "ErrorCode": 0, "Message": "OK"}),
            200,
            &[],
        ),
    )]);

    let sent = send_order_shipped(&app).await.unwrap();

    assert_eq!(sent.message_id(), "pm-123");
    let request = sent_request();
    assert_eq!(request.url(), "https://api.postmarkapp.com/email");
    assert!(request.has_header_value("X-Postmark-Server-Token", "postmark-token"));
    assert_eq!(request["From"], "\"Example\" <hello@example.com>");
    assert_eq!(request["To"], "\"Taylor Otwell\" <taylor@example.com>");
    assert_eq!(request["Cc"], "abigail@example.com");
    assert_eq!(request["Bcc"], "secret@example.com");
    assert_eq!(request["ReplyTo"], "\"Support\" <support@example.com>");
    assert_eq!(request["Subject"], "Order #1 Shipped");
    assert_eq!(request["TextBody"], "Order #1 shipped!");
    assert_eq!(request["Tag"], "shipment");
    assert_eq!(request["Metadata"], json!({"order_id": "1"}));
    assert_eq!(request["MessageStream"], "outbound");
    assert_eq!(
        request["Headers"],
        json!([{"Name": "X-Order", "Value": "1"}])
    );

    let attachments = request["Attachments"].as_array().unwrap();
    assert_eq!(attachments.len(), 2);
    let csv = attachments
        .iter()
        .find(|a| a["Name"] == "order.csv")
        .unwrap();
    assert_eq!(decode(&csv["Content"]), b"id,total\n1,10.00");
    assert!(csv.get("ContentID").is_none());
    let logo = attachments
        .iter()
        .find(|a| a["Name"] == "logo.png")
        .unwrap();
    assert_eq!(decode(&logo["Content"]), PNG);
    assert_eq!(logo["ContentType"], "image/png");
    // The inline image is referenced by the HTML body.
    let cid = logo["ContentID"].as_str().unwrap();
    assert!(cid.starts_with("cid:"));
    assert!(request["HtmlBody"].as_str().unwrap().contains(cid));
}

#[tokio::test]
async fn mailables_are_sent_through_resend() {
    let app = App::new().using("resend");
    Http::fake_urls([(
        "api.resend.com/*",
        Http::response(json!({"id": "re-123"}), 200, &[]),
    )]);

    let sent = send_order_shipped(&app).await.unwrap();

    assert_eq!(sent.message_id(), "re-123");
    assert_eq!(
        sent.original_message().get_header("X-Resend-Email-ID"),
        Some("re-123")
    );
    let request = sent_request();
    assert_eq!(request.url(), "https://api.resend.com/emails");
    assert!(request.has_header_value("Authorization", "Bearer re_123"));
    assert_eq!(request["from"], "\"Example\" <hello@example.com>");
    assert_eq!(
        request["to"],
        json!(["\"Taylor Otwell\" <taylor@example.com>"])
    );
    assert_eq!(request["cc"], json!(["abigail@example.com"]));
    assert_eq!(request["bcc"], json!(["secret@example.com"]));
    assert_eq!(
        request["reply_to"],
        json!(["\"Support\" <support@example.com>"])
    );
    assert_eq!(request["subject"], "Order #1 Shipped");
    assert_eq!(request["text"], "Order #1 shipped!");
    assert_eq!(request["headers"], json!({"X-Order": "1"}));
    assert_eq!(
        request["tags"],
        json!([{"name": "shipment", "value": "true"}, {"name": "order_id", "value": "1"}])
    );

    let attachments = request["attachments"].as_array().unwrap();
    let csv = attachments
        .iter()
        .find(|a| a["filename"] == "order.csv")
        .unwrap();
    assert_eq!(csv["content_type"], "text/csv");
    assert_eq!(decode(&csv["content"]), b"id,total\n1,10.00");
    let logo = attachments
        .iter()
        .find(|a| a["filename"] == "logo.png")
        .unwrap();
    let cid = logo["content_id"].as_str().unwrap();
    assert!(
        request["html"]
            .as_str()
            .unwrap()
            .contains(&format!("cid:{cid}"))
    );
}

#[tokio::test]
async fn mailables_are_sent_through_mailgun() {
    let app = App::new().using("mailgun");
    Http::fake_urls([(
        "api.mailgun.net/*",
        Http::response(
            json!({"id": "<mg-123@mg.example.com>", "message": "Queued. Thank you."}),
            200,
            &[],
        ),
    )]);

    let sent = send_order_shipped(&app).await.unwrap();

    assert_eq!(sent.message_id(), "mg-123@mg.example.com");
    let request = sent_request();
    assert_eq!(
        request.url(),
        "https://api.mailgun.net/v3/mg.example.com/messages.mime"
    );
    assert!(request.has_header_value(
        "Authorization",
        &format!("Basic {}", STANDARD.encode("api:key-secret"))
    ));
    assert!(request.has_file_with(
        "to",
        Some("taylor@example.com,abigail@example.com,secret@example.com"),
        None
    ));
    assert!(request.has_file_with("o:tag", Some("shipment"), None));
    assert!(request.has_file_with("v:order_id", Some("1"), None));

    let mime = request
        .parts()
        .iter()
        .find(|part| part.name == "message")
        .unwrap();
    assert_eq!(mime.filename.as_deref(), Some("message.mime"));
    let mime = String::from_utf8_lossy(&mime.contents);
    assert!(mime.contains("Subject: Order #1 Shipped"));
    assert!(mime.contains("Reply-To: Support <support@example.com>"));
    assert!(mime.contains("Cc: abigail@example.com"));
    assert!(mime.contains("X-Order: 1"));
    assert!(mime.contains("filename=\"order.csv\""));
    assert!(mime.contains("Content-ID: <"));
    assert!(!mime.contains("secret@example.com"));
}

#[tokio::test]
async fn mailables_are_sent_through_ses() {
    for (mailer, configuration_set) in [("ses", Some("Orders")), ("ses-v2", None)] {
        let app = App::new().using(mailer);
        Http::fake_urls([(
            "email.eu-west-1.amazonaws.com/*",
            Http::response(json!({"MessageId": format!("{mailer}-123")}), 200, &[]),
        )]);

        let sent = send_order_shipped(&app).await.unwrap();

        assert_eq!(sent.message_id(), format!("{mailer}-123"));
        assert_eq!(
            sent.original_message().get_header("X-SES-Message-ID"),
            Some(format!("{mailer}-123").as_str())
        );
        let request = sent_request();
        assert_eq!(
            request.url(),
            "https://email.eu-west-1.amazonaws.com/v2/email/outbound-emails"
        );
        assert!(request.has_header_value("X-Amz-Security-Token", "session-token"));
        let authorization = &request.header("Authorization")[0];
        assert!(authorization.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"));
        assert!(authorization.contains("/eu-west-1/ses/aws4_request, SignedHeaders=content-type;host;x-amz-date;x-amz-security-token, Signature="));

        assert_eq!(
            request["ConfigurationSetName"].as_str(),
            configuration_set,
            "{mailer}"
        );
        assert_eq!(
            request["EmailTags"],
            json!([{"Name": "order_id", "Value": "1"}])
        );
        assert_eq!(
            request["Destination"]["ToAddresses"],
            json!([
                "taylor@example.com",
                "abigail@example.com",
                "secret@example.com"
            ])
        );
        let raw = String::from_utf8(decode(&request["Content"]["Raw"]["Data"])).unwrap();
        assert!(raw.contains("Subject: Order #1 Shipped"));
        assert!(raw.contains("From: Example <hello@example.com>"));
        assert!(raw.contains("filename=\"order.csv\""));
        assert!(!raw.contains("secret@example.com"));
    }
}

#[tokio::test]
async fn provider_errors_surface_as_transport_exceptions() {
    let app = App::new().using("postmark");
    Http::fake_urls([(
        "api.postmarkapp.com/*",
        Http::response(
            json!({"ErrorCode": 406, "Message": "You tried to send to a recipient that has been marked as inactive."}),
            422,
            &[],
        ),
    )]);

    let error = send_order_shipped(&app).await.unwrap_err();

    let exception = error.downcast_ref::<TransportException>().unwrap();
    assert_eq!(exception.code, 406);
    assert_eq!(
        exception.message,
        "Unable to send an email: You tried to send to a recipient that has been marked as inactive. (code 406)."
    );
}

#[tokio::test]
async fn faked_mail_never_reaches_the_api() {
    let app = App::new().using("resend");
    Http::fake();
    let fake = Mail::fake();

    let sent = Mail::to("taylor@example.com")
        .send(OrderShipped {
            id: 2,
            logo: app.logo(),
        })
        .await
        .unwrap();

    assert!(sent.is_none());
    fake.assert_sent_to::<OrderShipped>("taylor@example.com");
    Http::assert_nothing_sent();
}
