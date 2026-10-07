//! End-to-end tests: notifications over the `slack` channel, with Slack faked
//! through the HTTP client.

use std::sync::{Arc, Mutex};

use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_events::Event;
use illuminate_http_client::{FakeResponse, Http, Request, RequestException};
use illuminate_notifications::facades::Notification;
use illuminate_notifications::slack::{LogicException, SlackMessage, SlackRoute};
use illuminate_notifications::{
    Notifiable, Notification as NotificationContract, NotificationFailed, NotificationSent,
    NotificationServiceProvider, QueuedNotification, SlackChannel,
};
use illuminate_support::error::RuntimeException;
use illuminate_support::{Value, json};
use serde::Serialize;

const WEBHOOK: &str = "https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXX";

struct App {
    container: Arc<Container>,
    api: Arc<Mutex<FakeResponse>>,
    webhook: Arc<Mutex<FakeResponse>>,
    _guard: LocalInstanceGuard,
}

impl App {
    /// An application with a bot token and a default channel.
    fn new() -> Self {
        Self::with_slack(json!({"bot_user_oauth_token": "xoxb-app", "channel": "#general"}))
    }

    /// An application with the given `services.slack.notifications`
    /// configuration, and Slack faked.
    fn with_slack(slack: Value) -> Self {
        let container = Arc::new(Container::new());
        let guard = Container::set_local_instance(container.clone());
        container.instance(Repository::new(json!({
            "app": {"name": "Laravel"},
            "queue": {"default": "array", "connections": {"array": {"driver": "array", "queue": "default"}}},
            "services": {"slack": {"notifications": slack}},
        })));
        NotificationServiceProvider.register(&container);
        NotificationServiceProvider.boot(&container);

        let app = Self {
            container,
            api: Arc::new(Mutex::new(
                json!({"ok": true, "channel": "C123", "ts": "1503435956.000247"}).into(),
            )),
            webhook: Arc::new(Mutex::new("ok".into())),
            _guard: guard,
        };
        let (api, webhook) = (app.api.clone(), app.webhook.clone());
        Http::fake_using(move |request: &Request| {
            if request.url().starts_with("https://slack.com/api/") {
                api.lock().unwrap().clone()
            } else if request.url().starts_with("https://hooks.slack.com/") {
                webhook.lock().unwrap().clone()
            } else {
                Http::response("Not Slack.", 418, &[])
            }
        });
        app
    }

    /// Make Slack's Web API respond with the given response.
    fn api_responds_with(&self, response: impl Into<FakeResponse>) {
        *self.api.lock().unwrap() = response.into();
    }

    /// Make Slack's webhooks respond with the given response.
    fn webhooks_respond_with(&self, response: impl Into<FakeResponse>) {
        *self.webhook.lock().unwrap() = response.into();
    }

    fn config(&self) -> Arc<Repository> {
        self.container.make::<Repository>()
    }
}

/// The requests sent to Slack.
fn sent() -> Vec<Request> {
    Http::recorded()
        .all()
        .iter()
        .map(|(request, _)| request.clone())
        .collect()
}

#[derive(Serialize)]
struct User {
    id: u64,
    #[serde(skip)]
    slack: Option<Value>,
}

impl User {
    fn new(slack: Option<Value>) -> Self {
        Self { id: 1, slack }
    }
}

impl Notifiable for User {
    fn notifiable_key(&self) -> Value {
        json!(self.id)
    }

    fn notifiable_type(&self) -> String {
        "App\\Models\\User".into()
    }

    fn route_notification_for_slack(
        &self,
        _notification: &dyn NotificationContract,
    ) -> Option<Value> {
        self.slack.clone()
    }
}

#[derive(Clone, Serialize)]
struct SlackNotification {
    #[serde(skip)]
    message: Option<SlackMessage>,
    queued: bool,
    #[serde(skip)]
    responses: Arc<Mutex<Vec<Value>>>,
}

impl SlackNotification {
    fn new(message: SlackMessage) -> Self {
        Self {
            message: Some(message),
            queued: false,
            responses: Arc::default(),
        }
    }

