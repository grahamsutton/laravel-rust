//! The `slack` log channel: post log entries to a Slack webhook.

use illuminate_log::{Handler, Level, LogRecord, Monolog};
use illuminate_support::{Result, Value, ValueExt, json};

/// Posts log records to a Slack incoming webhook, shaped like Monolog's
/// `SlackWebhookHandler` (an attachment with the message and its level).
///
/// Messages are sent in the background, so logging never waits on Slack.
pub struct SlackWebhookHandler {
    url: String,
    channel: Option<String>,
    username: String,
    emoji: Option<String>,
    short: bool,
    context: bool,
    level: Level,
    bubble: bool,
}

impl SlackWebhookHandler {
    /// Create a handler from a `slack` channel's configuration.
    pub fn from_config(config: &Value) -> Result<Self> {
        let string = |key: &str| {
            config
                .get(key)
                .filter(|value| !value.is_null())
                .map(ValueExt::to_string_lossy)
                .filter(|value| !value.is_empty())
        };
        let level = match string("level") {
            Some(level) => Level::parse(&level)
                .ok_or_else(|| illuminate_support::error::InvalidArgumentException::new("Invalid log level."))?,
            None => Level::Critical,
        };
        Ok(Self {
            url: string("url").unwrap_or_default(),
            channel: string("channel"),
            username: string("username").unwrap_or_else(|| "Laravel Log".into()),
            emoji: string("emoji").or_else(|| Some(":boom:".into())),
            short: config.get("short").is_some_and(ValueExt::truthy),
            context: config.get("context").is_some_and(ValueExt::truthy),
            level,
            bubble: config.get("bubble").is_none_or(ValueExt::truthy),
        })
    }

    /// The payload Slack receives for the record.
    pub fn payload(&self, record: &LogRecord) -> Value {
        let color = match record.level {
            level if level >= Level::Error => "danger",
            level if level >= Level::Warning => "warning",
            level if level >= Level::Info => "good",
            _ => "#e3e4e6",
        };
        let level_name = record.level.name().to_uppercase();

        let mut attachment = json!({
            "fallback": record.message,
            "text": record.message,
            "color": color,
            "fields": [],
            "mrkdwn_in": ["fields"],
            "ts": record.datetime.timestamp(),
            "footer": self.username,
        });
        if self.short {
            attachment["title"] = json!(level_name);
        } else {
            attachment["title"] = json!(record.message);
            attachment["fields"] = json!([{"title": "Level", "value": level_name, "short": false}]);
        }
        if self.context {
            let fields = attachment["fields"].as_array_mut().expect("fields is an array");
            for (title, data) in [("Extra", &record.extra), ("Context", &record.context)] {
                if !data.is_empty() {
                    fields.push(json!({
                        "title": title,
                        "value": format!("```{}```", serde_json::to_string_pretty(data).unwrap_or_default()),
                        "short": false,
                    }));
                }
            }
        }

        let mut payload = json!({
            "username": self.username,
            "attachments": [attachment],
        });
        if let Some(channel) = &self.channel {
            payload["channel"] = json!(channel);
        }
        match &self.emoji {
            Some(emoji) if emoji.starts_with(':') => payload["icon_emoji"] = json!(emoji),
            Some(icon) => payload["icon_url"] = json!(icon),
            None => {}
        }
        payload
    }
}

impl Handler for SlackWebhookHandler {
    fn is_handling(&self, level: Level) -> bool {
        level >= self.level
    }

    fn handle(&self, record: &LogRecord) -> Result<bool> {
        if !self.is_handling(record.level) {
            return Ok(false);
        }
        let payload = self.payload(record);
        let url = self.url.clone();
        if !url.is_empty() && tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(illuminate_container::Container::scope_current(async move {
                if let Err(error) = illuminate_http_client::Http::post(&url, payload).await {
                    eprintln!("Unable to send the log entry to Slack: {error}");
                }
            }));
        }
        Ok(!self.bubble)
    }
}

/// Register the `slack` log driver.
pub(crate) fn boot() {
    illuminate_log::Log::extend("slack", |_, config| {
        let name = config
            .get("name")
            .filter(|name| !name.is_null())
            .map(ValueExt::to_string_lossy)
            .unwrap_or_else(|| crate::application::Application::current().environment());
        Ok(Monolog::new(name).with_handler(SlackWebhookHandler::from_config(config)?))
    });
}
