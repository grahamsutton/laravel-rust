//! The Redis service provider.

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};

use crate::connection::Connection;
use crate::manager::RedisManager;

/// Registers the [`RedisManager`] (the `Redis` facade) and the default
/// [`Connection`].
///
/// Configuration is read from `database.redis`: `client`, the shared
/// `options` (`prefix`, ...) and the named connections (`default`,
/// `cache`, ...), each with a `url` or `host`, `port`, `username`,
/// `password` and `database`.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_redis::{Connection, Redis, RedisServiceProvider};
/// use illuminate_support::json;
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({
///     "app": {"name": "Laravel"},
///     "database": {"redis": {
///         "client": "phpredis",
///         "options": {"cluster": "redis"},
///         "default": {"host": "127.0.0.1", "port": 6379, "database": 0},
///     }},
/// })));
/// RedisServiceProvider.register(&container);
///
/// let redis = Redis::connection(None).unwrap();
/// assert_eq!(redis.prefix(), "laravel-database-");
/// assert!(redis.same_as(&container.make::<Connection>()));
/// ```
pub struct RedisServiceProvider;

pub(crate) fn make_manager(container: &Container) -> Arc<RedisManager> {
    let config = container
        .try_make::<Repository>()
        .unwrap_or_else(|_| Arc::new(Repository::empty()));
    Arc::new(RedisManager::from_config(&config))
}

impl ServiceProvider for RedisServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<RedisManager>(make_manager);

        app.bind::<Connection>(|container| {
            container.singleton_if::<RedisManager>(make_manager);
            match container.make::<RedisManager>().connection(None) {
                Ok(connection) => Arc::new(connection),
                Err(error) => panic!("{error}"),
            }
        });
    }
}
