//! The cookie service provider.

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};

use crate::jar::CookieJar;

/// Registers the [`CookieJar`], configured from `session.path`,
/// `session.domain`, `session.secure`, and `session.same_site`.
pub struct CookieServiceProvider;

impl ServiceProvider for CookieServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<CookieJar>(|container| match container.try_make::<Repository>() {
            Ok(config) => Arc::new(CookieJar::from_config(&config)),
            Err(_) => Arc::new(CookieJar::new()),
        });
    }
}