    fn text(text: &str) -> Self {
        Self::new(SlackMessage::new().text(text))
    }

    fn queued(mut self) -> Self {
        self.queued = true;
        self
    }
}

impl NotificationContract for SlackNotification {
    fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
        vec!["slack".into()]
    }

    fn to_slack(&self, _notifiable: &dyn Notifiable) -> Option<SlackMessage> {
        self.message.clone()
    }

    fn should_queue(&self) -> bool {
        self.queued
    }

    fn after_sending(&self, _notifiable: &dyn Notifiable, _channel: &str, response: &Value) {
        self.responses.lock().unwrap().push(response.clone());
    }
}

fn invoice_paid() -> SlackMessage {
    SlackMessage::new()
        .text("One of your invoices has been paid!")
        .header_block("Invoice Paid")
        .context_block(|block| {
            block.text("Customer #1234");
        })
        .section_block(|block| {
            block.text("An invoice has been paid.");
            block.field("*Invoice No:*\n1000").markdown();
            block
                .field("*Invoice Recipient:*\ntaylor@laravel.com")
                .markdown();
        })
        .divider_block()
        .section_block(|block| {
            block.text("Congratulations!");
        })
}

#[tokio::test]
async fn messages_are_posted_to_the_web_api_with_the_bot_token() {
    let _app = App::new();
    let notification = SlackNotification::new(invoice_paid());
    User::new(None).notify(notification.clone()).await.unwrap();

    Http::assert_sent_count(1);
    let request = &sent()[0];
    assert_eq!(request.method(), "POST");
    assert_eq!(request.url(), "https://slack.com/api/chat.postMessage");
    assert!(request.is_json());
    assert_eq!(request.header("Authorization"), vec!["Bearer xoxb-app"]);
    assert_eq!(
        request.data(),
        &json!({
            "channel": "#general",
            "text": "One of your invoices has been paid!",
            "blocks": [
                {"type": "header", "text": {"type": "plain_text", "text": "Invoice Paid"}},
                {"type": "context", "elements": [{"type": "plain_text", "text": "Customer #1234"}]},
                {
                    "type": "section",
                    "text": {"type": "plain_text", "text": "An invoice has been paid."},
                    "fields": [
                        {"type": "mrkdwn", "text": "*Invoice No:*\n1000"},
                        {"type": "mrkdwn", "text": "*Invoice Recipient:*\ntaylor@laravel.com"},
                    ],
                },
                {"type": "divider"},
                {"type": "section", "text": {"type": "plain_text", "text": "Congratulations!"}},
            ],
        })
    );
    // The channel leads the payload, like Slack's own examples.
    assert_eq!(
        request.data().as_object().unwrap().keys().next().unwrap(),
        "channel"
    );

    // The notification hears Slack's response.
    assert_eq!(
        notification.responses.lock().unwrap()[0],
        json!({"ok": true, "channel": "C123", "ts": "1503435956.000247"})
    );
}

#[tokio::test]
async fn interactive_messages_post_their_buttons() {
    let _app = App::new();
    let message = SlackMessage::new()
        .text("One of your invoices has been paid!")
        .actions_block(|block| {
            block.button("Acknowledge Invoice").primary().confirm(
                "Acknowledge the payment and send a thank you email?",
                |dialog| {
                    dialog.confirm("Yes");
                    dialog.deny("No");
                },
            );
            block.button("Deny").danger().id("deny_invoice");
        });
    User::new(None)
        .notify(SlackNotification::new(message))
        .await
        .unwrap();

    Http::assert_sent(|request: &Request| {
        request["blocks"][0]
            == json!({
                "type": "actions",
                "elements": [
                    {
                        "type": "button",
                        "text": {"type": "plain_text", "text": "Acknowledge Invoice"},
                        "action_id": "button_acknowledge_invoice",
                        "style": "primary",
                        "confirm": {
                            "title": {"type": "plain_text", "text": "Are you sure?"},
                            "text": {"type": "plain_text", "text": "Acknowledge the payment and send a thank you email?"},
                            "confirm": {"type": "plain_text", "text": "Yes"},
                            "deny": {"type": "plain_text", "text": "No"},
                        },
                    },
                    {
                        "type": "button",
                        "text": {"type": "plain_text", "text": "Deny"},
                        "action_id": "deny_invoice",
                        "style": "danger",
                    },
                ],
            })
    });
}

