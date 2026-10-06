//! Path helpers: normalization (with traversal protection), PHP-style
//! `pathinfo()`, glob matching, and MIME type detection.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use illuminate_support::Result;

use crate::exceptions::{CorruptedPathDetected, PathTraversalDetected};

static FUNKY_WHITESPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\p{C}").expect("valid control character pattern"));

/// Normalize a disk-relative path, rejecting anything that would escape the
/// disk's root.
///
/// This is Flysystem's `WhitespacePathNormalizer`: backslashes become forward
/// slashes, `.` and empty segments disappear, `..` pops a segment — and a
/// `..` with nothing left to pop is a [`PathTraversalDetected`] error.
///
/// ```
/// use illuminate_filesystem::normalize_path;
///
/// assert_eq!(normalize_path("/avatars/./1.jpg").unwrap(), "avatars/1.jpg");
/// assert_eq!(normalize_path("avatars/old/../1.jpg").unwrap(), "avatars/1.jpg");
/// assert!(normalize_path("../../etc/passwd").is_err());
/// ```
pub fn normalize_path(path: &str) -> Result<String> {
    let path = path.replace('\\', "/");

    if FUNKY_WHITESPACE.is_match(&path) {
        return Err(CorruptedPathDetected::new(path).into());
    }

    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(PathTraversalDetected::new(path.clone()).into());
                }
            }
            other => parts.push(other),
        }
    }

    Ok(parts.join("/"))
}

/// Anything that can be turned into a list of paths: a single path, or a
/// list of them. Mirrors Laravel methods that accept `string|array`.
///
/// ```
/// use illuminate_filesystem::IntoPaths;
///
/// assert_eq!("a.txt".into_paths(), vec!["a.txt"]);
/// assert_eq!(["a.txt", "b.txt"].into_paths(), vec!["a.txt", "b.txt"]);
/// ```
pub trait IntoPaths {
    /// Convert into a list of paths.
    fn into_paths(self) -> Vec<String>;
}

