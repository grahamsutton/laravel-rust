//! Process output: which stream a line came from, and splitting raw bytes
//! into lines.

use std::fmt;

/// The stream a piece of process output was written to.
///
/// Output callbacks receive the type along with each line, just like the
/// `$type` argument of Laravel's output closures (`"out"` or `"err"`):
///
/// ```
/// use illuminate_process::OutputType;
///
/// assert_eq!(OutputType::Out, "out");
/// assert_eq!(OutputType::Err.to_string(), "err");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OutputType {
    /// Standard output.
    Out,
    /// Standard error.
    Err,
}

impl OutputType {
    /// The type's name: `"out"` or `"err"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputType::Out => "out",
            OutputType::Err => "err",
        }
    }

    /// Determine if this is standard output.
    pub fn is_out(&self) -> bool {
        matches!(self, OutputType::Out)
    }

    /// Determine if this is standard error.
    pub fn is_err(&self) -> bool {
        matches!(self, OutputType::Err)
    }
}

impl fmt::Display for OutputType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq<&str> for OutputType {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<str> for OutputType {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

/// Collects raw bytes and hands back complete lines (each keeping its
/// trailing newline), holding on to any partial line until it is finished.
#[derive(Debug, Default)]
pub(crate) struct LineBuffer {
    partial: Vec<u8>,
}

impl LineBuffer {
    /// Push bytes into the buffer, returning every line they completed.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in bytes {
            self.partial.push(byte);
            if byte == b'\n' {
                lines.push(String::from_utf8_lossy(&self.partial).into_owned());
                self.partial.clear();
            }
        }
        lines
    }

    /// Take whatever partial line is left over.
    pub(crate) fn flush(&mut self) -> Option<String> {
        if self.partial.is_empty() {
            return None;
        }
        let line = String::from_utf8_lossy(&self.partial).into_owned();
        self.partial.clear();
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_types_have_laravel_names() {
        assert_eq!(OutputType::Out.as_str(), "out");
        assert_eq!(OutputType::Err.as_str(), "err");
        assert!(OutputType::Out.is_out());
        assert!(OutputType::Err.is_err());
        assert!(OutputType::Out == *"out");
        assert_ne!(OutputType::Err, "out");
    }

    #[test]
    fn line_buffers_split_on_newlines() {
        let mut buffer = LineBuffer::default();
        assert_eq!(buffer.push(b"one\ntw"), vec!["one\n"]);
        assert_eq!(buffer.push(b"o\nthree"), vec!["two\n"]);
        assert_eq!(buffer.flush().as_deref(), Some("three"));
        assert_eq!(buffer.flush(), None);
    }
}