#[tokio::test]
async fn message_options_and_templates_are_posted() {
    let _app = App::new();
    let message = SlackMessage::new()
        .text("Boo!")
        .username("larabot")
        .image("https://laravel.com/img/favicon/favicon-32x32.png")
        .unfurl_links(true)
        .unfurl_media(true)
        .thread_timestamp("123456.7890")
        .broadcast_reply(true)
        .disable_markdown_parsing()
        .metadata("task_created", json!({"id": "11223"}))
        .using_block_kit_template(
            r#"{"blocks": [{"type": "section", "text": {"type": "plain_text", "text": "We are hiring!"}}]}"#,
        );
    User::new(None)
        .notify(SlackNotification::new(message))
        .await
        .unwrap();

    assert_eq!(
        sent()[0].data(),
        &json!({
            "channel": "#general",
            "text": "Boo!",
            "blocks": [{"type": "section", "text": {"type": "plain_text", "text": "We are hiring!"}}],
            "icon_url": "https://laravel.com/img/favicon/favicon-32x32.png",
            "metadata": {"event_type": "task_created", "event_payload": {"id": "11223"}},
            "mrkdwn": false,
            "thread_ts": "123456.7890",
            "reply_broadcast": true,
            "unfurl_links": true,
            "unfurl_media": true,
            "username": "larabot",
        })
    );
}

#[tokio::test]
async fn channels_are_resolved_from_the_route_the_message_and_the_config() {
    let _app = App::new();

    // The configured default channel...
    User::new(None)
        .notify(SlackNotification::text("Hello"))
        .await
        .unwrap();
    // ...loses to the message's channel...
    User::new(None)
        .notify(SlackNotification::new(
            SlackMessage::new().text("Hello").to("#ops"),
        ))
        .await
        .unwrap();
    // ...which loses to the notifiable's route.
    User::new(Some(json!("#support-channel")))
        .notify(SlackNotification::new(
            SlackMessage::new().text("Hello").to("#ops"),
        ))
        .await
        .unwrap();

    let channels: Vec<Value> = sent()
        .iter()
        .map(|request| request["channel"].clone())
        .collect();
    assert_eq!(
        channels,
        vec![json!("#general"), json!("#ops"), json!("#support-channel")]
    );
    assert!(
        sent()
            .iter()
            .all(|request| request.has_header_value("Authorization", "Bearer xoxb-app"))
    );
}

#[tokio::test]
async fn slack_routes_notify_external_workspaces() {
    let _app = App::new();
    User::new(Some(SlackRoute::make("#team-alerts", "xoxb-team").into()))
        .notify(SlackNotification::text("Hello, team!"))
        .await
        .unwrap();
    // A route with only a token uses the message's (or the default) channel.
    let token_only = SlackRoute {
        channel: None,
        token: Some("xoxb-other".into()),
    };
    User::new(Some(token_only.into()))
        .notify(SlackNotification::new(
            SlackMessage::new().text("Hi").to("#random"),
        ))
        .await
        .unwrap();
    // And a channel-only route uses the application's token.
    User::new(Some(SlackRoute::channel("#ops").into()))
        .notify(SlackNotification::text("Hi"))
        .await
        .unwrap();

    let requests = sent();
    assert_eq!(requests[0]["channel"], "#team-alerts");
    assert!(requests[0].has_header_value("Authorization", "Bearer xoxb-team"));
    assert_eq!(requests[1]["channel"], "#random");
    assert!(requests[1].has_header_value("Authorization", "Bearer xoxb-other"));
    assert_eq!(requests[2]["channel"], "#ops");
    assert!(requests[2].has_header_value("Authorization", "Bearer xoxb-app"));
}

