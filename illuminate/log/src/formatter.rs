//! Formatters turn log records into the lines written by handlers.

use std::sync::LazyLock;

use regex::Regex;

use illuminate_support::{Map, Value, ValueExt};

use crate::record::{Context, LogRecord};

/// Formats a log record for output.
pub trait Formatter: Send + Sync {
    /// Format the record into a string (usually a single line ending in `\n`).
    fn format(&self, record: &LogRecord) -> String;
}

impl<F> Formatter for F
where
    F: Fn(&LogRecord) -> String + Send + Sync,
{
    fn format(&self, record: &LogRecord) -> String {
        self(record)
    }
}

/// Monolog's `LineFormatter`: one line per record.
///
/// Laravel configures it with the `Y-m-d H:i:s` date format, inline line
/// breaks, and empty context / extra omitted, which produces the familiar
/// lines in `storage/logs/laravel.log`:
///
/// ```text
/// [2024-01-01 12:00:00] local.INFO: User logged in. {"id":1}
/// ```
///
/// ```
/// use illuminate_log::{Formatter, Level, LineFormatter, LogRecord, to_context};
/// use illuminate_support::{json, Carbon};
///
/// let mut record = LogRecord::new("local", Level::Info, "User logged in.", to_context(json!({"id": 1})));
/// record.datetime = Carbon::parse("2024-01-01 12:00:00").unwrap();
///
/// assert_eq!(
///     LineFormatter::laravel().format(&record),
///     "[2024-01-01 12:00:00] local.INFO: User logged in. {\"id\":1} \n",
/// );
/// ```
#[derive(Debug, Clone)]
pub struct LineFormatter {
    format: String,
    date_format: String,
    allow_inline_line_breaks: bool,
    ignore_empty_context_and_extra: bool,
}

impl Default for LineFormatter {
    fn default() -> Self {
        Self::new()
    }
}

static LEFTOVER_PLACEHOLDERS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"%(?:extra|context)\..+?%").expect("valid regex"));

impl LineFormatter {
    /// Monolog's default line format.
    pub const SIMPLE_FORMAT: &'static str =
        "[%datetime%] %channel%.%level_name%: %message% %context% %extra%\n";

    /// Monolog's default date format (`Y-m-d\TH:i:sP`).
    pub const SIMPLE_DATE: &'static str = "Y-m-d\\TH:i:sP";

    /// The date format Laravel uses for its log files.
    pub const LARAVEL_DATE: &'static str = "Y-m-d H:i:s";

    /// A formatter with Monolog's defaults.
    pub fn new() -> Self {
        Self {
            format: Self::SIMPLE_FORMAT.to_string(),
            date_format: Self::SIMPLE_DATE.to_string(),
            allow_inline_line_breaks: false,
            ignore_empty_context_and_extra: false,
        }
    }

    /// The formatter Laravel configures for its channels.
    pub fn laravel() -> Self {
        Self::new()
            .with_date_format(Self::LARAVEL_DATE)
            .allow_inline_line_breaks(true)
            .ignore_empty_context_and_extra(true)
    }

    /// Use a custom line format (`%datetime%`, `%channel%`, `%level_name%`,
    /// `%level%`, `%message%`, `%context%`, `%extra%`, `%context.key%`, ...).
    pub fn with_format(mut self, format: impl Into<String>) -> Self {
        self.format = format.into();
        self
    }

    /// Use a custom PHP-style date format.
    pub fn with_date_format(mut self, format: impl Into<String>) -> Self {
        self.date_format = format.into();
        self
    }

    /// Keep line breaks inside messages instead of replacing them with spaces.
    pub fn allow_inline_line_breaks(mut self, allow: bool) -> Self {
        self.allow_inline_line_breaks = allow;
        self
    }

    /// Omit the context and extra placeholders entirely when they're empty
    /// (instead of printing `[]`).
    pub fn ignore_empty_context_and_extra(mut self, ignore: bool) -> Self {
        self.ignore_empty_context_and_extra = ignore;
        self
    }

    fn stringify(&self, value: &Value) -> String {
        self.replace_newlines(convert_to_string(value))
    }

