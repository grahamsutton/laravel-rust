//! The encrypter: AES-CBC (with an HMAC-SHA256 MAC) and AES-GCM.

use std::fmt;
use std::str::FromStr;

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockCipher, BlockDecryptMut, BlockEncryptMut, KeyInit, KeyIvInit};
use aes::{Aes128, Aes256};
use aes_gcm::aead::{Aead, Nonce};
use aes_gcm::{Aes128Gcm, Aes256Gcm};
use base64::Engine;
use base64::alphabet;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::Sha256;
use subtle::ConstantTimeEq;

use illuminate_config::Repository;
use illuminate_support::error::RuntimeException;
use illuminate_support::{Value, ValueExt};

use crate::exceptions::{DecryptException, EncryptException, MissingAppKeyException};

/// The length of an AES-GCM authentication tag.
const TAG_LENGTH: usize = 16;

/// A base64 engine as forgiving as PHP's `base64_decode`: padding is optional
/// and stray trailing bits are ignored.
const LENIENT_BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// The cipher algorithms supported by the encrypter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Cipher {
    /// AES-128 in CBC mode, authenticated with an HMAC-SHA256 MAC.
    Aes128Cbc,
    /// AES-256 in CBC mode, authenticated with an HMAC-SHA256 MAC (Laravel's default).
    #[default]
    Aes256Cbc,
    /// AES-128 in GCM mode (an AEAD cipher).
    Aes128Gcm,
    /// AES-256 in GCM mode (an AEAD cipher).
    Aes256Gcm,
}

impl Cipher {
    /// Every supported cipher, in Laravel's order.
    pub const ALL: [Cipher; 4] = [
        Cipher::Aes128Cbc,
        Cipher::Aes256Cbc,
        Cipher::Aes128Gcm,
        Cipher::Aes256Gcm,
    ];

    /// Parse a cipher name such as `AES-256-CBC` (case-insensitive).
    ///
    /// ```
    /// use illuminate_encryption::Cipher;
    ///
    /// assert_eq!(Cipher::parse("AES-256-CBC"), Some(Cipher::Aes256Cbc));
    /// assert_eq!(Cipher::parse("aes-128-gcm"), Some(Cipher::Aes128Gcm));
    /// assert_eq!(Cipher::parse("des"), None);
    /// ```
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|cipher| cipher.name().eq_ignore_ascii_case(name.trim()))
    }

    /// The cipher's OpenSSL name (`aes-256-cbc`, ...).
    pub fn name(&self) -> &'static str {
        match self {
            Cipher::Aes128Cbc => "aes-128-cbc",
            Cipher::Aes256Cbc => "aes-256-cbc",
            Cipher::Aes128Gcm => "aes-128-gcm",
            Cipher::Aes256Gcm => "aes-256-gcm",
        }
    }

    /// The key size, in bytes.
    pub fn key_size(&self) -> usize {
        match self {
            Cipher::Aes128Cbc | Cipher::Aes128Gcm => 16,
            Cipher::Aes256Cbc | Cipher::Aes256Gcm => 32,
        }
    }

    /// The initialization vector size, in bytes (OpenSSL's `openssl_cipher_iv_length`).
    pub fn iv_size(&self) -> usize {
        if self.is_aead() { 12 } else { 16 }
    }

    /// Determine if the cipher authenticates its own payloads (AEAD).
    pub fn is_aead(&self) -> bool {
        matches!(self, Cipher::Aes128Gcm | Cipher::Aes256Gcm)
    }

    /// The comma-separated list used in error messages.
    fn supported_list() -> String {
        Self::ALL.map(|c| c.name()).join(", ")
    }
}

impl fmt::Display for Cipher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Cipher {
    type Err = RuntimeException;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(unsupported_cipher)
    }
}

fn unsupported_cipher() -> RuntimeException {
    RuntimeException::new(format!(
        "Unsupported cipher or incorrect key length. Supported ciphers are: {}.",
        Cipher::supported_list()
    ))
}

/// The encrypter.
///
/// Every payload is a base64-encoded JSON document holding the `iv`, the
/// encrypted `value`, a hex HMAC-SHA256 `mac` over `iv . value` (CBC
/// ciphers), and the authentication `tag` (GCM ciphers) — exactly the
/// format Laravel produces, so payloads interoperate with PHP applications
/// sharing the same key.
///
/// ```
/// use illuminate_encryption::Encrypter;
///
/// let encrypter = Encrypter::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "AES-256-CBC").unwrap();
///
/// let payload = encrypter.encrypt_string("Hello World").unwrap();
/// assert!(Encrypter::appears_encrypted(&payload));
/// assert_eq!(encrypter.decrypt_string(&payload).unwrap(), "Hello World");
/// ```
#[derive(Clone)]
pub struct Encrypter {
    key: Vec<u8>,
    cipher: Cipher,
    previous_keys: Vec<Vec<u8>>,
}

