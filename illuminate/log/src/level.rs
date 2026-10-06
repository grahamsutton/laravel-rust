//! The RFC 5424 log levels.

use std::fmt;
use std::str::FromStr;

use illuminate_support::error::InvalidArgumentException;

/// The eight log levels defined in RFC 5424, from least to most severe.
///
/// ```
/// use illuminate_log::Level;
///
/// assert!(Level::Error > Level::Warning);
/// assert_eq!(Level::Info.name(), "INFO");
/// assert_eq!("critical".parse::<Level>().unwrap(), Level::Critical);
/// assert!(Level::Debug.includes(Level::Info));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Level {
    /// Detailed debug information.
    Debug = 100,
    /// Interesting events, such as a user logging in or SQL logs.
    Info = 200,
    /// Normal but significant events.
    Notice = 250,
    /// Exceptional occurrences that are not errors.
    Warning = 300,
    /// Runtime errors that do not require immediate action.
    Error = 400,
    /// Critical conditions, such as an unavailable component.
    Critical = 500,
    /// Action must be taken immediately.
    Alert = 550,
    /// The system is unusable.
    Emergency = 600,
}

impl Level {
    /// Every level, from least to most severe.
    pub const ALL: [Level; 8] = [
        Level::Debug,
        Level::Info,
        Level::Notice,
        Level::Warning,
        Level::Error,
        Level::Critical,
        Level::Alert,
        Level::Emergency,
    ];

    /// Parse a level name (`"debug"`, `"INFO"`, ...), ignoring case.
    pub fn parse(name: &str) -> Option<Level> {
        Self::ALL
            .into_iter()
            .find(|level| level.as_str().eq_ignore_ascii_case(name.trim()))
    }

    /// The level's lowercase name, as used in configuration (`"info"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Notice => "notice",
            Level::Warning => "warning",
            Level::Error => "error",
            Level::Critical => "critical",
            Level::Alert => "alert",
            Level::Emergency => "emergency",
        }
    }

    /// The level's name as printed in log files (`"INFO"`).
    pub fn name(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Notice => "NOTICE",
            Level::Warning => "WARNING",
            Level::Error => "ERROR",
            Level::Critical => "CRITICAL",
            Level::Alert => "ALERT",
            Level::Emergency => "EMERGENCY",
        }
    }

    /// Monolog's numeric value for the level (`Info` is `200`).
    pub fn value(self) -> u16 {
        self as u16
    }

    /// The RFC 5424 (syslog) severity, from `0` (emergency) to `7` (debug).
    pub fn to_rfc5424(self) -> u8 {
        match self {
            Level::Debug => 7,
            Level::Info => 6,
            Level::Notice => 5,
            Level::Warning => 4,
            Level::Error => 3,
            Level::Critical => 2,
            Level::Alert => 1,
            Level::Emergency => 0,
        }
    }

    /// Whether a handler set to this minimum level handles `other`.
    pub fn includes(self, other: Level) -> bool {
        self <= other
    }

    /// Whether this level is more severe than `other`.
    pub fn is_higher_than(self, other: Level) -> bool {
        self > other
    }

    /// Whether this level is less severe than `other`.
    pub fn is_lower_than(self, other: Level) -> bool {
        self < other
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Level {
    type Err = InvalidArgumentException;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Level::parse(name).ok_or_else(|| InvalidArgumentException::new("Invalid log level."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_ordered_by_severity() {
        let mut sorted = Level::ALL;
        sorted.sort();
        assert_eq!(sorted, Level::ALL);
        assert!(Level::Emergency.is_higher_than(Level::Alert));
        assert!(Level::Debug.is_lower_than(Level::Info));
        assert!(!Level::Error.includes(Level::Warning));
        assert!(Level::Error.includes(Level::Error));
    }

    #[test]
    fn levels_parse_and_print() {
        for level in Level::ALL {
            assert_eq!(Level::parse(level.as_str()), Some(level));
            assert_eq!(Level::parse(level.name()), Some(level));
            assert_eq!(level.to_string(), level.as_str());
        }
        assert_eq!(Level::Notice.value(), 250);
        assert_eq!(Level::Emergency.to_rfc5424(), 0);
        assert_eq!(
            "verbose".parse::<Level>().unwrap_err().to_string(),
            "Invalid log level."
        );
    }
}