    fn replace_newlines(&self, string: String) -> String {
        if self.allow_inline_line_breaks {
            if string.starts_with('{') || string.starts_with('[') {
                return unescape_newlines(&string);
            }
            return string;
        }
        string.replace("\r\n", " ").replace(['\r', '\n'], " ")
    }
}

impl Formatter for LineFormatter {
    fn format(&self, record: &LogRecord) -> String {
        let mut output = self.format.clone();
        let mut context = record.context.clone();
        let mut extra = record.extra.clone();

        for (key, value) in &record.extra {
            let placeholder = format!("%extra.{key}%");
            if output.contains(&placeholder) {
                output = output.replace(&placeholder, &self.stringify(value));
                extra.remove(key);
            }
        }
        for (key, value) in &record.context {
            let placeholder = format!("%context.{key}%");
            if output.contains(&placeholder) {
                output = output.replace(&placeholder, &self.stringify(value));
                context.remove(key);
            }
        }

        let mut vars: Vec<(&str, Value)> = vec![
            ("message", Value::String(record.message.clone())),
            ("context", Value::Object(context)),
            ("level", Value::from(record.level.value())),
            ("level_name", Value::String(record.level.name().to_string())),
            ("channel", Value::String(record.channel.clone())),
            (
                "datetime",
                Value::String(record.datetime.format(&self.date_format)),
            ),
            ("extra", Value::Object(extra)),
        ];

        if self.ignore_empty_context_and_extra {
            vars.retain(|(name, value)| {
                let empty = matches!(*name, "context" | "extra") && value.count() == 0;
                if empty {
                    output = output.replace(&format!("%{name}%"), "");
                }
                !empty
            });
        }

        for (name, value) in &vars {
            let placeholder = format!("%{name}%");
            if output.contains(&placeholder) {
                output = output.replace(&placeholder, &self.stringify(value));
            }
        }

        if output.contains('%') {
            output = LEFTOVER_PLACEHOLDERS.replace_all(&output, "").into_owned();
        }

        output
    }
}

/// Monolog's `JsonFormatter`: one JSON document per line.
///
/// ```text
/// {"message":"User logged in.","context":{"id":1},"level":200,"level_name":"INFO","channel":"local","datetime":"2024-01-01T12:00:00.000000+00:00","extra":{}}
/// ```
#[derive(Debug, Clone, Default)]
pub struct JsonFormatter;

impl JsonFormatter {
    /// Create a JSON formatter.
    pub fn new() -> Self {
        Self
    }
}

impl Formatter for JsonFormatter {
    fn format(&self, record: &LogRecord) -> String {
        let section = |data: &Context| {
            if data.is_empty() {
                Value::Object(Map::new())
            } else {
                normalize(&Value::Object(data.clone()))
            }
        };
        let mut document = Map::new();
        document.insert("message".into(), Value::String(record.message.clone()));
        document.insert("context".into(), section(&record.context));
        document.insert("level".into(), Value::from(record.level.value()));
        document.insert(
            "level_name".into(),
            Value::String(record.level.name().into()),
        );
        document.insert("channel".into(), Value::String(record.channel.clone()));
        document.insert(
            "datetime".into(),
            Value::String(record.datetime.format("Y-m-d\\TH:i:s.uP")),
        );
        document.insert("extra".into(), section(&record.extra));
        let mut line = Value::Object(document).to_string();
        line.push('\n');
        line
    }
}

/// Convert a value to a string the way Monolog's normalizer does: `null`
/// and booleans are exported, scalars are cast, and arrays become JSON.
fn convert_to_string(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Number(_) | Value::String(_) => value.to_string_lossy(),
        Value::Array(_) | Value::Object(_) => to_php_json(value),
    }
}

/// Encode a value like PHP's `json_encode` would encode the equivalent
/// array: empty objects are `[]` and integer-keyed objects are lists.
pub(crate) fn to_php_json(value: &Value) -> String {
    normalize(value).to_string()
}