fn lossy(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl IntoPaths for &str {
    fn into_paths(self) -> Vec<String> {
        vec![self.to_string()]
    }
}

impl IntoPaths for String {
    fn into_paths(self) -> Vec<String> {
        vec![self]
    }
}

impl IntoPaths for &String {
    fn into_paths(self) -> Vec<String> {
        vec![self.clone()]
    }
}

impl IntoPaths for &Path {
    fn into_paths(self) -> Vec<String> {
        vec![lossy(self)]
    }
}

impl IntoPaths for PathBuf {
    fn into_paths(self) -> Vec<String> {
        vec![lossy(&self)]
    }
}

impl IntoPaths for &PathBuf {
    fn into_paths(self) -> Vec<String> {
        vec![lossy(self)]
    }
}

impl<T: AsRef<Path>> IntoPaths for Vec<T> {
    fn into_paths(self) -> Vec<String> {
        self.iter().map(|p| lossy(p.as_ref())).collect()
    }
}

impl<T: AsRef<Path>> IntoPaths for &[T] {
    fn into_paths(self) -> Vec<String> {
        self.iter().map(|p| lossy(p.as_ref())).collect()
    }
}

impl<T: AsRef<Path>, const N: usize> IntoPaths for [T; N] {
    fn into_paths(self) -> Vec<String> {
        self.iter().map(|p| lossy(p.as_ref())).collect()
    }
}

// ----------------------------------------------------------------------
// pathinfo()
// ----------------------------------------------------------------------

/// The parts of a path, the way PHP's `pathinfo()` sees them.
pub(crate) struct PathInfo {
    pub dirname: String,
    pub basename: String,
    pub extension: String,
    pub filename: String,
}

pub(crate) fn pathinfo(path: &str) -> PathInfo {
    let normalized = if path.len() > 1 { path.trim_end_matches('/') } else { path };
    let (dirname, basename) = match normalized.rfind('/') {
        Some(0) => ("/".to_string(), normalized[1..].to_string()),
        Some(index) => (normalized[..index].to_string(), normalized[index + 1..].to_string()),
        None => (".".to_string(), normalized.to_string()),
    };
    let (filename, extension) = match basename.rfind('.') {
        Some(index) => (basename[..index].to_string(), basename[index + 1..].to_string()),
        None => (basename.clone(), String::new()),
    };
    PathInfo { dirname, basename, extension, filename }
}

// ----------------------------------------------------------------------
// Glob matching
// ----------------------------------------------------------------------

/// Expand `{a,b}` alternatives in a glob pattern.
pub(crate) fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let mut depth = 0;
    let mut close = None;
    for (index, c) in pattern[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + index);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return vec![pattern.to_string()];
    };

    let (prefix, body, suffix) = (&pattern[..open], &pattern[open + 1..close], &pattern[close + 1..]);
    let mut alternatives = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (index, c) in body.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                alternatives.push(&body[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    alternatives.push(&body[start..]);

    alternatives
        .into_iter()
        .flat_map(|alternative| expand_braces(&format!("{prefix}{alternative}{suffix}")))
        .collect()
}

/// Match a single path segment against a glob segment (`*`, `?`, `[a-z]`).
pub(crate) fn glob_match(pattern: &str, name: &str) -> bool {
    // Like glob(3), wildcards never match a leading dot.
    if name.starts_with('.') && !pattern.starts_with('.') {
        return false;
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    match_from(&pattern, &name)
}

fn match_from(pattern: &[char], name: &[char]) -> bool {
    match pattern.first() {
        None => name.is_empty(),
        Some('*') => (0..=name.len()).any(|skip| match_from(&pattern[1..], &name[skip..])),
        Some('?') => !name.is_empty() && match_from(&pattern[1..], &name[1..]),
        Some('[') => {
            let Some(c) = name.first() else { return false };
            match match_class(&pattern[1..], *c) {
                Some((matched, consumed)) => matched && match_from(&pattern[1 + consumed..], &name[1..]),
                None => *c == '[' && match_from(&pattern[1..], &name[1..]),
            }
        }
        Some('\\') if pattern.len() > 1 => {
            name.first() == Some(&pattern[1]) && match_from(&pattern[2..], &name[1..])
        }
        Some(p) => name.first() == Some(p) && match_from(&pattern[1..], &name[1..]),
    }
}

/// Match a character class; returns whether it matched and how many pattern
/// characters (including the closing `]`) were consumed.
fn match_class(pattern: &[char], c: char) -> Option<(bool, usize)> {
    let mut index = 0;
    let negated = matches!(pattern.first(), Some('!') | Some('^'));
    if negated {
        index += 1;
    }
    let mut matched = false;
    let mut first = true;
    while index < pattern.len() {
        let current = pattern[index];
        if current == ']' && !first {
            return Some((matched != negated, index + 1));
        }
        first = false;
        if index + 2 < pattern.len() && pattern[index + 1] == '-' && pattern[index + 2] != ']' {
            if current <= c && c <= pattern[index + 2] {
                matched = true;
            }
            index += 3;
        } else {
            if current == c {
                matched = true;
            }
            index += 1;
        }
    }
    None
}

pub(crate) fn has_wildcards(segment: &str) -> bool {
    segment.contains(['*', '?', '['])
}

// ----------------------------------------------------------------------
// MIME types
// ----------------------------------------------------------------------

/// Detect a MIME type from a file name and (optionally) its leading bytes.
///
/// Content wins when it is recognizable (images, PDFs, archives), then the
/// extension, then a plain-text sniff.
pub(crate) fn detect_mime_type(path: &str, contents: Option<&[u8]>) -> Option<String> {
    if let Some(sniffed) = contents.and_then(sniff_magic) {
        return Some(sniffed.to_string());
    }
    if let Some(guess) = mime_guess::from_path(path).first() {
        return Some(guess.essence_str().to_string());
    }
    match contents {
        Some([]) => Some("application/x-empty".to_string()),
        Some(bytes) if std::str::from_utf8(bytes).is_ok() || looks_like_text(bytes) => {
            Some("text/plain".to_string())
        }
        Some(_) => Some("application/octet-stream".to_string()),
        None => None,
    }
}

fn looks_like_text(bytes: &[u8]) -> bool {
    // A multi-byte character may have been cut off at the end of the sample.
    let trimmed = &bytes[..bytes.len().saturating_sub(3)];
    !trimmed.is_empty() && std::str::from_utf8(trimmed).is_ok()
}

fn sniff_magic(bytes: &[u8]) -> Option<&'static str> {
    const SIGNATURES: &[(&[u8], &str)] = &[
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"GIF87a", "image/gif"),
        (b"GIF89a", "image/gif"),
        (b"%PDF-", "application/pdf"),
        (b"PK\x03\x04", "application/zip"),
        (b"\x1f\x8b", "application/gzip"),
        (b"BM", "image/bmp"),
        (b"\x00\x00\x01\x00", "image/vnd.microsoft.icon"),
    ];
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    SIGNATURES
        .iter()
        .find(|(signature, _)| bytes.starts_with(signature))
        .map(|(_, mime)| *mime)
}

