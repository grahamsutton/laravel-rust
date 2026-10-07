use laravel::prelude::*;

pub fn config() -> Value {
    json!({
        /*
        |--------------------------------------------------------------------------
        | Default Cache Store
        |--------------------------------------------------------------------------
        |
        | This option controls the default cache store that will be used by the
        | framework. This connection is utilized if another isn't explicitly
        | specified when running a cache operation inside the application.
        |
        */

        "default": env("CACHE_STORE", "database"),

        /*
        |--------------------------------------------------------------------------
        | Cache Stores
        |--------------------------------------------------------------------------
        |
        | Here you may define all of the cache "stores" for your application as
        | well as their drivers. You may even define multiple stores for the
        | same cache driver to group types of items stored in your caches.
        |
        | Supported drivers: "array", "database", "file", "null"
        |
        */

        "stores": {
            "array": {
                "driver": "array",
                "serialize": false,
            },

            "database": {
                "driver": "database",
                "connection": env("DB_CACHE_CONNECTION", Value::Null),
                "table": env("DB_CACHE_TABLE", "cache"),
                "lock_connection": env("DB_CACHE_LOCK_CONNECTION", Value::Null),
                "lock_table": env("DB_CACHE_LOCK_TABLE", Value::Null),
            },

            "file": {
                "driver": "file",
                "path": storage_path("framework/cache/data"),
                "lock_path": storage_path("framework/cache/data"),
            },

            "null": {
                "driver": "null",
            },
        },

        /*
        |--------------------------------------------------------------------------
        | Cache Key Prefix
        |--------------------------------------------------------------------------
        |
        | When utilizing the database cache store, there might be other
        | applications using the same cache. For that reason, you may prefix
        | every cache key to avoid collisions.
        |
        */

        "prefix": env("CACHE_PREFIX", format!("{}-cache-", Str::slug(&env("APP_NAME", "laravel").to_string_lossy()))),
    })
}
