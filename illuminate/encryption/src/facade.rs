//! The `Crypt` facade.

use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;

use illuminate_support::Result;

use crate::encrypter::Encrypter;
use crate::helpers::encrypter;

/// The `Crypt` facade: encryption using the application's key.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::Container;
/// use illuminate_encryption::Crypt;
/// use illuminate_support::json;
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({
///     "app": {"key": "base64:J63qRTDLub5NuZvP+kb8YIorGS6qFYHKVo6u7179stY=", "cipher": "AES-256-CBC"},
/// })));
///
/// let encrypted = Crypt::encrypt_string("my-secret-token").unwrap();
/// assert_eq!(Crypt::decrypt_string(&encrypted).unwrap(), "my-secret-token");
/// ```
pub struct Crypt;

impl Crypt {
    /// Get the encrypter instance behind the facade.
    pub fn encrypter() -> Result<Arc<Encrypter>> {
        encrypter()
    }

    /// Encrypt the given value (serialized as JSON).
    pub fn encrypt<T: Serialize + ?Sized>(value: &T) -> Result<String> {
        Ok(encrypter()?.encrypt(value)?)
    }

    /// Encrypt a string without serialization.
    pub fn encrypt_string(value: &str) -> Result<String> {
        Ok(encrypter()?.encrypt_string(value)?)
    }

    /// Decrypt the given payload, deserializing its JSON contents.
    pub fn decrypt<T: DeserializeOwned>(payload: &str) -> Result<T> {
        Ok(encrypter()?.decrypt(payload)?)
    }

    /// Decrypt the given payload without deserialization.
    pub fn decrypt_string(payload: &str) -> Result<String> {
        Ok(encrypter()?.decrypt_string(payload)?)
    }

    /// The encryption key in use.
    pub fn get_key() -> Result<Vec<u8>> {
        Ok(encrypter()?.get_key().to_vec())
    }

    /// The current key followed by every previous key.
    pub fn get_all_keys() -> Result<Vec<Vec<u8>>> {
        Ok(encrypter()?
            .get_all_keys()
            .into_iter()
            .map(<[u8]>::to_vec)
            .collect())
    }

    /// The previous encryption keys.
    pub fn get_previous_keys() -> Result<Vec<Vec<u8>>> {
        Ok(encrypter()?.get_previous_keys().to_vec())
    }

    /// Determine if the given key and cipher combination is valid.
    pub fn supported(key: impl AsRef<[u8]>, cipher: &str) -> bool {
        Encrypter::supported(key, cipher)
    }

    /// Create a new encryption key for the given cipher.
    pub fn generate_key(cipher: &str) -> Vec<u8> {
        Encrypter::generate_key(cipher)
    }

    /// Determine if the given value appears to be encrypted.
    pub fn appears_encrypted(value: &str) -> bool {
        Encrypter::appears_encrypted(value)
    }
}
