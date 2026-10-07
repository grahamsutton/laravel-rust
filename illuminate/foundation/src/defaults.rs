//! The framework's default configuration.
//!
//! Like Laravel 11+, configuration files in your application are optional:
//! every option has a sensible default here, and any file you publish in
//! `config/` is merged over these defaults.

use illuminate_support::{Str, Value, ValueExt, env, json};

use crate::bootstrap::ConfigFile;
use crate::helpers::{database_path, public_path, resource_path, storage_path};

/// Every default configuration file.
pub fn all() -> Vec<ConfigFile> {
    vec![
        ConfigFile::new("app", app),
        ConfigFile::new("auth", auth),
        ConfigFile::new("broadcasting", broadcasting),
        ConfigFile::new("cache", cache),
        ConfigFile::new("cors", cors),
        ConfigFile::new("database", database),
        ConfigFile::new("filesystems", filesystems),
        ConfigFile::new("hashing", hashing),
        ConfigFile::new("logging", logging),
        ConfigFile::new("mail", mail),
        ConfigFile::new("queue", queue),
        ConfigFile::new("services", services),
        ConfigFile::new("session", session),
        ConfigFile::new("view", view),
    ]
}

fn app_name_slug(separator: &str) -> String {
    Str::slug_with(&env("APP_NAME", "laravel").to_string_lossy(), separator)
}

fn previous_keys() -> Value {
    let raw = env("APP_PREVIOUS_KEYS", "").to_string_lossy();
    Value::Array(
        raw.split(',')
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(|k| Value::String(k.to_string()))
            .collect(),
    )
}

pub fn app() -> Value {
    json!({
        "name": env("APP_NAME", "Laravel"),
        "env": env("APP_ENV", "production"),
        "debug": env("APP_DEBUG", false).truthy(),
        "url": env("APP_URL", "http://localhost"),
        "frontend_url": env("FRONTEND_URL", "http://localhost:3000"),
        "asset_url": env("ASSET_URL", Value::Null),
        "timezone": "UTC",
        "locale": env("APP_LOCALE", "en"),
        "fallback_locale": env("APP_FALLBACK_LOCALE", "en"),
        "faker_locale": env("APP_FAKER_LOCALE", "en_US"),
        "lang_path": crate::helpers::lang_path(""),
        "cipher": "AES-256-CBC",
        "key": env("APP_KEY", Value::Null),
        "previous_keys": previous_keys(),
        "maintenance": {
            "driver": env("APP_MAINTENANCE_DRIVER", "file"),
            "store": env("APP_MAINTENANCE_STORE", "database"),
        },
    })
}

pub fn auth() -> Value {
    json!({
        "defaults": {
            "guard": env("AUTH_GUARD", "web"),
            "passwords": env("AUTH_PASSWORD_BROKER", "users"),
        },
        "guards": {
            "web": {"driver": "session", "provider": "users"},
        },
        "providers": {
            "users": {"driver": "eloquent", "model": env("AUTH_MODEL", "User")},
        },
        "passwords": {
            "users": {
                "provider": "users",
                "table": env("AUTH_PASSWORD_RESET_TOKEN_TABLE", "password_reset_tokens"),
                "expire": 60,
                "throttle": 60,
            },
        },
        "password_timeout": env("AUTH_PASSWORD_TIMEOUT", 10800),
    })
}

pub fn broadcasting() -> Value {
    let https = |scheme: &str| env(scheme, "https").to_string_lossy() == "https";
    json!({
        "default": env("BROADCAST_CONNECTION", "null"),
        "connections": {
            "reverb": {
                "driver": "reverb",
                "key": env("REVERB_APP_KEY", Value::Null),
                "secret": env("REVERB_APP_SECRET", Value::Null),
                "app_id": env("REVERB_APP_ID", Value::Null),
                "options": {
                    "host": env("REVERB_HOST", Value::Null),
                    "port": env("REVERB_PORT", 443),
                    "scheme": env("REVERB_SCHEME", "https"),
                    "useTLS": https("REVERB_SCHEME"),
                },
                "client_options": {},
            },
            "pusher": {
                "driver": "pusher",
                "key": env("PUSHER_APP_KEY", Value::Null),
                "secret": env("PUSHER_APP_SECRET", Value::Null),
                "app_id": env("PUSHER_APP_ID", Value::Null),
                "options": {
                    "cluster": env("PUSHER_APP_CLUSTER", Value::Null),
                    "host": match env("PUSHER_HOST", Value::Null) {
                        Value::Null => Value::String(format!("api-{}.pusher.com", env("PUSHER_APP_CLUSTER", "mt1").to_string_lossy())),
                        host => host,
                    },
                    "port": env("PUSHER_PORT", 443),
                    "scheme": env("PUSHER_SCHEME", "https"),
                    "encrypted": true,
                    "useTLS": https("PUSHER_SCHEME"),
                },
                "client_options": {},
            },
            "ably": {
                "driver": "ably",
                "key": env("ABLY_KEY", Value::Null),
            },
            "log": {"driver": "log"},
            "null": {"driver": "null"},
        },
    })
}

