//! The encryption service provider.

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};

use crate::encrypter::Encrypter;

/// Registers the application's [`Encrypter`], built from the `app.key`,
/// `app.cipher` and `app.previous_keys` configuration values.
///
/// Like Laravel, resolving the encrypter without a valid key is an error:
/// `app::<Encrypter>()` panics with the `MissingAppKeyException` message.
/// Framework code uses [`crate::encrypter()`], which reports it as an error
/// instead.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_encryption::{Encrypter, EncryptionServiceProvider};
/// use illuminate_support::json;
///
/// let container = Container::new();
/// container.instance(Repository::new(json!({"app": {"key": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}})));
/// EncryptionServiceProvider.register(&container);
///
/// assert_eq!(container.make::<Encrypter>().get_key(), b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
/// ```
pub struct EncryptionServiceProvider;

impl ServiceProvider for EncryptionServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<Encrypter>(|container| {
            let config = container
                .try_make::<Repository>()
                .unwrap_or_else(|_| Arc::new(Repository::empty()));
            match Encrypter::from_config(&config) {
                Ok(encrypter) => Arc::new(encrypter),
                Err(error) => panic!("{error}"),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Crypt, MissingAppKeyException, decrypt, encrypt};
    use illuminate_support::json;

    fn container(app: illuminate_support::Value) -> Arc<Container> {
        let container = Arc::new(Container::new());
        container.instance(Repository::new(json!({ "app": app })));
        EncryptionServiceProvider.register(&container);
        container
    }

    #[test]
    fn the_facade_uses_the_configured_key() {
        let container = container(
            json!({"key": "base64:J63qRTDLub5NuZvP+kb8YIorGS6qFYHKVo6u7179stY=", "cipher": "AES-256-GCM"}),
        );
        let _guard = Container::set_local_instance(container.clone());

        let payload = Crypt::encrypt_string("secret").unwrap();
        assert_eq!(Crypt::decrypt_string(&payload).unwrap(), "secret");
        assert_eq!(
            container
                .make::<Encrypter>()
                .decrypt_string(&payload)
                .unwrap(),
            "secret"
        );
        assert_eq!(Crypt::get_key().unwrap().len(), 32);
        assert_eq!(Crypt::get_all_keys().unwrap().len(), 1);
        assert!(Crypt::get_previous_keys().unwrap().is_empty());
        assert!(Crypt::appears_encrypted(&payload));

        let payload = encrypt(&json!({"id": 1})).unwrap();
        assert_eq!(
            decrypt::<illuminate_support::Value>(&payload).unwrap(),
            json!({"id": 1})
        );
        assert_eq!(
            Crypt::decrypt::<illuminate_support::Value>(&payload).unwrap(),
            json!({"id": 1})
        );
        assert!(Crypt::encrypt(&5).is_ok());
    }

    #[test]
    fn a_missing_key_is_reported_as_an_error() {
        let container = container(json!({"key": null, "cipher": "AES-256-CBC"}));
        let _guard = Container::set_local_instance(container);

        let error = Crypt::encrypt_string("secret").unwrap_err();
        assert!(error.downcast_ref::<MissingAppKeyException>().is_some());
        assert_eq!(
            error.to_string(),
            "No application encryption key has been specified."
        );
    }

    #[test]
    #[should_panic(expected = "No application encryption key has been specified.")]
    fn resolving_the_encrypter_without_a_key_panics() {
        let container = container(json!({"key": ""}));
        container.make::<Encrypter>();
    }

    #[test]
    fn an_encrypter_can_be_built_without_the_provider() {
        let container = Arc::new(Container::new());
        container.instance(Repository::new(
            json!({"app": {"key": "bbbbbbbbbbbbbbbb", "cipher": "aes-128-cbc"}}),
        ));
        let _guard = Container::set_local_instance(container);
        assert_eq!(Crypt::encrypter().unwrap().cipher().name(), "aes-128-cbc");
        assert!(Crypt::supported("bbbbbbbbbbbbbbbb", "aes-128-cbc"));
        assert_eq!(Crypt::generate_key("aes-128-cbc").len(), 16);
    }
}
