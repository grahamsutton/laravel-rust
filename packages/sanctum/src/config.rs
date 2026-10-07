//! Sanctum's configuration: the `sanctum.*` keys of `config/sanctum.php`.
//!
//! Every option has Laravel's default, so Sanctum works without a
//! configuration file at all:
//!
//! | Key                    | Default                                                        |
//! | ---------------------- | -------------------------------------------------------------- |
//! | `sanctum.stateful`     | `localhost`, `localhost:3000`, `127.0.0.1`, `127.0.0.1:8000`, `::1`, plus the hosts of `app.url` and `app.frontend_url` |
//! | `sanctum.guard`        | `["web"]`                                                      |
//! | `sanctum.expiration`   | `null` (tokens never expire)                                   |
//! | `sanctum.token_prefix` | `""`                                                           |
//! | `sanctum.middleware`   | `authenticate_session`, `encrypt_cookies`, `validate_csrf_token` all `true` |
//! | `sanctum.prefix`       | `"sanctum"` (the `/sanctum/csrf-cookie` route's prefix)        |
//! | `sanctum.routes`       | `true` (set to `false` to skip the CSRF cookie route)          |
//!
//! Each `sanctum.middleware` entry may be `true` (Sanctum's default
//! middleware), `false` or `null` (skip it), or the name of a route
//! middleware alias to run instead — the Rust spelling of Laravel's class
//! names.
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_config::Repository;
//! use illuminate_container::Container;
//! use illuminate_support::json;
//! use laravel_sanctum::config;
//!
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container.clone());
//! container.instance(Repository::new(json!({
//!     "app": {"url": "https://laravel.test", "frontend_url": "http://spa.test:5173"},
//!     "sanctum": {"expiration": 525600},
//! })));
//!
//! assert_eq!(config::expiration(), Some(525600.0));
//! assert_eq!(config::guards(), ["web"]);
//! assert!(config::stateful_domains().contains(&"laravel.test".to_string()));
//! assert!(config::stateful_domains().contains(&"spa.test:5173".to_string()));
//! ```

use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::try_app;
use illuminate_support::{Value, ValueExt, json};

use crate::support::host_with_port;

/// The domains that are always stateful (Laravel's default list).
pub const DEFAULT_STATEFUL_DOMAINS: [&str; 5] = [
    "localhost",
    "localhost:3000",
    "127.0.0.1",
    "127.0.0.1:8000",
    "::1",
];

fn repository() -> Option<Arc<Repository>> {
    try_app::<Repository>()
}

fn get(key: &str) -> Option<Value> {
    let repository = repository()?;
    repository.has(key).then(|| repository.get(key))
}

