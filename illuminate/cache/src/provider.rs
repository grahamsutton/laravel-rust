//! The cache service provider.

use std::sync::Arc;

use illuminate_config::Repository as Config;
use illuminate_container::{Container, ServiceProvider, try_app};
use illuminate_support::Result;

use crate::manager::CacheManager;
use crate::rate_limiter::RateLimiter;
use crate::repository::Repository;

/// Registers the [`CacheManager`] (the `Cache` facade), the default cache
/// [`Repository`], and the [`RateLimiter`].
///
/// Configuration is read from the `cache` key of the configuration
/// repository: `cache.default`, `cache.stores.*`, `cache.prefix`, and
/// `cache.limiter` (the store the rate limiter uses).
pub struct CacheServiceProvider;

pub(crate) fn make_manager(container: &Container) -> Arc<CacheManager> {
    let config = container
        .try_make::<Config>()
        .unwrap_or_else(|_| Arc::new(Config::empty()));
    Arc::new(CacheManager::new(config))
}

fn resolve_manager(container: &Container) -> Arc<CacheManager> {
    container.singleton_if::<CacheManager>(make_manager);
    container.make::<CacheManager>()
}

impl ServiceProvider for CacheServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<CacheManager>(make_manager);

        app.singleton::<Repository>(
            |container| match resolve_manager(container).default_store() {
                Ok(repository) => Arc::new(repository),
                Err(error) => panic!("{error}"),
            },
        );

        app.singleton::<RateLimiter>(|container| {
            match resolve_manager(container).limiter_store() {
                Ok(repository) => Arc::new(RateLimiter::new(repository)),
                Err(error) => panic!("{error}"),
            }
        });
    }
}

/// Resolve the rate limiter, registering it on first use.
pub(crate) fn rate_limiter() -> Result<Arc<RateLimiter>> {
    if let Some(limiter) = try_app::<RateLimiter>() {
        return Ok(limiter);
    }
    let repository = crate::facade::manager()?.limiter_store()?;
    let container = Container::get_instance();
    container.singleton_if::<RateLimiter>(move |_| Arc::new(RateLimiter::new(repository.clone())));
    Ok(container.try_make::<RateLimiter>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[tokio::test]
    async fn it_registers_the_cache_services() {
        let container = Container::new();
        container.instance(Config::new(json!({
            "cache": {
                "default": "array",
                "limiter": "limits",
                "stores": {"array": {"driver": "array"}, "limits": {"driver": "array"}},
            },
        })));
        CacheServiceProvider.register(&container);

        let manager = container.make::<CacheManager>();
        let repository = container.make::<Repository>();
        assert_eq!(repository.get_name(), Some("array"));
        assert!(Arc::ptr_eq(
            &repository.get_store(),
            &manager.store("array").unwrap().get_store()
        ));

        let limiter = container.make::<RateLimiter>();
        assert_eq!(limiter.cache().get_name(), Some("limits"));
    }

    #[test]
    fn facades_register_services_on_demand() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container.clone());
        container.instance(Config::new(json!({"cache": {"default": "null"}})));
        let limiter = rate_limiter().unwrap();
        assert!(Arc::ptr_eq(&limiter, &rate_limiter().unwrap()));
        assert_eq!(limiter.cache().get_name(), Some("null"));
    }
}