#[tokio::test]
async fn webhook_routes_receive_the_message_directly() {
    // Webhooks need neither a token nor a channel.
    let _app = App::with_slack(json!({}));
    let notification =
        SlackNotification::new(SlackMessage::new().text("Deployed!").divider_block());
    User::new(Some(json!(WEBHOOK)))
        .notify(notification.clone())
        .await
        .unwrap();

    Http::assert_sent_count(1);
    let request = &sent()[0];
    assert_eq!(request.method(), "POST");
    assert_eq!(request.url(), WEBHOOK);
    assert!(!request.has_header("Authorization"));
    assert_eq!(
        request.data(),
        &json!({"text": "Deployed!", "blocks": [{"type": "divider"}]})
    );
    assert_eq!(notification.responses.lock().unwrap()[0], json!("ok"));
}

#[tokio::test]
async fn failed_webhooks_throw_request_exceptions() {
    let app = App::new();
    app.webhooks_respond_with(Http::response("no_service", 404, &[]));
    let error = User::new(Some(json!(WEBHOOK)))
        .notify(SlackNotification::text("Hi"))
        .await
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<RequestException>()
            .unwrap()
            .response
            .status(),
        404
    );
}

#[tokio::test]
async fn slack_api_errors_are_thrown() {
    let app = App::new();
    app.api_responds_with(json!({"ok": false, "error": "channel_not_found"}));

    let failures = Arc::new(Mutex::new(Vec::new()));
    let recorder = failures.clone();
    Event::listen(move |event: Arc<NotificationFailed>| {
        let recorder = recorder.clone();
        async move {
            recorder.lock().unwrap().push(event.error.clone());
            Ok(())
        }
    });

    let error = User::new(None)
        .notify(SlackNotification::text("Hello"))
        .await
        .unwrap_err();
    assert!(error.is::<RuntimeException>());
    assert_eq!(
        error.to_string(),
        "Slack API call failed with error [channel_not_found]."
    );
    assert_eq!(
        failures.lock().unwrap().as_slice(),
        ["Slack API call failed with error [channel_not_found]."]
    );

    // Server errors surface as request exceptions.
    app.api_responds_with(Http::response("", 500, &[]));
    let error = User::new(None)
        .notify(SlackNotification::text("Hello"))
        .await
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<RequestException>()
            .unwrap()
            .response
            .status(),
        500
    );
}

#[tokio::test]
async fn responses_that_are_not_json_are_accepted() {
    let app = App::new();
    app.api_responds_with("");
    User::new(None)
        .notify(SlackNotification::text("Hello"))
        .await
        .unwrap();
    Http::assert_sent_count(1);
}

#[tokio::test]
async fn sent_events_carry_slacks_response() {
    let _app = App::new();
    let responses = Arc::new(Mutex::new(Vec::new()));
    let recorder = responses.clone();
    Event::listen(move |event: Arc<NotificationSent>| {
        let recorder = recorder.clone();
        async move {
            assert_eq!(event.channel, "slack");
            recorder.lock().unwrap().push(event.response.clone());
            Ok(())
        }
    });
    User::new(None)
        .notify(SlackNotification::text("Hello"))
        .await
        .unwrap();
    assert_eq!(responses.lock().unwrap()[0]["ts"], "1503435956.000247");
}

#[tokio::test]
async fn missing_channels_and_tokens_are_logic_errors() {
    let _app = App::with_slack(json!({"bot_user_oauth_token": "xoxb-app"}));
    let error = User::new(None)
        .notify(SlackNotification::text("Hello"))
        .await
        .unwrap_err();
    assert!(error.is::<LogicException>());
    assert_eq!(error.to_string(), "Slack notification channel is not set.");

    let _app = App::with_slack(json!({"channel": "#general"}));
    let error = User::new(None)
        .notify(SlackNotification::text("Hello"))
        .await
        .unwrap_err();
    assert!(error.is::<LogicException>());
    assert_eq!(
        error.to_string(),
        "Slack API authentication token is not set."
    );
    // A route's own token is enough.
    User::new(Some(SlackRoute::make("#general", "xoxb-team").into()))
        .notify(SlackNotification::text("Hello"))
        .await
        .unwrap();
    Http::assert_sent_count(1);
}

