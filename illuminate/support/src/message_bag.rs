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

    /// Merge another bag's messages into this one (alias of `merge`).
    pub fn add_message_bag(&mut self, other: &MessageBag) -> &mut Self {
        self.merge(other)
    }

    /// Determine if messages exist for all of the given keys.
    pub fn has_all(&self, keys: &[&str]) -> bool {
        !keys.is_empty() && keys.iter().all(|k| self.has(k))
    }

    /// Get the first message for the given key, formatted. `:message` and
    /// `:key` are replaced in the format.
    ///
    /// ```
    /// use illuminate_support::MessageBag;
    ///
    /// let errors = MessageBag::from([("email", "The email field is required.")]);
    /// assert_eq!(
    ///     errors.first_with_format("email", "<p>:message</p>").as_deref(),
    ///     Some("<p>The email field is required.</p>")
    /// );
    /// ```
    pub fn first_with_format(&self, key: &str, format: &str) -> Option<String> {
        self.get_with_format(key, format).into_iter().next()
    }

    /// Get all of the messages for the given key, formatted.
    pub fn get_with_format(&self, key: &str, format: &str) -> Vec<String> {
        if let Some(messages) = self.messages.get(key) {
            return messages.iter().map(|m| apply_format(m, format, key)).collect();
        }
        if key.contains('*') {
            return self
                .messages
                .iter()
                .filter(|(k, _)| Str::is(key, k))
                .flat_map(|(k, m)| m.iter().map(move |message| apply_format(message, format, k)))
                .collect();
        }
        Vec::new()
    }

    /// Get every message in the bag, formatted.
    pub fn all_with_format(&self, format: &str) -> Vec<String> {
        self.messages
            .iter()
            .flat_map(|(k, m)| m.iter().map(move |message| apply_format(message, format, k)))
            .collect()
    }

    /// Get the messages matching a wildcard key, keyed by their own keys.
    ///
    /// ```
    /// use illuminate_support::MessageBag;
    ///
    /// let errors = MessageBag::from([("email.0", "Invalid."), ("email.1", "Taken."), ("name", "Required.")]);
    /// let matching = errors.get_matching("email.*");
    /// assert_eq!(matching.keys().collect::<Vec<_>>(), vec!["email.0", "email.1"]);
    /// ```
    pub fn get_matching(&self, pattern: &str) -> IndexMap<String, Vec<String>> {
        self.messages
            .iter()
            .filter(|(k, _)| Str::is(pattern, k))
            .map(|(k, m)| (k.clone(), m.clone()))
            .collect()
    }

    /// Get every unique message in the bag.
    ///
    /// ```
    /// use illuminate_support::MessageBag;
    ///
    /// let errors = MessageBag::from([("a", "Required."), ("b", "Required."), ("c", "Invalid.")]);
    /// assert_eq!(errors.unique(), vec!["Required.", "Invalid."]);
    /// ```
    pub fn unique(&self) -> Vec<&str> {
        let mut unique: Vec<&str> = Vec::new();
        for message in self.all() {
            if !unique.contains(&message) {
                unique.push(message);
            }
        }
        unique
    }

    /// Alias of `messages`.
    pub fn get_messages(&self) -> &IndexMap<String, Vec<String>> {
        &self.messages
    }

    /// Get the bag itself (for parity with Laravel's `MessageProvider`).
    pub fn get_message_bag(&self) -> &MessageBag {
        self
    }

    /// Convert the bag to its JSON representation.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.messages).unwrap_or_default()
    }

    /// Convert the bag to pretty-printed JSON.
    pub fn to_pretty_json(&self) -> String {
        serde_json::to_string_pretty(&self.messages).unwrap_or_default()
    }

    /// Run the callback over each message, replacing it with the result.
    pub fn transform(&mut self, mut callback: impl FnMut(&str, &str) -> String) -> &mut Self {
        for (key, messages) in self.messages.iter_mut() {
            for message in messages.iter_mut() {
                *message = callback(message, key);
            }
        }
        self
    }
}

fn apply_format(message: &str, format: &str, key: &str) -> String {
    format.replace(":message", message).replace(":key", key)
}

impl From<IndexMap<String, Vec<String>>> for MessageBag {
    fn from(messages: IndexMap<String, Vec<String>>) -> Self {
        MessageBag::from_map(messages)
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

impl From<crate::Value> for MessageBag {
    /// `{"email": "Invalid."}` or `{"email": ["Invalid.", "Taken."]}`; a
    /// lone string is stored under the `0` key.
    ///
    /// ```
    /// use illuminate_support::{MessageBag, json};
    ///
    /// let bag = MessageBag::from(json!({"email": "Invalid.", "name": ["Too short.", "Taken."]}));
    /// assert_eq!(bag.first("email"), Some("Invalid."));
    /// assert_eq!(bag.get("name").len(), 2);
    /// ```
    fn from(messages: crate::Value) -> Self {
        use crate::ValueExt;

        let mut bag = MessageBag::new();
        match messages {
            crate::Value::Object(map) => {
                for (key, value) in map {
                    match value {
                        crate::Value::Array(items) => {
                            for item in items {
                                bag.add(key.clone(), item.to_string_lossy());
                            }
                        }
                        crate::Value::Null => {}
                        other => {
                            bag.add(key, other.to_string_lossy());
                        }
                    }
                }
            }
            crate::Value::Array(items) => {
                for item in items {
                    bag.add("0", item.to_string_lossy());
                }
            }
            crate::Value::Null => {}
            other => {
                bag.add("0", other.to_string_lossy());
            }
        }
        bag
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_adds_and_reads_messages() {
        let mut bag = MessageBag::new();
        bag.add("email", "Required.").add("email", "Required.").add("email", "Invalid.");
        bag.add_if(false, "name", "Never added.");
        assert_eq!(bag.get("email"), vec!["Required.", "Invalid."]);
        assert_eq!(bag.count(), 2);
        assert!(bag.missing("name"));
        assert!(bag.has_all(&["email"]));
        assert!(!bag.has_all(&["email", "name"]));
        assert!(!bag.has_all(&[]));
        assert_eq!(bag.first_any(), Some("Required."));
        assert_eq!(bag.to_json(), r#"{"email":["Required.","Invalid."]}"#);
    }

    #[test]
    fn it_formats_messages() {
        let bag = MessageBag::from([("email", "Required."), ("name", "Too short.")]);
        assert_eq!(bag.first_with_format("email", ":key - :message").as_deref(), Some("email - Required."));
        assert_eq!(bag.get_with_format("name", "<li>:message</li>"), vec!["<li>Too short.</li>"]);
        assert_eq!(bag.all_with_format("[:message]"), vec!["[Required.]", "[Too short.]"]);
        assert_eq!(bag.get_with_format("missing", ":message"), Vec::<String>::new());
    }

    #[test]
    fn it_merges_and_transforms() {
        let mut bag = MessageBag::from([("email", "Required.")]);
        let other = MessageBag::from([("email", "Required."), ("name", "Required.")]);
        bag.add_message_bag(&other);
        assert_eq!(bag.count(), 2);
        assert_eq!(bag.unique(), vec!["Required."]);
        bag.transform(|message, key| format!("{key}: {message}"));
        assert_eq!(bag.first("name"), Some("name: Required."));
        assert_eq!(bag.get_message_bag().keys(), vec!["email", "name"]);
        let wildcard = MessageBag::from([("items.0.name", "Required."), ("items.1.name", "Invalid.")]);
        assert_eq!(wildcard.get("items.*.name"), vec!["Required.", "Invalid."]);
        assert!(wildcard.has("items.*"));
    }
}
