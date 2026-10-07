//! Slack's [Block Kit](https://api.slack.com/block-kit): the blocks,
//! elements and composition objects a [`SlackMessage`](super::SlackMessage)
//! is built from.
//!
//! You rarely name these types yourself: the message's block methods hand
//! them to your closures.

pub mod blocks;
pub mod composites;
pub mod elements;

use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Result, Str};

/// Fit text into a field holding `max_length` characters, truncating it
/// with an ellipsis when it is too long.
pub(crate) fn fit(text: &str, max_length: usize) -> Result<String> {
    if text.is_empty() {
        return Err(
            InvalidArgumentException::new("Text must be at least 1 character long.").into(),
        );
    }
    if text.chars().count() <= max_length {
        return Ok(text.to_string());
    }
    let truncated: String = text.chars().take(max_length.saturating_sub(3)).collect();
    Ok(format!("{truncated}..."))
}

/// Ensure a value fits in a field, failing like the Slack channel does.
pub(crate) fn ensure_max_length(value: Option<&str>, max: usize, field: &str) -> Result<()> {
    match value {
        Some(value) if value.chars().count() > max => Err(InvalidArgumentException::new(format!(
            "Maximum length for the {field} field is {max} {}.",
            Str::plural_count("character", max as i64)
        ))
        .into()),
        _ => Ok(()),
    }
}

/// Ensure a block's ID fits Slack's 255 character limit.
pub(crate) fn ensure_block_id(block_id: Option<&str>) -> Result<()> {
    ensure_max_length(block_id, 255, "block_id")
}

/// Helpers for building JSON objects with optional fields.
pub(crate) mod object {
    use illuminate_support::{Map, Value};

    /// Insert a value.
    pub(crate) fn put(object: &mut Map<String, Value>, key: &str, value: impl Into<Value>) {
        object.insert(key.to_string(), value.into());
    }

    /// Insert a value when there is one.
    pub(crate) fn put_some(
        object: &mut Map<String, Value>,
        key: &str,
        value: Option<impl Into<Value>>,
    ) {
        if let Some(value) = value {
            put(object, key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_fit_into_fields() {
        assert_eq!(fit("Hello", 5).unwrap(), "Hello");
        assert_eq!(fit("Hello!", 5).unwrap(), "He...");
        assert_eq!(fit("Hello", 2).unwrap(), "...");
        assert!(fit("", 5).is_err());
    }

    #[test]
    fn lengths_are_validated() {
        assert!(ensure_max_length(None, 1, "value").is_ok());
        assert!(ensure_max_length(Some("a"), 1, "value").is_ok());
        assert_eq!(
            ensure_max_length(Some("ab"), 1, "value")
                .unwrap_err()
                .to_string(),
            "Maximum length for the value field is 1 character."
        );
        assert_eq!(
            ensure_block_id(Some(&"a".repeat(256)))
                .unwrap_err()
                .to_string(),
            "Maximum length for the block_id field is 255 characters."
        );
    }
}
