//! The hashing service provider.

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};

use crate::manager::HashManager;

/// Registers the [`HashManager`], configured by `hashing.*`.
pub struct HashServiceProvider;

impl ServiceProvider for HashServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<HashManager>(|container| {
            let config = container
                .try_make::<Repository>()
                .unwrap_or_else(|_| Arc::new(Repository::empty()));
            Arc::new(HashManager::new(config))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BcryptHasher, Hash, HashOptions, bcrypt};
    use illuminate_support::json;

    fn app() -> Arc<Container> {
        let container = Arc::new(Container::new());
        container.instance(Repository::new(json!({
            "hashing": {
                "driver": "bcrypt",
                "bcrypt": {"rounds": 4, "verify": true},
                "argon": {"memory": 1024, "threads": 1, "time": 1, "verify": true},
            },
        })));
        HashServiceProvider.register(&container);
        container
    }

    #[test]
    fn the_facade_hashes_with_the_configured_driver() {
        let container = app();
        let _guard = Container::set_local_instance(container.clone());

        let hashed = Hash::make("password").unwrap();
        assert!(hashed.starts_with("$2y$04$"));
        assert!(Hash::check("password", &hashed));
        assert!(!Hash::check("nope", &hashed));
        assert!(Hash::try_check("password", &hashed).unwrap());
        assert!(!Hash::needs_rehash(&hashed));
        assert!(Hash::needs_rehash_with(
            &hashed,
            HashOptions::new().rounds(5)
        ));
        assert!(Hash::is_hashed(&hashed));
        assert_eq!(Hash::info(&hashed).algo_name, "bcrypt");
        assert!(Hash::verify_configuration(&hashed));
        assert!(
            Hash::make_with("password", HashOptions::new().rounds(5))
                .unwrap()
                .starts_with("$2y$05$")
        );
        assert!(bcrypt("password").unwrap().starts_with("$2y$04$"));
        assert!(Arc::ptr_eq(
            &Hash::manager(),
            &container.make::<HashManager>()
        ));

        let argon = Hash::driver("argon").unwrap().make("password").unwrap();
        assert!(argon.starts_with("$argon2i$v=19$m=1024,t=1,p=1$"));
        // Verification is on, so the bcrypt driver refuses to check an argon hash.
        assert!(Hash::try_check("password", &argon).is_err());
    }

    #[test]
    fn custom_drivers_can_be_registered() {
        let container = app();
        let _guard = Container::set_local_instance(container);
        Hash::extend("legacy", |_| Arc::new(BcryptHasher::new().rounds(5)));
        assert!(
            Hash::driver("legacy")
                .unwrap()
                .make("x")
                .unwrap()
                .starts_with("$2y$05$")
        );
    }

    #[test]
    fn the_facade_works_without_the_provider() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container);
        assert_eq!(Hash::manager().get_default_driver(), "bcrypt");
        assert!(Hash::check(
            "password",
            "$2y$10$92IXUNpkjO0rOQ5byMi.Ye4oKoEa3Ro9llC/.og/at2.uheWG/igi"
        ));
    }
}
