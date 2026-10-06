//! The exceptions thrown by the filesystem.
//!
//! Like Flysystem, every failed operation has its own, descriptive error so
//! you can tell a missing file from a permissions problem at a glance — and
//! downcast to the one you care about.

use std::fmt;

/// Thrown when a file that should exist does not.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("File does not exist at path {path}.")]
pub struct FileNotFoundException {
    pub path: String,
}

impl FileNotFoundException {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

/// Thrown when a path tries to escape the root of a disk (`../../etc/passwd`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Path traversal detected: {path}")]
pub struct PathTraversalDetected {
    pub path: String,
}

impl PathTraversalDetected {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

/// Thrown when a path contains control or otherwise invisible characters.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Corrupted path detected: {path}")]
pub struct CorruptedPathDetected {
    pub path: String,
}

impl CorruptedPathDetected {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

/// A filesystem operation that could not be completed.
///
/// The variants mirror Flysystem's exceptions (`UnableToReadFile`,
/// `UnableToWriteFile`, ...), and so do their messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesystemException {
    UnableToReadFile { location: String, reason: String },
    UnableToWriteFile { location: String, reason: String },
    UnableToDeleteFile { location: String, reason: String },
    UnableToDeleteDirectory { location: String, reason: String },
    UnableToCreateDirectory { location: String, reason: String },
    UnableToCopyFile { from: String, to: String, reason: String },
    UnableToMoveFile { from: String, to: String, reason: String },
    UnableToRetrieveMetadata { metadata: String, location: String, reason: String },
    UnableToSetVisibility { location: String, reason: String },
}

impl FilesystemException {
    /// The location (path) the failed operation was working with.
    pub fn location(&self) -> &str {
        match self {
            Self::UnableToReadFile { location, .. }
            | Self::UnableToWriteFile { location, .. }
            | Self::UnableToDeleteFile { location, .. }
            | Self::UnableToDeleteDirectory { location, .. }
            | Self::UnableToCreateDirectory { location, .. }
            | Self::UnableToRetrieveMetadata { location, .. }
            | Self::UnableToSetVisibility { location, .. } => location,
            Self::UnableToCopyFile { from, .. } | Self::UnableToMoveFile { from, .. } => from,
        }
    }

    /// Why the operation failed.
    pub fn reason(&self) -> &str {
        match self {
            Self::UnableToReadFile { reason, .. }
            | Self::UnableToWriteFile { reason, .. }
            | Self::UnableToDeleteFile { reason, .. }
            | Self::UnableToDeleteDirectory { reason, .. }
            | Self::UnableToCreateDirectory { reason, .. }
            | Self::UnableToCopyFile { reason, .. }
            | Self::UnableToMoveFile { reason, .. }
            | Self::UnableToRetrieveMetadata { reason, .. }
            | Self::UnableToSetVisibility { reason, .. } => reason,
        }
    }

    pub(crate) fn read(location: &str, reason: impl fmt::Display) -> Self {
        Self::UnableToReadFile { location: location.into(), reason: reason.to_string() }
    }

    pub(crate) fn write(location: &str, reason: impl fmt::Display) -> Self {
        Self::UnableToWriteFile { location: location.into(), reason: reason.to_string() }
    }

    pub(crate) fn delete(location: &str, reason: impl fmt::Display) -> Self {
        Self::UnableToDeleteFile { location: location.into(), reason: reason.to_string() }
    }

    pub(crate) fn delete_directory(location: &str, reason: impl fmt::Display) -> Self {
        Self::UnableToDeleteDirectory { location: location.into(), reason: reason.to_string() }
    }

    pub(crate) fn create_directory(location: &str, reason: impl fmt::Display) -> Self {
        Self::UnableToCreateDirectory { location: location.into(), reason: reason.to_string() }
    }

    pub(crate) fn copy(from: &str, to: &str, reason: impl fmt::Display) -> Self {
        Self::UnableToCopyFile { from: from.into(), to: to.into(), reason: reason.to_string() }
    }

    pub(crate) fn move_(from: &str, to: &str, reason: impl fmt::Display) -> Self {
        Self::UnableToMoveFile { from: from.into(), to: to.into(), reason: reason.to_string() }
    }

    pub(crate) fn metadata(metadata: &str, location: &str, reason: impl fmt::Display) -> Self {
        Self::UnableToRetrieveMetadata {
            metadata: metadata.into(),
            location: location.into(),
            reason: reason.to_string(),
        }
    }

    pub(crate) fn visibility(location: &str, reason: impl fmt::Display) -> Self {
        Self::UnableToSetVisibility { location: location.into(), reason: reason.to_string() }
    }
}

impl fmt::Display for FilesystemException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::UnableToReadFile { location, reason } => {
                format!("Unable to read file from location: {location}. {reason}")
            }
            Self::UnableToWriteFile { location, reason } => {
                format!("Unable to write file at location: {location}. {reason}")
            }
            Self::UnableToDeleteFile { location, reason } => {
                format!("Unable to delete file located at: {location}. {reason}")
            }
            Self::UnableToDeleteDirectory { location, reason } => {
                format!("Unable to delete directory located at: {location}. {reason}")
            }
            Self::UnableToCreateDirectory { location, reason } => {
                format!("Unable to create a directory at {location}. {reason}")
            }
            Self::UnableToCopyFile { from, to, reason } => {
                format!("Unable to copy file from {from} to {to}. {reason}")
            }
            Self::UnableToMoveFile { from, to, reason } => {
                format!("Unable to move file from {from} to {to}. {reason}")
            }
            Self::UnableToRetrieveMetadata { metadata, location, reason } => {
                format!("Unable to retrieve the {metadata} for file at location: {location}. {reason}")
            }
            Self::UnableToSetVisibility { location, reason } => {
                format!("Unable to set visibility for file {location}. {reason}")
            }
        };
        f.write_str(message.trim_end())
    }
}

impl std::error::Error for FilesystemException {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_match_flysystem() {
        assert_eq!(
            FilesystemException::read("a.txt", "").to_string(),
            "Unable to read file from location: a.txt."
        );
        assert_eq!(
            FilesystemException::copy("a", "b", "").to_string(),
            "Unable to copy file from a to b."
        );
        assert_eq!(PathTraversalDetected::new("../x").to_string(), "Path traversal detected: ../x");
        assert_eq!(FileNotFoundException::new("/a").to_string(), "File does not exist at path /a.");
    }
}