/// The conventional extension for a MIME type.
pub(crate) fn extension_for_mime(mime: &str) -> Option<String> {
    let preferred = match mime {
        "text/plain" => Some("txt"),
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/svg+xml" => Some("svg"),
        "application/pdf" => Some("pdf"),
        "application/json" => Some("json"),
        "application/zip" => Some("zip"),
        "application/gzip" => Some("gz"),
        "text/html" => Some("html"),
        "text/css" => Some("css"),
        "text/csv" => Some("csv"),
        "application/javascript" | "text/javascript" => Some("js"),
        "audio/mpeg" => Some("mp3"),
        "video/mp4" => Some("mp4"),
        _ => None,
    };
    preferred.map(str::to_string).or_else(|| {
        mime_guess::get_mime_extensions_str(mime)
            .and_then(|extensions| extensions.first())
            .map(|extension| extension.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_normalizes_paths() {
        assert_eq!(normalize_path("").unwrap(), "");
        assert_eq!(normalize_path("/").unwrap(), "");
        assert_eq!(normalize_path("a\\b\\c.txt").unwrap(), "a/b/c.txt");
        assert_eq!(normalize_path("a//b/./c/..").unwrap(), "a/b");
        assert_eq!(normalize_path("a/..").unwrap(), "");
    }

    #[test]
    fn it_rejects_traversal_and_corrupted_paths() {
        let error = normalize_path("a/../../secret").unwrap_err();
        assert!(error.downcast_ref::<PathTraversalDetected>().is_some());
        assert_eq!(error.to_string(), "Path traversal detected: a/../../secret");

        let error = normalize_path("bad\u{0000}name").unwrap_err();
        assert!(error.downcast_ref::<CorruptedPathDetected>().is_some());
    }

    #[test]
    fn pathinfo_matches_php() {
        let info = pathinfo("/var/www/html/index.blade.php");
        assert_eq!(info.dirname, "/var/www/html");
        assert_eq!(info.basename, "index.blade.php");
        assert_eq!(info.extension, "php");
        assert_eq!(info.filename, "index.blade");

        let info = pathinfo("file");
        assert_eq!(info.dirname, ".");
        assert_eq!(info.extension, "");

        let info = pathinfo(".htaccess");
        assert_eq!(info.extension, "htaccess");
        assert_eq!(info.filename, "");

        assert_eq!(pathinfo("/file.txt").dirname, "/");
    }

    #[test]
    fn globs_match_segments() {
        assert!(glob_match("*.txt", "notes.txt"));
        assert!(!glob_match("*.txt", "notes.md"));
        assert!(!glob_match("*", ".env"));
        assert!(glob_match(".*", ".env"));
        assert!(glob_match("file?.log", "file1.log"));
        assert!(glob_match("[a-c]*", "beta"));
        assert!(!glob_match("[!a-c]*", "beta"));
        assert_eq!(expand_braces("*.{jpg,png}"), vec!["*.jpg", "*.png"]);
    }

    #[test]
    fn mime_types_are_detected() {
        assert_eq!(detect_mime_type("a.txt", None).unwrap(), "text/plain");
        assert_eq!(detect_mime_type("a", Some(b"\x89PNG\r\n\x1a\nxxxx")).unwrap(), "image/png");
        assert_eq!(detect_mime_type("README", Some(b"hello")).unwrap(), "text/plain");
        assert_eq!(extension_for_mime("image/jpeg").unwrap(), "jpg");
    }
}