#[tokio::test]
async fn invalid_messages_are_never_sent() {
    let _app = App::new();
    let error = User::new(None)
        .notify(SlackNotification::new(SlackMessage::new()))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Slack messages must contain at least a text message or block."
    );

    let error = User::new(None)
        .notify(SlackNotification::new(
            SlackMessage::new().text("Hi").actions_block(|_| {}),
        ))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "There must be at least one element in each actions block."
    );

    let error = User::new(None)
        .notify(SlackNotification {
            message: None,
            queued: false,
            responses: Arc::default(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Notification [SlackNotification] is missing a to_slack method."
    );
    Http::assert_nothing_sent();
}

#[tokio::test]
async fn notifiables_may_decline_slack_notifications() {
    let _app = App::new();
    User::new(Some(json!(false)))
        .notify(SlackNotification::text("Hello"))
        .await
        .unwrap();
    Http::assert_nothing_sent();
}

#[tokio::test]
async fn on_demand_notifications_route_to_slack() {
    let _app = App::new();
    Notification::route("slack", "#on-demand")
        .notify(SlackNotification::text("Hello"))
        .await
        .unwrap();
    Notification::route("slack", SlackRoute::make("#external", "xoxb-external"))
        .notify(SlackNotification::text("Hello"))
        .await
        .unwrap();
    Notification::route("slack", WEBHOOK)
        .notify(SlackNotification::text("Hello"))
        .await
        .unwrap();

    let requests = sent();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["channel"], "#on-demand");
    assert!(requests[0].has_header_value("Authorization", "Bearer xoxb-app"));
    assert_eq!(requests[1]["channel"], "#external");
    assert!(requests[1].has_header_value("Authorization", "Bearer xoxb-external"));
    assert_eq!(requests[2].url(), WEBHOOK);
}

#[tokio::test]
async fn the_slack_channel_is_built_in() {
    let _app = App::new();
    assert!(
        Notification::channel(Some("slack"))
            .unwrap()
            .supports_queueing()
    );
    // It can be registered under other names, too.
    Notification::extend("alerts", SlackChannel);
    assert!(Notification::channel(Some("alerts")).is_ok());
}

#[tokio::test]
async fn queued_notifications_are_pre_rendered_for_slack() {
    let app = App::new();
    let queued = Arc::new(Mutex::new(Vec::<QueuedNotification>::new()));
    let recorder = queued.clone();
    Notification::queue_using(move |notification: QueuedNotification| {
        let recorder = recorder.clone();
        async move {
            recorder.lock().unwrap().push(notification);
            Ok(())
        }
    });

    User::new(None)
        .notify(SlackNotification::new(invoice_paid()).queued())
        .await
        .unwrap();
    User::new(Some(SlackRoute::make("#team", "xoxb-team").into()))
        .notify(SlackNotification::text("Hello, team!").queued())
        .await
        .unwrap();
    User::new(Some(json!(WEBHOOK)))
        .notify(SlackNotification::text("Hello, webhook!").queued())
        .await
        .unwrap();
    Http::assert_nothing_sent();

    let jobs = queued.lock().unwrap().clone();
    assert_eq!(jobs.len(), 3);
    assert!(jobs.iter().all(|job| job.channel == "slack"));
    assert_eq!(jobs[0].payload["message"]["channel"], "#general");
    assert_eq!(jobs[0].payload["message"]["blocks"][0]["type"], "header");
    // The application's token isn't written to the queue...
    assert!(jobs[0].payload.get("token").is_none());
    // ...but a route's own token has to be.
    assert_eq!(jobs[1].payload["token"], "xoxb-team");
    assert_eq!(jobs[2].payload["webhook"], WEBHOOK);

    // The worker side: the application's token is read when delivering.
    app.config().set(
        "services.slack.notifications.bot_user_oauth_token",
        "xoxb-rotated",
    );
    for job in jobs {
        let payload = serde_json::to_string(&job).unwrap();
        Notification::send_queued(serde_json::from_str(&payload).unwrap())
            .await
            .unwrap();
    }

    let requests = sent();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].has_header_value("Authorization", "Bearer xoxb-rotated"));
    assert_eq!(requests[0]["text"], "One of your invoices has been paid!");
    assert_eq!(requests[0]["channel"], "#general");
    assert!(requests[1].has_header_value("Authorization", "Bearer xoxb-team"));
    assert_eq!(requests[1]["channel"], "#team");
    assert_eq!(requests[2].url(), WEBHOOK);
    assert_eq!(requests[2]["text"], "Hello, webhook!");
}

