//! A collection of named [`MessageBag`]s, shared with every view as `$errors`.
//!
//! ```
//! use illuminate_support::{MessageBag, ViewErrorBag};
//!
//! let mut errors = ViewErrorBag::new();
//! errors.put("default", MessageBag::from([("email", "The email field is required.")]));
//! errors.put("login", MessageBag::from([("password", "The password is incorrect.")]));
//!
//! // Calls on the error bag itself forward to the "default" bag...
//! assert!(errors.has("email"));
//! assert_eq!(errors.first("email"), Some("The email field is required."));
//!
//! // ...while named bags are available too.
//! assert_eq!(errors.get_bag("login").first("password"), Some("The password is incorrect."));
//! assert!(errors.get_bag("missing").is_empty());
//! ```

use std::fmt;
use std::ops::Deref;
use std::sync::LazyLock;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::message_bag::MessageBag;

static EMPTY: LazyLock<MessageBag> = LazyLock::new(MessageBag::new);

/// Named message bags. Dereferences to the `default` bag.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ViewErrorBag {
    bags: IndexMap<String, MessageBag>,
}

impl ViewErrorBag {
    /// Create an empty error bag.
    pub fn new() -> Self {
        Self::default()
    }

    /// Determine if a named bag exists.
    pub fn has_bag(&self, key: &str) -> bool {
        self.bags.contains_key(key)
    }

    /// Get a bag by name (an empty bag when it doesn't exist).
    pub fn get_bag(&self, key: &str) -> &MessageBag {
        self.bags.get(key).unwrap_or(&EMPTY)
    }

    /// Get a mutable bag by name, creating it when necessary.
    pub fn get_bag_mut(&mut self, key: &str) -> &mut MessageBag {
        self.bags.entry(key.to_string()).or_default()
    }

    /// Get every bag.
    pub fn get_bags(&self) -> &IndexMap<String, MessageBag> {
        &self.bags
    }

    /// Add a new bag.
    pub fn put(&mut self, key: impl Into<String>, bag: MessageBag) -> &mut Self {
        self.bags.insert(key.into(), bag);
        self
    }

    /// Determine if the default bag has any messages.
    pub fn any(&self) -> bool {
        self.count() > 0
    }

    /// The number of messages in the default bag.
    pub fn count(&self) -> usize {
        self.get_bag("default").count()
    }
}

impl Deref for ViewErrorBag {
    type Target = MessageBag;

    fn deref(&self) -> &MessageBag {
        self.get_bag("default")
    }
}

impl fmt::Display for ViewErrorBag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.get_bag("default"), f)
    }
}

impl From<MessageBag> for ViewErrorBag {
    /// Wrap a bag as the "default" bag.
    fn from(bag: MessageBag) -> Self {
        let mut errors = ViewErrorBag::new();
        errors.put("default", bag);
        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_manages_named_bags() {
        let mut errors = ViewErrorBag::new();
        assert!(!errors.has_bag("default"));
        assert!(!errors.any());
        assert_eq!(errors.count(), 0);
        errors.put("default", MessageBag::from([("name", "Required."), ("email", "Invalid.")]));
        errors.get_bag_mut("login").add("password", "Wrong.");
        assert!(errors.has_bag("default") && errors.has_bag("login"));
        assert!(errors.any());
        assert_eq!(errors.count(), 2);
        assert_eq!(errors.get("name"), vec!["Required."]);
        assert_eq!(errors.get_bags().len(), 2);
        assert_eq!(errors.to_string(), r#"{"name":["Required."],"email":["Invalid."]}"#);
    }

    #[test]
    fn it_serializes_bags_by_name() {
        let errors = ViewErrorBag::from(MessageBag::from([("email", "Required.")]));
        let json = serde_json::to_string(&errors).unwrap();
        assert_eq!(json, r#"{"default":{"email":["Required."]}}"#);
        let back: ViewErrorBag = serde_json::from_str(&json).unwrap();
        assert_eq!(back, errors);
    }
}
