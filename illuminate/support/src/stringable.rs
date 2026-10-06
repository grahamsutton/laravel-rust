//! Fluent strings.
//!
//! ```
//! use illuminate_support::Str;
//!
//! let slug = Str::of("  Laravel Framework  ").trim().slug().to_string();
//! assert_eq!(slug, "laravel-framework");
//!
//! let title = Str::of("tony stark").when_contains("tony", |s| s.title());
//! assert_eq!(title, "Tony Stark");
//! ```

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;

use regex::Captures;
use serde::{Deserialize, Serialize};

use crate::carbon::Carbon;
use crate::collection::Collection;
use crate::html_string::HtmlString;
use crate::str::{ExcerptOptions, Str};
use crate::traits::{Conditionable, Tappable};
use crate::uri::Uri;

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

macro_rules! fluent_when {
    ($( $(#[$meta:meta])* $name:ident($($arg:ident: $ty:ty),*) => $check:expr ),* $(,)?) => {
        $(
            $(#[$meta])*
            pub fn $name(self, $($arg: $ty,)* callback: impl FnOnce(Self) -> Self) -> Self {
                #[allow(clippy::redundant_closure_call)]
                let passes = ($check)(&self $(, $arg)*);
                if passes { callback(self) } else { self }
            }
        )*
    };
}

impl Stringable {
    /// Create a new fluent string.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }

    /// Get the raw string value.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Get the raw string value as a `&str`.
    pub fn as_str(&self) -> &str {
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
        /// Convert to Pascal case (an alias of `studly`).
        pascal => Str::pascal,
        /// Convert to snake case.
        snake => Str::snake,
        /// Convert to kebab case.
        kebab => Str::kebab,
        /// Convert to title case.
        title => Str::title,
        /// Convert to headline case.
        headline => Str::headline,
        /// Convert to APA-style title case.
        apa => Str::apa,
        /// Convert to lower case.
        lower => Str::lower,
        /// Convert to upper case.
        upper => Str::upper,
        /// Uppercase the first character.
        ucfirst => Str::ucfirst,
        /// Lowercase the first character.
        lcfirst => Str::lcfirst,
        /// Uppercase the first character of each word.
        ucwords => Str::ucwords,
        /// Get the plural form.
        plural => Str::plural,
        /// Pluralize the last word of a studly caps case string.
        plural_studly => Str::plural_studly,
        /// Pluralize the last word of a Pascal case string.
        plural_pascal => Str::plural_pascal,
        /// Get the singular form.
        singular => Str::singular,
        /// Generate a URL friendly slug.
        slug => Str::slug,
        /// Remove extra whitespace.
        squish => Str::squish,
        /// Transliterate to ASCII.
        ascii => Str::ascii,
        /// Transliterate to ASCII, using `?` for unknown characters.
        transliterate => Str::transliterate,
        /// Reverse the string.
        reverse => Str::reverse,
        /// Get the class basename.
        class_basename => Str::class_basename,
        /// Get the initials of each word.
        initials => Str::initials,
        /// Remove all non-numeric characters.
        numbers => Str::numbers,
        /// Replace consecutive spaces with a single space.
        deduplicate => Str::deduplicate,
        /// Strip HTML tags.
        strip_tags => Str::strip_tags,
        /// Convert the string to Base64 encoding.
        to_base64 => Str::to_base64,
    }

    /// Trim whitespace (including invisible characters) from both ends.
    pub fn trim(self) -> Self {
        Self::new(Str::trim(&self.value))
    }

    /// Trim the given characters from both ends.
    pub fn trim_chars(self, chars: &str) -> Self {
        Self::new(Str::trim_chars(&self.value, chars))
    }

    /// Trim whitespace from the start.
    pub fn ltrim(self) -> Self {
        Self::new(Str::ltrim(&self.value))
    }

    /// Trim the given characters from the start.
    pub fn ltrim_chars(self, chars: &str) -> Self {
        Self::new(Str::ltrim_chars(&self.value, chars))
    }

    /// Trim whitespace from the end.
    pub fn rtrim(self) -> Self {
        Self::new(Str::rtrim(&self.value))
    }

    /// Trim the given characters from the end.
    pub fn rtrim_chars(self, chars: &str) -> Self {
        Self::new(Str::rtrim_chars(&self.value, chars))
    }

    /// Return the remainder of the string after the first occurrence of a value.
    pub fn after(self, search: &str) -> Self {
        Self::new(Str::after(&self.value, search))
    }

    /// Return the remainder of the string after the last occurrence of a value.
    pub fn after_last(self, search: &str) -> Self {
        Self::new(Str::after_last(&self.value, search))
    }

    /// Get the portion of the string before the first occurrence of a value.
    pub fn before(self, search: &str) -> Self {
        Self::new(Str::before(&self.value, search))
    }

    /// Get the portion of the string before the last occurrence of a value.
    pub fn before_last(self, search: &str) -> Self {
        Self::new(Str::before_last(&self.value, search))
    }

    /// Get the portion of the string between two values.
    pub fn between(self, from: &str, to: &str) -> Self {
        Self::new(Str::between(&self.value, from, to))
    }

    /// Get the smallest possible portion of the string between two values.
    pub fn between_first(self, from: &str, to: &str) -> Self {
        Self::new(Str::between_first(&self.value, from, to))
    }

    /// Append the given value to the string.
    pub fn append(mut self, value: &str) -> Self {
        self.value.push_str(value);
        self
    }

    /// Append each of the given values to the string.
    pub fn append_many(mut self, values: &[&str]) -> Self {
        for value in values {
            self.value.push_str(value);
        }
        self
    }

    /// Append a new line to the string.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::of("Laravel").new_line().append("Framework"), "Laravel\nFramework");
    /// ```
    pub fn new_line(self) -> Self {
        self.new_lines(1)
    }

    /// Append the given number of new lines to the string.
    pub fn new_lines(self, count: usize) -> Self {
        let lines = "\n".repeat(count);
        self.append(&lines)
    }

    /// Prepend the given value to the string.
    pub fn prepend(self, value: &str) -> Self {
        Self::new(format!("{value}{}", self.value))
    }

    /// Get the trailing name component of the path.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::of("/foo/bar/baz").basename(), "baz");
    /// assert_eq!(Str::of("/foo/bar/baz.jpg").basename_without(".jpg"), "baz");
    /// ```
    pub fn basename(self) -> Self {
        self.basename_without("")
    }

    /// Get the trailing name component of the path, removing the given suffix.
    pub fn basename_without(self, suffix: &str) -> Self {
        let trimmed = self.value.trim_end_matches('/');
        let base = trimmed.rsplit('/').next().unwrap_or(trimmed);
        let base = match base.strip_suffix(suffix) {
            Some(stripped) if !suffix.is_empty() && !stripped.is_empty() => stripped,
            _ => base,
        };
        Self::new(base)
    }

    /// Get the extension of the path (`"jpg"` for `photo.jpg`, empty when there is none).
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::of("/foo/bar/baz.jpg").extension(), "jpg");
    /// assert_eq!(Str::of("archive.tar.gz").extension(), "gz");
    /// assert_eq!(Str::of("README").extension(), "");
    /// ```
    pub fn extension(self) -> Self {
        let base = self.basename();
        match base.value.rsplit_once('.') {
            Some((_, extension)) => Self::new(extension),
            None => Self::new(""),
        }
    }

    /// Get the parent directory's path.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::of("/foo/bar/baz").dirname(), "/foo/bar");
    /// assert_eq!(Str::of("/foo/bar/baz").dirname_levels(2), "/foo");
    /// ```
    pub fn dirname(self) -> Self {
        self.dirname_levels(1)
    }

    /// Get the path of the directory `levels` levels up.
    pub fn dirname_levels(self, levels: usize) -> Self {
        let mut path = self.value;
        for _ in 0..levels {
            path = php_dirname(&path);
        }
        Self::new(path)
    }

    /// Get the character at the given index.
    pub fn char_at(&self, index: isize) -> Option<char> {
        Str::char_at(&self.value, index)
    }

    /// Remove the given string from the start, if present.
    pub fn chop_start(self, needle: &str) -> Self {
        Self::new(Str::chop_start(&self.value, needle))
    }

    /// Remove the first matching needle from the start.
    pub fn chop_start_any(self, needles: &[&str]) -> Self {
        Self::new(Str::chop_start_any(&self.value, needles))
    }

    /// Remove the given string from the end, if present.
    pub fn chop_end(self, needle: &str) -> Self {
        Self::new(Str::chop_end(&self.value, needle))
    }

    /// Remove the first matching needle from the end.
    pub fn chop_end_any(self, needles: &[&str]) -> Self {
        Self::new(Str::chop_end_any(&self.value, needles))
    }

    /// Cap the string with a single instance of a given value.
    pub fn finish(self, cap: &str) -> Self {
        Self::new(Str::finish(&self.value, cap))
    }

    /// Begin the string with a single instance of a given value.
    pub fn start(self, prefix: &str) -> Self {
        Self::new(Str::start(&self.value, prefix))
    }

    /// Limit the number of characters in the string.
    pub fn limit(self, limit: usize) -> Self {
        Self::new(Str::limit(&self.value, limit))
    }

    /// Limit the number of characters in the string, with a custom ending.
    pub fn limit_with(self, limit: usize, end: &str) -> Self {
        Self::new(Str::limit_with(&self.value, limit, end))
    }

    /// Limit the number of characters without cutting words in half.
    pub fn limit_preserving_words(self, limit: usize, end: &str) -> Self {
        Self::new(Str::limit_preserving_words(&self.value, limit, end))
    }

    /// Limit the number of words in the string.
    pub fn words(self, words: usize) -> Self {
        Self::new(Str::words(&self.value, words))
    }

    /// Limit the number of words in the string, with a custom ending.
    pub fn words_with(self, words: usize, end: &str) -> Self {
        Self::new(Str::words_with(&self.value, words, end))
    }

    /// Count the number of words in the string.
    pub fn word_count(&self) -> usize {
        Str::word_count(&self.value)
    }

    /// Wrap the string to a given number of characters.
    pub fn word_wrap(self, characters: usize, break_with: &str, cut_long_words: bool) -> Self {
        Self::new(Str::word_wrap(&self.value, characters, break_with, cut_long_words))
    }

    /// Replace all occurrences of the search string.
    pub fn replace(self, search: &str, replace: &str) -> Self {
        Self::new(Str::replace(search, replace, &self.value))
    }

    /// Replace all occurrences of the search string, ignoring case.
    pub fn replace_ignore_case(self, search: &str, replace: &str) -> Self {
        Self::new(Str::replace_ignore_case(search, replace, &self.value))
    }

    /// Replace each of the search strings with the replacement.
    pub fn replace_many(self, searches: &[&str], replace: &str) -> Self {
        Self::new(Str::replace_many(searches, replace, &self.value))
    }

    /// Replace a given value sequentially with an array of values.
    pub fn replace_array(self, search: &str, replace: &[&str]) -> Self {
        Self::new(Str::replace_array(search, replace, &self.value))
    }

    /// Replace the first occurrence of a given value.
    pub fn replace_first(self, search: &str, replace: &str) -> Self {
        Self::new(Str::replace_first(search, replace, &self.value))
    }

    /// Replace the first occurrence of a value only if it is at the start.
    pub fn replace_start(self, search: &str, replace: &str) -> Self {
        Self::new(Str::replace_start(search, replace, &self.value))
    }

    /// Replace the last occurrence of a given value.
    pub fn replace_last(self, search: &str, replace: &str) -> Self {
        Self::new(Str::replace_last(search, replace, &self.value))
    }

    /// Replace the last occurrence of a value only if it is at the end.
    pub fn replace_end(self, search: &str, replace: &str) -> Self {
        Self::new(Str::replace_end(search, replace, &self.value))
    }

    /// Replace the patterns matching the given regular expression.
    pub fn replace_matches(self, pattern: &str, replace: &str) -> Self {
        Self::new(Str::replace_matches(pattern, replace, &self.value))
    }

    /// Replace the matches of the given regular expression using a callback.
    pub fn replace_matches_using(
        self,
        pattern: &str,
        callback: impl FnMut(&Captures<'_>) -> String,
    ) -> Self {
        Self::new(Str::replace_matches_using(pattern, callback, &self.value))
    }

    /// Swap multiple keywords with other keywords.
    pub fn swap(self, map: &[(&str, &str)]) -> Self {
        Self::new(Str::swap(map, &self.value))
    }

    /// Remove any occurrence of the given string.
    pub fn remove(self, search: &str) -> Self {
        Self::new(Str::remove(search, &self.value))
    }

    /// Remove any occurrence of the given string, ignoring case.
    pub fn remove_ignore_case(self, search: &str) -> Self {
        Self::new(Str::remove_ignore_case(search, &self.value))
    }

    /// Remove every occurrence of each of the given strings.
    pub fn remove_many(self, searches: &[&str]) -> Self {
        Self::new(Str::remove_many(searches, &self.value))
    }

    /// Returns the portion of the string specified by the start and length.
    pub fn substr(self, start: isize, length: Option<isize>) -> Self {
        Self::new(Str::substr(&self.value, start, length))
    }

    /// Returns the number of substring occurrences.
    pub fn substr_count(&self, needle: &str) -> usize {
        Str::substr_count(&self.value, needle)
    }

    /// Replace text within a portion of the string.
    pub fn substr_replace(self, replace: &str, offset: isize, length: Option<isize>) -> Self {
        Self::new(Str::substr_replace(&self.value, replace, offset, length))
    }

    /// Take the first (or, when negative, last) `limit` characters.
    pub fn take(self, limit: isize) -> Self {
        Self::new(Str::take(&self.value, limit))
    }

    /// Mask a portion of the string with a repeated character.
    pub fn mask(self, character: char, index: isize, length: Option<usize>) -> Self {
        Self::new(Str::mask(&self.value, character, index, length))
    }

    /// Mask a portion of the string, supporting negative lengths.
    pub fn mask_with(self, character: &str, index: isize, length: Option<isize>) -> Self {
        Self::new(Str::mask_with(&self.value, character, index, length))
    }

    /// Pad both sides of the string with another.
    pub fn pad_both(self, length: usize, pad: &str) -> Self {
        Self::new(Str::pad_both(&self.value, length, pad))
    }

    /// Pad the left side of the string with another.
    pub fn pad_left(self, length: usize, pad: &str) -> Self {
        Self::new(Str::pad_left(&self.value, length, pad))
    }

    /// Pad the right side of the string with another.
    pub fn pad_right(self, length: usize, pad: &str) -> Self {
        Self::new(Str::pad_right(&self.value, length, pad))
    }

    /// Repeat the string.
    pub fn repeat(self, times: usize) -> Self {
        Self::new(self.value.repeat(times))
    }

    /// Wrap the string with the given strings.
    pub fn wrap(self, before: &str, after: &str) -> Self {
        Self::new(Str::wrap(&self.value, before, after))
    }

    /// Unwrap the string with the given strings.
    pub fn unwrap(self, before: &str, after: &str) -> Self {
        Self::new(Str::unwrap(&self.value, before, after))
    }

    /// Convert to snake case using a custom delimiter.
    pub fn snake_with(self, delimiter: &str) -> Self {
        Self::new(Str::snake_with(&self.value, delimiter))
    }

    /// Generate a URL friendly slug using the given separator.
    pub fn slug_with(self, separator: &str) -> Self {
        Self::new(Str::slug_with(&self.value, separator))
    }

    /// Transliterate to ASCII using a language's conventions.
    pub fn ascii_with_language(self, language: &str) -> Self {
        Self::new(Str::ascii_with_language(&self.value, language))
    }

    /// Get the plural form, given a count.
    pub fn plural_count(self, count: i64) -> Self {
        Self::new(Str::plural_count(&self.value, count))
    }

    /// Pluralize the last word of a studly caps case string, given a count.
    pub fn plural_studly_count(self, count: i64) -> Self {
        Self::new(Str::plural_studly_count(&self.value, count))
    }

    /// Pluralize the word and prefix it with its formatted count.
    pub fn counted(self, count: i64) -> Self {
        Self::new(Str::counted(&self.value, count))
    }

    /// Replace consecutive instances of the given character with a single one.
    pub fn deduplicate_with(self, character: &str) -> Self {
        Self::new(Str::deduplicate_with(&self.value, character))
    }

    /// Strip HTML tags, keeping the given ones.
    pub fn strip_tags_except(self, allowed: &[&str]) -> Self {
        Self::new(Str::strip_tags_except(&self.value, allowed))
    }

    /// Decode the string from Base64 (an empty string when it is invalid).
    pub fn from_base64(self) -> Self {
        Self::new(Str::from_base64(&self.value).unwrap_or_default())
    }

    /// Get the string matching the given pattern.
    pub fn match_(self, pattern: &str) -> Self {
        Self::new(Str::match_(pattern, &self.value))
    }

    /// Get all of the strings matching the given pattern.
    pub fn match_all(&self, pattern: &str) -> Collection<String> {
        Str::match_all(pattern, &self.value)
    }

    /// Determine if the string matches the given regular expression.
    pub fn is_match(&self, pattern: &str) -> bool {
        Str::is_match(pattern, &self.value)
    }

    /// Determine if the string matches any of the given regular expressions.
    pub fn is_match_any(&self, patterns: &[&str]) -> bool {
        Str::is_match_any(patterns, &self.value)
    }

    /// Determine if the string matches the given regular expression.
    pub fn test(&self, pattern: &str) -> bool {
        self.is_match(pattern)
    }

    /// Extract an excerpt around the first instance of a phrase.
    pub fn excerpt(&self, phrase: &str, options: ExcerptOptions) -> Option<String> {
        Str::excerpt(&self.value, phrase, options)
    }

    /// Split the string by a regular expression.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::of("one, two, three").split("/[\\s,]+/").all(), &["one", "two", "three"]);
    /// ```
    pub fn split(&self, pattern: &str) -> Collection<String> {
        crate::preg::split(pattern, &self.value, None)
            .map(Collection::make)
            .unwrap_or_default()
    }

    /// Split the string into chunks of the given length.
    pub fn split_chunks(&self, length: usize) -> Collection<String> {
        let chars: Vec<char> = self.value.chars().collect();
        Collection::make(chars.chunks(length.max(1)).map(|c| c.iter().collect::<String>()))
    }

    /// Explode the string into a collection of pieces.
    pub fn explode(&self, delimiter: &str) -> crate::Collection<String> {
        crate::Collection::make(self.value.split(delimiter).map(String::from))
    }

    /// Explode the string into at most `limit` pieces.
    pub fn explode_limit(&self, delimiter: &str, limit: usize) -> Collection<String> {
        Collection::make(self.value.splitn(limit.max(1), delimiter).map(String::from))
    }

    /// Split the string into pieces by uppercase characters.
    pub fn ucsplit(&self) -> Collection<String> {
        Collection::make(Str::ucsplit(&self.value))
    }

    /// Parse input from the string according to a format, like PHP's `sscanf`.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::of("filename.jpg").scan("%[^.].%s").all(), &["filename", "jpg"]);
    /// ```
    pub fn scan(&self, format: &str) -> Collection<String> {
        Collection::make(sscanf(&self.value, format))
    }

    /// Determine if the string contains a given substring.
    pub fn contains(&self, needle: &str) -> bool {
        Str::contains(&self.value, needle)
    }

    /// Determine if the string contains any of the given substrings.
    pub fn contains_any(&self, needles: &[&str]) -> bool {
        Str::contains_any(&self.value, needles)
    }

    /// Determine if the string contains all of the given substrings.
    pub fn contains_all(&self, needles: &[&str]) -> bool {
        Str::contains_all(&self.value, needles)
    }

    /// Determine if the string contains a substring, ignoring case.
    pub fn contains_ignore_case(&self, needle: &str) -> bool {
        Str::contains_ignore_case(&self.value, needle)
    }

    /// Determine if the string contains all of the substrings, ignoring case.
    pub fn contains_all_ignore_case(&self, needles: &[&str]) -> bool {
        Str::contains_all_ignore_case(&self.value, needles)
    }

    /// Determine if the string doesn't contain a given substring.
    pub fn doesnt_contain(&self, needle: &str) -> bool {
        Str::doesnt_contain(&self.value, needle)
    }

    /// Determine if the string doesn't contain any of the given substrings.
    pub fn doesnt_contain_any(&self, needles: &[&str]) -> bool {
        Str::doesnt_contain_any(&self.value, needles)
    }

    /// Determine if the string starts with a given substring.
    pub fn starts_with(&self, needle: &str) -> bool {
        Str::starts_with(&self.value, needle)
    }

    /// Determine if the string starts with any of the given substrings.
    pub fn starts_with_any(&self, needles: &[&str]) -> bool {
        Str::starts_with_any(&self.value, needles)
    }

    /// Determine if the string doesn't start with a given substring.
    pub fn doesnt_start_with(&self, needle: &str) -> bool {
        Str::doesnt_start_with(&self.value, needle)
    }

    /// Determine if the string ends with a given substring.
    pub fn ends_with(&self, needle: &str) -> bool {
        Str::ends_with(&self.value, needle)
    }

    /// Determine if the string ends with any of the given substrings.
    pub fn ends_with_any(&self, needles: &[&str]) -> bool {
        Str::ends_with_any(&self.value, needles)
    }

    /// Determine if the string doesn't end with a given substring.
    pub fn doesnt_end_with(&self, needle: &str) -> bool {
        Str::doesnt_end_with(&self.value, needle)
    }

    /// Determine if the string matches a given wildcard pattern.
    pub fn is(&self, pattern: &str) -> bool {
        Str::is(pattern, &self.value)
    }

    /// Determine if the string matches a given wildcard pattern, ignoring case.
    pub fn is_ignore_case(&self, pattern: &str) -> bool {
        Str::is_ignore_case(pattern, &self.value)
    }

    /// Determine if the string is 7 bit ASCII.
    pub fn is_ascii(&self) -> bool {
        Str::is_ascii(&self.value)
    }

    /// Determine if the string is valid JSON.
    pub fn is_json(&self) -> bool {
        Str::is_json(&self.value)
    }

    /// Determine if the string is a valid URL.
    pub fn is_url(&self) -> bool {
        Str::is_url(&self.value)
    }

    /// Determine if the string is a valid URL using one of the given protocols.
    pub fn is_url_with_protocols(&self, protocols: &[&str]) -> bool {
        Str::is_url_with_protocols(&self.value, protocols)
    }

    /// Determine if the string is a valid UUID.
    pub fn is_uuid(&self) -> bool {
        Str::is_uuid(&self.value)
    }

    /// Determine if the string is a valid UUID of the given version.
    pub fn is_uuid_version(&self, version: u8) -> bool {
        Str::is_uuid_version(&self.value, version)
    }

    /// Determine if the string is a valid ULID.
    pub fn is_ulid(&self) -> bool {
        Str::is_ulid(&self.value)
    }

    /// Determine if the string is exactly the given value.
    pub fn exactly(&self, value: &str) -> bool {
        self.value == value
    }

    /// Determine if the string is empty.
    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// Determine if the string is not empty.
    pub fn is_not_empty(&self) -> bool {
        !self.value.is_empty()
    }

    /// The length of the string, in characters.
    pub fn length(&self) -> usize {
        Str::length(&self.value)
    }

    /// Find the position of the first occurrence of a substring.
    pub fn position(&self, needle: &str) -> Option<usize> {
        Str::position(&self.value, needle)
    }

    /// Call the given callback with the string and return the result.
    pub fn pipe(self, callback: impl FnOnce(String) -> String) -> Self {
        Self::new(callback(self.value))
    }

    /// Hash the string using the given algorithm (`md5`, `sha1`, `sha224`,
    /// `sha256`, `sha384` or `sha512`).
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// let hashed = Str::of("secret").hash("sha256").unwrap();
    /// assert_eq!(hashed, "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b");
    /// ```
    pub fn hash(self, algorithm: &str) -> crate::Result<Self> {
        use sha2::Digest;
        let bytes = self.value.as_bytes();
        let digest = match algorithm.to_ascii_lowercase().as_str() {
            "md5" => hex::encode(md5::Md5::digest(bytes)),
            "sha1" => hex::encode(sha1::Sha1::digest(bytes)),
            "sha224" => hex::encode(sha2::Sha224::digest(bytes)),
            "sha256" => hex::encode(sha2::Sha256::digest(bytes)),
            "sha384" => hex::encode(sha2::Sha384::digest(bytes)),
            "sha512" => hex::encode(sha2::Sha512::digest(bytes)),
            other => {
                return Err(crate::error::InvalidArgumentException::new(format!(
                    "hash(): Argument #1 ($algo) must be a valid hashing algorithm, [{other}] given."
                ))
                .into());
            }
        };
        Ok(Self::new(digest))
    }

    /// Convert the string into an [`HtmlString`] that won't be escaped.
    pub fn to_html_string(&self) -> HtmlString {
        HtmlString::new(self.value.clone())
    }

    /// Parse the string as a date.
    pub fn to_date(&self) -> Option<Carbon> {
        Carbon::parse(&self.value).ok()
    }

    /// Parse the string as a date in the given (PHP) format.
    pub fn to_date_from_format(&self, format: &str) -> Option<Carbon> {
        Carbon::create_from_format(format, &self.value).ok()
    }

    /// Convert the string into a [`Uri`].
    pub fn to_uri(&self) -> Uri {
        Uri::of(&self.value)
    }

    /// Convert the string into an integer, like PHP's `intval`.
    pub fn to_integer(&self) -> i64 {
        php_intval(&self.value)
    }

    /// Convert the string into an integer using the given base.
    pub fn to_integer_with_base(&self, base: u32) -> i64 {
        let trimmed = self.value.trim_start();
        let (negative, digits) = match trimmed.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
        };
        let digits: String = digits.chars().take_while(|c| c.is_digit(base)).collect();
        let value = i64::from_str_radix(&digits, base).unwrap_or(0);
        if negative { -value } else { value }
    }

    /// Convert the string into a float, like PHP's `floatval`.
    pub fn to_float(&self) -> f64 {
        php_floatval(&self.value)
    }

    /// Interpret the string as a boolean ("1", "true", "on", "yes").
    pub fn to_boolean(&self) -> bool {
        matches!(
            self.value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        )
    }

    /// Execute the callback if the string is empty.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// let string = Str::of("  ").trim().when_empty(|s| s.prepend("Laravel"));
    /// assert_eq!(string, "Laravel");
    /// ```
    pub fn when_empty(self, callback: impl FnOnce(Self) -> Self) -> Self {
        if self.is_empty() { callback(self) } else { self }
    }

    /// Execute the callback if the string is not empty.
    pub fn when_not_empty(self, callback: impl FnOnce(Self) -> Self) -> Self {
        if self.is_not_empty() { callback(self) } else { self }
    }

    /// Execute the callback if the string is 7 bit ASCII.
    pub fn when_is_ascii(self, callback: impl FnOnce(Self) -> Self) -> Self {
        if self.is_ascii() { callback(self) } else { self }
    }

    /// Execute the callback if the string is a valid UUID.
    pub fn when_is_uuid(self, callback: impl FnOnce(Self) -> Self) -> Self {
        if self.is_uuid() { callback(self) } else { self }
    }

    /// Execute the callback if the string is a valid ULID.
    pub fn when_is_ulid(self, callback: impl FnOnce(Self) -> Self) -> Self {
        if self.is_ulid() { callback(self) } else { self }
    }

    fluent_when! {
        /// Execute the callback if the string contains the given substring.
        when_contains(needle: &str) => |s: &Self, needle| s.contains(needle),
        /// Execute the callback if the string contains any of the given substrings.
        when_contains_any(needles: &[&str]) => |s: &Self, needles| s.contains_any(needles),
        /// Execute the callback if the string contains all of the given substrings.
        when_contains_all(needles: &[&str]) => |s: &Self, needles| s.contains_all(needles),
        /// Execute the callback if the string ends with the given substring.
        when_ends_with(needle: &str) => |s: &Self, needle| s.ends_with(needle),
        /// Execute the callback if the string doesn't end with the given substring.
        when_doesnt_end_with(needle: &str) => |s: &Self, needle| s.doesnt_end_with(needle),
        /// Execute the callback if the string starts with the given substring.
        when_starts_with(needle: &str) => |s: &Self, needle| s.starts_with(needle),
        /// Execute the callback if the string doesn't start with the given substring.
        when_doesnt_start_with(needle: &str) => |s: &Self, needle| s.doesnt_start_with(needle),
        /// Execute the callback if the string is exactly the given value.
        when_exactly(value: &str) => |s: &Self, value| s.exactly(value),
        /// Execute the callback if the string is not exactly the given value.
        when_not_exactly(value: &str) => |s: &Self, value| !s.exactly(value),
        /// Execute the callback if the string matches the given wildcard pattern.
        when_is(pattern: &str) => |s: &Self, pattern| s.is(pattern),
        /// Execute the callback if the string matches the given regular expression.
        when_test(pattern: &str) => |s: &Self, pattern| s.test(pattern),
    }
}

/// PHP's `dirname` for a single level.
fn php_dirname(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    match trimmed.rfind('/') {
        Some(index) => {
            let parent = trimmed[..index].trim_end_matches('/');
            if parent.is_empty() { "/".to_string() } else { parent.to_string() }
        }
        None => ".".to_string(),
    }
}

/// The leading numeric portion of a string, as PHP sees it.
fn numeric_prefix(value: &str) -> &str {
    let value = value.trim_start_matches([' ', '\t', '\n', '\r', '\u{0B}', '\u{0C}']);
    let bytes = value.as_bytes();
    let mut end = 0;
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    let digits_start = end;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    let mut has_digits = end > digits_start;
    if end < bytes.len() && bytes[end] == b'.' {
        let mut frac = end + 1;
        while frac < bytes.len() && bytes[frac].is_ascii_digit() {
            frac += 1;
        }
        if frac > end + 1 || has_digits {
            has_digits = has_digits || frac > end + 1;
            end = frac;
        }
    }
    if !has_digits {
        return "";
    }
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        let mut exp = end + 1;
        if exp < bytes.len() && (bytes[exp] == b'+' || bytes[exp] == b'-') {
            exp += 1;
        }
        let exp_digits = exp;
        while exp < bytes.len() && bytes[exp].is_ascii_digit() {
            exp += 1;
        }
        if exp > exp_digits {
            end = exp;
        }
    }
    &value[..end]
}

/// Convert a string to an integer the way PHP's `intval` does.
pub(crate) fn php_intval(value: &str) -> i64 {
    let prefix = numeric_prefix(value);
    if let Ok(i) = prefix.parse::<i64>() {
        return i;
    }
    prefix
        .parse::<f64>()
        .map(|f| if f.is_finite() { f as i64 } else { 0 })
        .unwrap_or(0)
}

/// Convert a string to a float the way PHP's `floatval` does.
pub(crate) fn php_floatval(value: &str) -> f64 {
    numeric_prefix(value).parse::<f64>().unwrap_or(0.0)
}

/// A small `sscanf` supporting `%s`, `%d`, `%f`, `%c`, `%[...]` and literals.
fn sscanf(input: &str, format: &str) -> Vec<String> {
    let input: Vec<char> = input.chars().collect();
    let format: Vec<char> = format.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut f) = (0, 0);
    while f < format.len() {
        let fc = format[f];
        if fc.is_whitespace() {
            while i < input.len() && input[i].is_whitespace() {
                i += 1;
            }
            f += 1;
            continue;
        }
        if fc != '%' {
            if i < input.len() && input[i] == fc {
                i += 1;
                f += 1;
                continue;
            }
            break;
        }
        f += 1;
        let Some(&spec) = format.get(f) else { break };
        if spec == '%' {
            if input.get(i) == Some(&'%') {
                i += 1;
                f += 1;
                continue;
            }
            break;
        }
        let start = i;
        match spec {
            's' => {
                while i < input.len() && input[i].is_whitespace() {
                    i += 1;
                }
                let begin = i;
                while i < input.len() && !input[i].is_whitespace() {
                    i += 1;
                }
                if i == begin {
                    break;
                }
                out.push(input[begin..i].iter().collect());
                f += 1;
            }
            'd' | 'f' => {
                while i < input.len() && input[i].is_whitespace() {
                    i += 1;
                }
                let begin = i;
                if i < input.len() && (input[i] == '-' || input[i] == '+') {
                    i += 1;
                }
                while i < input.len()
                    && (input[i].is_ascii_digit() || (spec == 'f' && (input[i] == '.' || input[i] == 'e')))
                {
                    i += 1;
                }
                if i == begin {
                    break;
                }
                out.push(input[begin..i].iter().collect());
                f += 1;
            }
            'c' => {
                if i >= input.len() {
                    break;
                }
                out.push(input[i].to_string());
                i += 1;
                f += 1;
            }
            '[' => {
                f += 1;
                let negate = format.get(f) == Some(&'^');
                if negate {
                    f += 1;
                }
                let mut set = Vec::new();
                if format.get(f) == Some(&']') {
                    set.push(']');
                    f += 1;
                }
                while f < format.len() && format[f] != ']' {
                    if format.get(f + 1) == Some(&'-') && format.get(f + 2).is_some_and(|c| *c != ']') {
                        set.extend(format[f]..=format[f + 2]);
                        f += 3;
                    } else {
                        set.push(format[f]);
                        f += 1;
                    }
                }
                f += 1;
                while i < input.len() && (set.contains(&input[i]) != negate) {
                    i += 1;
                }
                if i == start {
                    break;
                }
                out.push(input[start..i].iter().collect());
            }
            _ => break,
        }
    }
    out
}

