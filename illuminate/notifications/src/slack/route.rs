//! [`SlackRoute`]: where a Slack notification is delivered.

use illuminate_support::{Value, ValueExt, to_value};
use serde::{Deserialize, Serialize};

/// A Slack channel and the bot token of the workspace it lives in. Return
/// one from your notifiable's `slack` route to notify an external
/// workspace (one your application's users installed your Slack App in).
///
/// ```
/// use illuminate_notifications::{Notifiable, Notification};
/// use illuminate_notifications::slack::SlackRoute;
/// use illuminate_support::{Value, json};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct Team {
///     id: u64,
///     slack_channel: String,
///     slack_token: String,
/// }
///
/// impl Notifiable for Team {
///     fn notifiable_key(&self) -> Value {
///         json!(self.id)
///     }
///
///     fn notifiable_type(&self) -> String {
///         "App\\Models\\Team".into()
///     }
///
///     fn route_notification_for_slack(&self, _notification: &dyn Notification) -> Option<Value> {
///         Some(SlackRoute::make(&self.slack_channel, &self.slack_token).into())
///     }
/// }
/// ```
///
/// Without a token, the route uses your application's bot token
/// (`services.slack.notifications.bot_user_oauth_token`); without a
/// channel, the message's [`to`](crate::slack::SlackMessage::to) channel or
/// `services.slack.notifications.channel`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackRoute {
    /// The channel to send the message to, like `#general`.
    pub channel: Option<String>,
    /// The bot user OAuth token of the workspace.
    pub token: Option<String>,
}

impl SlackRoute {
    /// Route notifications to the given channel, with the given token.
    pub fn make(channel: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            channel: Some(channel.into()),
            token: Some(token.into()),
        }
    }

    /// Route notifications to the given channel, with your application's token.
    pub fn channel(channel: impl Into<String>) -> Self {
        Self {
            channel: Some(channel.into()),
            token: None,
        }
    }
}

impl From<SlackRoute> for Value {
    fn from(route: SlackRoute) -> Self {
        to_value(&route)
    }
}

/// Where a notifiable's `slack` route sends a notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SlackDestination {
    /// Slack's Web API (`chat.postMessage`).
    WebApi(SlackRoute),
    /// An incoming webhook URL.
    Webhook(String),
}

impl SlackDestination {
    /// Interpret a route: a channel name, a [`SlackRoute`], a webhook URL,
    /// or nothing (the defaults). `false` means "don't send".
    pub(crate) fn from_route(route: Option<Value>) -> Option<Self> {
        match route {
            Some(Value::Bool(false)) => None,
            Some(Value::String(url))
                if url.starts_with("https://") || url.starts_with("http://") =>
            {
                Some(Self::Webhook(url))
            }
            Some(Value::String(channel)) => Some(Self::WebApi(SlackRoute {
                channel: (!channel.trim().is_empty()).then_some(channel),
                token: None,
            })),
            Some(Value::Object(route)) => Some(Self::WebApi(SlackRoute {
                channel: route.get("channel").and_then(filled),
                token: route.get("token").and_then(filled),
            })),
            _ => Some(Self::WebApi(SlackRoute::default())),
        }
    }
}

/// A non-blank string value.
pub(crate) fn filled(value: &Value) -> Option<String> {
    (!value.is_blank()).then(|| value.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn routes_become_values_and_back() {
        let route = SlackRoute::make("#general", "xoxb-1");
        assert_eq!(
            Value::from(route.clone()),
            json!({"channel": "#general", "token": "xoxb-1"})
        );
        assert_eq!(
            SlackDestination::from_route(Some(route.clone().into())),
            Some(SlackDestination::WebApi(route))
        );
        assert_eq!(
            SlackRoute::channel("#ops"),
            SlackRoute {
                channel: Some("#ops".into()),
                token: None
            }
        );
    }

    #[test]
    fn routes_are_interpreted_like_laravel() {
        assert_eq!(
            SlackDestination::from_route(Some(json!("#support"))),
            Some(SlackDestination::WebApi(SlackRoute::channel("#support")))
        );
        assert_eq!(
            SlackDestination::from_route(Some(json!("https://hooks.slack.com/services/T/B/X"))),
            Some(SlackDestination::Webhook(
                "https://hooks.slack.com/services/T/B/X".into()
            ))
        );
        assert_eq!(
            SlackDestination::from_route(Some(json!("http://localhost/hook"))),
            Some(SlackDestination::Webhook("http://localhost/hook".into()))
        );
        assert_eq!(
            SlackDestination::from_route(Some(json!({"token": "xoxb-2"}))),
            Some(SlackDestination::WebApi(SlackRoute {
                channel: None,
                token: Some("xoxb-2".into())
            }))
        );
        for defaults in [None, Some(Value::Null), Some(json!("")), Some(json!(true))] {
            assert_eq!(
                SlackDestination::from_route(defaults),
                Some(SlackDestination::WebApi(SlackRoute::default()))
            );
        }
        assert_eq!(SlackDestination::from_route(Some(json!(false))), None);
    }
}