/// A string or list of strings (`"web"`, `["web", "admin"]`, `"a,b"`).
fn strings(value: &Value, split_commas: bool) -> Vec<String> {
    let items: Vec<String> = match value {
        Value::Array(items) => items
            .iter()
            .filter(|item| !item.is_null())
            .map(ValueExt::to_string_lossy)
            .collect(),
        Value::Null => Vec::new(),
        Value::String(text) if split_commas => text.split(',').map(str::to_string).collect(),
        other => vec![other.to_string_lossy()],
    };
    items
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// The domains whose requests receive stateful (session) authentication:
/// `sanctum.stateful`, or Laravel's defaults.
pub fn stateful_domains() -> Vec<String> {
    match get("sanctum.stateful") {
        Some(value) => strings(&value, true),
        None => default_stateful_domains(),
    }
}

/// Laravel's default stateful domains: the local development hosts plus
/// the hosts (with ports) of `app.url` and `app.frontend_url`.
pub fn default_stateful_domains() -> Vec<String> {
    let mut domains: Vec<String> = DEFAULT_STATEFUL_DOMAINS
        .iter()
        .map(|domain| domain.to_string())
        .collect();
    for key in ["app.url", "app.frontend_url"] {
        let host = get(key)
            .filter(|url| !url.is_null())
            .and_then(|url| host_with_port(&url.to_string_lossy()));
        if let Some(host) = host
            && !domains.contains(&host)
        {
            domains.push(host);
        }
    }
    domains
}

/// The guards Sanctum checks for a session-authenticated user before
/// looking for an API token: `sanctum.guard` (default `["web"]`).
pub fn guards() -> Vec<String> {
    match get("sanctum.guard") {
        Some(value) => strings(&value, false),
        None => vec!["web".to_string()],
    }
}

/// The number of minutes until an issued token is considered expired
/// (`sanctum.expiration`). `None` — the default — means tokens never
/// expire on their own.
pub fn expiration() -> Option<f64> {
    get("sanctum.expiration")
        .and_then(|value| value.to_f64_lossy())
        .filter(|minutes| *minutes > 0.0)
}

/// The prefix added to new tokens (`sanctum.token_prefix`), which lets
/// secret scanners recognize them.
pub fn token_prefix() -> String {
    get("sanctum.token_prefix")
        .filter(|prefix| !prefix.is_null())
        .map(|prefix| prefix.to_string_lossy())
        .unwrap_or_default()
}

/// The URI prefix of Sanctum's routes (`sanctum.prefix`, default `sanctum`).
pub fn prefix() -> String {
    get("sanctum.prefix")
        .filter(|prefix| !prefix.is_null())
        .map(|prefix| prefix.to_string_lossy())
        .unwrap_or_else(|| "sanctum".to_string())
}

/// Whether Sanctum registers its routes (`sanctum.routes`, default `true`).
pub fn routes_enabled() -> bool {
    !matches!(get("sanctum.routes"), Some(Value::Bool(false)))
}

/// How one of the stateful middleware is configured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MiddlewareSetting {
    /// Run Sanctum's default middleware.
    Default,
    /// Skip it.
    Disabled,
    /// Run the route middleware registered under this alias instead.
    Alias(String),
}

/// The setting for one of the `sanctum.middleware.*` entries
/// (`authenticate_session`, `encrypt_cookies` or `validate_csrf_token`).
///
/// Like Laravel, a published `middleware` list without an
/// `authenticate_session` entry (an older config file) leaves that
/// middleware off, and `validate_csrf_token` falls back to the older
/// `verify_csrf_token` key.
pub fn middleware(key: &str) -> MiddlewareSetting {
    let mut value = get(&format!("sanctum.middleware.{key}"));
    if value.is_none() && key == "validate_csrf_token" {
        value = get("sanctum.middleware.verify_csrf_token");
    }
    if value.is_none() && key == "authenticate_session" && get("sanctum.middleware").is_some() {
        return MiddlewareSetting::Disabled;
    }
    match value {
        None | Some(Value::Bool(true)) => MiddlewareSetting::Default,
        Some(Value::Null) | Some(Value::Bool(false)) => MiddlewareSetting::Disabled,
        Some(Value::String(alias)) if alias.trim().is_empty() => MiddlewareSetting::Disabled,
        Some(other) => MiddlewareSetting::Alias(other.to_string_lossy().trim().to_string()),
    }
}

/// Sanctum's default configuration (what `config/sanctum.php` would
/// contain), with the stateful domains computed from the current `app.url`
/// and `app.frontend_url`.
///
/// The foundation merges it under any published `config/sanctum.rs`; a
/// published file usually reads `SANCTUM_STATEFUL_DOMAINS` and
/// `SANCTUM_TOKEN_PREFIX` from the environment.
pub fn defaults() -> Value {
    json!({
        "stateful": default_stateful_domains(),
        "guard": ["web"],
        "expiration": null,
        "token_prefix": "",
        "middleware": {
            "authenticate_session": true,
            "encrypt_cookies": true,
            "validate_csrf_token": true,
        },
    })
}

