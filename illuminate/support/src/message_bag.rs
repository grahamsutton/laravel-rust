//! A bag of messages keyed by name — most often, validation errors.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::str::Str;

/// A collection of messages, keyed by attribute.
///
/// ```
/// use illuminate_support::MessageBag;
///
/// let mut errors = MessageBag::new();
/// errors.add("email", "The email field is required.");
///
/// assert!(errors.has("email"));
/// assert_eq!(errors.first("email"), Some("The email field is required."));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageBag {
    messages: IndexMap<String, Vec<String>>,
}

impl MessageBag {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a bag from a map of messages.
    pub fn from_map(messages: IndexMap<String, Vec<String>>) -> Self {
        Self { messages }
    }

    /// Add a message to the bag (duplicates for the same key are ignored).
    pub fn add(&mut self, key: impl Into<String>, message: impl Into<String>) -> &mut Self {
        let message = message.into();
        let list = self.messages.entry(key.into()).or_default();
        if !list.contains(&message) {
            list.push(message);
        }
        self
    }

    /// Add a message to the bag if the condition is true.
    pub fn add_if(&mut self, condition: bool, key: impl Into<String>, message: impl Into<String>) -> &mut Self {
        if condition {
            self.add(key, message);
        }
        self
    }

    /// Merge another bag into this one.
    pub fn merge(&mut self, other: &MessageBag) -> &mut Self {
        for (key, messages) in &other.messages {
            for message in messages {
                self.add(key.clone(), message.clone());
            }
        }
        self
    }

    /// Determine if messages exist for the given key (supports `*` wildcards).
    pub fn has(&self, key: &str) -> bool {
        !self.get(key).is_empty()
    }

    /// Determine if messages exist for any of the given keys.
    pub fn has_any(&self, keys: &[&str]) -> bool {
        keys.iter().any(|k| self.has(k))
    }

    /// Determine if there are no messages for the given key.
    pub fn missing(&self, key: &str) -> bool {
        !self.has(key)
    }

    /// Get the first message for the given key.
    pub fn first(&self, key: &str) -> Option<&str> {
        self.get(key).into_iter().next()
    }

    /// Get the very first message in the bag.
    pub fn first_any(&self) -> Option<&str> {
        self.messages
            .values()
            .flat_map(|m| m.iter())
            .map(String::as_str)
            .next()
    }

    /// Get all of the messages for the given key (supports `*` wildcards).
    pub fn get(&self, key: &str) -> Vec<&str> {
        if let Some(messages) = self.messages.get(key) {
            return messages.iter().map(String::as_str).collect();
        }
        if key.contains('*') {
            return self
                .messages
                .iter()
                .filter(|(k, _)| Str::is(key, k))
                .flat_map(|(_, m)| m.iter().map(String::as_str))
                .collect();
        }
        Vec::new()
    }

    /// Get every message in the bag, flattened.
    pub fn all(&self) -> Vec<&str> {
        self.messages
            .values()
            .flat_map(|m| m.iter())
            .map(String::as_str)
            .collect()
    }

    /// Get the raw messages, keyed by attribute.
    pub fn messages(&self) -> &IndexMap<String, Vec<String>> {
        &self.messages
    }

    /// Alias of `messages`.
    pub fn to_array(&self) -> &IndexMap<String, Vec<String>> {
        &self.messages
    }

    /// Get the keys present in the bag.
    pub fn keys(&self) -> Vec<&str> {
        self.messages.keys().map(String::as_str).collect()
    }

    /// Remove all messages for the given key.
    pub fn forget(&mut self, key: &str) -> &mut Self {
        self.messages.shift_remove(key);
        self
    }

    /// The total number of messages in the bag.
    pub fn count(&self) -> usize {
        self.messages.values().map(Vec::len).sum()
    }

    pub fn any(&self) -> bool {
        self.count() > 0
    }

    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    pub fn is_not_empty(&self) -> bool {
        self.any()
    }
}

impl std::fmt::Display for MessageBag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&serde_json::to_string(&self.messages).unwrap_or_default())
    }
}

impl From<&str> for MessageBag {
    /// A lone message is stored under the `0` key, like Laravel.
    fn from(message: &str) -> Self {
        let mut bag = MessageBag::new();
        bag.add("0", message);
        bag
    }
}

impl From<String> for MessageBag {
    fn from(message: String) -> Self {
        MessageBag::from(message.as_str())
    }
}

impl<K: Into<String>, V: Into<String>, const N: usize> From<[(K, V); N]> for MessageBag {
    fn from(messages: [(K, V); N]) -> Self {
        let mut bag = MessageBag::new();
        for (key, message) in messages {
            bag.add(key, message);
        }
        bag
    }
}

impl<K: Into<String>, V: Into<String>> From<Vec<(K, V)>> for MessageBag {
    fn from(messages: Vec<(K, V)>) -> Self {
        let mut bag = MessageBag::new();
        for (key, message) in messages {
            bag.add(key, message);
        }
        bag
    }
}
