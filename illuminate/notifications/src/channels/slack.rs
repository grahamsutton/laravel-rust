//! The `slack` channel.

use async_trait::async_trait;
use illuminate_config::Repository;
use illuminate_container::try_app;
use illuminate_http_client::Http;
use illuminate_support::error::RuntimeException;
use illuminate_support::{Map, Result, Value, ValueExt};
use serde::{Deserialize, Serialize};

use super::Channel;
use crate::notifiable::Notifiable;
use crate::notification::Notification;
use crate::slack::{LogicException, SlackDestination, filled};

/// Sends notifications to Slack.
///
/// The notification's [`to_slack`](Notification::to_slack) message is
/// posted with Slack's `chat.postMessage` Web API method, authenticated
/// with your App's bot token — or, when the notifiable routes `slack`
/// notifications to an incoming webhook URL, posted to that webhook
/// (Laravel's `SlackNotificationRouterChannel`, `SlackWebApiChannel` and
/// `SlackWebhookChannel`).
///
/// Requests go through the [`Http`] client, so `Http::fake()` keeps your
/// tests from reaching Slack:
///
/// ```
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::Container;
/// use illuminate_http_client::{Http, Request};
/// use illuminate_notifications::facades::Notification;
/// use illuminate_notifications::slack::SlackMessage;
/// use illuminate_notifications::{Notifiable, Notification as _};
/// use illuminate_support::json;
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct DeploymentFinished;
///
/// impl illuminate_notifications::Notification for DeploymentFinished {
///     fn via(&self, _notifiable: &dyn Notifiable) -> Vec<String> {
///         vec!["slack".into()]
///     }
///
///     fn to_slack(&self, _notifiable: &dyn Notifiable) -> Option<SlackMessage> {
///         Some(SlackMessage::new().text("Deployed! :rocket:"))
///     }
/// }
///
/// let app = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(app.clone());
/// app.instance(Repository::new(json!({
///     "services": {"slack": {"notifications": {"bot_user_oauth_token": "xoxb-token"}}},
/// })));
///
/// Http::fake_urls([("slack.com/api/*", Http::response(json!({"ok": true}), 200, &[]))]);
///
/// Notification::route("slack", "#deployments").notify(DeploymentFinished).await?;
///
/// Http::assert_sent(|request: &Request| {
///     request.url() == "https://slack.com/api/chat.postMessage"
///         && request.has_header_value("Authorization", "Bearer xoxb-token")
///         && request["channel"] == "#deployments"
///         && request["text"] == "Deployed! :rocket:"
/// });
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct SlackChannel;

/// A Slack message, ready to post (and to travel through the queue).
#[derive(Debug, Serialize, Deserialize)]
struct PendingSlackMessage {
    /// The incoming webhook to post to, instead of the Web API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    webhook: Option<String>,
    /// The route's bot token. Your application's own token isn't stored:
    /// it is read from the configuration when the message is posted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    /// The `chat.postMessage` payload.
    message: Value,
}

impl SlackChannel {
    /// The Web API method messages are posted to.
    pub const API_URL: &'static str = "https://slack.com/api/chat.postMessage";

    /// Build the message for a notification, without posting it. `None`
    /// means the notifiable doesn't want Slack notifications.
    fn compose(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
    ) -> Result<Option<PendingSlackMessage>> {
        let route = notifiable.route_notification_for("slack", notification);
        let Some(destination) = SlackDestination::from_route(route) else {
            return Ok(None);
        };
        let message = notification.to_slack(notifiable).ok_or_else(|| {
            RuntimeException::new(format!(
                "Notification [{}] is missing a to_slack method.",
                notification.notification_name()
            ))
        })?;
        let payload = message.to_array()?;

        let route = match destination {
            SlackDestination::Webhook(url) => {
                return Ok(Some(PendingSlackMessage {
                    webhook: Some(url),
                    token: None,
                    message: payload,
                }));
            }
            SlackDestination::WebApi(route) => route,
        };

        let Some(channel) = route
            .channel
            .or(message.channel)
            .or_else(|| config("channel"))
        else {
            return Err(LogicException::new("Slack notification channel is not set.").into());
        };
        if route.token.is_none() {
            // Fail now, rather than on the worker, when there's no token.
            default_token()?;
        }

        // The channel leads the payload, like in Slack's documentation.
        let mut body = Map::from_iter([("channel".to_string(), Value::String(channel))]);
        if let Value::Object(payload) = payload {
            body.extend(payload.into_iter().filter(|(key, _)| key != "channel"));
        }
        Ok(Some(PendingSlackMessage {
            webhook: None,
            token: route.token,
            message: Value::Object(body),
        }))
    }

    /// Post a message to Slack.
    async fn post(&self, pending: PendingSlackMessage) -> Result<Value> {
        if let Some(webhook) = pending.webhook {
            let response = Http::post(webhook, &pending.message).await?;
            response.throw()?;
            return Ok(Value::String(response.body().into_owned()));
        }

        let token = match pending.token {
            Some(token) => token,
            None => default_token()?,
        };
        let response = Http::with_token(&token, "Bearer")
            .post(Self::API_URL, &pending.message)
            .await?;
        response.throw()?;

        let body = response.json();
        if body.is_object() && !body["ok"].truthy() {
            return Err(RuntimeException::new(format!(
                "Slack API call failed with error [{}].",
                body["error"].to_string_lossy()
            ))
            .into());
        }
        Ok(body)
    }
}

/// Read a `services.slack.notifications.*` configuration value.
fn config(key: &str) -> Option<String> {
    try_app::<Repository>()
        .map(|config| config.get(&format!("services.slack.notifications.{key}")))
        .as_ref()
        .and_then(filled)
}

/// Your application's bot token.
fn default_token() -> Result<String> {
    config("bot_user_oauth_token")
        .ok_or_else(|| LogicException::new("Slack API authentication token is not set.").into())
}

#[async_trait]
impl Channel for SlackChannel {
    async fn send(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
        _id: &str,
    ) -> Result<Value> {
        match self.compose(notifiable, notification)? {
            Some(pending) => self.post(pending).await,
            None => Ok(Value::Null),
        }
    }

    async fn prepare(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
        _id: &str,
    ) -> Result<Option<Value>> {
        match self.compose(notifiable, notification)? {
            Some(pending) => Ok(Some(serde_json::to_value(pending)?)),
            None => Ok(None),
        }
    }

    fn supports_queueing(&self) -> bool {
        true
    }

    async fn deliver(&self, payload: Value) -> Result<Value> {
        self.post(serde_json::from_value(payload)?).await
    }
}
