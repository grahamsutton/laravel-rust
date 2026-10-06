//! String helpers.
//!
//! ```
//! use illuminate_support::Str;
//!
//! assert_eq!(Str::snake("FooBar"), "foo_bar");
//! assert_eq!(Str::studly("foo_bar"), "FooBar");
//! assert_eq!(Str::plural("car"), "cars");
//! assert_eq!(Str::slug("Laravel Framework"), "laravel-framework");
//! ```

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use rand::Rng;
use regex::Regex;

use crate::pluralizer::Pluralizer;
use crate::stringable::Stringable;

static SNAKE_CACHE: LazyLock<Mutex<HashMap<(String, String), String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Static string helpers, mirroring `Illuminate\Support\Str`.
pub struct Str;

impl Str {
    /// Get a new fluent [`Stringable`] for the given string.
    pub fn of(value: impl Into<String>) -> Stringable {
        Stringable::new(value)
    }

    /// Return the remainder of a string after the first occurrence of a value.
    pub fn after(subject: &str, search: &str) -> String {
        if search.is_empty() {
            return subject.to_string();
        }
        match subject.find(search) {
            Some(i) => subject[i + search.len()..].to_string(),
            None => subject.to_string(),
        }
    }

    /// Return the remainder of a string after the last occurrence of a value.
    pub fn after_last(subject: &str, search: &str) -> String {
        if search.is_empty() {
            return subject.to_string();
        }
        match subject.rfind(search) {
            Some(i) => subject[i + search.len()..].to_string(),
            None => subject.to_string(),
        }
    }

    /// Get the portion of a string before the first occurrence of a value.
    pub fn before(subject: &str, search: &str) -> String {
        if search.is_empty() {
            return subject.to_string();
        }
        match subject.find(search) {
            Some(i) => subject[..i].to_string(),
            None => subject.to_string(),
        }
    }

    /// Get the portion of a string before the last occurrence of a value.
    pub fn before_last(subject: &str, search: &str) -> String {
        if search.is_empty() {
            return subject.to_string();
        }
        match subject.rfind(search) {
            Some(i) => subject[..i].to_string(),
            None => subject.to_string(),
        }
    }

    /// Get the portion of a string between two values.
    pub fn between(subject: &str, from: &str, to: &str) -> String {
        if from.is_empty() || to.is_empty() {
            return subject.to_string();
        }
        Self::before_last(&Self::after(subject, from), to)
    }

    /// Get the smallest possible portion of a string between two values.
    pub fn between_first(subject: &str, from: &str, to: &str) -> String {
        if from.is_empty() || to.is_empty() {
            return subject.to_string();
        }
        Self::before(&Self::after(subject, from), to)
    }

    /// Convert a value to camel case.
    pub fn camel(value: &str) -> String {
        Self::lcfirst(&Self::studly(value))
    }

    /// Convert a value to studly caps case.
    pub fn studly(value: &str) -> String {
        let normalized = value.replace(['-', '_'], " ");
        normalized
            .split_whitespace()
            .map(Self::ucfirst)
            .collect::<Vec<_>>()
            .join("")
    }

    /// Alias of `studly`.
    pub fn pascal(value: &str) -> String {
        Self::studly(value)
    }

    /// Convert a string to snake case.
    pub fn snake(value: &str) -> String {
        Self::snake_with(value, "_")
    }

    /// Convert a string to snake case using a custom delimiter.
    pub fn snake_with(value: &str, delimiter: &str) -> String {
        let key = (value.to_string(), delimiter.to_string());
        if let Some(cached) = SNAKE_CACHE.lock().unwrap().get(&key) {
            return cached.clone();
        }

        let mut result = value.to_string();
        if !value.chars().all(|c| c.is_lowercase() || !c.is_alphabetic()) {
            // ucwords, then strip whitespace, then insert delimiters before capitals.
            let words: String = Self::ucwords(value).split_whitespace().collect();
            let mut out = String::new();
            let chars: Vec<char> = words.chars().collect();
            for (i, c) in chars.iter().enumerate() {
                if c.is_uppercase() && i > 0 {
                    out.push_str(delimiter);
                }
                out.extend(c.to_lowercase());
            }
            result = out;
        }

        SNAKE_CACHE.lock().unwrap().insert(key, result.clone());
        result
    }

