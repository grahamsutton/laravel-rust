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
//!
//! Methods that accept a regular expression (`match_`, `is_match`,
//! `replace_matches`, ...) take PHP-style patterns such as `/foo (.*)/i`;
//! see the [`preg`] module for the supported syntax.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use base64::Engine;
use rand::Rng;
use regex::{Captures, Regex};

use crate::carbon::Carbon;
use crate::collection::Collection;
use crate::number::Number;
use crate::pluralizer::Pluralizer;
use crate::preg;
use crate::stringable::Stringable;
use crate::transliteration;

static SNAKE_CACHE: LazyLock<Mutex<HashMap<(String, String), String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The characters Laravel considers "invisible" when trimming strings.
pub const INVISIBLE_CHARACTERS: &[char] = &[
    '\u{0009}', '\u{0020}', '\u{00A0}', '\u{00AD}', '\u{034F}', '\u{061C}', '\u{115F}', '\u{1160}',
    '\u{17B4}', '\u{17B5}', '\u{180E}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}',
    '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{200B}', '\u{200C}',
    '\u{200D}', '\u{200E}', '\u{200F}', '\u{202F}', '\u{205F}', '\u{2060}', '\u{2061}', '\u{2062}',
    '\u{2063}', '\u{2064}', '\u{2065}', '\u{206A}', '\u{206B}', '\u{206C}', '\u{206D}', '\u{206E}',
    '\u{206F}', '\u{3000}', '\u{2800}', '\u{3164}', '\u{FEFF}', '\u{FFA0}', '\u{1D159}', '\u{1D173}',
    '\u{1D174}', '\u{1D175}', '\u{1D176}', '\u{1D177}', '\u{1D178}', '\u{1D179}', '\u{1D17A}',
    '\u{E0020}',
];

type UuidFactory = Box<dyn FnMut() -> uuid::Uuid>;
type UlidFactory = Box<dyn FnMut() -> ulid::Ulid>;
type RandomFactory = Box<dyn FnMut(usize) -> String>;

thread_local! {
    static UUID_FACTORY: RefCell<Option<UuidFactory>> = RefCell::new(None);
    static ULID_FACTORY: RefCell<Option<UlidFactory>> = RefCell::new(None);
    static RANDOM_FACTORY: RefCell<Option<RandomFactory>> = RefCell::new(None);
}

/// Call a faked factory stored in a thread-local slot, if there is one.
fn call_factory<F: ?Sized, R>(
    slot: &'static std::thread::LocalKey<RefCell<Option<Box<F>>>>,
    call: impl FnOnce(&mut Box<F>) -> R,
) -> Option<R> {
    let mut factory = slot.with(|cell| cell.borrow_mut().take())?;
    let result = call(&mut factory);
    slot.with(|cell| {
        let mut current = cell.borrow_mut();
        if current.is_none() {
            *current = Some(factory);
        }
    });
    Some(result)
}

/// Options for [`Str::excerpt`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExcerptOptions {
    /// How many characters to keep on each side of the phrase (default 100).
    pub radius: usize,
    /// The string used to mark omitted text (default `...`).
    pub omission: String,
}

impl Default for ExcerptOptions {
    fn default() -> Self {
        Self {
            radius: 100,
            omission: "...".to_string(),
        }
    }
}

impl ExcerptOptions {
    /// The default options: a radius of 100 and a `...` omission.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the number of characters to keep on each side of the phrase.
    pub fn radius(mut self, radius: usize) -> Self {
        self.radius = radius;
        self
    }

