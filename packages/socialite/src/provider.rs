//! The Socialite service provider.

use std::sync::Arc;

use illuminate_container::{Container, ServiceProvider};

use crate::manager::SocialiteManager;

/// Registers the [`SocialiteManager`] (the [`Socialite`](crate::Socialite)
/// facade's service) as a singleton.
///
/// Socialite has no configuration file of its own: each driver reads its
/// credentials from `config/services` (`services.github.client_id`, ...).
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::{Container, ServiceProvider};
/// use laravel_socialite::{Socialite, SocialiteManager, SocialiteServiceProvider};
///
/// let app = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(app.clone());
///
/// SocialiteServiceProvider.register(&app);
///
/// assert!(Arc::ptr_eq(&Socialite::manager(), &app.make::<SocialiteManager>()));
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct SocialiteServiceProvider;

impl ServiceProvider for SocialiteServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton_if::<SocialiteManager>(|_| Arc::new(SocialiteManager::new()));
    }
}