pub fn cache() -> Value {
    json!({
        "default": env("CACHE_STORE", "database"),
        "stores": {
            "array": {"driver": "array", "serialize": false},
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
            "redis": {
                "driver": "redis",
                "connection": env("REDIS_CACHE_CONNECTION", "cache"),
                "lock_connection": env("REDIS_CACHE_LOCK_CONNECTION", "default"),
            },
            "null": {"driver": "null"},
        },
        "prefix": env("CACHE_PREFIX", format!("{}-cache-", app_name_slug("-"))),
    })
}

pub fn cors() -> Value {
    json!({
        "paths": ["api/*", "sanctum/csrf-cookie"],
        "allowed_methods": ["*"],
        "allowed_origins": ["*"],
        "allowed_origins_patterns": [],
        "allowed_headers": ["*"],
        "exposed_headers": [],
        "max_age": 0,
        "supports_credentials": false,
    })
}

pub fn database() -> Value {
    json!({
        "default": env("DB_CONNECTION", "sqlite"),
        "connections": {
            "sqlite": {
                "driver": "sqlite",
                "url": env("DB_URL", Value::Null),
                "database": env("DB_DATABASE", database_path("database.sqlite")),
                "prefix": "",
                "foreign_key_constraints": env("DB_FOREIGN_KEYS", true),
                "busy_timeout": Value::Null,
                "journal_mode": Value::Null,
                "synchronous": Value::Null,
            },
            "mysql": {
                "driver": "mysql",
                "url": env("DB_URL", Value::Null),
                "host": env("DB_HOST", "127.0.0.1"),
                "port": env("DB_PORT", "3306"),
                "database": env("DB_DATABASE", "laravel"),
                "username": env("DB_USERNAME", "root"),
                "password": env("DB_PASSWORD", ""),
                "unix_socket": env("DB_SOCKET", ""),
                "charset": env("DB_CHARSET", "utf8mb4"),
                "collation": env("DB_COLLATION", "utf8mb4_unicode_ci"),
                "prefix": "",
                "prefix_indexes": true,
                "strict": true,
                "engine": Value::Null,
            },
            "mariadb": {
                "driver": "mariadb",
                "url": env("DB_URL", Value::Null),
                "host": env("DB_HOST", "127.0.0.1"),
                "port": env("DB_PORT", "3306"),
                "database": env("DB_DATABASE", "laravel"),
                "username": env("DB_USERNAME", "root"),
                "password": env("DB_PASSWORD", ""),
                "unix_socket": env("DB_SOCKET", ""),
                "charset": env("DB_CHARSET", "utf8mb4"),
                "collation": env("DB_COLLATION", "utf8mb4_unicode_ci"),
                "prefix": "",
                "prefix_indexes": true,
                "strict": true,
                "engine": Value::Null,
            },
            "pgsql": {
                "driver": "pgsql",
                "url": env("DB_URL", Value::Null),
                "host": env("DB_HOST", "127.0.0.1"),
                "port": env("DB_PORT", "5432"),
                "database": env("DB_DATABASE", "laravel"),
                "username": env("DB_USERNAME", "root"),
                "password": env("DB_PASSWORD", ""),
                "charset": env("DB_CHARSET", "utf8"),
                "prefix": "",
                "prefix_indexes": true,
                "search_path": "public",
                "sslmode": "prefer",
            },
        },
        "migrations": {
            "table": "migrations",
            "update_date_on_publish": true,
        },
    })
}