/// Fill in every `sanctum.*` option the application didn't configure
/// (Laravel's `mergeConfigFrom`).
pub(crate) fn merge_defaults(repository: &Repository) {
    if let Value::Object(defaults) = defaults() {
        for (key, value) in defaults {
            let key = format!("sanctum.{key}");
            if !repository.has(&key) {
                repository.set(&key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_container::Container;

    fn with_config(config: Value) -> illuminate_container::LocalInstanceGuard {
        let container = Arc::new(Container::new());
        let guard = Container::set_local_instance(container.clone());
        container.instance(Repository::new(config));
        guard
    }

    #[test]
    fn defaults_apply_without_configuration() {
        let _guard = with_config(json!({}));
        assert_eq!(stateful_domains(), DEFAULT_STATEFUL_DOMAINS);
        assert_eq!(guards(), ["web"]);
        assert_eq!(expiration(), None);
        assert_eq!(token_prefix(), "");
        assert_eq!(prefix(), "sanctum");
        assert!(routes_enabled());
        assert_eq!(middleware("encrypt_cookies"), MiddlewareSetting::Default);
    }

    #[test]
    fn defaults_apply_without_a_container() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container);
        assert_eq!(guards(), ["web"]);
        assert_eq!(stateful_domains().len(), 5);
    }

    #[test]
    fn the_application_urls_are_stateful() {
        let _guard = with_config(json!({"app": {
            "url": "https://laravel.test:8443",
            "frontend_url": "http://localhost:3000",
        }}));
        let domains = stateful_domains();
        assert!(domains.contains(&"laravel.test:8443".to_string()));
        assert_eq!(
            domains.iter().filter(|d| *d == "localhost:3000").count(),
            1,
            "hosts are only listed once"
        );
    }

    #[test]
    fn options_are_read_from_configuration() {
        let _guard = with_config(json!({"sanctum": {
            "stateful": "spa.test, admin.test:8080,",
            "guard": "admin",
            "expiration": "60",
            "token_prefix": "laravel_",
            "prefix": "auth",
            "routes": false,
            "middleware": {
                "authenticate_session": null,
                "encrypt_cookies": "cookies",
                "verify_csrf_token": false,
            },
        }}));
        assert_eq!(stateful_domains(), ["spa.test", "admin.test:8080"]);
        assert_eq!(guards(), ["admin"]);
        assert_eq!(expiration(), Some(60.0));
        assert_eq!(token_prefix(), "laravel_");
        assert_eq!(prefix(), "auth");
        assert!(!routes_enabled());
        assert_eq!(
            middleware("authenticate_session"),
            MiddlewareSetting::Disabled
        );
        assert_eq!(
            middleware("encrypt_cookies"),
            MiddlewareSetting::Alias("cookies".into())
        );
        assert_eq!(
            middleware("validate_csrf_token"),
            MiddlewareSetting::Disabled
        );
    }

    #[test]
    fn older_middleware_lists_leave_session_authentication_off() {
        let _guard = with_config(json!({"sanctum": {"middleware": {
            "encrypt_cookies": true,
        }}}));
        assert_eq!(
            middleware("authenticate_session"),
            MiddlewareSetting::Disabled
        );
        assert_eq!(
            middleware("validate_csrf_token"),
            MiddlewareSetting::Default
        );
    }

    #[test]
    fn a_zero_expiration_never_expires() {
        let _guard = with_config(json!({"sanctum": {"expiration": 0}}));
        assert_eq!(expiration(), None);
    }

    #[test]
    fn defaults_are_merged_under_the_application_config() {
        let repository = Repository::new(json!({"sanctum": {"expiration": 30}}));
        merge_defaults(&repository);
        assert_eq!(repository.get("sanctum.expiration"), json!(30));
        assert_eq!(repository.get("sanctum.guard"), json!(["web"]));
        assert_eq!(repository.get("sanctum.token_prefix"), json!(""));
        assert_eq!(
            repository.get("sanctum.middleware.validate_csrf_token"),
            json!(true)
        );
    }
}