fn normalize(value: &Value) -> Value {
    match value {
        Value::Object(map) if map.is_empty() => Value::Array(Vec::new()),
        Value::Object(map) if is_list(map) => Value::Array(map.values().map(normalize).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), normalize(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        other => other.clone(),
    }
}

fn is_list(map: &Map<String, Value>) -> bool {
    map.keys()
        .enumerate()
        .all(|(index, key)| key.parse::<usize>().ok() == Some(index))
}

/// Turn escaped `\n` / `\r` sequences (not preceded by a backslash) back
/// into real line breaks, so stack traces in the context read naturally.
fn unescape_newlines(string: &str) -> String {
    let chars: Vec<char> = string.chars().collect();
    let mut out = String::with_capacity(string.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\'
            && matches!(chars.get(i + 1), Some('n') | Some('r'))
            && (i == 0 || chars[i - 1] != '\\')
        {
            out.push('\n');
            i += 2;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::Level;
    use crate::record::to_context;
    use illuminate_support::{Carbon, json};

    fn record(message: &str, context: Value) -> LogRecord {
        let mut record = LogRecord::new("local", Level::Info, message, to_context(context));
        record.datetime = Carbon::parse("2024-01-01 12:00:00").unwrap();
        record
    }

    #[test]
    fn laravel_lines_omit_empty_context() {
        let formatter = LineFormatter::laravel();
        assert_eq!(
            formatter.format(&record("User logged in.", json!({}))),
            "[2024-01-01 12:00:00] local.INFO: User logged in.  \n"
        );
        assert_eq!(
            formatter.format(&record(
                "User logged in.",
                json!({"id": 1, "path": "/a/b", "name": "Zoë"})
            )),
            "[2024-01-01 12:00:00] local.INFO: User logged in. {\"id\":1,\"path\":\"/a/b\",\"name\":\"Zoë\"} \n"
        );
    }

    #[test]
    fn monolog_defaults_print_empty_arrays() {
        let formatter = LineFormatter::new();
        assert_eq!(
            formatter.format(&record("Hello", json!(null))),
            "[2024-01-01T12:00:00+00:00] local.INFO: Hello [] []\n"
        );
    }

    #[test]
    fn line_breaks_are_kept_or_flattened() {
        let message = record("first\nsecond", json!({"trace": "#0 a\n#1 b"}));
        assert_eq!(
            LineFormatter::laravel().format(&message),
            "[2024-01-01 12:00:00] local.INFO: first\nsecond {\"trace\":\"#0 a\n#1 b\"} \n"
        );
        assert_eq!(
            LineFormatter::new().format(&message),
            "[2024-01-01T12:00:00+00:00] local.INFO: first second {\"trace\":\"#0 a\\n#1 b\"} []\n"
        );
    }

    #[test]
    fn lists_and_nested_empty_objects_encode_like_php() {
        let formatter = LineFormatter::laravel();
        assert_eq!(
            formatter.format(&record("Items", json!(["a", "b"]))),
            "[2024-01-01 12:00:00] local.INFO: Items [\"a\",\"b\"] \n"
        );
        assert_eq!(
            formatter.format(&record("Nested", json!({"user": {}, "price": 1.0}))),
            "[2024-01-01 12:00:00] local.INFO: Nested {\"user\":[],\"price\":1.0} \n"
        );
    }

    #[test]
    fn custom_formats_support_context_placeholders() {
        let formatter = LineFormatter::new()
            .with_format(
                "%level_name% (%level%) %context.user%: %message% %context% %context.missing%",
            )
            .ignore_empty_context_and_extra(true);
        assert_eq!(
            formatter.format(&record("Hi", json!({"user": "taylor"}))),
            "INFO (200) taylor: Hi  "
        );
        let values = LineFormatter::new().with_format("%context.a%|%context.b%|%context.c%");
        assert_eq!(
            values.format(&record("", json!({"a": null, "b": true, "c": 2.5}))),
            "NULL|true|2.5"
        );
    }

    #[test]
    fn json_lines() {
        let line = JsonFormatter.format(&record("User logged in.", json!({"id": 1})));
        assert_eq!(
            line,
            "{\"message\":\"User logged in.\",\"context\":{\"id\":1},\"level\":200,\"level_name\":\"INFO\",\"channel\":\"local\",\"datetime\":\"2024-01-01T12:00:00.000000+00:00\",\"extra\":{}}\n"
        );
    }

    #[test]
    fn escaped_backslashes_are_left_alone() {
        assert_eq!(
            unescape_newlines(r#"{"a":"x\ny\\nz"}"#),
            "{\"a\":\"x\ny\\\\nz\"}"
        );
    }
}