pub fn filesystems() -> Value {
    let app_url = env("APP_URL", "").to_string_lossy();
    let mut links = illuminate_support::Map::new();
    links.insert(public_path("storage"), Value::String(storage_path("app/public")));
    json!({
        "default": env("FILESYSTEM_DISK", "local"),
        "disks": {
            "local": {
                "driver": "local",
                "root": storage_path("app/private"),
                "serve": true,
                "throw": false,
                "report": false,
            },
            "public": {
                "driver": "local",
                "root": storage_path("app/public"),
                "url": format!("{}/storage", app_url.trim_end_matches('/')),
                "visibility": "public",
                "throw": false,
                "report": false,
            },
        },
        "links": links,
    })
}

pub fn hashing() -> Value {
    json!({
        "driver": env("HASH_DRIVER", "bcrypt"),
        "bcrypt": {
            "rounds": env("BCRYPT_ROUNDS", 12),
            "verify": env("HASH_VERIFY", true),
            "limit": env("BCRYPT_LIMIT", Value::Null),
        },
        "argon": {
            "memory": env("ARGON_MEMORY", 65536),
            "threads": env("ARGON_THREADS", 1),
            "time": env("ARGON_TIME", 4),
            "verify": env("HASH_VERIFY", true),
        },
        "rehash_on_login": true,
    })
}

pub fn logging() -> Value {
    let stack: Vec<String> = env("LOG_STACK", "single")
        .to_string_lossy()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    json!({
        "default": env("LOG_CHANNEL", "stack"),
        "deprecations": {
            "channel": env("LOG_DEPRECATIONS_CHANNEL", "null"),
            "trace": env("LOG_DEPRECATIONS_TRACE", false),
        },
        "channels": {
            "stack": {
                "driver": "stack",
                "channels": stack,
                "ignore_exceptions": false,
            },
            "single": {
                "driver": "single",
                "path": storage_path("logs/laravel.log"),
                "level": env("LOG_LEVEL", "debug"),
                "replace_placeholders": true,
            },
            "daily": {
                "driver": "daily",
                "path": storage_path("logs/laravel.log"),
                "level": env("LOG_LEVEL", "debug"),
                "days": env("LOG_DAILY_DAYS", 14),
                "replace_placeholders": true,
            },
            "stderr": {
                "driver": "stderr",
                "level": env("LOG_LEVEL", "debug"),
            },
            "slack": {
                "driver": "slack",
                "url": env("LOG_SLACK_WEBHOOK_URL", Value::Null),
                "username": env("LOG_SLACK_USERNAME", "Laravel Log"),
                "emoji": env("LOG_SLACK_EMOJI", ":boom:"),
                "level": env("LOG_LEVEL", "critical"),
                "replace_placeholders": true,
            },
            "papertrail": {
                "driver": "monolog",
                "level": env("LOG_LEVEL", "debug"),
                "handler": env("LOG_PAPERTRAIL_HANDLER", "SyslogUdpHandler"),
                "handler_with": {
                    "host": env("PAPERTRAIL_URL", Value::Null),
                    "port": env("PAPERTRAIL_PORT", Value::Null),
                    "connectionString": format!("tls://{}:{}", env("PAPERTRAIL_URL", "").to_string_lossy(), env("PAPERTRAIL_PORT", "").to_string_lossy()),
                },
                "processors": ["PsrLogMessageProcessor"],
            },
            "errorlog": {
                "driver": "errorlog",
                "level": env("LOG_LEVEL", "debug"),
            },
            "null": {
                "driver": "null",
            },
            "emergency": {
                "path": storage_path("logs/laravel.log"),
            },
        },
    })
}