    /// Set the string used to mark omitted text.
    pub fn omission(mut self, omission: impl Into<String>) -> Self {
        self.omission = omission.into();
        self
    }
}

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

    /// Get the character at the given index (negative indexes count from the end).
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::char_at("This is my name.", 6), Some('s'));
    /// assert_eq!(Str::char_at("Laravel", -1), Some('l'));
    /// assert_eq!(Str::char_at("Laravel", 100), None);
    /// ```
    pub fn char_at(subject: &str, index: isize) -> Option<char> {
        let length = subject.chars().count() as isize;
        let index = if index < 0 { length + index } else { index };
        if index < 0 || index >= length {
            return None;
        }
        subject.chars().nth(index as usize)
    }

    /// Remove the given string from the start of the subject, if present.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::chop_start("https://laravel.com", "https://"), "laravel.com");
    /// ```
    pub fn chop_start(subject: &str, needle: &str) -> String {
        Self::chop_start_any(subject, &[needle])
    }

    /// Remove the first matching needle from the start of the subject.
    pub fn chop_start_any(subject: &str, needles: &[&str]) -> String {
        for needle in needles {
            if let Some(rest) = subject.strip_prefix(needle).filter(|_| !needle.is_empty()) {
                return rest.to_string();
            }
        }
        subject.to_string()
    }

    /// Remove the given string from the end of the subject, if present.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::chop_end("app/Models/Photograph.php", ".php"), "app/Models/Photograph");
    /// ```
    pub fn chop_end(subject: &str, needle: &str) -> String {
        Self::chop_end_any(subject, &[needle])
    }

    /// Remove the first matching needle from the end of the subject.
    pub fn chop_end_any(subject: &str, needles: &[&str]) -> String {
        for needle in needles {
            if let Some(rest) = subject.strip_suffix(needle).filter(|_| !needle.is_empty()) {
                return rest.to_string();
            }
        }
        subject.to_string()
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
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::snake_with("fooBar", "-"), "foo-bar");
    /// assert_eq!(Str::snake("Laravel Php Framework"), "laravel_php_framework");
    /// ```
    pub fn snake_with(value: &str, delimiter: &str) -> String {
        let key = (value.to_string(), delimiter.to_string());
        if let Some(cached) = SNAKE_CACHE.lock().unwrap().get(&key) {
            return cached.clone();
        }

        // Like PHP's ctype_lower(): only plain lowercase ASCII letters.
        let all_lower = !value.is_empty() && value.bytes().all(|b| b.is_ascii_lowercase());
        let mut result = value.to_string();
        if !all_lower {
            let words: String = php_ucwords(value)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            let chars: Vec<char> = words.chars().collect();
            let mut out = String::with_capacity(words.len() + 4);
            for (i, c) in chars.iter().enumerate() {
                out.push(*c);
                if chars.get(i + 1).is_some_and(|next| next.is_ascii_uppercase()) {
                    out.push_str(delimiter);
                }
            }
            result = out.to_lowercase();
        }

        SNAKE_CACHE.lock().unwrap().insert(key, result.clone());
        result
    }

    /// Convert a string to kebab case.
    pub fn kebab(value: &str) -> String {
        Self::snake_with(value, "-")
    }

    /// Convert the given string to title case, exactly like PHP's
    /// `mb_convert_case($value, MB_CASE_TITLE)`.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::title("a nice title uses the correct case"), "A Nice Title Uses The Correct Case");
    /// ```
    pub fn title(value: &str) -> String {
        let mut out = String::with_capacity(value.len());
        let mut in_word = false;
        for c in value.chars() {
            if in_word {
                out.extend(c.to_lowercase());
            } else {
                push_titlecase(&mut out, c);
            }
            if !is_case_ignorable(c) {
                in_word = is_cased(c);
            }
        }
        out
    }

    /// Convert the given string to title case for each word, splitting on
    /// case changes, hyphens and underscores ("email_address" => "Email Address").
    pub fn headline(value: &str) -> String {
        let parts: Vec<&str> = value.split_whitespace().collect();
        let parts: Vec<String> = if parts.len() > 1 {
            parts.iter().map(|p| Self::title(p)).collect()
        } else {
            Self::ucsplit(&parts.join("_"))
                .iter()
                .map(|p| Self::title(p))
                .collect()
        };
        parts
            .join("_")
            .replace(['-', ' '], "_")
            .split('_')
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Convert the given string to APA-style title case.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::apa("Creating A Project"), "Creating a Project");
    /// assert_eq!(Str::apa("back to the future"), "Back to the Future");
    /// ```
    pub fn apa(value: &str) -> String {
        if value.trim().is_empty() {
            return value.to_string();
        }
        const MINOR: &[&str] = &[
            "and", "as", "but", "for", "if", "nor", "or", "so", "yet", "a", "an", "the", "at",
            "by", "in", "of", "off", "on", "per", "to", "up", "via", "et", "ou", "un", "une", "la",
            "le", "les", "de", "du", "des", "par", "à",
        ];
        const END_PUNCTUATION: &[char] = &['.', '!', '?', ':', '—', ','];
        let is_minor = |word: &str| MINOR.contains(&word) && word.chars().count() <= 3;

        let words: Vec<&str> = value.split_whitespace().collect();
        let mut out: Vec<String> = Vec::with_capacity(words.len());
        for (i, word) in words.iter().enumerate() {
            let lower = word.to_lowercase();
            if lower.contains('-') {
                let parts: Vec<String> = lower
                    .split('-')
                    .map(|part| {
                        if is_minor(part) {
                            part.to_string()
                        } else {
                            mb_ucfirst(part)
                        }
                    })
                    .collect();
                out.push(parts.join("-"));
            } else {
                let after_punctuation = i > 0
                    && words[i - 1]
                        .chars()
                        .last()
                        .is_some_and(|c| END_PUNCTUATION.contains(&c));
                if is_minor(&lower) && i != 0 && !after_punctuation {
                    out.push(lower);
                } else {
                    out.push(mb_ucfirst(&lower));
                }
            }
        }
        out.join(" ")
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
        Self::ucwords_with(value, " \t\r\n\u{0C}\u{0B}")
    }

    /// Uppercase the first character of each word, using custom word separators.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::ucwords_with("hello_world-foo", "_-"), "Hello_World-Foo");
    /// ```
    pub fn ucwords_with(value: &str, separators: &str) -> String {
        let mut out = String::with_capacity(value.len());
        let mut capitalize = true;
        for c in value.chars() {
            if capitalize && c.is_lowercase() {
                out.extend(c.to_uppercase());
            } else {
                out.push(c);
            }
            capitalize = separators.contains(c);
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

    /// Get the initials of each word in the string.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::initials("taylor otwell"), "to");
    /// assert_eq!(Str::initials_upper("taylor otwell"), "TO");
    /// ```
    pub fn initials(value: &str) -> String {
        value
            .split_whitespace()
            .filter_map(|word| word.chars().next())
            .collect()
    }

    /// Get the capitalized initials of each word in the string.
    pub fn initials_upper(value: &str) -> String {
        Self::upper(&Self::initials(value))
    }

    /// Get the plural form of an English word.
    pub fn plural(value: &str) -> String {
        Pluralizer::plural(value, 2)
    }

    /// Get the plural form of an English word, given a count.
    pub fn plural_count(value: &str, count: i64) -> String {
        Pluralizer::plural(value, count)
    }

    /// Pluralize a word and prefix it with its formatted count.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::counted("order", 1), "1 order");
    /// assert_eq!(Str::counted("order", 1000), "1,000 orders");
    /// ```
    pub fn counted(value: &str, count: i64) -> String {
        format!(
            "{} {}",
            Number::format(count as f64, None),
            Pluralizer::plural(value, count)
        )
    }

    /// Pluralize the last word of an English, studly caps case string.
    pub fn plural_studly(value: &str) -> String {
        Self::plural_studly_count(value, 2)
    }

    /// Pluralize the last word of a studly caps case string, given a count.
    pub fn plural_studly_count(value: &str, count: i64) -> String {
        let parts = Self::ucsplit(value);
        match parts.split_last() {
            Some((last, rest)) => format!("{}{}", rest.concat(), Self::plural_count(last, count)),
            None => String::new(),
        }
    }

    /// Alias of `plural_studly`.
    pub fn plural_pascal(value: &str) -> String {
        Self::plural_studly(value)
    }

    /// Alias of `plural_studly_count`.
    pub fn plural_pascal_count(value: &str, count: i64) -> String {
        Self::plural_studly_count(value, count)
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
        Self::slug_with_dictionary(title, separator, &[("@", "at")])
    }

    /// Generate a URL friendly "slug", replacing dictionary words first.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::slug_with_dictionary("500$ bill", "-", &[("$", "dollar")]), "500-dollar-bill");
    /// ```
    pub fn slug_with_dictionary(title: &str, separator: &str, dictionary: &[(&str, &str)]) -> String {
        let mut title = Self::ascii(title);

        // Convert all dashes/underscores into the separator.
        let flip = if separator == "-" { '_' } else { '-' };
        title = collapse_runs(&title, |c| c == flip, separator);

        for (key, value) in dictionary {
            if !key.is_empty() {
                title = title.replace(key, &format!("{separator}{value}{separator}"));
            }
        }

        // Remove everything that isn't the separator, a letter, a number or whitespace.
        let lower = title.to_lowercase();
        let cleaned: String = lower
            .chars()
            .filter(|c| separator.contains(*c) || c.is_alphanumeric() || c.is_whitespace())
            .collect();

        // Collapse separators and whitespace into a single separator.
        let collapsed =
            collapse_runs(&cleaned, |c| separator.contains(c) || c.is_whitespace(), separator);
        collapsed
            .trim_matches(|c| separator.contains(c))
            .to_string()
    }

    /// Transliterate a UTF-8 value to ASCII (best effort). Characters that
    /// have no ASCII equivalent are removed.
    pub fn ascii(value: &str) -> String {
        transliteration::to_ascii(value, None, None)
    }

    /// Transliterate a UTF-8 value to ASCII using a language's conventions
    /// (`de` turns `ä` into `ae`, for example).
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::ascii_with_language("Grüße", "de"), "Gruesse");
    /// ```
    pub fn ascii_with_language(value: &str, language: &str) -> String {
        transliteration::to_ascii(value, Some(language), None)
    }

    /// Transliterate a string to its closest ASCII representation, using `?`
    /// for characters that cannot be transliterated.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::transliterate("ⓣⓔⓢⓣ@ⓛⓐⓡⓐⓥⓔⓛ.ⓒⓞⓜ"), "test@laravel.com");
    /// ```
    pub fn transliterate(value: &str) -> String {
        Self::transliterate_with(value, "?")
    }

    /// Transliterate a string to ASCII, using `unknown` for characters that
    /// cannot be transliterated.
    pub fn transliterate_with(value: &str, unknown: &str) -> String {
        transliteration::to_ascii(value, None, Some(unknown))
    }

    /// Determine if a given string contains a given substring.
    pub fn contains(haystack: &str, needle: &str) -> bool {
        !needle.is_empty() && haystack.contains(needle)
    }

    /// Determine if a given string contains any of the given substrings.
    pub fn contains_any(haystack: &str, needles: &[&str]) -> bool {
        needles.iter().any(|n| Self::contains(haystack, n))
    }

    /// Determine if a given string contains any of the given substrings, ignoring case.
    pub fn contains_any_ignore_case(haystack: &str, needles: &[&str]) -> bool {
        needles.iter().any(|n| Self::contains_ignore_case(haystack, n))
    }

    /// Determine if a given string contains all of the given substrings.
    pub fn contains_all(haystack: &str, needles: &[&str]) -> bool {
        !needles.is_empty() && needles.iter().all(|n| Self::contains(haystack, n))
    }

    /// Determine if a given string contains all of the given substrings, ignoring case.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert!(Str::contains_all_ignore_case("This is my name", &["MY", "NAME"]));
    /// ```
    pub fn contains_all_ignore_case(haystack: &str, needles: &[&str]) -> bool {
        !needles.is_empty() && needles.iter().all(|n| Self::contains_ignore_case(haystack, n))
    }

    /// Determine if a given string contains a substring, ignoring case.
    pub fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
        Self::contains(&haystack.to_lowercase(), &needle.to_lowercase())
    }

    /// Determine if a given string doesn't contain a given substring.
    pub fn doesnt_contain(haystack: &str, needle: &str) -> bool {
        !Self::contains(haystack, needle)
    }

    /// Determine if a given string doesn't contain any of the given substrings.
    pub fn doesnt_contain_any(haystack: &str, needles: &[&str]) -> bool {
        !Self::contains_any(haystack, needles)
    }

    /// Determine if a given string doesn't contain a given substring, ignoring case.
    pub fn doesnt_contain_ignore_case(haystack: &str, needle: &str) -> bool {
        !Self::contains_ignore_case(haystack, needle)
    }

    /// Replace consecutive instances of a space with a single space.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::deduplicate("The   Laravel   Framework"), "The Laravel Framework");
    /// assert_eq!(Str::deduplicate_with("The---Laravel---Framework", "-"), "The-Laravel-Framework");
    /// ```
    pub fn deduplicate(value: &str) -> String {
        Self::deduplicate_with(value, " ")
    }

    /// Replace consecutive instances of the given character with a single one.
    pub fn deduplicate_with(value: &str, character: &str) -> String {
        if character.is_empty() {
            return value.to_string();
        }
        let doubled = format!("{character}{character}");
        let mut out = value.to_string();
        while out.contains(&doubled) {
            out = out.replace(&doubled, character);
        }
        out
    }

    /// Replace consecutive instances of each of the given characters.
    pub fn deduplicate_many(value: &str, characters: &[&str]) -> String {
        characters
            .iter()
            .fold(value.to_string(), |carry, c| Self::deduplicate_with(&carry, c))
    }

    /// Determine if a given string starts with a given substring.
    pub fn starts_with(haystack: &str, needle: &str) -> bool {
        !needle.is_empty() && haystack.starts_with(needle)
    }

    /// Determine if a given string starts with any of the given substrings.
    pub fn starts_with_any(haystack: &str, needles: &[&str]) -> bool {
        needles.iter().any(|n| Self::starts_with(haystack, n))
    }

    /// Determine if a given string doesn't start with a given substring.
    pub fn doesnt_start_with(haystack: &str, needle: &str) -> bool {
        !Self::starts_with(haystack, needle)
    }

    /// Determine if a given string doesn't start with any of the given substrings.
    pub fn doesnt_start_with_any(haystack: &str, needles: &[&str]) -> bool {
        !Self::starts_with_any(haystack, needles)
    }

    /// Determine if a given string ends with a given substring.
    pub fn ends_with(haystack: &str, needle: &str) -> bool {
        !needle.is_empty() && haystack.ends_with(needle)
    }

    /// Determine if a given string ends with any of the given substrings.
    pub fn ends_with_any(haystack: &str, needles: &[&str]) -> bool {
        needles.iter().any(|n| Self::ends_with(haystack, n))
    }

    /// Determine if a given string doesn't end with a given substring.
    pub fn doesnt_end_with(haystack: &str, needle: &str) -> bool {
        !Self::ends_with(haystack, needle)
    }

    /// Determine if a given string doesn't end with any of the given substrings.
    pub fn doesnt_end_with_any(haystack: &str, needles: &[&str]) -> bool {
        !Self::ends_with_any(haystack, needles)
    }

    /// Extract an excerpt from text that matches the first instance of a phrase.
    ///
    /// ```
    /// use illuminate_support::Str;
    /// use illuminate_support::str::ExcerptOptions;
    ///
    /// let excerpt = Str::excerpt("This is my name", "my", ExcerptOptions::new().radius(3));
    /// assert_eq!(excerpt.as_deref(), Some("...is my na..."));
    ///
    /// let excerpt = Str::excerpt("This is my name", "name", ExcerptOptions::new().radius(3).omission("(...) "));
    /// assert_eq!(excerpt.as_deref(), Some("(...) my name"));
    /// ```
    pub fn excerpt(text: &str, phrase: &str, options: ExcerptOptions) -> Option<String> {
        let pattern = format!("(?i)^(.*?)({})(.*)$", regex::escape(phrase));
        let regex = preg::compile_raw(&pattern).ok()?;
        let caps = regex.captures(text)?;
        let radius = options.radius;
        let omission = options.omission.as_str();

        let start = Self::ltrim(&caps[1]);
        let start_length = start.chars().count();
        let skip = start_length.saturating_sub(radius);
        let start_with_radius = Self::ltrim(&start.chars().skip(skip).take(radius).collect::<String>());
        let start = if start_with_radius == start {
            start_with_radius
        } else {
            format!("{omission}{start_with_radius}")
        };

        let end = Self::rtrim(&caps[3]);
        let end_with_radius = Self::rtrim(&end.chars().take(radius).collect::<String>());
        let end = if end_with_radius == end {
            end_with_radius
        } else {
            format!("{end_with_radius}{omission}")
        };

        Some(format!("{start}{}{end}", &caps[2]))
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
        wildcard_match(pattern, value, false)
    }

    /// Determine if a given string matches a given pattern, ignoring case.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert!(Str::is_ignore_case("*.jpg", "photo.JPG"));
    /// ```
    pub fn is_ignore_case(pattern: &str, value: &str) -> bool {
        wildcard_match(pattern, value, true)
    }

    /// Determine if a given string matches any of the given patterns.
    pub fn is_any(patterns: &[&str], value: &str) -> bool {
        patterns.iter().any(|p| Self::is(p, value))
    }

    /// Determine if a given string matches any of the given patterns, ignoring case.
    pub fn is_any_ignore_case(patterns: &[&str], value: &str) -> bool {
        patterns.iter().any(|p| Self::is_ignore_case(p, value))
    }

    /// Determine if a given string is valid JSON.
    pub fn is_json(value: &str) -> bool {
        serde_json::from_str::<serde_json::Value>(value).is_ok()
    }

    /// Determine if a given string is a valid UUID.
    pub fn is_uuid(value: &str) -> bool {
        uuid::Uuid::parse_str(value).is_ok() && value.len() == 36
    }

    /// Determine if a given string is a valid UUID of the given version
    /// (`0` checks for the "nil" UUID).
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert!(Str::is_uuid_version("a0a2a2d2-0b87-4a18-83f2-2529882be2de", 4));
    /// assert!(!Str::is_uuid_version("a0a2a2d2-0b87-4a18-83f2-2529882be2de", 1));
    /// ```
    pub fn is_uuid_version(value: &str, version: u8) -> bool {
        if !Self::is_uuid(value) {
            return false;
        }
        match uuid::Uuid::parse_str(value) {
            Ok(uuid) if version == 0 => uuid.is_nil(),
            Ok(uuid) => uuid.get_version_num() == version as usize,
            Err(_) => false,
        }
    }

    /// Determine if a given string is a valid ULID.
    pub fn is_ulid(value: &str) -> bool {
        value.len() == 26
            && value.as_bytes()[0] <= b'7'
            && ulid::Ulid::from_string(value).is_ok()
    }

    /// Determine if a given string is 7 bit ASCII.
    pub fn is_ascii(value: &str) -> bool {
        value.is_ascii()
    }

    /// Determine if a given value is a valid URL.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert!(Str::is_url("https://laravel.com"));
    /// assert!(!Str::is_url("laravel"));
    /// ```
    pub fn is_url(value: &str) -> bool {
        static URL: LazyLock<Option<Regex>> =
            LazyLock::new(|| preg::compile(&url_pattern(URL_PROTOCOLS)).ok());
        URL.as_ref().is_some_and(|r| r.is_match(value))
    }

    /// Determine if a given value is a valid URL using one of the given protocols.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert!(Str::is_url_with_protocols("https://laravel.com", &["http", "https"]));
    /// assert!(!Str::is_url_with_protocols("ftp://laravel.com", &["http", "https"]));
    /// ```
    pub fn is_url_with_protocols(value: &str, protocols: &[&str]) -> bool {
        if protocols.is_empty() {
            return Self::is_url(value);
        }
        let list = protocols
            .iter()
            .map(|p| regex::escape(p))
            .collect::<Vec<_>>()
            .join("|");
        preg::is_match(&url_pattern(&list), value)
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
    ///
    /// Like PHP's `mb_strimwidth`, wide (East Asian) characters count twice.
    pub fn limit_with(value: &str, limit: usize, end: &str) -> String {
        if str_width(value) <= limit {
            return value.to_string();
        }
        format!("{}{}", strimwidth(value, limit).trim_end(), end)
    }

    /// Limit the number of characters in a string without cutting words in half.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::limit_preserving_words("The quick brown fox", 12, "..."), "The quick...");
    /// ```
    pub fn limit_preserving_words(value: &str, limit: usize, end: &str) -> String {
        if str_width(value) <= limit {
            return value.to_string();
        }
        let value = Self::strip_tags(value)
            .split(['\n', '\r'])
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let value = value.trim();
        let trimmed = strimwidth(value, limit).trim_end().to_string();
        if value.chars().nth(limit) == Some(' ') {
            return format!("{trimmed}{end}");
        }
        match trimmed.rfind(char::is_whitespace) {
            Some(index) => format!("{}{end}", &trimmed[..index]),
            None => format!("{trimmed}{end}"),
        }
    }

    /// Limit the number of words in a string.
    pub fn words(value: &str, words: usize) -> String {
        Self::words_with(value, words, "...")
    }

    /// Limit the number of words in a string, with a custom ending.
    pub fn words_with(value: &str, words: usize, end: &str) -> String {
        let mut count = 0;
        let mut in_word = false;
        let mut cut = None;
        for (index, c) in value.char_indices() {
            if c.is_whitespace() {
                in_word = false;
            } else if !in_word {
                in_word = true;
                if count == words {
                    cut = Some(index);
                    break;
                }
                count += 1;
            }
        }
        match cut {
            Some(index) if words > 0 => format!("{}{end}", value[..index].trim_end()),
            _ => value.to_string(),
        }
    }

    /// Count the number of words in a string, like PHP's `str_word_count`.
    pub fn word_count(value: &str) -> usize {
        Self::word_count_with(value, "")
    }

    /// Count the number of words in a string, treating the given characters
    /// as part of words.
    pub fn word_count_with(value: &str, characters: &str) -> usize {
        let is_word = |c: char| c.is_alphabetic() || c == '\'' || c == '-' || characters.contains(c);
        let chars: Vec<char> = value.chars().collect();
        let mut start = 0;
        let mut end = chars.len();
        if chars.first().is_some_and(|c| (*c == '\'' || *c == '-') && !characters.contains(*c)) {
            start = 1;
        }
        if end > start && chars[end - 1] == '-' && !characters.contains('-') {
            end -= 1;
        }
        let mut count = 0;
        let mut i = start;
        while i < end {
            let begin = i;
            while i < end && is_word(chars[i]) {
                i += 1;
            }
            if i > begin {
                count += 1;
            } else {
                i += 1;
            }
        }
        count
    }

    /// Wrap a string to a given number of characters, like PHP's `wordwrap`.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// let text = "The quick brown fox jumped over the lazy dog.";
    /// assert_eq!(
    ///     Str::word_wrap(text, 20, "<br />\n", false),
    ///     "The quick brown fox<br />\njumped over the lazy<br />\ndog."
    /// );
    /// ```
    pub fn word_wrap(value: &str, characters: usize, break_with: &str, cut_long_words: bool) -> String {
        word_wrap(value, characters, break_with, cut_long_words)
    }

    /// Generate a "random" alpha-numeric string.
    pub fn random(length: usize) -> String {
        if let Some(fake) = call_factory(&RANDOM_FACTORY, |f| f(length)) {
            return fake;
        }
        random_string(length)
    }

    /// Set the callback that will be used to generate random strings (on this thread).
    pub fn create_random_strings_using(factory: impl FnMut(usize) -> String + 'static) {
        RANDOM_FACTORY.with(|cell| *cell.borrow_mut() = Some(Box::new(factory)));
    }

    /// Return the given strings from `random`, in order, then fall back to
    /// truly random strings.
    pub fn create_random_strings_using_sequence(sequence: Vec<String>) {
        let mut sequence = sequence.into_iter();
        Self::create_random_strings_using(move |length| {
            sequence.next().unwrap_or_else(|| random_string(length))
        });
    }

    /// Indicate that random strings should be created normally again.
    pub fn create_random_strings_normally() {
        RANDOM_FACTORY.with(|cell| *cell.borrow_mut() = None);
    }

    /// Generate a UUID (version 4).
    pub fn uuid() -> uuid::Uuid {
        call_factory(&UUID_FACTORY, |f| f()).unwrap_or_else(uuid::Uuid::new_v4)
    }

    /// Generate a time-ordered UUID (version 7).
    pub fn ordered_uuid() -> uuid::Uuid {
        call_factory(&UUID_FACTORY, |f| f()).unwrap_or_else(uuid::Uuid::now_v7)
    }

    /// Alias of `ordered_uuid`.
    pub fn uuid7() -> uuid::Uuid {
        Self::ordered_uuid()
    }

    /// Generate a time-ordered UUID (version 7) for the given moment.
    pub fn uuid7_at(time: Carbon) -> uuid::Uuid {
        if let Some(fake) = call_factory(&UUID_FACTORY, |f| f()) {
            return fake;
        }
        let millis = time.timestamp_millis().max(0) as u64;
        let timestamp = uuid::Timestamp::from_unix(
            uuid::NoContext,
            millis / 1000,
            ((millis % 1000) * 1_000_000) as u32,
        );
        uuid::Uuid::new_v7(timestamp)
    }

    /// Set the callback that will be used to generate UUIDs (on this thread).
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// let fixed = uuid::Uuid::parse_str("eadbfeac-5258-45c2-bab7-ccb9b5ef74f9").unwrap();
    /// Str::create_uuids_using(move || fixed);
    /// assert_eq!(Str::uuid(), fixed);
    /// Str::create_uuids_normally();
    /// assert_ne!(Str::uuid(), fixed);
    /// ```
    pub fn create_uuids_using(factory: impl FnMut() -> uuid::Uuid + 'static) {
        UUID_FACTORY.with(|cell| *cell.borrow_mut() = Some(Box::new(factory)));
    }

    /// Return the given UUIDs, in order, then fall back to random ones.
    pub fn create_uuids_using_sequence(sequence: Vec<uuid::Uuid>) {
        let mut sequence = sequence.into_iter();
        Self::create_uuids_using(move || sequence.next().unwrap_or_else(uuid::Uuid::new_v4));
    }

    /// Always return the same UUID when one is generated, and return it.
    pub fn freeze_uuids() -> uuid::Uuid {
        let uuid = Self::uuid();
        Self::create_uuids_using(move || uuid);
        uuid
    }

    /// Freeze UUIDs while the callback runs, then restore normal generation.
    pub fn freeze_uuids_with<R>(callback: impl FnOnce(uuid::Uuid) -> R) -> R {
        let uuid = Self::freeze_uuids();
        let result = callback(uuid);
        Self::create_uuids_normally();
        result
    }

    /// Indicate that UUIDs should be created normally again.
    pub fn create_uuids_normally() {
        UUID_FACTORY.with(|cell| *cell.borrow_mut() = None);
    }

    /// Generate a ULID.
    pub fn ulid() -> ulid::Ulid {
        call_factory(&ULID_FACTORY, |f| f()).unwrap_or_else(ulid::Ulid::new)
    }

    /// Generate a ULID for the given moment.
    pub fn ulid_at(time: Carbon) -> ulid::Ulid {
        if let Some(fake) = call_factory(&ULID_FACTORY, |f| f()) {
            return fake;
        }
        let millis = time.timestamp_millis().max(0) as u64;
        ulid::Ulid::from_datetime(std::time::UNIX_EPOCH + std::time::Duration::from_millis(millis))
    }

    /// Set the callback that will be used to generate ULIDs (on this thread).
    pub fn create_ulids_using(factory: impl FnMut() -> ulid::Ulid + 'static) {
        ULID_FACTORY.with(|cell| *cell.borrow_mut() = Some(Box::new(factory)));
    }

    /// Return the given ULIDs, in order, then fall back to new ones.
    pub fn create_ulids_using_sequence(sequence: Vec<ulid::Ulid>) {
        let mut sequence = sequence.into_iter();
        // `Ulid::default()` is the nil ULID, so a fresh one must be generated.
        #[allow(clippy::unwrap_or_default)]
        Self::create_ulids_using(move || sequence.next().unwrap_or_else(ulid::Ulid::new));
    }

    /// Always return the same ULID when one is generated, and return it.
    pub fn freeze_ulids() -> ulid::Ulid {
        let ulid = Self::ulid();
        Self::create_ulids_using(move || ulid);
        ulid
    }

    /// Indicate that ULIDs should be created normally again.
    pub fn create_ulids_normally() {
        ULID_FACTORY.with(|cell| *cell.borrow_mut() = None);
    }

    /// Reset every faked factory (UUIDs, ULIDs and random strings).
    pub fn reset_factory_state() {
        Self::create_random_strings_normally();
        Self::create_ulids_normally();
        Self::create_uuids_normally();
    }

    /// Clear the internal case-conversion caches.
    pub fn flush_cache() {
        SNAKE_CACHE.lock().unwrap().clear();
    }

    /// Replace the first occurrence of a given value in the string.
    pub fn replace_first(search: &str, replace: &str, subject: &str) -> String {
        if search.is_empty() {
            return subject.to_string();
        }
        subject.replacen(search, replace, 1)
    }

    /// Replace the first occurrence of the value only if it appears at the start.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::replace_start("Hello", "Laravel", "Hello World"), "Laravel World");
    /// assert_eq!(Str::replace_start("World", "Laravel", "Hello World"), "Hello World");
    /// ```
    pub fn replace_start(search: &str, replace: &str, subject: &str) -> String {
        if Self::starts_with(subject, search) {
            Self::replace_first(search, replace, subject)
        } else {
            subject.to_string()
        }
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

    /// Replace the last occurrence of the value only if it appears at the end.
    pub fn replace_end(search: &str, replace: &str, subject: &str) -> String {
        if Self::ends_with(subject, search) {
            Self::replace_last(search, replace, subject)
        } else {
            subject.to_string()
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
        if search.is_empty() {
            return subject.to_string();
        }
        subject.replace(search, replace)
    }

    /// Replace all occurrences of the search string, ignoring case.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(
    ///     Str::replace_ignore_case("php", "Laravel", "PHP Framework for Web Artisans"),
    ///     "Laravel Framework for Web Artisans"
    /// );
    /// ```
    pub fn replace_ignore_case(search: &str, replace: &str, subject: &str) -> String {
        if search.is_empty() {
            return subject.to_string();
        }
        match preg::compile_raw(&format!("(?i){}", regex::escape(search))) {
            Ok(regex) => regex.replace_all(subject, regex::NoExpand(replace)).into_owned(),
            Err(_) => subject.to_string(),
        }
    }

    /// Replace each of the search strings with the replacement, in order.
    pub fn replace_many(searches: &[&str], replace: &str, subject: &str) -> String {
        searches
            .iter()
            .fold(subject.to_string(), |carry, search| Self::replace(search, replace, &carry))
    }

    /// Replace the patterns matching the given regular expression.
    ///
    /// `$1` / `\1` style references to capture groups are supported.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::replace_matches("/[^A-Za-z0-9]++/", "", "(+1) 501-555-1000"), "15015551000");
    /// ```
    pub fn replace_matches(pattern: &str, replace: &str, subject: &str) -> String {
        preg::replace(pattern, replace, subject).unwrap_or_else(|_| subject.to_string())
    }

    /// Replace at most `limit` matches of the given regular expression.
    pub fn replace_matches_limit(pattern: &str, replace: &str, subject: &str, limit: usize) -> String {
        preg::replace_limit(pattern, replace, subject, Some(limit))
            .unwrap_or_else(|_| subject.to_string())
    }

    /// Replace the matches of the given regular expression using a callback.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// let replaced = Str::replace_matches_using("/\\d/", |matches| format!("[{}]", &matches[0]), "123");
    /// assert_eq!(replaced, "[1][2][3]");
    /// ```
    pub fn replace_matches_using(
        pattern: &str,
        callback: impl FnMut(&Captures<'_>) -> String,
        subject: &str,
    ) -> String {
        preg::replace_callback(pattern, callback, subject, None)
            .unwrap_or_else(|_| subject.to_string())
    }

    /// Remove any occurrence of the given string in the subject.
    pub fn remove(search: &str, subject: &str) -> String {
        Self::replace(search, "", subject)
    }

    /// Remove any occurrence of the given string, ignoring case.
    pub fn remove_ignore_case(search: &str, subject: &str) -> String {
        Self::replace_ignore_case(search, "", subject)
    }

    /// Remove every occurrence of each of the given strings.
    pub fn remove_many(searches: &[&str], subject: &str) -> String {
        Self::replace_many(searches, "", subject)
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

    /// Unwrap the string with the given strings.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::unwrap("-Laravel-", "-", "-"), "Laravel");
    /// assert_eq!(Str::unwrap("{framework: \"Laravel\"}", "{", "}"), "framework: \"Laravel\"");
    /// ```
    pub fn unwrap(value: &str, before: &str, after: &str) -> String {
        let value = Self::chop_start(value, before);
        Self::chop_end(&value, after)
    }

    /// Remove all whitespace (including invisible characters) from both ends.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::trim(" foo bar \u{FEFF}"), "foo bar");
    /// ```
    pub fn trim(value: &str) -> String {
        value.trim_matches(is_trimmable).to_string()
    }

    /// Remove all whitespace (including invisible characters) from the start.
    pub fn ltrim(value: &str) -> String {
        value.trim_start_matches(is_trimmable).to_string()
    }

    /// Remove all whitespace (including invisible characters) from the end.
    pub fn rtrim(value: &str) -> String {
        value.trim_end_matches(is_trimmable).to_string()
    }

    /// Strip the given characters from both ends, like PHP's `trim($value, $chars)`.
    /// Ranges such as `a..z` are supported.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::trim_chars("-foo  bar_", "-_"), "foo  bar");
    /// ```
    pub fn trim_chars(value: &str, characters: &str) -> String {
        let set = char_list(characters);
        value.trim_matches(|c| set.contains(&c)).to_string()
    }

    /// Strip the given characters from the start.
    pub fn ltrim_chars(value: &str, characters: &str) -> String {
        let set = char_list(characters);
        value.trim_start_matches(|c| set.contains(&c)).to_string()
    }

    /// Strip the given characters from the end.
    pub fn rtrim_chars(value: &str, characters: &str) -> String {
        let set = char_list(characters);
        value.trim_end_matches(|c| set.contains(&c)).to_string()
    }

    /// Remove all "extra" blank space from the given string.
    pub fn squish(value: &str) -> String {
        Self::trim(value)
            .split(|c: char| c.is_whitespace() || c == '\u{3164}' || c == '\u{1160}')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
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

    /// Returns the number of non-overlapping substring occurrences.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::substr_count("If you like ice cream, you will like snow cones.", "like"), 2);
    /// ```
    pub fn substr_count(haystack: &str, needle: &str) -> usize {
        if needle.is_empty() {
            return 0;
        }
        haystack.matches(needle).count()
    }

    /// Count substring occurrences within a portion of the haystack.
    pub fn substr_count_in(haystack: &str, needle: &str, offset: isize, length: Option<isize>) -> usize {
        Self::substr_count(&Self::substr(haystack, offset, length), needle)
    }

    /// Replace text within a portion of a string.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::substr_replace("1300", ":", 2, None), "13:");
    /// assert_eq!(Str::substr_replace("1300", ":", 2, Some(0)), "13:00");
    /// ```
    pub fn substr_replace(value: &str, replace: &str, offset: isize, length: Option<isize>) -> String {
        let length = length.unwrap_or(value.chars().count() as isize);
        format!(
            "{}{}{}",
            Self::substr(value, 0, Some(offset)),
            replace,
            Self::substr(&Self::substr(value, offset, None), length, None)
        )
    }

    /// Swap multiple keywords in a string with other keywords, like PHP's `strtr`.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// let swapped = Str::swap(&[("Tacos", "Burritos"), ("great", "fantastic")], "Tacos are great!");
    /// assert_eq!(swapped, "Burritos are fantastic!");
    /// ```
    pub fn swap(map: &[(&str, &str)], subject: &str) -> String {
        let mut pairs: Vec<&(&str, &str)> = map.iter().filter(|(k, _)| !k.is_empty()).collect();
        if pairs.is_empty() {
            return subject.to_string();
        }
        pairs.sort_by_key(|pair| std::cmp::Reverse(pair.0.len()));
        let mut out = String::with_capacity(subject.len());
        let mut rest = subject;
        'outer: while !rest.is_empty() {
            for (key, value) in &pairs {
                if let Some(after) = rest.strip_prefix(key) {
                    out.push_str(value);
                    rest = after;
                    continue 'outer;
                }
            }
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
        out
    }

    /// Take the first (or, when negative, last) `limit` characters of a string.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::take("Build something amazing!", 5), "Build");
    /// assert_eq!(Str::take("abcdef", -2), "ef");
    /// ```
    pub fn take(value: &str, limit: isize) -> String {
        if limit < 0 {
            Self::substr(value, limit, None)
        } else {
            Self::substr(value, 0, Some(limit))
        }
    }

    /// Convert the given string to Base64 encoding.
    pub fn to_base64(value: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(value)
    }

    /// Decode the given Base64 encoded string. Invalid characters are ignored,
    /// like PHP's non-strict `base64_decode`.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::from_base64("TGFyYXZlbA==").as_deref(), Some("Laravel"));
    /// ```
    pub fn from_base64(value: &str) -> Option<String> {
        let cleaned: String = value
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/')
            .collect();
        decode_base64(&cleaned)
    }

    /// Decode the given Base64 string, rejecting any invalid characters.
    pub fn from_base64_strict(value: &str) -> Option<String> {
        let trimmed = value.trim_end_matches('=');
        if !trimmed.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/') {
            return None;
        }
        decode_base64(trimmed)
    }

    /// Find the (character) position of the first occurrence of a substring.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::position("Hello, World!", "Hello"), Some(0));
    /// assert_eq!(Str::position("Hello, World!", "W"), Some(7));
    /// assert_eq!(Str::position("Hello, World!", "X"), None);
    /// ```
    pub fn position(haystack: &str, needle: &str) -> Option<usize> {
        Self::position_from(haystack, needle, 0)
    }

    /// Find the position of a substring, starting the search at `offset`
    /// characters (negative offsets count from the end).
    pub fn position_from(haystack: &str, needle: &str, offset: isize) -> Option<usize> {
        if needle.is_empty() {
            return None;
        }
        let length = haystack.chars().count() as isize;
        let offset = if offset < 0 { length + offset } else { offset };
        if offset < 0 || offset > length {
            return None;
        }
        let byte_offset = haystack
            .char_indices()
            .nth(offset as usize)
            .map(|(i, _)| i)
            .unwrap_or(haystack.len());
        haystack[byte_offset..]
            .find(needle)
            .map(|i| haystack[..byte_offset + i].chars().count())
    }

    /// Get the string matching the given pattern (the first capture group,
    /// or the whole match when there are no groups).
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::match_("/bar/", "foo bar"), "bar");
    /// assert_eq!(Str::match_("/foo (.*)/", "foo bar"), "bar");
    /// assert_eq!(Str::match_("/nothing/", "foo bar"), "");
    /// ```
    pub fn match_(pattern: &str, subject: &str) -> String {
        let Ok(regex) = preg::compile(pattern) else {
            return String::new();
        };
        match regex.captures(subject) {
            Some(caps) => caps
                .get(1)
                .or_else(|| caps.get(0))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            None => String::new(),
        }
    }

    /// Get all of the strings matching the given pattern.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::match_all("/bar/", "bar foo bar").all(), &["bar", "bar"]);
    /// assert_eq!(Str::match_all("/f(\\w*)/", "bar fun bar fly").all(), &["un", "ly"]);
    /// ```
    pub fn match_all(pattern: &str, subject: &str) -> Collection<String> {
        let Ok(regex) = preg::compile(pattern) else {
            return Collection::new();
        };
        let group = if regex.captures_len() > 1 { 1 } else { 0 };
        regex
            .captures_iter(subject)
            .map(|caps| caps.get(group).map(|m| m.as_str().to_string()).unwrap_or_default())
            .collect()
    }

    /// Determine if a given string matches a given regular expression.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert!(Str::is_match("/foo (.*)/", "foo bar"));
    /// assert!(!Str::is_match("/foo (.*)/", "laravel"));
    /// ```
    pub fn is_match(pattern: &str, value: &str) -> bool {
        preg::is_match(pattern, value)
    }

    /// Determine if a given string matches any of the given regular expressions.
    pub fn is_match_any(patterns: &[&str], value: &str) -> bool {
        patterns.iter().any(|p| preg::is_match(p, value))
    }

    /// Remove all non-numeric characters from a string.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::numbers("(555) 123-4567"), "5551234567");
    /// ```
    pub fn numbers(value: &str) -> String {
        value.chars().filter(char::is_ascii_digit).collect()
    }

    /// Masks a portion of a string with a repeated character.
    pub fn mask(value: &str, character: char, index: isize, length: Option<usize>) -> String {
        Self::mask_with(
            value,
            &character.to_string(),
            index,
            length.map(|l| l.min(isize::MAX as usize) as isize),
        )
    }

    /// Masks a portion of a string, supporting negative lengths and a
    /// multi-character mask (only its first character is used).
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::mask_with("taylor@example.com", "*", -15, Some(3)), "tay***@example.com");
    /// assert_eq!(Str::mask_with("taylor@example.com", "*", 4, Some(-4)), "tayl**********.com");
    /// ```
    pub fn mask_with(value: &str, character: &str, index: isize, length: Option<isize>) -> String {
        let Some(mask) = character.chars().next() else {
            return value.to_string();
        };
        let segment = Self::substr(value, index, length);
        if segment.is_empty() {
            return value.to_string();
        }
        let len = value.chars().count() as isize;
        let start = if index < 0 { (len + index).max(0) } else { index };
        let segment_length = segment.chars().count() as isize;
        format!(
            "{}{}{}",
            Self::substr(value, 0, Some(start)),
            mask.to_string().repeat(segment_length as usize),
            Self::substr(value, start + segment_length, None)
        )
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

    /// Strip HTML and PHP tags from a string.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// assert_eq!(Str::strip_tags("<a href=\"https://laravel.com\">Taylor <b>Otwell</b></a>"), "Taylor Otwell");
    /// ```
    pub fn strip_tags(value: &str) -> String {
        Self::strip_tags_except(value, &[])
    }

    /// Strip HTML tags from a string, keeping the given tags (`"b"`, `"i"`, ...).
    pub fn strip_tags_except(value: &str, allowed: &[&str]) -> String {
        let allowed: Vec<String> = allowed
            .iter()
            .map(|t| t.trim_matches(|c| c == '<' || c == '>' || c == '/').to_lowercase())
            .collect();
        let mut out = String::with_capacity(value.len());
        let mut rest = value;
        while let Some(start) = rest.find('<') {
            out.push_str(&rest[..start]);
            let after = &rest[start..];
            match after.find('>') {
                Some(end) => {
                    let tag = &after[..=end];
                    let name: String = tag
                        .trim_start_matches(['<', '/'])
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric())
                        .collect::<String>()
                        .to_lowercase();
                    if !name.is_empty() && allowed.contains(&name) {
                        out.push_str(tag);
                    }
                    rest = &after[end + 1..];
                }
                None => {
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Generate a more truly "random" password.
    pub fn password(length: usize) -> String {
        Self::password_with(length, true, true, true, false).unwrap_or_default()
    }

    /// Generate a random password using the selected character pools.
    ///
    /// ```
    /// use illuminate_support::Str;
    ///
    /// let password = Str::password_with(12, true, true, false, false).unwrap();
    /// assert_eq!(password.len(), 12);
    /// assert!(password.chars().all(|c| c.is_ascii_alphanumeric()));
    /// ```
    pub fn password_with(
        length: usize,
        letters: bool,
        numbers: bool,
        symbols: bool,
        spaces: bool,
    ) -> crate::Result<String> {
        use rand::seq::SliceRandom;

        const LETTERS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
        const NUMBERS: &str = "0123456789";
        const SYMBOLS: &str = "~!#$%^&*()-_.,<>?/\\{}[]|:;";
        let mut pools: Vec<Vec<char>> = Vec::new();
        if letters {
            pools.push(LETTERS.chars().collect());
        }
        if numbers {
            pools.push(NUMBERS.chars().collect());
        }
        if symbols {
            pools.push(SYMBOLS.chars().collect());
        }
        if spaces {
            pools.push(vec![' ']);
        }
        if pools.is_empty() {
            return Err(crate::error::InvalidArgumentException::new(
                "At least one character pool must be enabled.",
            )
            .into());
        }
        let mut rng = rand::rng();
        pools.shuffle(&mut rng);
        let mut out: Vec<char> = pools
            .iter()
            .take(length)
            .map(|pool| pool[rng.random_range(0..pool.len())])
            .collect();
        let all: Vec<char> = pools.concat();
        while out.len() < length {
            out.push(all[rng.random_range(0..all.len())]);
        }
        out.shuffle(&mut rng);
        Ok(out.into_iter().collect())
    }
}

/// The protocols Laravel accepts in `Str::is_url` by default.
const URL_PROTOCOLS: &str = "aaa|aaas|about|acap|acct|acd|acr|adiumxtra|adt|afp|afs|aim|amss|android|appdata|apt|ark|attachment|aw|barion|beshare|bitcoin|bitcoincash|blob|bolo|browserext|calculator|callto|cap|cast|casts|chrome|chrome-extension|cid|coap|coap\\+tcp|coap\\+ws|coaps|coaps\\+tcp|coaps\\+ws|com-eventbrite-attendee|content|conti|crid|cvs|dab|data|dav|diaspora|dict|did|dis|dlna-playcontainer|dlna-playsingle|dns|dntp|dpp|drm|drop|dtn|dvb|ed2k|elsi|example|facetime|fax|feed|feedready|file|filesystem|finger|first-run-pen-experience|fish|fm|ftp|fuchsia-pkg|geo|gg|git|gizmoproject|go|gopher|graph|gtalk|h323|ham|hcap|hcp|http|https|hxxp|hxxps|hydrazone|iax|icap|icon|im|imap|info|iotdisco|ipn|ipp|ipps|irc|irc6|ircs|iris|iris\\.beep|iris\\.lwz|iris\\.xpc|iris\\.xpcs|isostore|itms|jabber|jar|jms|keyparc|lastfm|ldap|ldaps|leaptofrogans|lorawan|lvlt|magnet|mailserver|mailto|maps|market|message|mid|mms|modem|mongodb|moz|ms-access|ms-browser-extension|ms-calculator|ms-drive-to|ms-enrollment|ms-excel|ms-eyecontrolspeech|ms-gamebarservices|ms-gamingoverlay|ms-getoffice|ms-help|ms-infopath|ms-inputapp|ms-lockscreencomponent-config|ms-media-stream-id|ms-mixedrealitycapture|ms-mobileplans|ms-officeapp|ms-people|ms-project|ms-powerpoint|ms-publisher|ms-restoretabcompanion|ms-screenclip|ms-screensketch|ms-search|ms-search-repair|ms-secondary-screen-controller|ms-secondary-screen-setup|ms-settings|ms-settings-airplanemode|ms-settings-bluetooth|ms-settings-camera|ms-settings-cellular|ms-settings-cloudstorage|ms-settings-connectabledevices|ms-settings-displays-topology|ms-settings-emailandaccounts|ms-settings-language|ms-settings-location|ms-settings-lock|ms-settings-nfctransactions|ms-settings-notifications|ms-settings-power|ms-settings-privacy|ms-settings-proximity|ms-settings-screenrotation|ms-settings-wifi|ms-settings-workplace|ms-spd|ms-sttoverlay|ms-transit-to|ms-useractivityset|ms-virtualtouchpad|ms-visio|ms-walk-to|ms-whiteboard|ms-whiteboard-cmd|ms-word|msnim|msrp|msrps|mss|mtqp|mumble|mupdate|mvn|news|nfs|ni|nih|nntp|notes|ocf|oid|onenote|onenote-cmd|opaquelocktoken|openpgp4fpr|pack|palm|paparazzi|payto|pkcs11|platform|pop|pres|prospero|proxy|pwid|psyc|pttp|qb|query|redis|rediss|reload|res|resource|rmi|rsync|rtmfp|rtmp|rtsp|rtsps|rtspu|s3|secondlife|service|session|sftp|sgn|shttp|sieve|simpleledger|sip|sips|skype|smb|sms|smtp|snews|snmp|soap\\.beep|soap\\.beeps|soldat|spiffe|spotify|ssh|steam|stun|stuns|submit|svn|tag|teamspeak|tel|teliaeid|telnet|tftp|tg|things|thismessage|tip|tn3270|tool|ts3server|turn|turns|tv|udp|unreal|urn|ut2004|v-event|vemmi|ventrilo|videotex|vnc|view-source|wais|webcal|wpid|ws|wss|wtai|wyciwyg|xcon|xcon-userid|xfire|xmlrpc\\.beep|xmlrpc\\.beeps|xmpp|xri|ymsgr|z39\\.50|z39\\.50r|z39\\.50s";

/// Laravel's URL validation pattern, for the given protocol alternation.
fn url_pattern(protocols: &str) -> String {
    let ipv4 = r"\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}";
    let ipv6 = r"\[[0-9a-f:.]+\]";
    format!(
        concat!(
            r"~^(?:{protocols})://",
            r"(((?:[\_\.\pL\pN-]|%[0-9A-Fa-f]{{2}})+:)?((?:[\_\.\pL\pN-]|%[0-9A-Fa-f]{{2}})+)@)?",
            r"((?:(?:(?:[\pL\pN\pS\pM\-\_]+\.)+(?:(?:xn--[a-z0-9-]+)|(?:[\pL\pN\pM]+)))|[a-z0-9\-\_]+)\.?|{ipv4}|{ipv6})",
            r"(:[0-9]+)?",
            r"(?:/(?:[\pL\pN\-._\~!$&'()*+,;=:@]|%[0-9A-Fa-f]{{2}})*)*",
            r"(?:\?(?:[\pL\pN\-._\~!$&'\[\]()*+,;=:@/?]|%[0-9A-Fa-f]{{2}})*)?",
            r"(?:\#(?:[\pL\pN\-._\~!$&'()*+,;=:@/?]|%[0-9A-Fa-f]{{2}})*)?$~iu"
        ),
        protocols = protocols,
        ipv4 = ipv4,
        ipv6 = ipv6,
    )
}

fn wildcard_match(pattern: &str, value: &str, ignore_case: bool) -> bool {
    if pattern == "*" || pattern == value {
        return true;
    }
    if ignore_case && pattern.to_lowercase() == value.to_lowercase() {
        return true;
    }
    if !pattern.contains('*') {
        return false;
    }
    let regex = format!(
        "(?s{})^{}\\z",
        if ignore_case { "i" } else { "" },
        regex::escape(pattern).replace("\\*", ".*")
    );
    preg::compile_raw(&regex)
        .map(|r| r.is_match(value))
        .unwrap_or(false)
}

fn random_string(length: usize) -> String {
    const POOL: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut rng = rand::rng();
    (0..length)
        .map(|_| POOL[rng.random_range(0..POOL.len())] as char)
        .collect()
}

fn decode_base64(value: &str) -> Option<String> {
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    let engine = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    );
    let bytes = engine.decode(value).ok()?;
    String::from_utf8(bytes).ok()
}

fn repeat_to(pad: &str, length: usize) -> String {
    pad.chars().cycle().take(length).collect()
}

/// PHP's (byte-oriented) `ucwords`: uppercase ASCII letters after whitespace.
fn php_ucwords(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut capitalize = true;
    for c in value.chars() {
        if capitalize && c.is_ascii_lowercase() {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push(c);
        }
        capitalize = matches!(c, ' ' | '\t' | '\r' | '\n' | '\u{0B}' | '\u{0C}');
    }
    out
}

/// `mb_ucfirst` without lower-casing the remainder.
fn mb_ucfirst(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => {
            let mut out = String::with_capacity(value.len());
            push_titlecase(&mut out, first);
            out.push_str(chars.as_str());
            out
        }
        None => String::new(),
    }
}

fn push_titlecase(out: &mut String, c: char) {
    match c {
        'ß' => out.push_str("Ss"),
        'ǆ' | 'ǅ' | 'Ǆ' => out.push('ǅ'),
        'ǉ' | 'ǈ' | 'Ǉ' => out.push('ǈ'),
        'ǌ' | 'ǋ' | 'Ǌ' => out.push('ǋ'),
        'ǳ' | 'ǲ' | 'Ǳ' => out.push('ǲ'),
        other => out.extend(other.to_uppercase()),
    }
}

fn is_cased(c: char) -> bool {
    c.is_lowercase() || c.is_uppercase() || matches!(c, 'ǅ' | 'ǈ' | 'ǋ' | 'ǲ')
}

/// An approximation of Unicode's `Case_Ignorable` property.
fn is_case_ignorable(c: char) -> bool {
    matches!(c,
        '\'' | '.' | ':' | '^' | '`'
            | '\u{00A8}' | '\u{00AD}' | '\u{00AF}' | '\u{00B4}' | '\u{00B7}' | '\u{00B8}'
            | '\u{02B0}'..='\u{036F}'
            | '\u{0374}' | '\u{0375}' | '\u{0384}' | '\u{0385}' | '\u{0387}'
            | '\u{0483}'..='\u{0489}'
            | '\u{0591}'..='\u{05BD}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2018}' | '\u{2019}' | '\u{2024}' | '\u{2027}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FE20}'..='\u{FE2F}'
            | '\u{FEFF}')
}

/// Laravel's notion of trimmable whitespace: Unicode whitespace, NUL and
/// the invisible characters.
fn is_trimmable(c: char) -> bool {
    c.is_whitespace() || c == '\0' || INVISIBLE_CHARACTERS.contains(&c)
}

/// Expand a PHP `trim` character list (supporting `a..z` ranges).
fn char_list(characters: &str) -> Vec<char> {
    let chars: Vec<char> = characters.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if i + 3 < chars.len() && chars[i + 1] == '.' && chars[i + 2] == '.' {
            let (from, to) = (chars[i], chars[i + 3]);
            if from <= to {
                out.extend(from..=to);
                i += 4;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Replace each run of characters matching the predicate with `replacement`.
fn collapse_runs(value: &str, matches: impl Fn(char) -> bool, replacement: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut in_run = false;
    for c in value.chars() {
        if matches(c) {
            if !in_run {
                out.push_str(replacement);
                in_run = true;
            }
        } else {
            in_run = false;
            out.push(c);
        }
    }
    out
}

/// The display width of a character, like PHP's `mb_strwidth`.
fn char_width(c: char) -> usize {
    match c as u32 {
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x2FFFD
        | 0x30000..=0x3FFFD => 2,
        _ => 1,
    }
}

fn str_width(value: &str) -> usize {
    value.chars().map(char_width).sum()
}

/// Truncate a string to the given display width, like `mb_strimwidth`.
fn strimwidth(value: &str, width: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for c in value.chars() {
        let w = char_width(c);
        if used + w > width {
            break;
        }
        used += w;
        out.push(c);
    }
    out
}

/// PHP's `wordwrap`, operating on characters rather than bytes.
fn word_wrap(value: &str, width: usize, break_with: &str, cut: bool) -> String {
    let text: Vec<char> = value.chars().collect();
    let brk: Vec<char> = break_with.chars().collect();
    if text.is_empty() || brk.is_empty() || (width == 0 && cut) {
        return value.to_string();
    }

    if brk.len() == 1 && !cut {
        let mut out = text.clone();
        let (mut last_start, mut last_space) = (0usize, 0usize);
        for current in 0..text.len() {
            if text[current] == brk[0] {
                last_start = current + 1;
                last_space = current + 1;
            } else if text[current] == ' ' {
                if current - last_start >= width {
                    out[current] = brk[0];
                    last_start = current + 1;
                }
                last_space = current;
            } else if current - last_start >= width && last_start != last_space {
                out[last_space] = brk[0];
                last_start = last_space + 1;
            }
        }
        return out.into_iter().collect();
    }

    let mut out: Vec<char> = Vec::with_capacity(text.len() + brk.len() * 4);
    let (mut last_start, mut last_space) = (0usize, 0usize);
    let mut current = 0;
    while current < text.len() {
        if text[current] == brk[0]
            && current + brk.len() < text.len()
            && text[current..current + brk.len()] == brk[..]
        {
            out.extend_from_slice(&text[last_start..current + brk.len()]);
            current += brk.len() - 1;
            last_start = current + 1;
            last_space = current + 1;
        } else if text[current] == ' ' {
            if current - last_start >= width {
                out.extend_from_slice(&text[last_start..current]);
                out.extend_from_slice(&brk);
                last_start = current + 1;
            }
            last_space = current;
        } else if current - last_start >= width && cut && last_start >= last_space {
            out.extend_from_slice(&text[last_start..current]);
            out.extend_from_slice(&brk);
            last_start = current;
            last_space = current;
        } else if current - last_start >= width && last_start < last_space {
            out.extend_from_slice(&text[last_start..last_space]);
            out.extend_from_slice(&brk);
            last_start = last_space + 1;
            last_space = last_start;
        }
        current += 1;
    }
    if last_start < text.len() {
        out.extend_from_slice(&text[last_start..]);
    }
    out.into_iter().collect()
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
    fn it_snakes_like_laravel() {
        assert_eq!(Str::snake("LaravelPhpFramework"), "laravel_php_framework");
        assert_eq!(Str::snake_with("LaravelPhpFramework", " "), "laravel php framework");
        assert_eq!(Str::snake("Laravel Php Framework"), "laravel_php_framework");
        assert_eq!(Str::snake("Laravel    Php      Framework   "), "laravel_php_framework");
        assert_eq!(Str::snake_with("LaravelPhpFramework", "__"), "laravel__php__framework");
        assert_eq!(Str::snake("LaravelPhpFramework_"), "laravel_php_framework_");
        assert_eq!(Str::snake("laravel php Framework"), "laravel_php_framework");
        assert_eq!(Str::snake("laravel php FrameWork"), "laravel_php_frame_work");
        assert_eq!(Str::snake("foo-bar"), "foo-bar");
        assert_eq!(Str::snake("Foo-Bar"), "foo-_bar");
        assert_eq!(Str::snake("Foo_Bar"), "foo__bar");
        assert_eq!(Str::snake("ŻółtaŁódka"), "żółtałódka");
    }

    #[test]
    fn it_titles_like_mb_convert_case() {
        assert_eq!(Str::title("jefferson costella"), "Jefferson Costella");
        assert_eq!(Str::title("jefFErson coSTella"), "Jefferson Costella");
        assert_eq!(Str::title(""), "");
        assert_eq!(Str::title("123 laravel"), "123 Laravel");
        assert_eq!(Str::title("❤laravel"), "❤Laravel");
        assert_eq!(Str::title("laravel ❤"), "Laravel ❤");
        assert_eq!(Str::title("laravel123"), "Laravel123");
        assert_eq!(Str::title("Laravel123"), "Laravel123");
        assert_eq!(Str::title("o'neil"), "O'neil");
    }

    #[test]
    fn it_makes_headlines() {
        assert_eq!(Str::headline("jefferson costella"), "Jefferson Costella");
        assert_eq!(Str::headline("jefFErson coSTella"), "Jefferson Costella");
        assert_eq!(Str::headline("jefferson_costella uses-_Laravel"), "Jefferson Costella Uses Laravel");
        assert_eq!(Str::headline("laravel_p_h_p_framework"), "Laravel P H P Framework");
        assert_eq!(Str::headline("laravel _p _h _p _framework"), "Laravel P H P Framework");
        assert_eq!(Str::headline("laravel_php_framework"), "Laravel Php Framework");
        assert_eq!(Str::headline("laravel-phP-framework"), "Laravel Ph P Framework");
        assert_eq!(Str::headline("laravel  -_-  php   -_-   framework   "), "Laravel Php Framework");
        assert_eq!(Str::headline("fooBar"), "Foo Bar");
        assert_eq!(Str::headline("foo-barBaz"), "Foo Bar Baz");
        assert_eq!(Str::headline("öffentliche-überraschungen"), "Öffentliche Überraschungen");
        assert_eq!(Str::headline("sindÖdeUndSo"), "Sind Öde Und So");
        assert_eq!(Str::headline("❤_multiByte-☆"), "❤ Multi Byte ☆");
        assert_eq!(Str::headline("orwell 1984"), "Orwell 1984");
        assert_eq!(Str::headline("-orwell-1984 -"), "Orwell 1984");
        assert_eq!(Str::headline("laravel rocks!"), "Laravel Rocks!");
    }

    #[test]
    fn it_formats_apa_titles() {
        assert_eq!(Str::apa("tom and jerry"), "Tom and Jerry");
        assert_eq!(Str::apa("TOM AND JERRY"), "Tom and Jerry");
        assert_eq!(Str::apa("back to the future"), "Back to the Future");
        assert_eq!(Str::apa("this, then that"), "This, Then That");
        assert_eq!(Str::apa("bond. james bond."), "Bond. James Bond.");
        assert_eq!(Str::apa("self-report"), "Self-Report");
        assert_eq!(
            Str::apa("as the world turns, so are the days of our lives"),
            "As the World Turns, So Are the Days of Our Lives"
        );
        assert_eq!(Str::apa("TO KILL A MOCKINGBIRD"), "To Kill a Mockingbird");
        assert_eq!(
            Str::apa("Être écrivain commence par être un lecteur."),
            "Être Écrivain Commence par Être un Lecteur."
        );
        assert_eq!(Str::apa("c'est-à-dire."), "C'est-à-Dire.");
        assert_eq!(Str::apa("❤ MULTIByte ☆"), "❤ Multibyte ☆");
        assert_eq!(Str::apa(""), "");
        assert_eq!(Str::apa("   "), "   ");
    }

    #[test]
    fn it_slices_strings() {
        assert_eq!(Str::after("This is my name", "This is"), " my name");
        assert_eq!(Str::before("This is my name", "my name"), "This is ");
        assert_eq!(Str::between("This is my name", "This", "name"), " is my ");
        assert_eq!(Str::between_first("[a] bc [d]", "[", "]"), "a");
        assert_eq!(Str::limit("The quick brown fox jumps over the lazy dog", 20), "The quick brown fox...");
        assert_eq!(Str::words("Perfectly balanced, as all things should be.", 3), "Perfectly balanced, as...");
        assert_eq!(Str::words_with("Perfectly balanced, as all things should be.", 3, " >>>"), "Perfectly balanced, as >>>");
        assert_eq!(Str::substr("Laravel", -3, None), "vel");
        assert_eq!(Str::substr("The Laravel Framework", 4, Some(7)), "Laravel");
        assert_eq!(Str::mask("taylor@example.com", '*', 3, None), "tay***************");
    }

    #[test]
    fn it_limits_like_laravel() {
        assert_eq!(Str::limit("Laravel is a free, open source PHP web application framework.", 10), "Laravel is...");
        assert_eq!(Str::limit("这是一段中文", 6), "这是一...");
        let string = "The PHP framework for web artisans.";
        assert_eq!(Str::limit(string, 7), "The PHP...");
        assert_eq!(Str::limit_with(string, 7, ""), "The PHP");
        assert_eq!(Str::limit(string, 100), string);
        assert_eq!(Str::limit_preserving_words(string, 10, "..."), "The PHP...");
        assert_eq!(Str::limit_preserving_words(string, 10, ""), "The PHP");
        assert_eq!(Str::limit_preserving_words(string, 100, "..."), string);
        assert_eq!(Str::limit_preserving_words(string, 20, "..."), "The PHP framework...");
        assert_eq!(
            Str::limit_preserving_words("Laravel is a free, open source PHP web application framework.", 15, "..."),
            "Laravel is a..."
        );
        assert_eq!(Str::limit_preserving_words("这是一段中文", 6, "..."), "这是一...");
    }

    #[test]
    fn it_counts_and_limits_words() {
        assert_eq!(Str::words(" ", 100), " ");
        assert_eq!(Str::words("\u{A0}", 100), "\u{A0}");
        assert_eq!(Str::words("Taylor  Otwell", 1), "Taylor...");
        assert_eq!(Str::words("Taylor  Otwell", 2), "Taylor  Otwell");
        assert_eq!(Str::word_count("Hello, world!"), 2);
        assert_eq!(Str::word_count("Hi, this is my first contribution to the Laravel framework."), 10);
        assert_eq!(Str::word_count("мама мыла раму"), 3);
    }

    #[test]
    fn it_wraps_words() {
        assert_eq!(Str::word_wrap("Hello World", 3, "<br />", false), "Hello<br />World");
        assert_eq!(Str::word_wrap("Hello World", 3, "<br />", true), "Hel<br />lo<br />Wor<br />ld");
        assert_eq!(Str::word_wrap("❤Multi Byte☆❤☆❤☆❤", 3, "<br />", false), "❤Multi<br />Byte☆❤☆❤☆❤");
        assert_eq!(Str::word_wrap("žltý kôň", 8, "\n", false), "žltý kôň");
        assert_eq!(Str::word_wrap("žltý kôň", 4, "\n", true), "žltý\nkôň");
        assert_eq!(Str::word_wrap("žltý", 2, "\n", true), "žl\ntý");
        assert_eq!(Str::word_wrap("😀😀😀😀", 2, "\n", true), "😀😀\n😀😀");
        assert_eq!(Str::word_wrap("é é", 1, "A\u{1A}B", false), "éA\u{1A}Bé");
        assert_eq!(
            Str::word_wrap("❤Multi Byte☆❤☆❤☆❤", 3, "<br />", true),
            "❤Mu<br />lti<br />Byt<br />e☆❤<br />☆❤☆<br />❤"
        );
        assert_eq!(
            Str::word_wrap("The quick brown fox jumped over the lazy dog.", 20, "\n", false),
            "The quick brown fox\njumped over the lazy\ndog."
        );
    }

    #[test]
    fn it_excerpts() {
        let o = |radius| ExcerptOptions::new().radius(radius);
        assert_eq!(Str::excerpt("This is a beautiful morning", "beautiful", o(5)).unwrap(), "...is a beautiful morn...");
        assert_eq!(Str::excerpt("This is a beautiful morning", "this", o(5)).unwrap(), "This is a...");
        assert_eq!(Str::excerpt("This is a beautiful morning", "morning", o(5)).unwrap(), "...iful morning");
        assert_eq!(Str::excerpt("This is a beautiful morning", "day", ExcerptOptions::new()), None);
        assert_eq!(Str::excerpt("This is a beautiful! morning", "Beautiful", o(5)).unwrap(), "...is a beautiful! mor...");
        assert_eq!(Str::excerpt("", "", o(0)).unwrap(), "");
        assert_eq!(Str::excerpt("a", "a", o(0)).unwrap(), "a");
        assert_eq!(Str::excerpt("abc", "B", o(0)).unwrap(), "...b...");
        assert_eq!(Str::excerpt("abc", "b", o(1)).unwrap(), "abc");
        assert_eq!(Str::excerpt("abcd", "b", o(1)).unwrap(), "abc...");
        assert_eq!(Str::excerpt("zabc", "b", o(1)).unwrap(), "...abc");
        assert_eq!(Str::excerpt("zabcd", "b", o(1)).unwrap(), "...abc...");
        assert_eq!(Str::excerpt("zabcd", "b", o(2)).unwrap(), "zabcd");
        assert_eq!(Str::excerpt("  zabcd  ", "b", o(4)).unwrap(), "zabcd");
        assert_eq!(Str::excerpt("z  abc  d", "b", o(1)).unwrap(), "...abc...");
        assert_eq!(
            Str::excerpt("This is a beautiful morning", "beautiful", o(5).omission("[...]")).unwrap(),
            "[...]is a beautiful morn[...]"
        );
        assert_eq!(Str::excerpt("taylor", "Y", o(1)).unwrap(), "...ayl...");
        assert_eq!(Str::excerpt("The article description", "", o(8)).unwrap(), "The arti...");
        assert_eq!(Str::excerpt("What is the article?", "What", o(2).omission("?")).unwrap(), "What i?");
        assert_eq!(Str::excerpt("åèö - 二 sān 大åèö", "二 sān", o(4)).unwrap(), "...ö - 二 sān 大åè...");
        assert_eq!(Str::excerpt("Como você está", "Ê", o(2)).unwrap(), "...ocê e...");
        assert_eq!(Str::excerpt("João Antônio", "JOÃO", o(5)).unwrap(), "João Antô...");
        assert_eq!(Str::excerpt("", "/", ExcerptOptions::new()), None);
    }

    #[test]
    fn it_trims_unicode_whitespace() {
        assert_eq!(Str::trim("   foo bar   "), "foo bar");
        assert_eq!(Str::trim(" foo    bar "), "foo    bar");
        assert_eq!(Str::trim("   だ    "), "だ");
        assert_eq!(Str::trim(" a b c\u{00A0}\u{FEFF}\n"), "a b c");
        assert_eq!(Str::rtrim(" a  b \u{3000}\r\n"), " a  b");
        assert_eq!(Str::ltrim(" foo    bar "), "foo    bar ");
        assert_eq!(Str::rtrim(" foo    bar "), " foo    bar");
        for c in [' ', '\n', '\r', '\t', '\u{0B}', '\0'] {
            assert_eq!(Str::trim(&format!(" {c} ")), "");
            assert_eq!(Str::trim(&format!("{c} foo bar {c}")), "foo bar");
        }
        assert_eq!(Str::trim_chars(" foo bar ", ""), " foo bar ");
        assert_eq!(Str::trim_chars("-foo  bar_", "-_"), "foo  bar");
        assert_eq!(Str::ltrim_chars("/Laravel/", "/"), "Laravel/");
        assert_eq!(Str::rtrim_chars("/Laravel/", "/"), "/Laravel");
        assert_eq!(Str::trim_chars("abcXYZcba", "a..c"), "XYZ");
    }

    #[test]
    fn it_squishes() {
        assert_eq!(Str::squish(" laravel   php  framework "), "laravel php framework");
        assert_eq!(Str::squish("laravel\t\tphp\n\nframework"), "laravel php framework");
        assert_eq!(Str::squish("laravelㅤㅤㅤphpㅤframework"), "laravel php framework");
        assert_eq!(Str::squish("laravelᅠᅠᅠᅠᅠᅠᅠᅠᅠᅠphpᅠᅠframework"), "laravel php framework");
    }

    #[test]
    fn it_slugs() {
        assert_eq!(Str::slug("Laravel 5 Framework"), "laravel-5-framework");
        assert_eq!(Str::slug("hello world_foo"), "hello-world-foo");
        assert_eq!(Str::slug("Crème Brûlée"), "creme-brulee");
        assert_eq!(Str::slug_with("Hello World", "_"), "hello_world");
        assert_eq!(Str::slug("hello-world"), "hello-world");
        assert_eq!(Str::slug_with("hello_world", "_"), "hello_world");
        assert_eq!(Str::slug("user@host"), "user-at-host");
        assert_eq!(Str::slug_with("some text", ""), "sometext");
        assert_eq!(Str::slug(""), "");
        let dollar = [("$", "dollar")];
        assert_eq!(Str::slug_with_dictionary("500$ bill", "-", &dollar), "500-dollar-bill");
        assert_eq!(Str::slug_with_dictionary("500--$----bill", "-", &dollar), "500-dollar-bill");
        assert_eq!(Str::slug_with_dictionary("500-$-bill", "-", &dollar), "500-dollar-bill");
    }

    #[test]
    fn it_transliterates() {
        assert_eq!(Str::ascii("@"), "@");
        assert_eq!(Str::ascii("ü"), "u");
        assert_eq!(Str::ascii("a!2ë"), "a!2e");
        assert_eq!(Str::ascii_with_language("ä ö ü Ä Ö Ü", "de"), "ae oe ue Ae Oe Ue");
        assert_eq!(Str::ascii_with_language("х Х щ Щ ъ Ъ иа йо", "bg"), "h H sht Sht a A ia yo");
        assert_eq!(Str::transliterate("ⓣⓔⓢⓣ@ⓛⓐⓡⓐⓥⓔⓛ.ⓒⓞⓜ"), "test@laravel.com");
        assert_eq!(Str::transliterate("🎂"), "?");
        assert_eq!(Str::transliterate_with("🎂", "*"), "*");
    }

    #[test]
    fn it_matches_patterns() {
        assert!(Str::is("foo*", "foobar"));
        assert!(Str::is("*.example.com", "api.example.com"));
        assert!(!Str::is("foo", "foobar"));
        assert!(Str::is("/", "/"));
        assert!(!Str::is("/", " /"));
        assert!(Str::is("*@*", "App\\Class@method"));
        assert!(!Str::is("*BAZ*", "foo/bar/baz"));
        assert!(Str::is_ignore_case("*BAZ*", "foo/bar/baz"));
        assert!(Str::is_ignore_case("A", "a"));
        assert!(Str::is_any_ignore_case(&["A*", "B*"], "a/"));
        assert!(Str::is("foo/*", "foo/bar/baz"));
        assert!(Str::is("*", "anything at all"));
    }

    #[test]
    fn it_uses_regular_expressions() {
        assert_eq!(Str::match_("/bar/", "foo bar"), "bar");
        assert_eq!(Str::match_("/foo (.*)/", "foo bar"), "bar");
        assert_eq!(Str::match_("/nothing/", "foo bar"), "");
        assert_eq!(Str::match_all("/bar/", "bar foo bar").into_vec(), vec!["bar", "bar"]);
        assert_eq!(Str::match_all("/f(\\w*)/", "bar fun bar fly").into_vec(), vec!["un", "ly"]);
        assert!(Str::match_all("/nothing/", "bar").is_empty());
        assert!(Str::is_match_any(&["/^laravel!/i", "/^.*$(.*)/"], "Hello, Laravel!"));
        assert_eq!(Str::replace_matches("/[^A-Za-z0-9]++/", "", "(+1) 501-555-1000"), "15015551000");
        assert_eq!(Str::replace_matches_using("/\\d/", |m| format!("[{}]", &m[0]), "123"), "[1][2][3]");
        assert_eq!(Str::replace_matches_limit("/a/", "b", "aaa", 1), "baa");
    }

    #[test]
    fn it_checks_urls_uuids_and_ulids() {
        for url in [
            "https://laravel.com",
            "http://localhost",
            "http://l",
            "http://l:8000",
            "http://l:8000/path",
            "http://a.b",
            "http://sub.domain.com",
            "http://my-site.com",
            "https://example.com:8080/path?q=1#frag",
            "https://xn--e1afmkfd.xn--p1ai",
            "https://1.xn--e1afmkfd.xn--p1ai",
            "http://127.0.0.1:8000",
            "https://taylor:secret@laravel.com/docs",
        ] {
            assert!(Str::is_url(url), "{url} should be a URL");
        }
        for url in ["invalid url", "http://.", "http://...", "http:///path", "laravel"] {
            assert!(!Str::is_url(url), "{url} should not be a URL");
        }
        assert!(Str::is_uuid("a0a2a2d2-0b87-4a18-83f2-2529882be2de"));
        assert!(!Str::is_uuid("laravel"));
        assert!(Str::is_uuid_version("00000000-0000-0000-0000-000000000000", 0));
        assert!(Str::is_ulid("01gd6r360bp37zj17nxb55yv40"));
        assert!(Str::is_ulid("01GD6R360BP37ZJ17NXB55YV40"));
        assert!(!Str::is_ulid("laravel"));
        assert!(!Str::is_ulid("81gd6r360bp37zj17nxb55yv40"));
    }

    #[test]
    fn it_pluralizes_studly_strings() {
        assert_eq!(Str::plural_studly("VerifiedHuman"), "VerifiedHumans");
        assert_eq!(Str::plural_studly("UserFeedback"), "UserFeedback");
        assert_eq!(Str::plural_studly_count("VerifiedHuman", 1), "VerifiedHuman");
        assert_eq!(Str::plural_pascal("RealHuman"), "RealHumans");
        assert_eq!(Str::counted("car", 1000), "1,000 cars");
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
        assert_eq!(Str::finish("abbcbc", "bc"), "abbc");
        assert_eq!(Str::finish("abcbbcbc", "bc"), "abcbbc");
    }

    #[test]
    fn it_chops_unwraps_and_takes() {
        assert_eq!(Str::chop_start_any("http://laravel.com", &["https://", "http://"]), "laravel.com");
        assert_eq!(Str::chop_end_any("laravel.com/index.php", &["/index.html", "/index.php"]), "laravel.com");
        assert_eq!(Str::unwrap("\"value\"", "\"", "\""), "value");
        assert_eq!(Str::unwrap("\"value", "\"", "\""), "value");
        assert_eq!(Str::unwrap("foo-bar-baz", "foo-", "-baz"), "bar");
        assert_eq!(Str::take("abcdef", 2), "ab");
        assert_eq!(Str::take("abcdef", 0), "");
        assert_eq!(Str::take("abcdef", 10), "abcdef");
        assert_eq!(Str::take("üöä", 1), "ü");
        assert_eq!(Str::char_at("Привет, мир!", 1), Some('р'));
        assert_eq!(Str::char_at("「こんにちは世界」", -2), Some('界'));
        assert_eq!(Str::char_at("「こんにちは世界」", -200), None);
    }

    #[test]
    fn it_finds_positions_and_counts() {
        assert_eq!(Str::position("This is a test string.", "test"), Some(10));
        assert_eq!(Str::position_from("This is a test string, test again.", "test", 15), Some(23));
        assert_eq!(Str::position_from("Hello, World!", "W", -6), Some(7));
        assert_eq!(Str::position_from("Äpfel, Birnen und Kirschen", "Kirschen", -10), Some(18));
        assert_eq!(Str::position("@%€/=!\"][$", "$"), Some(9));
        assert_eq!(Str::position("Hello, World!", "w"), None);
        assert_eq!(Str::position("", "test"), None);
        assert_eq!(Str::substr_count("hello hello", "ll"), 2);
    }

    #[test]
    fn it_replaces_substrings() {
        assert_eq!(Str::substr_replace("1200", ":", 2, Some(0)), "12:00");
        assert_eq!(Str::substr_replace("The Framework", "Laravel ", 4, Some(0)), "The Laravel Framework");
        assert_eq!(Str::substr_replace("1234", "567", -3, Some(3)), "1567");
        assert_eq!(Str::substr_replace("1234", "567", 2, Some(-1)), "125674");
        assert_eq!(Str::substr_replace("1234", "567", -2, Some(-1)), "125674");
        assert_eq!(Str::substr_replace("kenkä", "ng", -3, Some(2)), "kengä");
        assert_eq!(Str::replace_start("bar", "qux", "foobar foobar"), "foobar foobar");
        assert_eq!(Str::replace_start("foo", "qux", "foobar foobar"), "quxbar foobar");
        assert_eq!(Str::replace_start("", "yyy", "Jönköping Malmö"), "Jönköping Malmö");
        assert_eq!(Str::replace_end("bar", "qux", "foobar foobar"), "foobar fooqux");
        assert_eq!(Str::replace_end("öping", "yyy", "Malmö Jönköping"), "Malmö Jönkyyy");
        assert_eq!(Str::replace_array("?", &["foo", "bar", "baz"], "?/?/?/?"), "foo/bar/baz/?");
        assert_eq!(Str::replace_array("?", &["foo?", "bar", "baz"], "?/?/?"), "foo?/bar/baz");
        assert_eq!(Str::replace_ignore_case("ÖSTERREICH", "Austria", "Grüße aus österreich"), "Grüße aus Austria");
        assert_eq!(Str::remove_ignore_case("E", "Peter Piper"), "Ptr Pipr");
        assert_eq!(Str::swap(&[("a", "b"), ("b", "a")], "ab"), "ba");
        assert_eq!(Str::swap(&[("a", "1"), ("ab", "2")], "abc"), "2c");
    }

    #[test]
    fn it_masks() {
        assert_eq!(Str::mask("taylor@email.com", '*', 3, None), "tay*************");
        assert_eq!(Str::mask("taylor@email.com", '*', 0, Some(6)), "******@email.com");
        assert_eq!(Str::mask("taylor@email.com", '*', -13, None), "tay*************");
        assert_eq!(Str::mask("taylor@email.com", '*', -13, Some(3)), "tay***@email.com");
        assert_eq!(Str::mask("taylor@email.com", '*', -17, None), "****************");
        assert_eq!(Str::mask("taylor@email.com", '*', -99, Some(5)), "*****r@email.com");
        assert_eq!(Str::mask("taylor@email.com", '*', 16, None), "taylor@email.com");
        assert_eq!(Str::mask_with("taylor@email.com", "", 3, None), "taylor@email.com");
        assert_eq!(Str::mask_with("taylor@email.com", "something", 3, None), "taysssssssssssss");
        assert_eq!(Str::mask("这是一段中文", '*', 3, None), "这是一***");
        assert_eq!(Str::mask("maria@email.com", '*', -1, Some(1)), "maria@email.co*");
    }

    #[test]
    fn it_handles_base64_and_misc() {
        assert_eq!(Str::to_base64("Laravel"), "TGFyYXZlbA==");
        assert_eq!(Str::from_base64("TGFyYXZlbA==").as_deref(), Some("Laravel"));
        assert_eq!(Str::from_base64("TGFyYXZlbA").as_deref(), Some("Laravel"));
        assert_eq!(Str::from_base64_strict("TGFy*YXZlbA=="), None);
        assert_eq!(Str::numbers("L4r4v3l!"), "443");
        assert_eq!(Str::deduplicate_many("a--b__c", &["-", "_"]), "a-b_c");
        assert_eq!(Str::initials("Taylor Otwell"), "TO");
        assert_eq!(Str::ucwords("laravel framework"), "Laravel Framework");
        assert_eq!(Str::ucsplit("Laravel_P_h_p_framework"), vec!["Laravel_", "P_h_p_framework"]);
        assert_eq!(Str::ucsplit("Foo Bar"), vec!["Foo ", "Bar"]);
        assert!(Str::contains_all("This is my name", &["my", "name"]));
        assert!(!Str::contains_all("This is my name", &[]));
        assert!(Str::starts_with_any("This is my name", &["That", "This"]));
        assert!(Str::doesnt_end_with_any("This is my name", &["this", "foo"]));
        assert_eq!(Str::strip_tags_except("<p>Taylor <b>Otwell</b></p>", &["b"]), "Taylor <b>Otwell</b>");
    }

    #[test]
    fn it_fakes_uuids_ulids_and_random_strings() {
        let fixed = Str::freeze_uuids();
        assert_eq!(Str::uuid(), fixed);
        assert_eq!(Str::ordered_uuid(), fixed);
        Str::create_uuids_normally();
        assert_ne!(Str::uuid(), fixed);

        let first = uuid::Uuid::new_v4();
        Str::create_uuids_using_sequence(vec![first]);
        assert_eq!(Str::uuid(), first);
        assert_ne!(Str::uuid(), first);
        Str::create_uuids_normally();

        Str::create_random_strings_using(|length| "x".repeat(length));
        assert_eq!(Str::random(3), "xxx");
        Str::create_random_strings_using_sequence(vec!["first".into()]);
        assert_eq!(Str::random(5), "first");
        assert_eq!(Str::random(7).len(), 7);
        Str::reset_factory_state();
        assert_ne!(Str::random(16), "x".repeat(16));

        let ulid = Str::freeze_ulids();
        assert_eq!(Str::ulid(), ulid);
        Str::create_ulids_normally();
        let at = Str::ulid_at(Carbon::parse("2024-01-01 00:00:00").unwrap());
        assert_eq!(at.timestamp_ms(), 1_704_067_200_000);
        let v7 = Str::uuid7_at(Carbon::parse("2024-01-01 00:00:00").unwrap());
        assert_eq!(v7.get_version_num(), 7);
    }

    #[test]
    fn it_generates_passwords() {
        assert_eq!(Str::password(32).chars().count(), 32);
        let password = Str::password_with(20, false, true, false, false).unwrap();
        assert!(password.chars().all(|c| c.is_ascii_digit()));
        assert!(Str::password_with(10, false, false, false, false).is_err());
    }
}
