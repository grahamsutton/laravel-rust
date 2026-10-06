//! PHP-flavoured regular expressions.
//!
//! Laravel's string helpers accept PCRE patterns written the PHP way: with
//! delimiters and trailing flags (`/^foo (.*)$/i`). This module translates
//! those patterns into the [`regex`] crate's syntax, caches the compiled
//! expression, and offers the handful of `preg_*` operations the framework
//! needs. Patterns without delimiters are used as-is.
//!
//! ```
//! use illuminate_support::preg;
//!
//! assert!(preg::is_match("/laravel/i", "Hello, Laravel!"));
//! assert_eq!(preg::replace("/[^A-Za-z0-9]++/", "", "(+1) 501-555-1000").unwrap(), "15015551000");
//! assert_eq!(preg::replace("/(\\w+) (\\w+)/", "$2 \\1", "hello world").unwrap(), "world hello");
//! ```
//!
//! The translation covers what real-world Laravel patterns use: the `i`,
//! `m`, `s`, `x`, `u`, `U`, `D` and `A` flags, possessive quantifiers
//! (`++`, `*+`), atomic groups, `\Q...\E` literals, `\h`, `\R` and PHP's
//! "superfluous" escapes such as `\/` or `\-`. Look-around assertions and
//! back-references are not supported by the underlying engine; compiling such
//! a pattern returns an error.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use regex::{Captures, Regex};

use crate::error::InvalidArgumentException;

static CACHE: LazyLock<Mutex<HashMap<String, Regex>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const CACHE_LIMIT: usize = 1024;

/// Characters with special meaning in the `regex` crate that may be escaped.
const RUST_META: &str = "\\.+*?()|[]{}^$#&-~";

/// Characters accepted as PHP pattern delimiters.
const DELIMITERS: &str = "/#~!@%|`;,+=:";

/// Compile a PHP-style pattern (`/foo/i`) or a plain pattern into a [`Regex`].
///
/// Compiled expressions are cached, so calling this in a loop is cheap.
pub fn compile(pattern: &str) -> crate::Result<Regex> {
    if let Some(regex) = CACHE.lock().unwrap().get(pattern) {
        return Ok(regex.clone());
    }
    let translated = translate_pattern(pattern);
    let regex = Regex::new(&translated).map_err(|e| {
        InvalidArgumentException::new(format!("Invalid regular expression [{pattern}]: {e}"))
    })?;
    let mut cache = CACHE.lock().unwrap();
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(pattern.to_string(), regex.clone());
    Ok(regex)
}

/// Compile a pattern already written in the `regex` crate's syntax, using
/// the same cache as [`compile`].
pub fn compile_raw(pattern: &str) -> crate::Result<Regex> {
    let key = format!("\u{0}raw:{pattern}");
    if let Some(regex) = CACHE.lock().unwrap().get(&key) {
        return Ok(regex.clone());
    }
    let regex = Regex::new(pattern).map_err(|e| {
        InvalidArgumentException::new(format!("Invalid regular expression [{pattern}]: {e}"))
    })?;
    let mut cache = CACHE.lock().unwrap();
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, regex.clone());
    Ok(regex)
}

/// Translate a PHP pattern into the `regex` crate's syntax without compiling it.
///
/// ```
/// use illuminate_support::preg;
///
/// assert_eq!(preg::translate_pattern("/^\\d++$/i"), "(?i)^\\d+$");
/// ```
pub fn translate_pattern(pattern: &str) -> String {
    let (body, flags) = split_delimiters(pattern);
    let extended = flags.contains('x');
    let mut prefix = String::new();
    let inline: String = flags
        .chars()
        .filter(|f| matches!(f, 'i' | 'm' | 's' | 'x' | 'U'))
        .collect();
    if !inline.is_empty() {
        prefix = format!("(?{inline})");
    }
    let translated = translate_body(&body, extended);
    if flags.contains('A') {
        format!("{prefix}\\A(?:{translated})")
    } else {
        format!("{prefix}{translated}")
    }
}