    /// Convert a string to kebab case.
    pub fn kebab(value: &str) -> String {
        Self::snake_with(value, "-")
    }

    /// Convert the given string to title case.
    pub fn title(value: &str) -> String {
        let lower = value.to_lowercase();
        let mut out = String::with_capacity(lower.len());
        let mut capitalize = true;
        for c in lower.chars() {
            if capitalize && c.is_alphanumeric() {
                out.extend(c.to_uppercase());
                capitalize = false;
            } else {
                out.push(c);
            }
            if c.is_whitespace() || c == '-' || c == '_' || c == '.' {
                capitalize = true;
            }
        }
        out
    }

    /// Convert the given string to title case for each word, splitting on
    /// case changes, hyphens and underscores ("email_address" => "Email Address").
    pub fn headline(value: &str) -> String {
        let spaced = value.replace(['_', '-'], " ");
        let parts: Vec<String> = spaced
            .split_whitespace()
            .flat_map(|word| Self::ucsplit(word))
            .collect();
        parts
            .iter()
            .map(|p| Self::ucfirst(&p.to_lowercase()))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Split a string into pieces by uppercase characters.
    pub fn ucsplit(value: &str) -> Vec<String> {
        let mut parts: Vec<String> = Vec::new();
        for c in value.chars() {
            if c.is_uppercase() || parts.is_empty() {
                parts.push(c.to_string());
            } else {
                parts.last_mut().unwrap().push(c);
            }
        }
        parts
    }

    /// Make a string's first character uppercase.
    pub fn ucfirst(value: &str) -> String {
        let mut chars = value.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    }

    /// Make a string's first character lowercase.
    pub fn lcfirst(value: &str) -> String {
        let mut chars = value.chars();
        match chars.next() {
            Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    }

    /// Uppercase the first character of each word.
    pub fn ucwords(value: &str) -> String {
        let mut out = String::with_capacity(value.len());
        let mut capitalize = true;
        for c in value.chars() {
            if capitalize {
                out.extend(c.to_uppercase());
            } else {
                out.push(c);
            }
            capitalize = c.is_whitespace();
        }
        out
    }

    /// Convert the given string to lower-case.
    pub fn lower(value: &str) -> String {
        value.to_lowercase()
    }

    /// Convert the given string to upper-case.
    pub fn upper(value: &str) -> String {
        value.to_uppercase()
    }

    /// Get the plural form of an English word.
    pub fn plural(value: &str) -> String {
        Pluralizer::plural(value, 2)
    }

    /// Get the plural form of an English word, given a count.
    pub fn plural_count(value: &str, count: i64) -> String {
        Pluralizer::plural(value, count)
    }

    /// Pluralize the last word of an English, studly caps case string.
    pub fn plural_studly(value: &str) -> String {
        let parts = Self::ucsplit(value);
        match parts.split_last() {
            Some((last, rest)) => format!("{}{}", rest.concat(), Self::plural(last)),
            None => String::new(),
        }
    }

    /// Get the singular form of an English word.
    pub fn singular(value: &str) -> String {
        Pluralizer::singular(value)
    }

    /// Generate a URL friendly "slug" from a given string.
    pub fn slug(title: &str) -> String {
        Self::slug_with(title, "-")
    }

    /// Generate a URL friendly "slug" using the given separator.
    pub fn slug_with(title: &str, separator: &str) -> String {
        let ascii = Self::ascii(title);
        let flip = if separator == "-" { "_" } else { "-" };
        let mut title = ascii.replace(flip, separator);
        title = title.replace('@', &format!("{separator}at{separator}"));
        let lower = title.to_lowercase();
        let mut out = String::new();
        let mut pending_separator = false;
        for c in lower.chars() {
            if c.is_alphanumeric() {
                if pending_separator && !out.is_empty() {
                    out.push_str(separator);
                }
                pending_separator = false;
                out.push(c);
            } else if c.is_whitespace() || separator.contains(c) {
                pending_separator = true;
            }
        }
        out
    }

    /// Transliterate a UTF-8 value to ASCII (best effort).
    pub fn ascii(value: &str) -> String {
        value
            .chars()
            .map(|c| match c {
                'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => "a".to_string(),
                'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' => "A".to_string(),
                'æ' => "ae".to_string(),
                'Æ' => "AE".to_string(),
                'ç' | 'ć' | 'č' => "c".to_string(),
                'Ç' | 'Ć' | 'Č' => "C".to_string(),
                'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => "e".to_string(),
                'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ę' | 'Ě' => "E".to_string(),
                'ì' | 'í' | 'î' | 'ï' | 'ī' => "i".to_string(),
                'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' => "I".to_string(),
                'ñ' | 'ń' | 'ň' => "n".to_string(),
                'Ñ' | 'Ń' | 'Ň' => "N".to_string(),
                'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => "o".to_string(),
                'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ō' => "O".to_string(),
                'œ' => "oe".to_string(),
                'Œ' => "OE".to_string(),
                'ß' => "ss".to_string(),
                'ś' | 'š' | 'ş' => "s".to_string(),
                'Ś' | 'Š' | 'Ş' => "S".to_string(),
                'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' => "u".to_string(),
                'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' | 'Ů' => "U".to_string(),
                'ý' | 'ÿ' => "y".to_string(),
                'Ý' | 'Ÿ' => "Y".to_string(),
                'ź' | 'ż' | 'ž' => "z".to_string(),
                'Ź' | 'Ż' | 'Ž' => "Z".to_string(),
                'ł' => "l".to_string(),
                'Ł' => "L".to_string(),
                'đ' | 'ð' => "d".to_string(),
                'Đ' | 'Ð' => "D".to_string(),
                'þ' => "th".to_string(),
                'Þ' => "TH".to_string(),
                c if c.is_ascii() => c.to_string(),
                _ => String::new(),
            })
            .collect()
    }

    /// Determine if a given string contains a given substring.
    pub fn contains(haystack: &str, needle: &str) -> bool {
        !needle.is_empty() && haystack.contains(needle)
    }

    /// Determine if a given string contains any of the given substrings.
    pub fn contains_any(haystack: &str, needles: &[&str]) -> bool {
        needles.iter().any(|n| Self::contains(haystack, n))
    }

    /// Determine if a given string contains all of the given substrings.
    pub fn contains_all(haystack: &str, needles: &[&str]) -> bool {
        needles.iter().all(|n| Self::contains(haystack, n))
    }

    /// Determine if a given string contains a substring, ignoring case.
    pub fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
        Self::contains(&haystack.to_lowercase(), &needle.to_lowercase())
    }

    /// Determine if a given string doesn't contain a given substring.
    pub fn doesnt_contain(haystack: &str, needle: &str) -> bool {
        !Self::contains(haystack, needle)
    }

    /// Determine if a given string starts with a given substring.
    pub fn starts_with(haystack: &str, needle: &str) -> bool {
        !needle.is_empty() && haystack.starts_with(needle)
    }

    /// Determine if a given string ends with a given substring.
    pub fn ends_with(haystack: &str, needle: &str) -> bool {
        !needle.is_empty() && haystack.ends_with(needle)
    }

    /// Determine if a given string matches a given pattern (`*` is a wildcard).
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert!(Str::is("admin/*", "admin/users"));
    /// assert!(!Str::is("admin/*", "users"));
    /// ```
    pub fn is(pattern: &str, value: &str) -> bool {
        if pattern == value {
            return true;
        }
        if !pattern.contains('*') {
            return false;
        }
        let regex = format!("^{}$", regex::escape(pattern).replace("\\*", ".*"));
        Regex::new(&format!("(?s){regex}"))
            .map(|r| r.is_match(value))
            .unwrap_or(false)
    }

    /// Determine if a given string matches any of the given patterns.
    pub fn is_any(patterns: &[&str], value: &str) -> bool {
        patterns.iter().any(|p| Self::is(p, value))
    }

    /// Determine if a given string is valid JSON.
    pub fn is_json(value: &str) -> bool {
        serde_json::from_str::<serde_json::Value>(value).is_ok()
    }

    /// Determine if a given string is a valid UUID.
    pub fn is_uuid(value: &str) -> bool {
        uuid::Uuid::parse_str(value).is_ok() && value.len() == 36
    }

    /// Determine if a given string is a valid ULID.
    pub fn is_ulid(value: &str) -> bool {
        value.len() == 26 && ulid::Ulid::from_string(value).is_ok()
    }

    /// Determine if a given string is 7 bit ASCII.
    pub fn is_ascii(value: &str) -> bool {
        value.is_ascii()
    }

    /// Determine if a given value is a valid URL.
    pub fn is_url(value: &str) -> bool {
        static URL: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^(?i)[a-z][a-z0-9+.-]*://([^\s/?#@]+@)?([a-z0-9\-._~%]+|\[[a-f0-9:.]+\])(:\d+)?(/[^\s?#]*)?(\?[^\s#]*)?(#\S*)?$").unwrap()
        });
        URL.is_match(value)
    }

    /// Return the length of the given string (in characters).
    pub fn length(value: &str) -> usize {
        value.chars().count()
    }

    /// Limit the number of characters in a string.
    pub fn limit(value: &str, limit: usize) -> String {
        Self::limit_with(value, limit, "...")
    }

    /// Limit the number of characters in a string, with a custom ending.
    pub fn limit_with(value: &str, limit: usize, end: &str) -> String {
        if value.chars().count() <= limit {
            return value.to_string();
        }
        let truncated: String = value.chars().take(limit).collect();
        format!("{}{}", truncated.trim_end(), end)
    }

    /// Limit the number of words in a string.
    pub fn words(value: &str, words: usize) -> String {
        Self::words_with(value, words, "...")
    }

    /// Limit the number of words in a string, with a custom ending.
    pub fn words_with(value: &str, words: usize, end: &str) -> String {
        let all: Vec<&str> = value.split_whitespace().collect();
        if all.len() <= words {
            return value.to_string();
        }
        format!("{}{}", all[..words].join(" "), end)
    }

    /// Count the number of words in a string.
    pub fn word_count(value: &str) -> usize {
        value.split_whitespace().count()
    }

    /// Generate a "random" alpha-numeric string.
    pub fn random(length: usize) -> String {
        const POOL: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let mut rng = rand::rng();
        (0..length)
            .map(|_| POOL[rng.random_range(0..POOL.len())] as char)
            .collect()
    }

    /// Generate a UUID (version 4).
    pub fn uuid() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    /// Generate a time-ordered UUID (version 7).
    pub fn ordered_uuid() -> uuid::Uuid {
        uuid::Uuid::now_v7()
    }

    /// Alias of `ordered_uuid`.
    pub fn uuid7() -> uuid::Uuid {
        Self::ordered_uuid()
    }

    /// Generate a ULID.
    pub fn ulid() -> ulid::Ulid {
        ulid::Ulid::new()
    }

    /// Replace the first occurrence of a given value in the string.
    pub fn replace_first(search: &str, replace: &str, subject: &str) -> String {
        if search.is_empty() {
            return subject.to_string();
        }
        subject.replacen(search, replace, 1)
    }

    /// Replace the last occurrence of a given value in the string.
    pub fn replace_last(search: &str, replace: &str, subject: &str) -> String {
        if search.is_empty() {
            return subject.to_string();
        }
        match subject.rfind(search) {
            Some(i) => format!("{}{}{}", &subject[..i], replace, &subject[i + search.len()..]),
            None => subject.to_string(),
        }
    }

    /// Replace a given value in the string sequentially with an array.
    pub fn replace_array(search: &str, replace: &[&str], subject: &str) -> String {
        let mut parts = subject.split(search);
        let mut out = parts.next().unwrap_or_default().to_string();
        let mut replacements = replace.iter();
        for part in parts {
            out.push_str(replacements.next().copied().unwrap_or(search));
            out.push_str(part);
        }
        out
    }

    /// Replace all occurrences of the search string with the replacement.
    pub fn replace(search: &str, replace: &str, subject: &str) -> String {
        subject.replace(search, replace)
    }

    /// Remove any occurrence of the given string in the subject.
    pub fn remove(search: &str, subject: &str) -> String {
        subject.replace(search, "")
    }

    /// Begin a string with a single instance of a given value.
    pub fn start(value: &str, prefix: &str) -> String {
        let mut stripped = value;
        while !prefix.is_empty() && stripped.starts_with(prefix) {
            stripped = &stripped[prefix.len()..];
        }
        format!("{prefix}{stripped}")
    }

    /// Cap a string with a single instance of a given value.
    pub fn finish(value: &str, cap: &str) -> String {
        let mut stripped = value;
        while !cap.is_empty() && stripped.ends_with(cap) {
            stripped = &stripped[..stripped.len() - cap.len()];
        }
        format!("{stripped}{cap}")
    }

    /// Wrap the string with the given strings.
    pub fn wrap(value: &str, before: &str, after: &str) -> String {
        format!("{before}{value}{after}")
    }

    /// Remove all "extra" blank space from the given string.
    pub fn squish(value: &str) -> String {
        value.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Returns the portion of the string specified by the start and length.
    pub fn substr(value: &str, start: isize, length: Option<isize>) -> String {
        let chars: Vec<char> = value.chars().collect();
        let len = chars.len() as isize;
        let start = if start < 0 { (len + start).max(0) } else { start.min(len) } as usize;
        let end = match length {
            Some(l) if l < 0 => (len + l).max(start as isize) as usize,
            Some(l) => (start + l as usize).min(chars.len()),
            None => chars.len(),
        };
        chars[start..end.max(start)].iter().collect()
    }

    /// Masks a portion of a string with a repeated character.
    pub fn mask(value: &str, character: char, index: isize, length: Option<usize>) -> String {
        let chars: Vec<char> = value.chars().collect();
        let len = chars.len() as isize;
        let start = if index < 0 { (len + index).max(0) } else { index.min(len) } as usize;
        let end = match length {
            Some(l) => (start + l).min(chars.len()),
            None => chars.len(),
        };
        chars
            .iter()
            .enumerate()
            .map(|(i, c)| if i >= start && i < end { character } else { *c })
            .collect()
    }

    /// Pad both sides of a string with another.
    pub fn pad_both(value: &str, length: usize, pad: &str) -> String {
        let current = value.chars().count();
        if current >= length || pad.is_empty() {
            return value.to_string();
        }
        let total = length - current;
        let left = total / 2;
        let right = total - left;
        format!("{}{}{}", repeat_to(pad, left), value, repeat_to(pad, right))
    }

    /// Pad the left side of a string with another.
    pub fn pad_left(value: &str, length: usize, pad: &str) -> String {
        let current = value.chars().count();
        if current >= length || pad.is_empty() {
            return value.to_string();
        }
        format!("{}{}", repeat_to(pad, length - current), value)
    }

    /// Pad the right side of a string with another.
    pub fn pad_right(value: &str, length: usize, pad: &str) -> String {
        let current = value.chars().count();
        if current >= length || pad.is_empty() {
            return value.to_string();
        }
        format!("{}{}", value, repeat_to(pad, length - current))
    }

    /// Repeat the given string.
    pub fn repeat(value: &str, times: usize) -> String {
        value.repeat(times)
    }

    /// Reverse the given string.
    pub fn reverse(value: &str) -> String {
        value.chars().rev().collect()
    }

    /// Get the class "basename" of the given type path (`app::models::User` => `User`).
    pub fn class_basename(path: &str) -> String {
        let without_generics = path.split('<').next().unwrap_or(path);
        without_generics
            .rsplit("::")
            .next()
            .unwrap_or(without_generics)
            .rsplit('\\')
            .next()
            .unwrap_or(without_generics)
            .to_string()
    }

    /// Generate a more truly "random" password.
    pub fn password(length: usize) -> String {
        const LETTERS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
        const NUMBERS: &str = "0123456789";
        const SYMBOLS: &str = "~!#$%^&*()-_.,<>?/\\{}[]|:;";
        let pool: Vec<char> = format!("{LETTERS}{NUMBERS}{SYMBOLS}").chars().collect();
        let mut rng = rand::rng();
        let mut out: Vec<char> = vec![
            LETTERS.chars().nth(rng.random_range(0..LETTERS.len())).unwrap(),
            NUMBERS.chars().nth(rng.random_range(0..NUMBERS.len())).unwrap(),
            SYMBOLS.chars().nth(rng.random_range(0..SYMBOLS.chars().count())).unwrap(),
        ];
        while out.len() < length {
            out.push(pool[rng.random_range(0..pool.len())]);
        }
        use rand::seq::SliceRandom;
        out.shuffle(&mut rng);
        out.into_iter().take(length).collect()
    }
}

