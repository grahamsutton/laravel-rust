//! Processors add to, or rewrite, records before they're handled.

use illuminate_support::{Value, ValueExt};

use crate::formatter::to_php_json;
use crate::record::LogRecord;

/// A log record processor.
///
/// Closures taking and returning a [`LogRecord`] are processors too:
///
/// ```
/// use illuminate_log::{Monolog, LogRecord};
/// use illuminate_support::json;
///
/// let logger = Monolog::new("local").with_processor(|mut record: LogRecord| {
///     record.extra.insert("hostname".into(), json!("web-1"));
///     record
/// });
/// ```
pub trait Processor: Send + Sync {
    /// Process the record.
    fn process(&self, record: LogRecord) -> LogRecord;
}

impl<F> Processor for F
where
    F: Fn(LogRecord) -> LogRecord + Send + Sync,
{
    fn process(&self, record: LogRecord) -> LogRecord {
        self(record)
    }
}

/// Replaces `{placeholders}` in the message with values from the context
/// (PSR-3 interpolation). Laravel enables it with `replace_placeholders`.
///
/// ```
/// use illuminate_log::{Level, LogRecord, Processor, PsrLogMessageProcessor, to_context};
/// use illuminate_support::json;
///
/// let record = LogRecord::new("local", Level::Info, "User {id} failed to login.", to_context(json!({"id": 7})));
///
/// assert_eq!(
///     PsrLogMessageProcessor::new().process(record).message,
///     "User 7 failed to login.",
/// );
/// ```
#[derive(Debug, Clone, Default)]
pub struct PsrLogMessageProcessor {
    remove_used_context_fields: bool,
}

impl PsrLogMessageProcessor {
    /// Create the processor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Remove the context values that were interpolated into the message.
    pub fn remove_used_context_fields(mut self, remove: bool) -> Self {
        self.remove_used_context_fields = remove;
        self
    }
}

impl Processor for PsrLogMessageProcessor {
    fn process(&self, mut record: LogRecord) -> LogRecord {
        if !record.message.contains('{') {
            return record;
        }
        let mut replacements: Vec<(String, String)> = Vec::new();
        let mut used = Vec::new();
        for (key, value) in &record.context {
            let placeholder = format!("{{{key}}}");
            if !record.message.contains(&placeholder) {
                continue;
            }
            let replacement = match value {
                Value::Array(_) | Value::Object(_) => format!("array{}", to_php_json(value)),
                scalar => scalar.to_string_lossy(),
            };
            replacements.push((placeholder, replacement));
            used.push(key.clone());
        }
        record.message = strtr(&record.message, &replacements);
        if self.remove_used_context_fields {
            for key in used {
                record.context.remove(&key);
            }
        }
        record
    }
}

/// PHP's `strtr` with an array: at each position the longest matching key
/// wins, and replaced text is never scanned again.
pub(crate) fn strtr(subject: &str, pairs: &[(String, String)]) -> String {
    let mut pairs: Vec<&(String, String)> =
        pairs.iter().filter(|(from, _)| !from.is_empty()).collect();
    if pairs.is_empty() {
        return subject.to_string();
    }
    pairs.sort_by_key(|pair| std::cmp::Reverse(pair.0.len()));
    let mut out = String::with_capacity(subject.len());
    let mut rest = subject;
    'outer: while !rest.is_empty() {
        for (from, to) in &pairs {
            if rest.starts_with(from.as_str()) {
                out.push_str(to);
                rest = &rest[from.len()..];
                continue 'outer;
            }
        }
        let next = rest.chars().next().map(char::len_utf8).unwrap_or(1);
        out.push_str(&rest[..next]);
        rest = &rest[next..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::Level;
    use crate::record::to_context;
    use illuminate_support::json;

    fn process(message: &str, context: Value) -> LogRecord {
        PsrLogMessageProcessor::new().process(LogRecord::new(
            "local",
            Level::Info,
            message,
            to_context(context),
        ))
    }

    #[test]
    fn placeholders_are_interpolated_like_psr3() {
        let record = process(
            "{name} ({id}) {active} {missing} {nothing} {tags} {price}",
            json!({"name": "Taylor", "id": 1, "active": true, "nothing": null, "tags": ["a"], "price": 2.0}),
        );
        assert_eq!(record.message, "Taylor (1) 1 {missing}  array[\"a\"] 2");
        assert_eq!(record.context.len(), 6);
    }

    #[test]
    fn used_fields_may_be_removed() {
        let record = PsrLogMessageProcessor::new()
            .remove_used_context_fields(true)
            .process(LogRecord::new(
                "local",
                Level::Info,
                "Hi {name}",
                to_context(json!({"name": "Taylor", "id": 1})),
            ));
        assert_eq!(record.message, "Hi Taylor");
        assert_eq!(record.context, to_context(json!({"id": 1})));
    }

    #[test]
    fn strtr_prefers_longer_keys_and_never_rescans() {
        let pairs = vec![
            (":name".to_string(), "Taylor :name".to_string()),
            (":names".to_string(), "everyone".to_string()),
        ];
        assert_eq!(
            strtr("Hi :name and :names, :é", &pairs),
            "Hi Taylor :name and everyone, :é"
        );
    }
}