impl fmt::Debug for Encrypter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Encrypter")
            .field("cipher", &self.cipher)
            .field("key", &"[redacted]")
            .field("previous_keys", &self.previous_keys.len())
            .finish()
    }
}

impl Encrypter {
    /// Create a new encrypter instance.
    ///
    /// Fails with a `RuntimeException` when the cipher is unknown or the key
    /// has the wrong length for it.
    pub fn new(key: impl AsRef<[u8]>, cipher: &str) -> Result<Self, RuntimeException> {
        let key = key.as_ref();
        if !Self::supported(key, cipher) {
            return Err(unsupported_cipher());
        }
        let cipher = Cipher::parse(cipher).ok_or_else(unsupported_cipher)?;
        Ok(Self {
            key: key.to_vec(),
            cipher,
            previous_keys: Vec::new(),
        })
    }

    /// Create a new encrypter for an already-parsed [`Cipher`].
    pub fn with_cipher(key: impl AsRef<[u8]>, cipher: Cipher) -> Result<Self, RuntimeException> {
        Self::new(key, cipher.name())
    }

    /// Build the encrypter described by the `app.key`, `app.cipher`, and
    /// `app.previous_keys` configuration values — just like Laravel's
    /// `EncryptionServiceProvider`.
    ///
    /// ```
    /// use illuminate_config::Repository;
    /// use illuminate_encryption::{Cipher, Encrypter};
    /// use illuminate_support::json;
    ///
    /// let config = Repository::new(json!({
    ///     "app": {
    ///         "key": "base64:J63qRTDLub5NuZvP+kb8YIorGS6qFYHKVo6u7179stY=",
    ///         "cipher": "AES-256-CBC",
    ///         "previous_keys": ["base64:2nLsGFGzyoae2ax3EF2Lyq/hH6QghBGLIq5uL+Gp8/w="],
    ///     },
    /// }));
    ///
    /// let encrypter = Encrypter::from_config(&config).unwrap();
    /// assert_eq!(encrypter.cipher(), Cipher::Aes256Cbc);
    /// assert_eq!(encrypter.get_previous_keys().len(), 1);
    /// ```
    pub fn from_config(config: &Repository) -> illuminate_support::Result<Self> {
        let key = parse_key(&config.string("app.key"))?;
        let cipher = config.string_or("app.cipher", Cipher::default().name());
        let previous = match config.get("app.previous_keys") {
            Value::String(keys) => keys
                .split(',')
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(parse_key)
                .collect::<Result<Vec<_>, _>>()?,
            Value::Array(keys) => keys
                .iter()
                .map(|key| key.to_string_lossy())
                .filter(|key| !key.trim().is_empty())
                .map(|key| parse_key(&key))
                .collect::<Result<Vec<_>, _>>()?,
            _ => Vec::new(),
        };
        Ok(Self::new(key, &cipher)?.previous_keys(previous)?)
    }

    /// Determine if the given key and cipher combination is valid.
    ///
    /// ```
    /// use illuminate_encryption::Encrypter;
    ///
    /// assert!(Encrypter::supported([0u8; 32], "AES-256-CBC"));
    /// assert!(!Encrypter::supported([0u8; 16], "AES-256-CBC"));
    /// assert!(!Encrypter::supported([0u8; 32], "AES-256-CFB"));
    /// ```
    pub fn supported(key: impl AsRef<[u8]>, cipher: &str) -> bool {
        Cipher::parse(cipher).is_some_and(|cipher| key.as_ref().len() == cipher.key_size())
    }

    /// Create a new, random encryption key for the given cipher.
    ///
    /// Unknown ciphers receive a 32 byte key, like Laravel.
    pub fn generate_key(cipher: &str) -> Vec<u8> {
        let size = Cipher::parse(cipher).map_or(32, |cipher| cipher.key_size());
        random_bytes(size)
    }

    // ------------------------------------------------------------------
    // Encryption
    // ------------------------------------------------------------------

    /// Encrypt the given value.
    ///
    /// Values are serialized as JSON before they are encrypted. Note that
    /// Laravel uses PHP's `serialize()` here, so payloads produced by
    /// `encrypt` are only readable by applications using this framework.
    /// Use [`Encrypter::encrypt_string`] for payloads shared with PHP.
    ///
    /// ```
    /// use illuminate_encryption::Encrypter;
    /// use std::collections::HashMap;
    ///
    /// let encrypter = Encrypter::new([1u8; 32], "aes-256-cbc").unwrap();
    ///
    /// let payload = encrypter.encrypt(&vec![1, 2, 3]).unwrap();
    /// assert_eq!(encrypter.decrypt::<Vec<i32>>(&payload).unwrap(), vec![1, 2, 3]);
    /// ```
    pub fn encrypt<T: Serialize + ?Sized>(&self, value: &T) -> Result<String, EncryptException> {
        let serialized =
            serde_json::to_vec(value).map_err(|_| EncryptException::could_not_encrypt())?;
        self.encrypt_bytes(&serialized)
    }

