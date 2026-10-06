//! Environment variables, with first-class `.env` file support.
//!
//! Variables defined in the real process environment always win over those
//! loaded from a `.env` file, exactly like Laravel's immutable dotenv
//! repository. Values are converted the same way Laravel converts them:
//! `true`, `(true)`, `false`, `(false)`, `null`, `(null)`, `empty` and
//! `(empty)` all have special meaning.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, RwLock};

use serde_json::Value;

static LOADED: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// The environment repository.
pub struct Env;

impl Env {
    /// Load a `.env` file into the repository. Missing files are ignored.
    ///
    /// Returns the number of variables that were loaded.
    pub fn load(path: impl AsRef<Path>) -> usize {
        match std::fs::read_to_string(path.as_ref()) {
            Ok(contents) => Self::load_str(&contents),
            Err(_) => 0,
        }
    }

    /// Parse the contents of a `.env` file and load its variables.
    pub fn load_str(contents: &str) -> usize {
        let parsed = Self::parse(contents);
        let count = parsed.len();
        let mut loaded = LOADED.write().unwrap();
        for (key, value) in parsed {
            loaded.insert(key, value);
        }
        count
    }

    /// Parse the contents of a `.env` file without loading it.
    ///
    /// Supports comments, `export` prefixes, single quotes (literal), double
    /// quotes (with escapes and `${VARIABLE}` interpolation), and multi-line
    /// double-quoted values.
    pub fn parse(contents: &str) -> Vec<(String, String)> {
        let mut entries: Vec<(String, String)> = Vec::new();
        let mut lines = contents.lines().peekable();

        while let Some(raw) = lines.next() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            let Some((key, rest)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().to_string();
            if key.is_empty() {
                continue;
            }
            let rest = rest.trim_start();

            let value = if let Some(stripped) = rest.strip_prefix('"') {
                // Double quoted: may span multiple lines.
                let mut buffer = stripped.to_string();
                while !ends_with_unescaped_quote(&buffer) {
                    match lines.next() {
                        Some(next) => {
                            buffer.push('\n');
                            buffer.push_str(next);
                        }
                        None => break,
                    }
                }
                let inner = match buffer.rfind('"') {
                    Some(end) if ends_with_unescaped_quote(&buffer) => buffer[..end].to_string(),
                    _ => buffer,
                };
                let unescaped = inner
                    .replace("\\n", "\n")
                    .replace("\\r", "\r")
                    .replace("\\t", "\t")
                    .replace("\\\"", "\"")
                    .replace("\\\\", "\\");
                interpolate(&unescaped, &entries)
            } else if let Some(stripped) = rest.strip_prefix('\'') {
                match stripped.rfind('\'') {
                    Some(end) => stripped[..end].to_string(),
                    None => stripped.to_string(),
                }
            } else {
                // Unquoted: strip inline comments.
                let value = match rest.find(" #") {
                    Some(idx) => &rest[..idx],
                    None => rest,
                };
                interpolate(value.trim(), &entries)
            };

            entries.retain(|(k, _)| k != &key);
            entries.push((key, value));
        }

        entries
    }

    /// Get the raw string value of an environment variable.
    pub fn raw(key: &str) -> Option<String> {
        if let Ok(value) = std::env::var(key) {
            return Some(value);
        }
        LOADED.read().unwrap().get(key).cloned()
    }

    /// Get the value of an environment variable, converted the Laravel way.
    pub fn get(key: &str) -> Option<Value> {
        Self::raw(key).map(|raw| Self::convert(&raw))
    }

    /// Get the value of an environment variable or a default.
    pub fn get_or(key: &str, default: impl Into<Value>) -> Value {
        Self::get(key).unwrap_or_else(|| default.into())
    }

    /// Get the value of a required environment variable.
    pub fn get_or_fail(key: &str) -> crate::Result<Value> {
        Self::get(key).ok_or_else(|| {
            crate::error::RuntimeException::new(format!(
                "Environment variable [{key}] has no value."
            ))
            .into()
        })
    }

    /// Manually set a variable in the repository (process env still wins).
    pub fn set(key: impl Into<String>, value: impl Into<String>) {
        LOADED.write().unwrap().insert(key.into(), value.into());
    }

    /// Remove a variable from the loaded repository.
    pub fn forget(key: &str) {
        LOADED.write().unwrap().remove(key);
    }

    /// Convert a raw environment string into a value.
    pub fn convert(raw: &str) -> Value {
        match raw.to_ascii_lowercase().as_str() {
            "true" | "(true)" => Value::Bool(true),
            "false" | "(false)" => Value::Bool(false),
            "empty" | "(empty)" => Value::String(String::new()),
            "null" | "(null)" => Value::Null,
            _ => {
                let bytes = raw.as_bytes();
                if bytes.len() > 1
                    && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
                        || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
                {
                    Value::String(raw[1..raw.len() - 1].to_string())
                } else {
                    Value::String(raw.to_string())
                }
            }
        }
    }
}

fn ends_with_unescaped_quote(s: &str) -> bool {
    let trimmed = s.trim_end();
    if !trimmed.ends_with('"') {
        return false;
    }
    let backslashes = trimmed[..trimmed.len() - 1]
        .chars()
        .rev()
        .take_while(|c| *c == '\\')
        .count();
    backslashes % 2 == 0
}

fn interpolate(value: &str, entries: &[(String, String)]) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                let replacement = std::env::var(name).ok().or_else(|| {
                    entries
                        .iter()
                        .rev()
                        .find(|(k, _)| k == name)
                        .map(|(_, v)| v.clone())
                        .or_else(|| LOADED.read().unwrap().get(name).cloned())
                });
                out.push_str(&replacement.unwrap_or_default());
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Gets the value of an environment variable, or the given default.
///
/// ```
/// use illuminate_support::{env, json};
///
/// assert_eq!(env("SOME_UNDEFINED_VARIABLE", "Laravel"), json!("Laravel"));
/// ```
pub fn env(key: &str, default: impl Into<Value>) -> Value {
    Env::get_or(key, default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_parses_dotenv_files() {
        let parsed = Env::parse(
            r#"
# A comment
APP_NAME=Laravel
APP_DEBUG=true
export APP_ENV=local
QUOTED="Hello ${APP_NAME}"
SINGLE='No ${APP_NAME} here'
INLINE=value # with comment
MULTI="line one
line two"
EMPTY=
"#,
        );
        let get = |k: &str| {
            parsed
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("APP_NAME"), Some("Laravel"));
        assert_eq!(get("APP_ENV"), Some("local"));
        assert_eq!(get("QUOTED"), Some("Hello Laravel"));
        assert_eq!(get("SINGLE"), Some("No ${APP_NAME} here"));
        assert_eq!(get("INLINE"), Some("value"));
        assert_eq!(get("MULTI"), Some("line one\nline two"));
        assert_eq!(get("EMPTY"), Some(""));
    }

    #[test]
    fn it_converts_special_values() {
        assert_eq!(Env::convert("true"), Value::Bool(true));
        assert_eq!(Env::convert("(false)"), Value::Bool(false));
        assert_eq!(Env::convert("null"), Value::Null);
        assert_eq!(Env::convert("empty"), Value::String(String::new()));
        assert_eq!(Env::convert("Laravel"), Value::String("Laravel".into()));
    }
}