fn repeat_to(pad: &str, length: usize) -> String {
    pad.chars().cycle().take(length).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_converts_case() {
        assert_eq!(Str::snake("LaravelPHPFramework"), "laravel_p_h_p_framework");
        assert_eq!(Str::snake("fooBar"), "foo_bar");
        assert_eq!(Str::snake("Foo Bar"), "foo_bar");
        assert_eq!(Str::kebab("fooBar"), "foo-bar");
        assert_eq!(Str::camel("foo_bar"), "fooBar");
        assert_eq!(Str::studly("foo-bar_baz"), "FooBarBaz");
        assert_eq!(Str::title("a nice title uses the correct case"), "A Nice Title Uses The Correct Case");
        assert_eq!(Str::headline("EmailNotificationSent"), "Email Notification Sent");
        assert_eq!(Str::headline("user_profile-name"), "User Profile Name");
    }

    #[test]
    fn it_slices_strings() {
        assert_eq!(Str::after("This is my name", "This is"), " my name");
        assert_eq!(Str::before("This is my name", "my name"), "This is ");
        assert_eq!(Str::between("This is my name", "This", "name"), " is my ");
        assert_eq!(Str::limit("The quick brown fox jumps over the lazy dog", 20), "The quick brown fox...");
        assert_eq!(Str::words("Perfectly balanced, as all things should be.", 3), "Perfectly balanced, as...");
        assert_eq!(Str::substr("Laravel", -3, None), "vel");
        assert_eq!(Str::mask("taylor@example.com", '*', 3, None), "tay***************");
    }

    #[test]
    fn it_slugs() {
        assert_eq!(Str::slug("Laravel 5 Framework"), "laravel-5-framework");
        assert_eq!(Str::slug("hello world_foo"), "hello-world-foo");
        assert_eq!(Str::slug("Crème Brûlée"), "creme-brulee");
        assert_eq!(Str::slug_with("Hello World", "_"), "hello_world");
    }

    #[test]
    fn it_matches_patterns() {
        assert!(Str::is("foo*", "foobar"));
        assert!(Str::is("*.example.com", "api.example.com"));
        assert!(!Str::is("foo", "foobar"));
    }

    #[test]
    fn it_pluralizes_studly_strings() {
        assert_eq!(Str::plural_studly("VerifiedHuman"), "VerifiedHumans");
        assert_eq!(Str::plural_studly("UserFeedback"), "UserFeedback");
    }

    #[test]
    fn it_finds_class_basenames() {
        assert_eq!(Str::class_basename("app::models::User"), "User");
        assert_eq!(Str::class_basename("App\\Models\\User"), "User");
        assert_eq!(Str::class_basename("User"), "User");
    }

    #[test]
    fn it_pads_and_finishes() {
        assert_eq!(Str::pad_both("James", 10, "_"), "__James___");
        assert_eq!(Str::pad_left("James", 10, "-="), "-=-=-James");
        assert_eq!(Str::finish("this/string", "/"), "this/string/");
        assert_eq!(Str::start("/this/string", "/"), "/this/string");
    }
}
