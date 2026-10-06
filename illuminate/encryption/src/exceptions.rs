//! The exceptions thrown by the encrypter.

/// Thrown when a payload can't be decrypted: it was tampered with, it isn't
/// a valid payload, or it was encrypted with a different key.
///
/// ```
/// use illuminate_encryption::{DecryptException, Encrypter};
///
/// let encrypter = Encrypter::new([7u8; 32], "aes-256-cbc").unwrap();
///
/// let error: DecryptException = encrypter.decrypt_string("not-a-payload").unwrap_err();
/// assert_eq!(error.to_string(), "The payload is invalid.");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct DecryptException {
    pub message: String,
}

impl DecryptException {
    /// Create a new exception with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// "The payload is invalid."
    pub fn invalid_payload() -> Self {
        Self::new("The payload is invalid.")
    }

    /// "The MAC is invalid."
    pub fn invalid_mac() -> Self {
        Self::new("The MAC is invalid.")
    }

    /// "Could not decrypt the data."
    pub fn could_not_decrypt() -> Self {
        Self::new("Could not decrypt the data.")
    }
}

/// Thrown when a value can't be encrypted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct EncryptException {
    pub message: String,
}

impl EncryptException {
    /// Create a new exception with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// "Could not encrypt the data."
    pub fn could_not_encrypt() -> Self {
        Self::new("Could not encrypt the data.")
    }
}

/// Thrown when the application has no encryption key (`APP_KEY`) configured.
///
/// Run `php artisan key:generate` — or rather, its Rust equivalent — to set one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct MissingAppKeyException {
    pub message: String,
}

impl MissingAppKeyException {
    /// Create the exception with Laravel's standard message.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for MissingAppKeyException {
    fn default() -> Self {
        Self {
            message: "No application encryption key has been specified.".to_string(),
        }
    }
}
