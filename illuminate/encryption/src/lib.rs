//! # Illuminate Encryption
//!
//! Laravel's encryption services provide a simple, convenient interface for
//! encrypting and decrypting text using AES-256 and AES-128 encryption. All
//! encrypted values are signed with a message authentication code (MAC), so
//! their underlying value can't be modified or tampered with once encrypted.
//!
//! Payloads are byte-for-byte compatible with Laravel: a value encrypted by a
//! PHP application with `Crypt::encryptString()` can be decrypted here with
//! [`Encrypter::decrypt_string`], and vice versa, as long as both share the
//! same `APP_KEY` and cipher.
//!
//! ```
//! use illuminate_encryption::Encrypter;
//!
//! let encrypter = Encrypter::new(Encrypter::generate_key("aes-256-cbc"), "aes-256-cbc").unwrap();
//!
//! let secret = encrypter.encrypt_string("my-digital-ocean-token").unwrap();
//! assert_eq!(encrypter.decrypt_string(&secret).unwrap(), "my-digital-ocean-token");
//! ```
//!
//! Inside an application, reach for the [`Crypt`] facade (or the
//! [`encrypt`] / [`decrypt`] helpers), which use the encrypter registered by
//! the [`EncryptionServiceProvider`] from your `app.key` configuration.

mod encrypter;
mod exceptions;
mod facade;
mod helpers;
mod provider;

pub use encrypter::{Cipher, Encrypter, parse_key};
pub use exceptions::{DecryptException, EncryptException, MissingAppKeyException};
pub use facade::Crypt;
pub use helpers::{decrypt, encrypt, encrypter};
pub use provider::EncryptionServiceProvider;

/// The facades provided by this component.
pub mod facades {
    pub use crate::facade::Crypt;
}
