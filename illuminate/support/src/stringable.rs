//! Fluent strings.
//!
//! ```
//! use illuminate_support::Str;
//!
//! let slug = Str::of("  Laravel Framework  ").trim().slug().to_string();
//! assert_eq!(slug, "laravel-framework");
//! ```

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::str::Str;
use crate::traits::{Conditionable, Tappable};

/// A fluent wrapper around a string.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Stringable {
    value: String,
}

macro_rules! fluent {
    ($( $(#[$meta:meta])* $name:ident => $func:expr ),* $(,)?) => {
        $(
            $(#[$meta])*
            pub fn $name(self) -> Self {
                Self::new(($func)(self.value.as_str()))
            }
        )*
    };
}

impl Stringable {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }

    /// Get the raw string value.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Get the underlying string.
    pub fn into_string(self) -> String {
        self.value
    }

    fluent! {
        /// Convert to camel case.
        camel => Str::camel,
        /// Convert to studly case.
        studly => Str::studly,
        /// Convert to snake case.
        snake => Str::snake,
        /// Convert to kebab case.
        kebab => Str::kebab,
        /// Convert to title case.
        title => Str::title,
        /// Convert to headline case.
        headline => Str::headline,
        /// Convert to lower case.
        lower => Str::lower,
        /// Convert to upper case.
        upper => Str::upper,
        /// Uppercase the first character.
        ucfirst => Str::ucfirst,
        /// Lowercase the first character.
        lcfirst => Str::lcfirst,
        /// Get the plural form.
        plural => Str::plural,
        /// Get the singular form.
        singular => Str::singular,
        /// Generate a URL friendly slug.
        slug => Str::slug,
        /// Remove extra whitespace.
        squish => Str::squish,
        /// Transliterate to ASCII.
        ascii => Str::ascii,
        /// Reverse the string.
        reverse => Str::reverse,
        /// Get the class basename.
        class_basename => Str::class_basename,
    }

    /// Trim whitespace from both ends.
    pub fn trim(self) -> Self {
        Self::new(self.value.trim())
    }

    /// Trim the given characters from both ends.
    pub fn trim_chars(self, chars: &str) -> Self {
        Self::new(self.value.trim_matches(|c| chars.contains(c)))
    }

    pub fn ltrim(self) -> Self {
        Self::new(self.value.trim_start())
    }

    pub fn rtrim(self) -> Self {
        Self::new(self.value.trim_end())
    }

    pub fn after(self, search: &str) -> Self {
        Self::new(Str::after(&self.value, search))
    }

    pub fn after_last(self, search: &str) -> Self {
        Self::new(Str::after_last(&self.value, search))
    }

    pub fn before(self, search: &str) -> Self {
        Self::new(Str::before(&self.value, search))
    }

    pub fn before_last(self, search: &str) -> Self {
        Self::new(Str::before_last(&self.value, search))
    }

    pub fn between(self, from: &str, to: &str) -> Self {
        Self::new(Str::between(&self.value, from, to))
    }

    pub fn append(mut self, value: &str) -> Self {
        self.value.push_str(value);
        self
    }

    pub fn prepend(self, value: &str) -> Self {
        Self::new(format!("{value}{}", self.value))
    }

    pub fn finish(self, cap: &str) -> Self {
        Self::new(Str::finish(&self.value, cap))
    }

    pub fn start(self, prefix: &str) -> Self {
        Self::new(Str::start(&self.value, prefix))
    }

    pub fn limit(self, limit: usize) -> Self {
        Self::new(Str::limit(&self.value, limit))
    }

    pub fn words(self, words: usize) -> Self {
        Self::new(Str::words(&self.value, words))
    }

    pub fn replace(self, search: &str, replace: &str) -> Self {
        Self::new(self.value.replace(search, replace))
    }

    pub fn replace_first(self, search: &str, replace: &str) -> Self {
        Self::new(Str::replace_first(search, replace, &self.value))
    }

    pub fn replace_last(self, search: &str, replace: &str) -> Self {
        Self::new(Str::replace_last(search, replace, &self.value))
    }

    pub fn remove(self, search: &str) -> Self {
        Self::new(Str::remove(search, &self.value))
    }

    pub fn substr(self, start: isize, length: Option<isize>) -> Self {
        Self::new(Str::substr(&self.value, start, length))
    }

    pub fn mask(self, character: char, index: isize, length: Option<usize>) -> Self {
        Self::new(Str::mask(&self.value, character, index, length))
    }

    pub fn pad_both(self, length: usize, pad: &str) -> Self {
        Self::new(Str::pad_both(&self.value, length, pad))
    }

    pub fn pad_left(self, length: usize, pad: &str) -> Self {
        Self::new(Str::pad_left(&self.value, length, pad))
    }

    pub fn pad_right(self, length: usize, pad: &str) -> Self {
        Self::new(Str::pad_right(&self.value, length, pad))
    }

    pub fn repeat(self, times: usize) -> Self {
        Self::new(self.value.repeat(times))
    }

    pub fn wrap(self, before: &str, after: &str) -> Self {
        Self::new(Str::wrap(&self.value, before, after))
    }

    /// Split the string into a collection of pieces.
    pub fn explode(&self, delimiter: &str) -> crate::Collection<String> {
        crate::Collection::make(self.value.split(delimiter).map(String::from))
    }

    pub fn contains(&self, needle: &str) -> bool {
        Str::contains(&self.value, needle)
    }

    pub fn starts_with(&self, needle: &str) -> bool {
        Str::starts_with(&self.value, needle)
    }

    pub fn ends_with(&self, needle: &str) -> bool {
        Str::ends_with(&self.value, needle)
    }

    pub fn is(&self, pattern: &str) -> bool {
        Str::is(pattern, &self.value)
    }

    pub fn exactly(&self, value: &str) -> bool {
        self.value == value
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    pub fn is_not_empty(&self) -> bool {
        !self.value.is_empty()
    }

    pub fn length(&self) -> usize {
        Str::length(&self.value)
    }

    /// Call the given callback with the string and return the result.
    pub fn pipe(self, callback: impl FnOnce(String) -> String) -> Self {
        Self::new(callback(self.value))
    }

    /// Convert the string into an integer.
    pub fn to_integer(&self) -> i64 {
        self.value.trim().parse().unwrap_or(0)
    }

    /// Convert the string into a float.
    pub fn to_float(&self) -> f64 {
        self.value.trim().parse().unwrap_or(0.0)
    }

    /// Interpret the string as a boolean ("1", "true", "on", "yes").
    pub fn to_boolean(&self) -> bool {
        matches!(
            self.value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        )
    }
}

impl Conditionable for Stringable {}
impl Tappable for Stringable {}

impl fmt::Display for Stringable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.value)
    }
}

impl From<Stringable> for String {
    fn from(s: Stringable) -> Self {
        s.value
    }
}

impl From<&str> for Stringable {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for Stringable {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl AsRef<str> for Stringable {
    fn as_ref(&self) -> &str {
        &self.value
    }
}

impl PartialEq<str> for Stringable {
    fn eq(&self, other: &str) -> bool {
        self.value == other
    }
}

impl PartialEq<&str> for Stringable {
    fn eq(&self, other: &&str) -> bool {
        self.value == *other
    }
}

impl From<Stringable> for serde_json::Value {
    fn from(s: Stringable) -> Self {
        serde_json::Value::String(s.value)
    }
}