    /// Encrypt a string without serialization.
    ///
    /// This is Laravel's `encryptString`, and its payloads can be decrypted
    /// by PHP applications sharing the same key.
    pub fn encrypt_string(&self, value: &str) -> Result<String, EncryptException> {
        self.encrypt_bytes(value.as_bytes())
    }

    /// Encrypt raw bytes without serialization.
    pub fn encrypt_bytes(&self, value: &[u8]) -> Result<String, EncryptException> {
        let iv = random_bytes(self.cipher.iv_size());
        self.encrypt_with_iv(value, &iv)
    }

    /// Encrypt the value with a specific initialization vector.
    pub(crate) fn encrypt_with_iv(
        &self,
        value: &[u8],
        iv: &[u8],
    ) -> Result<String, EncryptException> {
        let key = self.key.as_slice();
        let (ciphertext, tag) = match self.cipher {
            Cipher::Aes128Cbc => (cbc_encrypt::<Aes128>(key, iv, value), Vec::new()),
            Cipher::Aes256Cbc => (cbc_encrypt::<Aes256>(key, iv, value), Vec::new()),
            Cipher::Aes128Gcm => gcm_encrypt::<Aes128Gcm>(key, iv, value)
                .ok_or_else(EncryptException::could_not_encrypt)?,
            Cipher::Aes256Gcm => gcm_encrypt::<Aes256Gcm>(key, iv, value)
                .ok_or_else(EncryptException::could_not_encrypt)?,
        };
        let ciphertext = ciphertext.ok_or_else(EncryptException::could_not_encrypt)?;

        let iv = STANDARD.encode(iv);
        let value = STANDARD.encode(ciphertext);
        let tag = STANDARD.encode(tag);

        // For AEAD algorithms, the tag is the MAC...
        let mac = if self.cipher.is_aead() {
            String::new()
        } else {
            hash(&iv, &value, key)
        };

        let json = serde_json::to_string(&Payload {
            iv: &iv,
            value: &value,
            mac: &mac,
            tag: &tag,
        })
        .map_err(|_| EncryptException::could_not_encrypt())?;

        Ok(STANDARD.encode(json))
    }

    // ------------------------------------------------------------------
    // Decryption
    // ------------------------------------------------------------------

    /// Decrypt the given payload, deserializing its JSON contents.
    pub fn decrypt<T: DeserializeOwned>(&self, payload: &str) -> Result<T, DecryptException> {
        let decrypted = self.decrypt_bytes(payload)?;
        serde_json::from_slice(&decrypted).map_err(|_| DecryptException::invalid_payload())
    }

    /// Decrypt the given payload without deserialization (Laravel's `decryptString`).
    pub fn decrypt_string(&self, payload: &str) -> Result<String, DecryptException> {
        String::from_utf8(self.decrypt_bytes(payload)?)
            .map_err(|_| DecryptException::could_not_decrypt())
    }

    /// Decrypt the given payload into raw bytes.
    ///
    /// The current key is tried first, followed by each of the previous keys.
    pub fn decrypt_bytes(&self, payload: &str) -> Result<Vec<u8>, DecryptException> {
        let payload = self.get_json_payload(payload)?;

        let iv = LENIENT_BASE64
            .decode(&payload.iv)
            .map_err(|_| DecryptException::invalid_payload())?;
        let tag = match payload.tag.as_deref() {
            None | Some("") => None,
            Some(tag) => Some(php_base64_decode(tag).unwrap_or_default()),
        };
        self.ensure_tag_is_valid(tag.as_deref())?;

        // OpenSSL receives the value base64 encoded; anything undecodable fails to decrypt.
        let ciphertext = php_base64_decode(&payload.value);

        if self.cipher.is_aead() {
            let ciphertext = ciphertext.ok_or_else(DecryptException::could_not_decrypt)?;
            let tag = tag.unwrap_or_default();
            return self
                .get_all_keys()
                .into_iter()
                .find_map(|key| self.decrypt_with_key(key, &iv, &ciphertext, &tag))
                .ok_or_else(DecryptException::could_not_decrypt);
        }

        let key = self
            .get_all_keys()
            .into_iter()
            .find(|key| valid_mac_for_key(&payload, key))
            .ok_or_else(DecryptException::invalid_mac)?;

        ciphertext
            .and_then(|ciphertext| self.decrypt_with_key(key, &iv, &ciphertext, &[]))
            .ok_or_else(DecryptException::could_not_decrypt)
    }

    fn decrypt_with_key(
        &self,
        key: &[u8],
        iv: &[u8],
        ciphertext: &[u8],
        tag: &[u8],
    ) -> Option<Vec<u8>> {
        match self.cipher {
            Cipher::Aes128Cbc => cbc_decrypt::<Aes128>(key, iv, ciphertext),
            Cipher::Aes256Cbc => cbc_decrypt::<Aes256>(key, iv, ciphertext),
            Cipher::Aes128Gcm => gcm_decrypt::<Aes128Gcm>(key, iv, ciphertext, tag),
            Cipher::Aes256Gcm => gcm_decrypt::<Aes256Gcm>(key, iv, ciphertext, tag),
        }
    }