impl Conditionable for Stringable {}
impl Tappable for Stringable {}

impl Deref for Stringable {
    type Target = str;

    fn deref(&self) -> &str {
        &self.value
    }
}

impl Borrow<str> for Stringable {
    fn borrow(&self) -> &str {
        &self.value
    }
}

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

impl PartialEq<String> for Stringable {
    fn eq(&self, other: &String) -> bool {
        &self.value == other
    }
}

impl From<Stringable> for serde_json::Value {
    fn from(s: Stringable) -> Self {
        serde_json::Value::String(s.value)
    }
}

impl From<Stringable> for HtmlString {
    fn from(s: Stringable) -> Self {
        HtmlString::new(s.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_chains_fluently() {
        assert_eq!(Str::of("Taylor").append(" Otwell"), "Taylor Otwell");
        assert_eq!(Str::of("Framework").prepend("Laravel "), "Laravel Framework");
        assert_eq!(Str::of("a nice title uses the correct case").apa(), "A Nice Title Uses the Correct Case");
        assert_eq!(Str::of("Taylor Otwell").initials().upper(), "TO");
        assert_eq!(Str::of("/Laravel/").trim_chars("/"), "Laravel");
        assert_eq!(Str::of("/Laravel/").ltrim_chars("/"), "Laravel/");
        assert_eq!(Str::of("  Laravel  ").rtrim(), "  Laravel");
        assert_eq!(Str::of("https://laravel.com").chop_end(".com"), "https://laravel");
        assert_eq!(Str::of("http://laravel.com").chop_end_any(&[".com", ".io"]), "http://laravel");
        assert_eq!(Str::of("This is my name.").char_at(6), Some('s'));
        assert_eq!(Str::of("Laravel Framework").substr(8, Some(5)), "Frame");
        assert_eq!(Str::of("The Framework").substr_replace(" Laravel", 3, Some(0)), "The Laravel Framework");
        assert_eq!(Str::of("Tacos are great!").swap(&[("Tacos", "Burritos"), ("great", "fantastic")]), "Burritos are fantastic!");
        assert_eq!(Str::of("Build something amazing!").take(5), "Build");
        assert_eq!(Str::of("taylor@example.com").mask_with("*", 4, Some(-4)), "tayl**********.com");
        assert_eq!(Str::of("order").counted(1000), "1,000 orders");
        assert_eq!(Str::of("car").plural_count(1), "car");
        assert_eq!(Str::of("Arkansas is quite beautiful!").remove("quite "), "Arkansas is beautiful!");
        assert_eq!(Str::of("macOS 13.x").replace_ignore_case("macos", "iOS"), "iOS 13.x");
        assert_eq!(Str::of("123").replace_matches_using("/\\d/", |m| format!("[{}]", &m[0])), "[1][2][3]");
        assert_eq!(Str::of("-Laravel-").unwrap("-", "-"), "Laravel");
        assert_eq!(Str::of("ⓣⓔⓢⓣ@ⓛⓐⓡⓐⓥⓔⓛ.ⓒⓞⓜ").transliterate(), "test@laravel.com");
        assert_eq!(Str::of("Laravel").to_base64().from_base64(), "Laravel");
    }

    #[test]
    fn it_works_with_paths() {
        assert_eq!(Str::of("/foo/bar/baz").basename(), "baz");
        assert_eq!(Str::of("/foo/bar/baz.jpg").basename_without(".jpg"), "baz");
        assert_eq!(Str::of("/foo/bar/baz/").basename(), "baz");
        assert_eq!(Str::of("/foo/bar/baz").dirname(), "/foo/bar");
        assert_eq!(Str::of("/foo/bar/baz").dirname_levels(2), "/foo");
        assert_eq!(Str::of("/foo").dirname(), "/");
        assert_eq!(Str::of("foo").dirname(), ".");
    }

    #[test]
    fn it_splits_scans_and_matches() {
        assert_eq!(Str::of("foo bar baz").explode(" ").all(), &["foo", "bar", "baz"]);
        assert_eq!(Str::of("a,b,c").explode_limit(",", 2).all(), &["a", "b,c"]);
        assert_eq!(Str::of("one, two, three").split("/[\\s,]+/").all(), &["one", "two", "three"]);
        assert_eq!(Str::of("foobarbaz").split_chunks(3).all(), &["foo", "bar", "baz"]);
        assert_eq!(Str::of("filename.jpg").scan("%[^.].%s").all(), &["filename", "jpg"]);
        assert_eq!(Str::of("age: 42").scan("age: %d").all(), &["42"]);
        assert_eq!(Str::of("Foo Bar").ucsplit().all(), &["Foo ", "Bar"]);
        assert_eq!(Str::of("foo bar").match_("/foo (.*)/"), "bar");
        assert_eq!(Str::of("bar fun bar fly").match_all("/f(\\w*)/").all(), &["un", "ly"]);
        assert!(Str::of("Laravel Framework").test("/Laravel/"));
        assert!(!Str::of("laravel").is_match("/foo (.*)/"));
    }

    #[test]
    fn it_runs_conditional_callbacks() {
        assert_eq!(Str::of("tony stark").when_contains("tony", |s| s.title()), "Tony Stark");
        assert_eq!(Str::of("tony stark").when_contains_any(&["tony", "hulk"], |s| s.title()), "Tony Stark");
        assert_eq!(Str::of("tony stark").when_contains_all(&["tony", "stark"], |s| s.title()), "Tony Stark");
        assert_eq!(Str::of("tony stark").when_contains_all(&["tony", "hulk"], |s| s.title()), "tony stark");
        assert_eq!(Str::of("disney world").when_doesnt_end_with("land", |s| s.title()), "Disney World");
        assert_eq!(Str::of("disney world").when_doesnt_start_with("sea", |s| s.title()), "Disney World");
        assert_eq!(Str::of("Framework").when_not_empty(|s| s.prepend("Laravel ")), "Laravel Framework");
        assert_eq!(Str::of("disney world").when_starts_with("disney", |s| s.title()), "Disney World");
        assert_eq!(Str::of("disney world").when_ends_with("world", |s| s.title()), "Disney World");
        assert_eq!(Str::of("laravel").when_exactly("laravel", |s| s.title()), "Laravel");
        assert_eq!(Str::of("framework").when_not_exactly("laravel", |s| s.title()), "Framework");
        assert_eq!(Str::of("foo/bar").when_is("foo/*", |s| s.append("/baz")), "foo/bar/baz");
        assert_eq!(Str::of("laravel").when_is_ascii(|s| s.title()), "Laravel");
        assert_eq!(Str::of("01gd6r360bp37zj17nxb55yv40").when_is_ulid(|s| s.substr(0, Some(8))), "01gd6r36");
        assert_eq!(
            Str::of("a0a2a2d2-0b87-4a18-83f2-2529882be2de").when_is_uuid(|s| s.substr(0, Some(8))),
            "a0a2a2d2"
        );
        assert_eq!(Str::of("laravel framework").when_test("/laravel/", |s| s.title()), "Laravel Framework");
        assert_eq!(Str::of("Taylor").when(true, |s| s.append(" Otwell")), "Taylor Otwell");
    }

    #[test]
    fn it_converts_values() {
        assert_eq!(Str::of("42").to_integer(), 42);
        assert_eq!(Str::of("12abc").to_integer(), 12);
        assert_eq!(Str::of(" -7.9").to_integer(), -7);
        assert_eq!(Str::of("1e3").to_integer(), 1000);
        assert_eq!(Str::of("ff").to_integer_with_base(16), 255);
        assert_eq!(Str::of("1.5kg").to_float(), 1.5);
        assert_eq!(Str::of("abc").to_float(), 0.0);
        assert!(Str::of("yes").to_boolean());
        assert!(!Str::of("nope").to_boolean());
        assert_eq!(Str::of("2024-03-12").to_date().unwrap().to_date_string(), "2024-03-12");
        assert_eq!(Str::of("12/03/2024").to_date_from_format("d/m/Y").unwrap().month(), 3);
        assert_eq!(Str::of("Nuno Maduro").to_html_string().to_html(), "Nuno Maduro");
        assert_eq!(Str::of("https://example.com/path").to_uri().host(), Some("example.com"));
        assert_eq!(Str::of("secret").hash("md5").unwrap(), "5ebe2294ecd0e0f08eab7690d2a6ee69");
        assert!(Str::of("secret").hash("nope").is_err());
        assert_eq!(Str::of("Laravel").len(), 7);
    }
}