#[tokio::test]
async fn queued_slack_notifications_fail_fast_and_on_delivery() {
    let app = App::new();
    let queued = Arc::new(Mutex::new(Vec::<QueuedNotification>::new()));
    let recorder = queued.clone();
    Notification::queue_using(move |notification: QueuedNotification| {
        let recorder = recorder.clone();
        async move {
            recorder.lock().unwrap().push(notification);
            Ok(())
        }
    });

    // Invalid messages are caught before they are queued.
    let error = User::new(None)
        .notify(SlackNotification::new(SlackMessage::new()).queued())
        .await
        .unwrap_err();
    assert!(error.is::<LogicException>());
    assert!(queued.lock().unwrap().is_empty());

    // Declined notifications aren't queued at all.
    User::new(Some(json!(false)))
        .notify(SlackNotification::text("Hello").queued())
        .await
        .unwrap();
    assert!(queued.lock().unwrap().is_empty());

    // Slack's errors surface on the worker.
    User::new(None)
        .notify(SlackNotification::text("Hello").queued())
        .await
        .unwrap();
    app.api_responds_with(json!({"ok": false, "error": "not_in_channel"}));
    let job = queued.lock().unwrap().remove(0);
    let error = Notification::send_queued(job.clone()).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Slack API call failed with error [not_in_channel]."
    );

    // As does a token removed from the configuration.
    app.config().set(
        "services.slack.notifications.bot_user_oauth_token",
        Value::Null,
    );
    let error = Notification::send_queued(job).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Slack API authentication token is not set."
    );
}

#[tokio::test]
async fn queued_slack_notifications_go_through_the_queue_component() {
    use illuminate_queue::{
        BusServiceProvider, Queue, QueueServiceProvider, Worker, WorkerOptions,
    };

    let app = App::new();
    QueueServiceProvider.register(&app.container);
    BusServiceProvider.register(&app.container);
    // Booting installs the hook that dispatches queued notifications as jobs.
    NotificationServiceProvider.boot(&app.container);

    User::new(None)
        .notify(SlackNotification::text("Queued hello").queued())
        .await
        .unwrap();
    assert_eq!(Queue::size(None).await.unwrap(), 1);
    Http::assert_nothing_sent();

    let worker = Worker::make();
    worker
        .daemon("array", "default", &WorkerOptions::new().stop_when_empty())
        .await
        .unwrap();
    assert_eq!(worker.jobs_processed(), 1);
    Http::assert_sent(|request: &Request| {
        request.has_header_value("Authorization", "Bearer xoxb-app")
            && request["channel"] == "#general"
            && request["text"] == "Queued hello"
    });
}

#[tokio::test]
async fn slack_notifications_can_be_faked() {
    let _app = App::new();
    Notification::fake();

    let user = User::new(Some(json!("#support-channel")));
    user.notify(SlackNotification::new(invoice_paid()))
        .await
        .unwrap();
    Notification::route("slack", "#general")
        .notify(SlackNotification::text("On demand"))
        .await
        .unwrap();

    Notification::assert_sent_to::<SlackNotification>(&user);
    Notification::assert_sent_to_with::<SlackNotification>(&user, |notification, channels| {
        let message = notification.to_slack(&User::new(None)).unwrap();
        channels == ["slack"]
            && message.to_array().unwrap()["blocks"][0]["text"]["text"] == "Invoice Paid"
    });
    Notification::assert_sent_on_demand_with::<SlackNotification>(
        |notification, channels, notifiable| {
            channels == ["slack"]
                && notifiable.routes["slack"] == "#general"
                && notification.message.as_ref().unwrap().text.as_deref() == Some("On demand")
        },
    );
    Notification::assert_count(2);
    Http::assert_nothing_sent();
}