    /// Decode and validate the JSON payload.
    fn get_json_payload(&self, payload: &str) -> Result<OwnedPayload, DecryptException> {
        let decoded = php_base64_decode(payload).ok_or_else(DecryptException::invalid_payload)?;
        let json: Value =
            serde_json::from_slice(&decoded).map_err(|_| DecryptException::invalid_payload())?;
        self.valid_payload(&json)
            .ok_or_else(DecryptException::invalid_payload)
    }

    /// Verify that the encryption payload is valid.
    fn valid_payload(&self, payload: &Value) -> Option<OwnedPayload> {
        let field = |name: &str| {
            payload
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let (iv, value, mac) = (field("iv")?, field("value")?, field("mac")?);
        let tag = match payload.get("tag") {
            None | Some(Value::Null) => None,
            Some(Value::String(tag)) => Some(tag.clone()),
            Some(_) => return None,
        };
        let iv_length = LENIENT_BASE64.decode(&iv).ok()?.len();
        (iv_length == self.cipher.iv_size()).then_some(OwnedPayload {
            iv,
            value,
            mac,
            tag,
        })
    }

    /// Ensure the given tag is a valid tag given the selected cipher.
    fn ensure_tag_is_valid(&self, tag: Option<&[u8]>) -> Result<(), DecryptException> {
        if self.cipher.is_aead() && tag.map_or(0, <[u8]>::len) != TAG_LENGTH {
            return Err(DecryptException::could_not_decrypt());
        }
        if !self.cipher.is_aead() && tag.is_some() {
            return Err(DecryptException::new(
                "Unable to use tag because the cipher algorithm does not support AEAD.",
            ));
        }
        Ok(())
    }

    /// Determine if the given value appears to be an encrypted payload.
    ///
    /// ```
    /// use illuminate_encryption::Encrypter;
    ///
    /// assert!(!Encrypter::appears_encrypted("plain text"));
    /// ```
    pub fn appears_encrypted(value: &str) -> bool {
        let Ok(decoded) = STANDARD.decode(value) else {
            return false;
        };
        serde_json::from_slice::<Value>(&decoded).is_ok_and(|payload| {
            ["iv", "value", "mac"]
                .iter()
                .all(|key| payload.get(key).is_some_and(|v| !v.is_null()))
        })
    }

    // ------------------------------------------------------------------
    // Keys
    // ------------------------------------------------------------------

    /// The encryption key the encrypter is currently using.
    pub fn get_key(&self) -> &[u8] {
        &self.key
    }

    /// Alias of [`Encrypter::get_key`].
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    /// The current encryption key followed by all previous keys.
    pub fn get_all_keys(&self) -> Vec<&[u8]> {
        std::iter::once(self.key.as_slice())
            .chain(self.previous_keys.iter().map(Vec::as_slice))
            .collect()
    }

    /// The previous encryption keys.
    pub fn get_previous_keys(&self) -> &[Vec<u8>] {
        &self.previous_keys
    }

    /// Set the previous / legacy keys that should be tried when decrypting.
    ///
    /// ```
    /// use illuminate_encryption::Encrypter;
    ///
    /// let old = Encrypter::new([1u8; 32], "aes-256-cbc").unwrap();
    /// let payload = old.encrypt_string("secret").unwrap();
    ///
    /// let new = Encrypter::new([2u8; 32], "aes-256-cbc")
    ///     .unwrap()
    ///     .previous_keys([[1u8; 32]])
    ///     .unwrap();
    ///
    /// assert_eq!(new.decrypt_string(&payload).unwrap(), "secret");
    /// ```
    pub fn previous_keys<K: AsRef<[u8]>>(
        mut self,
        keys: impl IntoIterator<Item = K>,
    ) -> Result<Self, RuntimeException> {
        let keys: Vec<Vec<u8>> = keys.into_iter().map(|key| key.as_ref().to_vec()).collect();
        if keys.iter().any(|key| key.len() != self.cipher.key_size()) {
            return Err(unsupported_cipher());
        }
        self.previous_keys = keys;
        Ok(self)
    }

    /// The cipher the encrypter uses.
    pub fn cipher(&self) -> Cipher {
        self.cipher
    }
}

/// Parse an encryption key from configuration, decoding `base64:` keys.
///
/// ```
/// use illuminate_encryption::parse_key;
///
/// assert_eq!(parse_key("base64:YWJj").unwrap(), b"abc");
/// assert_eq!(parse_key("plain-key").unwrap(), b"plain-key");
/// assert!(parse_key("").is_err());
/// ```
pub fn parse_key(key: &str) -> Result<Vec<u8>, MissingAppKeyException> {
    if key.is_empty() {
        return Err(MissingAppKeyException::new());
    }
    match key.strip_prefix("base64:") {
        Some(encoded) => Ok(php_base64_decode(encoded).unwrap_or_default()),
        None => Ok(key.as_bytes().to_vec()),
    }
}

/// The serialized payload, in Laravel's field order.
#[derive(Serialize)]
struct Payload<'a> {
    iv: &'a str,
    value: &'a str,
    mac: &'a str,
    tag: &'a str,
}