pub fn mail() -> Value {
    let app_url = env("APP_URL", "http://localhost").to_string_lossy();
    let host = app_url
        .split("://")
        .nth(1)
        .unwrap_or("localhost")
        .split(['/', ':'])
        .next()
        .unwrap_or("localhost")
        .to_string();
    json!({
        "default": env("MAIL_MAILER", "log"),
        "mailers": {
            "smtp": {
                "transport": "smtp",
                "scheme": env("MAIL_SCHEME", Value::Null),
                "url": env("MAIL_URL", Value::Null),
                "host": env("MAIL_HOST", "127.0.0.1"),
                "port": env("MAIL_PORT", 2525),
                "username": env("MAIL_USERNAME", Value::Null),
                "password": env("MAIL_PASSWORD", Value::Null),
                "timeout": Value::Null,
                "local_domain": env("MAIL_EHLO_DOMAIN", host),
            },
            "ses": {
                "transport": "ses",
            },
            "postmark": {
                "transport": "postmark",
            },
            "resend": {
                "transport": "resend",
            },
            "sendmail": {
                "transport": "sendmail",
                "path": env("MAIL_SENDMAIL_PATH", "/usr/sbin/sendmail -bs -i"),
            },
            "log": {
                "transport": "log",
                "channel": env("MAIL_LOG_CHANNEL", Value::Null),
            },
            "array": {
                "transport": "array",
            },
            "failover": {
                "transport": "failover",
                "mailers": ["smtp", "log"],
                "retry_after": 60,
            },
            "roundrobin": {
                "transport": "roundrobin",
                "mailers": ["ses", "postmark"],
                "retry_after": 60,
            },
        },
        "from": {
            "address": env("MAIL_FROM_ADDRESS", "hello@example.com"),
            "name": env("MAIL_FROM_NAME", "Example"),
        },
        "markdown": {
            "theme": env("MAIL_MARKDOWN_THEME", "default"),
            "paths": [resource_path("views/vendor/mail")],
        },
    })
}

pub fn queue() -> Value {
    json!({
        "default": env("QUEUE_CONNECTION", "database"),
        "connections": {
            "sync": {"driver": "sync"},
            "database": {
                "driver": "database",
                "connection": env("DB_QUEUE_CONNECTION", Value::Null),
                "table": env("DB_QUEUE_TABLE", "jobs"),
                "queue": env("DB_QUEUE", "default"),
                "retry_after": env("DB_QUEUE_RETRY_AFTER", 90).to_i64_lossy().unwrap_or(90),
                "after_commit": false,
            },
            "redis": {
                "driver": "redis",
                "connection": env("REDIS_QUEUE_CONNECTION", "default"),
                "queue": env("REDIS_QUEUE", "default"),
                "retry_after": env("REDIS_QUEUE_RETRY_AFTER", 90).to_i64_lossy().unwrap_or(90),
                "block_for": Value::Null,
                "after_commit": false,
            },
            "deferred": {"driver": "deferred"},
            "null": {"driver": "null"},
        },
        "batching": {
            "database": env("DB_CONNECTION", "sqlite"),
            "table": "job_batches",
        },
        "failed": {
            "driver": env("QUEUE_FAILED_DRIVER", "database-uuids"),
            "database": env("DB_CONNECTION", "sqlite"),
            "table": "failed_jobs",
        },
    })
}

pub fn services() -> Value {
    json!({
        "postmark": {"token": env("POSTMARK_TOKEN", Value::Null)},
        "resend": {"key": env("RESEND_KEY", Value::Null)},
        "ses": {
            "key": env("AWS_ACCESS_KEY_ID", Value::Null),
            "secret": env("AWS_SECRET_ACCESS_KEY", Value::Null),
            "region": env("AWS_DEFAULT_REGION", "us-east-1"),
        },
        "slack": {
            "notifications": {
                "bot_user_oauth_token": env("SLACK_BOT_USER_OAUTH_TOKEN", Value::Null),
                "channel": env("SLACK_BOT_USER_DEFAULT_CHANNEL", Value::Null),
            },
        },
    })
}

pub fn session() -> Value {
    json!({
        "driver": env("SESSION_DRIVER", "database"),
        "lifetime": env("SESSION_LIFETIME", 120).to_i64_lossy().unwrap_or(120),
        "expire_on_close": env("SESSION_EXPIRE_ON_CLOSE", false),
        "encrypt": env("SESSION_ENCRYPT", false),
        "files": storage_path("framework/sessions"),
        "connection": env("SESSION_CONNECTION", Value::Null),
        "table": env("SESSION_TABLE", "sessions"),
        "store": env("SESSION_STORE", Value::Null),
        "lottery": [2, 100],
        "cookie": env("SESSION_COOKIE", format!("{}-session", app_name_slug("-"))),
        "path": env("SESSION_PATH", "/"),
        "domain": env("SESSION_DOMAIN", Value::Null),
        "secure": env("SESSION_SECURE_COOKIE", Value::Null),
        "http_only": env("SESSION_HTTP_ONLY", true),
        "same_site": env("SESSION_SAME_SITE", "lax"),
        "partitioned": env("SESSION_PARTITIONED_COOKIE", false),
    })
}

pub fn view() -> Value {
    json!({
        "paths": [resource_path("views")],
        "compiled": env("VIEW_COMPILED_PATH", storage_path("framework/views")),
    })
}
