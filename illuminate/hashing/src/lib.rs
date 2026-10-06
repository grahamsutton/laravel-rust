//! # Illuminate Hashing
//!
//! The [`Hash`] facade provides secure Bcrypt and Argon2 hashing for storing
//! user passwords. Bcrypt is the default: its "work factor" is adjustable,
//! so the time it takes to generate a hash can grow as hardware gets faster.
//! When hashing passwords, slow is good.
//!
//! ```
//! use illuminate_hashing::{BcryptHasher, Hasher};
//!
//! let hasher = BcryptHasher::new().rounds(4);
//!
//! let hashed = hasher.make("secret").unwrap();
//! assert!(hashed.starts_with("$2y$04$"));
//! assert!(hasher.check("secret", &hashed));
//! assert!(!hasher.check("wrong", &hashed));
//! ```
//!
//! Hashes are interchangeable with PHP's `password_hash()`: a password
//! hashed by a Laravel application verifies here, and vice versa.

mod argon_hasher;
mod bcrypt_hasher;
mod facade;
mod hasher;
mod manager;
mod provider;

pub use argon_hasher::ArgonHasher;
pub use bcrypt_hasher::BcryptHasher;
pub use facade::{Hash, bcrypt, bcrypt_with};
pub use hasher::{HashInfo, HashOptions, Hasher, password_get_info, password_verify};
pub use manager::HashManager;
pub use provider::HashServiceProvider;

/// The facades provided by this component.
pub mod facades {
    pub use crate::facade::Hash;
}
