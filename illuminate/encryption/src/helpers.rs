//! The `encrypt()` / `decrypt()` helpers and encrypter resolution.

use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;

use illuminate_config::Repository;
use illuminate_container::Container;
use illuminate_support::Result;

use crate::encrypter::Encrypter;

/// Resolve the application's encrypter.
///
/// Unlike `app::<Encrypter>()` — which panics when the application has no
/// key, just as resolving the encrypter throws in Laravel — this reports a
/// missing or invalid key as an error (`MissingAppKeyException` or a
/// `RuntimeException`) that the exception handler can render.
///
/// When no encrypter is bound, one is built straight from the `app.*`
/// configuration.
pub fn encrypter() -> Result<Arc<Encrypter>> {
    let container = Container::get_instance();

    if container.resolved::<Encrypter>() {
        return Ok(container.try_make::<Encrypter>()?);
    }

    let config = container
        .try_make::<Repository>()
        .unwrap_or_else(|_| Arc::new(Repository::empty()));

    // Validate the configuration before the container's factory runs, so a
    // missing key surfaces as an error instead of a panic.
    let built = Encrypter::from_config(&config);

    if container.bound::<Encrypter>() {
        built?;
        return Ok(container.try_make::<Encrypter>()?);
    }

    Ok(Arc::new(built?))
}

/// Encrypt the given value (serialized as JSON).
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_encryption::{decrypt, encrypt, Encrypter};
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Encrypter::new([4u8; 32], "aes-256-cbc").unwrap());
///
/// let payload = encrypt(&"secret").unwrap();
/// assert_eq!(decrypt::<String>(&payload).unwrap(), "secret");
/// ```
pub fn encrypt<T: Serialize + ?Sized>(value: &T) -> Result<String> {
    Ok(encrypter()?.encrypt(value)?)
}

/// Decrypt the given payload, deserializing its JSON contents.
pub fn decrypt<T: DeserializeOwned>(payload: &str) -> Result<T> {
    Ok(encrypter()?.decrypt(payload)?)
}