/// A decoded, validated payload.
struct OwnedPayload {
    iv: String,
    value: String,
    mac: String,
    tag: Option<String>,
}

/// Create a MAC for the given (base64) IV and value.
fn hash(iv: &str, value: &str, key: &[u8]) -> String {
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(iv.as_bytes());
    mac.update(value.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Determine if the MAC is valid for the given payload and key (in constant time).
fn valid_mac_for_key(payload: &OwnedPayload, key: &[u8]) -> bool {
    let expected = hash(&payload.iv, &payload.value, key);
    expected.len() == payload.mac.len()
        && bool::from(expected.as_bytes().ct_eq(payload.mac.as_bytes()))
}

/// Decode base64 the way PHP's non-strict `base64_decode` does, skipping
/// characters outside of the alphabet.
fn php_base64_decode(input: &str) -> Option<Vec<u8>> {
    let filtered: String = input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/')
        .collect();
    LENIENT_BASE64.decode(filtered).ok()
}

fn random_bytes(length: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; length];
    rand::rng().fill_bytes(&mut bytes);
    bytes
}

fn cbc_encrypt<C>(key: &[u8], iv: &[u8], plaintext: &[u8]) -> Option<Vec<u8>>
where
    C: BlockEncryptMut + BlockCipher + KeyInit,
{
    let encryptor = cbc::Encryptor::<C>::new_from_slices(key, iv).ok()?;
    Some(encryptor.encrypt_padded_vec_mut::<Pkcs7>(plaintext))
}

fn cbc_decrypt<C>(key: &[u8], iv: &[u8], ciphertext: &[u8]) -> Option<Vec<u8>>
where
    C: BlockDecryptMut + BlockCipher + KeyInit,
{
    let decryptor = cbc::Decryptor::<C>::new_from_slices(key, iv).ok()?;
    decryptor.decrypt_padded_vec_mut::<Pkcs7>(ciphertext).ok()
}

fn gcm_encrypt<A>(key: &[u8], iv: &[u8], plaintext: &[u8]) -> Option<(Option<Vec<u8>>, Vec<u8>)>
where
    A: Aead + aes_gcm::aead::KeyInit,
{
    let cipher = <A as aes_gcm::aead::KeyInit>::new_from_slice(key).ok()?;
    if iv.len() != Nonce::<A>::default().len() {
        return None;
    }
    let mut sealed = cipher.encrypt(Nonce::<A>::from_slice(iv), plaintext).ok()?;
    let tag = sealed.split_off(sealed.len().checked_sub(TAG_LENGTH)?);
    Some((Some(sealed), tag))
}

fn gcm_decrypt<A>(key: &[u8], iv: &[u8], ciphertext: &[u8], tag: &[u8]) -> Option<Vec<u8>>
where
    A: Aead + aes_gcm::aead::KeyInit,
{
    let cipher = <A as aes_gcm::aead::KeyInit>::new_from_slice(key).ok()?;
    if iv.len() != Nonce::<A>::default().len() || tag.len() != TAG_LENGTH {
        return None;
    }
    let mut sealed = ciphertext.to_vec();
    sealed.extend_from_slice(tag);
    cipher
        .decrypt(Nonce::<A>::from_slice(iv), sealed.as_slice())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    const KEY_256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const KEY_128: &str = "bbbbbbbbbbbbbbbb";

    // Payloads produced by PHP 8.3 running Laravel's exact `Encrypter::encrypt($value, false)`
    // algorithm for the value "Hello from Laravel!".
    const PHP_AES_256_CBC: &str = "eyJpdiI6IlFRSjFVcnFGNVMzUzliVy9iMm4rcUE9PSIsInZhbHVlIjoiUi9GOXVSWWdQalZUT2xlTTVTTWpJL1dhT3RpRFgzeUphb29oMGZvdWV1Zz0iLCJtYWMiOiI0MmM2Y2VlZDQyNjAzYjQyOTVhYWU4ODM0MjMwMDc0NjBmNDNkMDU0YTFmN2NkNDAzZTA4ZTYzNjIwZWU5ZGY0IiwidGFnIjoiIn0=";
    const PHP_AES_128_CBC: &str = "eyJpdiI6IkNqZ3FQc3BkYUZNcGduTDYxcUJZOXc9PSIsInZhbHVlIjoiaHBGeGZGSlVXVWhCUG5MZHE5b2h5dHJkVTUyVFB4blpsYmJwV1gwTWczVT0iLCJtYWMiOiJkMGVlZTI5MmZmNDU3NGUwMDM0YzJmMDhiMjRjNWI0Mzg1Yjk3ZmI0OThiZWI5MTgyNjBkNjZiYWE4YjgwMGQ3IiwidGFnIjoiIn0=";
    const PHP_AES_256_GCM: &str = "eyJpdiI6ImtxZGliTWFTQlVMU0dWZFciLCJ2YWx1ZSI6IkZBN1BSTEw5aTdLRmpqcXg2S0dYTHJ0ZlBnPT0iLCJtYWMiOiIiLCJ0YWciOiJZNVNGeUtlUzM2VVBsLzIraXVoMlZBPT0ifQ==";
    const PHP_AES_128_GCM: &str = "eyJpdiI6ImsvVmVXVW5CTmM5Qy9kcnIiLCJ2YWx1ZSI6IktySmorWFJ4OGU0emJZMlNjbWkzN0haRVlRPT0iLCJtYWMiOiIiLCJ0YWciOiI0NmJKVnF0emdHclh4UW5LQnRCbXVRPT0ifQ==";

    // "Taylor Otwell" encrypted by PHP with a fixed IV ("0123456789abcdef" / "0123456789ab").
    const PHP_FIXED_IV_CBC: &str = "eyJpdiI6Ik1ERXlNelExTmpjNE9XRmlZMlJsWmc9PSIsInZhbHVlIjoiRUh1YzlVb3NBVlpyS1gwTTA4Y2tadz09IiwibWFjIjoiYzI2NmRjMzZiNjUwZTg2OTA3NGMxMGRmNGI2Y2RmYTVlYjE5NDc1ZjE5MTdlMzAzOWIwZGMyMjJlM2ZkMzdkMyIsInRhZyI6IiJ9";
    const PHP_FIXED_IV_GCM: &str = "eyJpdiI6Ik1ERXlNelExTmpjNE9XRmkiLCJ2YWx1ZSI6IjJrT3FJamkxWjJya25WYlYvdz09IiwibWFjIjoiIiwidGFnIjoiZkF0Z3N3L1FwNUdhSkNwNzBBTDJadz09In0=";

    fn encrypter(cipher: &str) -> Encrypter {
        let key = if cipher.contains("128") {
            KEY_128
        } else {
            KEY_256
        };
        Encrypter::new(key, cipher).unwrap()
    }

    fn decode_payload(payload: &str) -> Value {
        serde_json::from_slice(&STANDARD.decode(payload).unwrap()).unwrap()
    }

    fn encode_payload(payload: &Value) -> String {
        STANDARD.encode(serde_json::to_string(payload).unwrap())
    }

    #[test]
    fn it_decrypts_payloads_produced_by_laravel() {
        for (cipher, payload) in [
            ("AES-256-CBC", PHP_AES_256_CBC),
            ("AES-128-CBC", PHP_AES_128_CBC),
            ("AES-256-GCM", PHP_AES_256_GCM),
            ("AES-128-GCM", PHP_AES_128_GCM),
        ] {
            assert_eq!(
                encrypter(cipher).decrypt_string(payload).unwrap(),
                "Hello from Laravel!",
                "{cipher}"
            );
        }
    }

    #[test]
    fn it_produces_byte_identical_payloads_to_laravel() {
        let cbc = encrypter("AES-256-CBC");
        assert_eq!(
            cbc.encrypt_with_iv(b"Taylor Otwell", b"0123456789abcdef")
                .unwrap(),
            PHP_FIXED_IV_CBC
        );

        let gcm = encrypter("AES-256-GCM");
        assert_eq!(
            gcm.encrypt_with_iv(b"Taylor Otwell", b"0123456789ab")
                .unwrap(),
            PHP_FIXED_IV_GCM
        );
    }

    #[test]
    fn payloads_follow_the_documented_algorithm() {
        // Compute a payload by hand: base64(json{iv, value, mac: hex(hmac_sha256(iv . value)), tag}).
        let key = KEY_256.as_bytes();
        let iv = b"fedcba9876543210";
        let ciphertext = cbc_encrypt::<Aes256>(key, iv, b"by hand").unwrap();
        let iv_b64 = STANDARD.encode(iv);
        let value_b64 = STANDARD.encode(&ciphertext);
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).unwrap();
        mac.update(format!("{iv_b64}{value_b64}").as_bytes());
        let payload = encode_payload(&json!({
            "iv": iv_b64,
            "value": value_b64,
            "mac": hex::encode(mac.finalize().into_bytes()),
            "tag": "",
        }));

        assert_eq!(
            encrypter("AES-256-CBC").decrypt_string(&payload).unwrap(),
            "by hand"
        );
        assert_eq!(
            encrypter("AES-256-CBC")
                .encrypt_with_iv(b"by hand", iv)
                .unwrap(),
            payload
        );
    }

    #[test]
    fn every_cipher_roundtrips() {
        for cipher in Cipher::ALL {
            let encrypter =
                Encrypter::new(Encrypter::generate_key(cipher.name()), cipher.name()).unwrap();
            let payload = encrypter.encrypt_string("foo").unwrap();
            assert_ne!(payload, "foo");
            assert_eq!(encrypter.decrypt_string(&payload).unwrap(), "foo");

            let payload = decode_payload(&payload);
            if cipher.is_aead() {
                assert_eq!(payload["mac"], json!(""));
                assert_eq!(
                    STANDARD
                        .decode(payload["tag"].as_str().unwrap())
                        .unwrap()
                        .len(),
                    16
                );
            } else {
                assert_eq!(payload["tag"], json!(""));
                assert_eq!(payload["mac"].as_str().unwrap().len(), 64);
            }
        }
    }

    #[test]
    fn serialized_values_roundtrip_as_json() {
        let encrypter = encrypter("AES-256-GCM");
        let payload = encrypter
            .encrypt(&json!({"name": "Taylor", "roles": ["admin"]}))
            .unwrap();
        let value: Value = encrypter.decrypt(&payload).unwrap();
        assert_eq!(value, json!({"name": "Taylor", "roles": ["admin"]}));
        assert_eq!(
            encrypter.decrypt_string(&payload).unwrap(),
            r#"{"name":"Taylor","roles":["admin"]}"#
        );
        assert!(encrypter.decrypt::<Vec<u8>>(&payload).is_err());
    }

    #[test]
    fn tampered_cbc_payloads_fail_the_mac_check() {
        let encrypter = encrypter("AES-256-CBC");
        let mut payload = decode_payload(&encrypter.encrypt_string("foo").unwrap());
        payload["value"] = json!(STANDARD.encode(b"0123456789abcdef"));
        let error = encrypter
            .decrypt_string(&encode_payload(&payload))
            .unwrap_err();
        assert_eq!(error.to_string(), "The MAC is invalid.");

        let mut payload = decode_payload(&encrypter.encrypt_string("foo").unwrap());
        payload["mac"] = json!("0".repeat(64));
        assert_eq!(
            encrypter
                .decrypt_string(&encode_payload(&payload))
                .unwrap_err(),
            DecryptException::invalid_mac()
        );
    }

    #[test]
    fn tampered_gcm_payloads_cannot_be_decrypted() {
        let encrypter = encrypter("AES-256-GCM");
        let mut payload = decode_payload(&encrypter.encrypt_string("foo").unwrap());
        payload["tag"] = json!(STANDARD.encode([0u8; 16]));
        assert_eq!(
            encrypter
                .decrypt_string(&encode_payload(&payload))
                .unwrap_err()
                .to_string(),
            "Could not decrypt the data."
        );

        let mut payload = decode_payload(&encrypter.encrypt_string("foo").unwrap());
        payload["tag"] = json!(STANDARD.encode([0u8; 8]));
        assert_eq!(
            encrypter
                .decrypt_string(&encode_payload(&payload))
                .unwrap_err(),
            DecryptException::could_not_decrypt()
        );
    }

    #[test]
    fn invalid_payloads_are_rejected() {
        let encrypter = encrypter("AES-256-CBC");
        for payload in [
            "",
            "foo",
            &STANDARD.encode("[]"),
            &encode_payload(&json!({"iv": "x", "value": "y"})),
            &encode_payload(&json!({"iv": STANDARD.encode([0u8; 4]), "value": "y", "mac": "z"})),
            &encode_payload(&json!({"iv": STANDARD.encode([0u8; 16]), "value": "y", "mac": 5})),
            &encode_payload(
                &json!({"iv": STANDARD.encode([0u8; 16]), "value": "y", "mac": "z", "tag": []}),
            ),
        ] {
            assert_eq!(
                encrypter.decrypt_string(payload).unwrap_err().to_string(),
                "The payload is invalid.",
                "{payload}"
            );
        }
    }

    #[test]
    fn cbc_payloads_may_not_carry_a_tag() {
        let encrypter = encrypter("AES-256-CBC");
        let mut payload = decode_payload(&encrypter.encrypt_string("foo").unwrap());
        payload["tag"] = json!(STANDARD.encode([1u8; 16]));
        assert_eq!(
            encrypter
                .decrypt_string(&encode_payload(&payload))
                .unwrap_err()
                .to_string(),
            "Unable to use tag because the cipher algorithm does not support AEAD."
        );
    }

    #[test]
    fn payloads_for_other_keys_are_rejected() {
        let payload = Encrypter::new([1u8; 32], "aes-256-cbc")
            .unwrap()
            .encrypt_string("foo")
            .unwrap();
        let other = Encrypter::new([2u8; 32], "aes-256-cbc").unwrap();
        assert_eq!(
            other.decrypt_string(&payload).unwrap_err(),
            DecryptException::invalid_mac()
        );

        let payload = Encrypter::new([1u8; 32], "aes-256-gcm")
            .unwrap()
            .encrypt_string("foo")
            .unwrap();
        let other = Encrypter::new([2u8; 32], "aes-256-gcm").unwrap();
        assert_eq!(
            other.decrypt_string(&payload).unwrap_err(),
            DecryptException::could_not_decrypt()
        );
    }

    #[test]
    fn previous_keys_can_decrypt_old_payloads() {
        for cipher in ["aes-256-cbc", "aes-128-gcm"] {
            let size = Cipher::parse(cipher).unwrap().key_size();
            let old = Encrypter::new(vec![1u8; size], cipher).unwrap();
            let payload = old.encrypt_string("legacy").unwrap();

            let current = Encrypter::new(vec![2u8; size], cipher)
                .unwrap()
                .previous_keys([vec![3u8; size], vec![1u8; size]])
                .unwrap();
            assert_eq!(current.decrypt_string(&payload).unwrap(), "legacy");
            assert_eq!(current.get_all_keys().len(), 3);
            assert_eq!(current.get_key(), vec![2u8; size].as_slice());

            // New payloads always use the current key.
            let fresh = current.encrypt_string("fresh").unwrap();
            assert!(old.decrypt_string(&fresh).is_err());
        }
    }

    #[test]
    fn invalid_keys_and_ciphers_are_rejected() {
        let message = "Unsupported cipher or incorrect key length. Supported ciphers are: aes-128-cbc, aes-256-cbc, aes-128-gcm, aes-256-gcm.";
        assert_eq!(
            Encrypter::new([0u8; 16], "aes-256-cbc")
                .unwrap_err()
                .to_string(),
            message
        );
        assert_eq!(
            Encrypter::new([0u8; 32], "aes-256-cfb")
                .unwrap_err()
                .to_string(),
            message
        );
        let encrypter = Encrypter::new([0u8; 32], "aes-256-cbc").unwrap();
        assert!(encrypter.previous_keys([[0u8; 16]]).is_err());
        assert!(Encrypter::with_cipher([0u8; 16], Cipher::Aes128Gcm).is_ok());
        assert_eq!("AES-128-GCM".parse::<Cipher>().unwrap(), Cipher::Aes128Gcm);
    }

    #[test]
    fn keys_can_be_generated() {
        assert_eq!(Encrypter::generate_key("aes-128-cbc").len(), 16);
        assert_eq!(Encrypter::generate_key("AES-256-GCM").len(), 32);
        assert_eq!(Encrypter::generate_key("unknown").len(), 32);
        assert_ne!(
            Encrypter::generate_key("aes-256-cbc"),
            Encrypter::generate_key("aes-256-cbc")
        );
    }

    #[test]
    fn it_detects_encrypted_values() {
        let payload = encrypter("AES-256-CBC").encrypt_string("foo").unwrap();
        assert!(Encrypter::appears_encrypted(&payload));
        assert!(Encrypter::appears_encrypted(PHP_AES_256_GCM));
        assert!(!Encrypter::appears_encrypted("foo"));
        assert!(!Encrypter::appears_encrypted(
            &STANDARD.encode(r#"{"iv":"a","value":"b"}"#)
        ));
    }

    #[test]
    fn it_builds_from_configuration() {
        let config = Repository::new(json!({"app": {"key": "", "cipher": "AES-256-CBC"}}));
        let error = Encrypter::from_config(&config).unwrap_err();
        assert!(error.downcast_ref::<MissingAppKeyException>().is_some());
        assert_eq!(
            error.to_string(),
            "No application encryption key has been specified."
        );

        let key = format!("base64:{}", STANDARD.encode([9u8; 16]));
        let previous = format!("base64:{}", STANDARD.encode([8u8; 16]));
        let config = Repository::new(json!({
            "app": {"key": key, "cipher": "aes-128-gcm", "previous_keys": format!("{previous}, ")}
        }));
        let encrypter = Encrypter::from_config(&config).unwrap();
        assert_eq!(encrypter.cipher(), Cipher::Aes128Gcm);
        assert_eq!(encrypter.get_key(), &[9u8; 16]);
        assert_eq!(encrypter.get_previous_keys(), &[vec![8u8; 16]]);

        let config = Repository::new(json!({"app": {"key": KEY_256}}));
        assert_eq!(
            Encrypter::from_config(&config).unwrap().cipher(),
            Cipher::Aes256Cbc
        );

        let config = Repository::new(json!({"app": {"key": KEY_128, "cipher": "AES-256-CBC"}}));
        assert!(Encrypter::from_config(&config).is_err());
    }

    #[test]
    fn the_key_is_never_printed() {
        let debug = format!("{:?}", encrypter("AES-256-CBC"));
        assert!(!debug.contains("aaaa"));
        assert!(debug.contains("redacted"));
    }
}