/// Determine if the pattern matches the subject. Invalid patterns never match.
pub fn is_match(pattern: &str, subject: &str) -> bool {
    compile(pattern).map(|r| r.is_match(subject)).unwrap_or(false)
}

/// Run the pattern against the subject, returning the full match followed by
/// each capture group (unmatched groups become empty strings), like PHP's
/// `preg_match` `$matches` array.
pub fn match_(pattern: &str, subject: &str) -> crate::Result<Option<Vec<String>>> {
    let regex = compile(pattern)?;
    Ok(regex.captures(subject).map(|caps| captures_to_vec(&caps)))
}

/// Find every match of the pattern, like PHP's `preg_match_all` with
/// `PREG_SET_ORDER`: one entry per match holding the full match and groups.
pub fn match_all(pattern: &str, subject: &str) -> crate::Result<Vec<Vec<String>>> {
    let regex = compile(pattern)?;
    Ok(regex
        .captures_iter(subject)
        .map(|caps| captures_to_vec(&caps))
        .collect())
}

/// Replace matches of the pattern using a PHP replacement string, where
/// `$1`, `${1}` and `\1` refer to capture groups.
pub fn replace(pattern: &str, replacement: &str, subject: &str) -> crate::Result<String> {
    replace_limit(pattern, replacement, subject, None)
}

/// Replace at most `limit` matches of the pattern (all of them when `None`).
pub fn replace_limit(
    pattern: &str,
    replacement: &str,
    subject: &str,
    limit: Option<usize>,
) -> crate::Result<String> {
    let regex = compile(pattern)?;
    let replacement = translate_replacement(replacement);
    Ok(match limit {
        Some(limit) => regex.replacen(subject, limit, replacement.as_str()).into_owned(),
        None => regex.replace_all(subject, replacement.as_str()).into_owned(),
    })
}

/// Replace matches of the pattern with the value returned by the callback.
pub fn replace_callback(
    pattern: &str,
    callback: impl FnMut(&Captures<'_>) -> String,
    subject: &str,
    limit: Option<usize>,
) -> crate::Result<String> {
    let regex = compile(pattern)?;
    let mut callback = callback;
    Ok(match limit {
        Some(limit) => regex
            .replacen(subject, limit, |caps: &Captures<'_>| callback(caps))
            .into_owned(),
        None => regex
            .replace_all(subject, |caps: &Captures<'_>| callback(caps))
            .into_owned(),
    })
}

/// Split the subject by the pattern, like PHP's `preg_split`.
pub fn split(pattern: &str, subject: &str, limit: Option<usize>) -> crate::Result<Vec<String>> {
    let regex = compile(pattern)?;
    Ok(match limit {
        Some(limit) if limit > 0 => regex.splitn(subject, limit).map(String::from).collect(),
        _ => regex.split(subject).map(String::from).collect(),
    })
}

/// Quote regular expression characters, like PHP's `preg_quote`.
///
/// ```
/// use illuminate_support::preg;
///
/// assert_eq!(preg::quote("Hello.World?", Some('/')), "Hello\\.World\\?");
/// ```
pub fn quote(value: &str, delimiter: Option<char>) -> String {
    let mut out = String::with_capacity(value.len() + 4);
    for c in value.chars() {
        if ".\\+*?[^]$(){}=!<>|:-#".contains(c) || Some(c) == delimiter {
            out.push('\\');
            out.push(c);
        } else if c == '\0' {
            out.push_str("\\000");
        } else {
            out.push(c);
        }
    }
    out
}

/// Translate a PHP replacement string (`$1`, `${1}`, `\1`) into the `regex`
/// crate's replacement syntax.
pub fn translate_replacement(replacement: &str) -> String {
    let chars: Vec<char> = replacement.chars().collect();
    let mut out = String::with_capacity(replacement.len() + 4);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if chars.get(i + 1).is_some_and(char::is_ascii_digit) => {
                let (digits, next) = take_digits(&chars, i + 1);
                out.push_str(&format!("${{{digits}}}"));
                i = next;
            }
            '$' if chars.get(i + 1).is_some_and(char::is_ascii_digit) => {
                let (digits, next) = take_digits(&chars, i + 1);
                out.push_str(&format!("${{{digits}}}"));
                i = next;
            }
            '$' if chars.get(i + 1) == Some(&'{') => {
                let close = chars[i + 2..].iter().position(|c| *c == '}');
                match close {
                    Some(offset)
                        if offset > 0
                            && chars[i + 2..i + 2 + offset].iter().all(char::is_ascii_digit) =>
                    {
                        let digits: String = chars[i + 2..i + 2 + offset].iter().collect();
                        out.push_str(&format!("${{{digits}}}"));
                        i += offset + 3;
                    }
                    _ => {
                        out.push_str("$$");
                        i += 1;
                    }
                }
            }
            '$' => {
                out.push_str("$$");
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    out
}

fn take_digits(chars: &[char], start: usize) -> (String, usize) {
    let mut end = start;
    while end < chars.len() && end - start < 2 && chars[end].is_ascii_digit() {
        end += 1;
    }
    (chars[start..end].iter().collect(), end)
}

fn captures_to_vec(caps: &Captures<'_>) -> Vec<String> {
    caps.iter()
        .map(|m| m.map(|m| m.as_str().to_string()).unwrap_or_default())
        .collect()
}

/// Split a delimited pattern into its body and flags. Plain patterns are
/// returned untouched with no flags.
fn split_delimiters(pattern: &str) -> (String, String) {
    let trimmed = pattern.trim_start();
    let Some(open) = trimmed.chars().next() else {
        return (pattern.to_string(), String::new());
    };
    let close = match open {
        '{' => '}',
        c if DELIMITERS.contains(c) => c,
        _ => return (pattern.to_string(), String::new()),
    };
    let rest = &trimmed[open.len_utf8()..];
    let Some(end) = rest.rfind(close) else {
        return (pattern.to_string(), String::new());
    };
    let flags = &rest[end + close.len_utf8()..];
    if !flags.chars().all(|c| "imsxuUDASXJn".contains(c)) {
        return (pattern.to_string(), String::new());
    }
    (rest[..end].to_string(), flags.to_string())
}

fn translate_body(body: &str, extended: bool) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len() + 8);
    let mut i = 0;
    let mut in_class = false;
    let mut class_start = 0;

    while i < chars.len() {
        let c = chars[i];

        if c == '\\' {
            let Some(&next) = chars.get(i + 1) else {
                out.push_str("\\\\");
                i += 1;
                continue;
            };
            i += 2;
            match next {
                'Z' if !in_class => out.push_str("\\z"),
                'h' => out.push_str(if in_class { " \\t" } else { "[ \\t]" }),
                'R' if !in_class => out.push_str("(?:\\r\\n|\\n|\\r)"),
                'Q' => {
                    let mut literal = String::new();
                    while i < chars.len() && !(chars[i] == '\\' && chars.get(i + 1) == Some(&'E')) {
                        literal.push(chars[i]);
                        i += 1;
                    }
                    if i < chars.len() {
                        i += 2;
                    }
                    out.push_str(&regex::escape(&literal));
                }
                ' ' if !extended => out.push(' '),
                c if c.is_ascii_punctuation() => {
                    // PHP permits escaping any punctuation; the regex crate
                    // only accepts escapes for its own meta characters.
                    if RUST_META.contains(c) {
                        out.push('\\');
                    }
                    out.push(c);
                }
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
            continue;
        }

        if in_class {
            match c {
                ']' if i > class_start => {
                    in_class = false;
                    out.push(']');
                }
                '[' if chars.get(i + 1) == Some(&':') => {
                    let close = (i + 2..chars.len().saturating_sub(1))
                        .find(|&j| chars[j] == ':' && chars[j + 1] == ']');
                    match close {
                        Some(j) => {
                            out.extend(&chars[i..j + 2]);
                            i = j + 2;
                            continue;
                        }
                        None => out.push_str("\\["),
                    }
                }
                '[' => out.push_str("\\["),
                '&' | '~' | '-' if chars.get(i + 1) == Some(&c) => {
                    out.push('\\');
                    out.push(c);
                }
                other => out.push(other),
            }
            i += 1;
            continue;
        }

        match c {
            '[' => {
                in_class = true;
                out.push('[');
                i += 1;
                if chars.get(i) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
                if chars.get(i) == Some(&']') {
                    out.push_str("\\]");
                    i += 1;
                }
                class_start = i;
            }
            '(' if chars.get(i + 1) == Some(&'?') && chars.get(i + 2) == Some(&'>') => {
                out.push_str("(?:");
                i += 3;
            }
            '+' | '*' | '?' | '}' => {
                out.push(c);
                i += 1;
                // Possessive quantifiers become plain greedy quantifiers.
                if chars.get(i) == Some(&'+') {
                    i += 1;
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_translates_delimited_patterns() {
        assert_eq!(translate_pattern("/bar/"), "bar");
        assert_eq!(translate_pattern("/bar/i"), "(?i)bar");
        assert_eq!(translate_pattern("#^a/b$#"), "^a/b$");
        assert_eq!(translate_pattern("/a\\/b/"), "a/b");
        assert_eq!(translate_pattern("/[^A-Za-z0-9]++/"), "[^A-Za-z0-9]+");
        assert_eq!(translate_pattern("/(?>foo)/"), "(?:foo)");
        assert_eq!(translate_pattern("\\d+"), "\\d+");
        assert_eq!(translate_pattern("/[[]/"), "[\\[]");
        assert_eq!(translate_pattern("/[[:alpha:]]+/"), "[[:alpha:]]+");
        assert_eq!(translate_pattern("/\\Qa.b\\E/"), "a\\.b");
    }

    #[test]
    fn it_matches_like_preg() {
        assert!(is_match("/.*,.*!/", "Hello, Laravel!"));
        assert!(is_match("/^.*$(.*)/", "Hello, Laravel!"));
        assert!(is_match("/laravel/i", "Hello, Laravel!"));
        assert!(!is_match("/H.o/", "Hello, Laravel!"));
        assert!(!is_match("/^laravel!/i", "Hello, Laravel!"));
        assert!(!is_match("/^[a-zA-Z,!]+$/", "Hello, Laravel!"));
        assert!(!is_match("/(?<=a)b/", "ab"), "unsupported patterns never match");
        assert!(is_match(&format!("/{}/", quote("a.b<c>-d", Some('/'))), "a.b<c>-d"));
    }

    #[test]
    fn it_replaces_using_php_references() {
        assert_eq!(replace("/(\\d)/", "[$1]", "123").unwrap(), "[1][2][3]");
        assert_eq!(replace("/(\\d)/", "[\\1]", "12").unwrap(), "[1][2]");
        assert_eq!(replace("/(\\d)/", "${1}0", "12").unwrap(), "1020");
        assert_eq!(replace("/x/", "$", "axb").unwrap(), "a$b");
        assert_eq!(replace_limit("/a/", "b", "aaa", Some(2)).unwrap(), "bba");
        assert_eq!(
            replace_callback("/\\d/", |caps| format!("[{}]", &caps[0]), "123", None).unwrap(),
            "[1][2][3]"
        );
    }

    #[test]
    fn it_splits_and_matches_all() {
        assert_eq!(split("/[\\s,]+/", "one, two, three", None).unwrap(), vec!["one", "two", "three"]);
        assert_eq!(
            match_all("/f(\\w*)/", "bar fun bar fly").unwrap(),
            vec![vec!["fun".to_string(), "un".to_string()], vec!["fly".to_string(), "ly".to_string()]]
        );
        assert_eq!(match_("/foo (.*)/", "foo bar").unwrap().unwrap()[1], "bar");
        assert!(match_("/nothing/", "foo").unwrap().is_none());
    }
}
