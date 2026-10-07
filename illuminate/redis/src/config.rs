//! Connection configuration: one entry of `database.redis`.

use std::path::PathBuf;
use std::time::Duration;

use percent_encoding::percent_decode_str;
use redis::{ConnectionAddr, ConnectionInfo, IntoConnectionInfo, RedisConnectionInfo};

use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Map, Result, Value, ValueExt};

/// The settings of a single Redis connection.
///
/// Parsed from a `database.redis.*` entry the way Laravel reads it: a `url`
/// (`redis://user:secret@127.0.0.1:6380/2`, `tcp://...`, `tls://...`)
/// overrides the individual `host`, `port`, `username`, `password` and
/// `database` keys, and the shared `options` (like the key `prefix`) apply
/// to every connection unless the connection overrides them.
///
/// ```
/// use illuminate_redis::ConnectionConfig;
/// use illuminate_support::json;
///
/// let config = ConnectionConfig::parse(
///     &json!({"url": "redis://:secret@10.0.0.5:6380/2", "host": "127.0.0.1"}),
///     &json!({"prefix": "laravel-database-"}),
/// ).unwrap();
///
/// assert_eq!(config.host, "10.0.0.5");
/// assert_eq!(config.port, 6380);
/// assert_eq!(config.database, 2);
/// assert_eq!(config.password.as_deref(), Some("secret"));
/// assert_eq!(config.prefix, "laravel-database-");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionConfig {
    /// `tcp`, `tls` or `unix`.
    pub scheme: String,
    /// The host name, or the path of a Unix socket.
    pub host: String,
    /// The port (`0` for Unix sockets).
    pub port: u16,
    /// The ACL user name.
    pub username: Option<String>,
    /// The password.
    pub password: Option<String>,
    /// The database index to `SELECT`.
    pub database: i64,
    /// The prefix applied to every key (phpredis' `OPT_PREFIX`).
    pub prefix: String,
    /// How long to wait for a connection (`timeout`, in seconds).
    pub timeout: Option<Duration>,
    /// How long to wait for a reply (`read_timeout`, in seconds; negative
    /// waits forever).
    pub read_timeout: Option<Duration>,
    /// How many times to retry connecting (`max_retries`).
    pub max_retries: usize,
    /// The base delay between reconnection attempts (`backoff_base`, in ms).
    pub backoff_base: Duration,
    /// The maximum delay between reconnection attempts (`backoff_cap`, in ms).
    pub backoff_cap: Duration,
    /// How many times to retry *any* command after a lost connection
    /// (`command_retries`). Safe read commands are always retried once.
    pub command_retries: usize,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            scheme: "tcp".to_string(),
            host: "127.0.0.1".to_string(),
            port: 6379,
            username: None,
            password: None,
            database: 0,
            prefix: String::new(),
            timeout: Some(Duration::from_secs(5)),
            read_timeout: Some(Duration::from_secs(60)),
            max_retries: 3,
            backoff_base: Duration::from_millis(100),
            backoff_cap: Duration::from_millis(1000),
            command_retries: 0,
        }
    }
}

impl ConnectionConfig {
    /// Parse a connection's configuration, merged with the shared `options`.
    pub fn parse(config: &Value, options: &Value) -> Result<Self> {
        let Some(connection) = config.as_object() else {
            return Err(InvalidArgumentException::new(
                "A Redis connection's configuration must be an object.",
            )
            .into());
        };

        // The connection's settings, overridden by its URL...
        let mut settings = connection.clone();
        if let Some(url) = connection.get("url").and_then(Value::as_str)
            && !url.trim().is_empty()
        {
            settings.extend(parse_url(url)?);
        }

        // ...then the shared options, its own options, and its own prefix.
        let mut merged = settings.clone();
        if let Some(options) = options.as_object() {
            merged.extend(options.clone());
        }
        if let Some(own) = settings.get("options").and_then(Value::as_object) {
            merged.extend(own.clone());
        }
        if let Some(prefix) = settings.get("prefix").filter(|prefix| !prefix.is_null()) {
            merged.insert("prefix".to_string(), prefix.clone());
        }

        let defaults = Self::default();
        let string = |key: &str| {
            merged
                .get(key)
                .filter(|value| !value.is_null())
                .map(ValueExt::to_string_lossy)
                .filter(|value| !value.is_empty())
        };
        let number = |key: &str| merged.get(key).and_then(ValueExt::to_i64_lossy);
        let seconds = |key: &str| {
            merged.get(key).and_then(|value| {
                value
                    .as_f64()
                    .or_else(|| value.to_string_lossy().trim().parse().ok())
            })
        };

        let host = string("host").unwrap_or(defaults.host.clone());
        let mut scheme = string("scheme")
            .map(|scheme| scheme.to_ascii_lowercase())
            .unwrap_or_else(|| "tcp".to_string());
        scheme = match scheme.as_str() {
            "tls" | "rediss" | "ssl" => "tls".to_string(),
            "unix" => "unix".to_string(),
            _ if host.starts_with('/') => "unix".to_string(),
            _ => "tcp".to_string(),
        };

        let port = match number("port") {
            Some(port) => u16::try_from(port).map_err(|_| {
                InvalidArgumentException::new(format!("Invalid Redis port [{port}]."))
            })?,
            None if scheme == "unix" => 0,
            None => defaults.port,
        };

        let database = match merged.get("database") {
            None | Some(Value::Null) => 0,
            Some(value) => value.to_i64_lossy().ok_or_else(|| {
                InvalidArgumentException::new(format!(
                    "Invalid Redis database [{}].",
                    value.to_string_lossy()
                ))
            })?,
        };

        let read_timeout = match seconds("read_timeout") {
            Some(timeout) if timeout < 0.0 => None,
            Some(timeout) if timeout > 0.0 => Some(Duration::from_secs_f64(timeout)),
            _ => defaults.read_timeout,
        };
        let timeout = match seconds("timeout") {
            Some(timeout) if timeout > 0.0 => Some(Duration::from_secs_f64(timeout)),
            _ => defaults.timeout,
        };
        let count =
            |key: &str, default: usize| number(key).map_or(default, |value| value.max(0) as usize);
        let millis = |key: &str, default: Duration| {
            number(key).map_or(default, |value| Duration::from_millis(value.max(0) as u64))
        };

        Ok(Self {
            scheme,
            host,
            port,
            username: string("username"),
            password: string("password"),
            database,
            prefix: string("prefix").unwrap_or_default(),
            timeout,
            read_timeout,
            max_retries: count("max_retries", defaults.max_retries),
            backoff_base: millis("backoff_base", defaults.backoff_base),
            backoff_cap: millis("backoff_cap", defaults.backoff_cap),
            command_retries: count("command_retries", defaults.command_retries),
        })
    }

    /// The redis-rs connection details for this connection.
    pub fn connection_info(&self) -> Result<ConnectionInfo> {
        let addr = match self.scheme.as_str() {
            "unix" => ConnectionAddr::Unix(PathBuf::from(&self.host)),
            "tls" => ConnectionAddr::TcpTls {
                host: self.host.clone(),
                port: self.port,
                insecure: false,
                tls_params: None,
            },
            _ => ConnectionAddr::Tcp(self.host.clone(), self.port),
        };
        if !addr.is_supported() {
            return Err(InvalidArgumentException::new(format!(
                "Redis [{}] connections are not supported on this platform.",
                self.scheme
            ))
            .into());
        }

        let mut redis = RedisConnectionInfo::default().set_db(self.database);
        if let Some(username) = &self.username {
            redis = redis.set_username(username);
        }
        if let Some(password) = &self.password {
            redis = redis.set_password(password);
        }
        Ok(addr.into_connection_info()?.set_redis_settings(redis))
    }
}

/// Parse a connection URL into configuration overrides (Laravel's
/// `ConfigurationUrlParser`): the scheme, host, port, credentials, the
/// database from the path, and any query string options.
fn parse_url(url: &str) -> Result<Map<String, Value>> {
    let parsed = url::Url::parse(url).map_err(|error| {
        InvalidArgumentException::new(format!("The Redis URL [{url}] is malformed: {error}."))
    })?;
    let decode = |value: &str| percent_decode_str(value).decode_utf8_lossy().into_owned();

    let mut overrides = Map::new();
    let scheme = parsed.scheme().to_ascii_lowercase();
    let scheme = match scheme.as_str() {
        "tls" | "rediss" | "ssl" => "tls",
        "unix" | "redis+unix" => "unix",
        _ => "tcp",
    };
    overrides.insert("scheme".into(), Value::from(scheme));

    if scheme == "unix" {
        overrides.insert("host".into(), Value::from(decode(parsed.path())));
    } else {
        if let Some(host) = parsed.host_str().filter(|host| !host.is_empty()) {
            let host = host.trim_start_matches('[').trim_end_matches(']');
            overrides.insert("host".into(), Value::from(decode(host)));
        }
        if let Some(port) = parsed.port() {
            overrides.insert("port".into(), Value::from(port));
        }
        let path = parsed.path().trim_start_matches('/');
        if !path.is_empty() {
            overrides.insert("database".into(), Value::from(decode(path)));
        }
    }
    if !parsed.username().is_empty() {
        overrides.insert("username".into(), Value::from(decode(parsed.username())));
    }
    if let Some(password) = parsed.password() {
        overrides.insert("password".into(), Value::from(decode(password)));
    }
    for (key, value) in parsed.query_pairs() {
        overrides.insert(key.into_owned(), Value::from(value.into_owned()));
    }
    Ok(overrides)
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn parse(config: Value) -> ConnectionConfig {
        ConnectionConfig::parse(&config, &json!({})).unwrap()
    }

    #[test]
    fn it_reads_laravels_connection_keys() {
        let config = ConnectionConfig::parse(
            &json!({
                "url": null,
                "host": "redis.internal",
                "username": "app",
                "password": "secret",
                "port": "6380",
                "database": "3",
                "max_retries": 5,
                "backoff_base": 50,
                "backoff_cap": 500,
                "read_timeout": 2.5,
                "timeout": 1,
                "command_retries": "2",
            }),
            &json!({"cluster": "redis", "prefix": "laravel-database-"}),
        )
        .unwrap();
        assert_eq!(config.scheme, "tcp");
        assert_eq!(config.host, "redis.internal");
        assert_eq!(config.port, 6380);
        assert_eq!(config.username.as_deref(), Some("app"));
        assert_eq!(config.password.as_deref(), Some("secret"));
        assert_eq!(config.database, 3);
        assert_eq!(config.prefix, "laravel-database-");
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.backoff_base, Duration::from_millis(50));
        assert_eq!(config.backoff_cap, Duration::from_millis(500));
        assert_eq!(config.read_timeout, Some(Duration::from_millis(2500)));
        assert_eq!(config.timeout, Some(Duration::from_secs(1)));
        assert_eq!(config.command_retries, 2);
    }

    #[test]
    fn defaults_follow_laravel() {
        let config = parse(json!({}));
        assert_eq!(config, ConnectionConfig::default());
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 6379);
        assert_eq!(config.database, 0);
        assert_eq!(config.prefix, "");

        let config = parse(json!({"username": null, "password": "", "read_timeout": -1}));
        assert_eq!(config.username, None);
        assert_eq!(config.password, None);
        assert_eq!(config.read_timeout, None);
    }

    #[test]
    fn urls_override_the_individual_keys() {
        let config = parse(json!({
            "url": "tcp://127.0.0.1:6379?database=0",
            "host": "ignored",
            "database": 5,
        }));
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.database, 0);

        let config = parse(json!({"url": "tls://user:p%40ss@cache.example.com:6380?database=1"}));
        assert_eq!(config.scheme, "tls");
        assert_eq!(config.host, "cache.example.com");
        assert_eq!(config.port, 6380);
        assert_eq!(config.username.as_deref(), Some("user"));
        assert_eq!(config.password.as_deref(), Some("p@ss"));
        assert_eq!(config.database, 1);

        let config = parse(json!({"url": "redis://localhost/4", "port": 7000}));
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 7000);
        assert_eq!(config.database, 4);

        let config = parse(json!({"url": "rediss://[::1]:6390?prefix=app:"}));
        assert_eq!(config.scheme, "tls");
        assert_eq!(config.host, "::1");
        assert_eq!(config.prefix, "app:");

        let config = parse(json!({"url": "unix:///run/redis/redis.sock?database=2"}));
        assert_eq!(config.scheme, "unix");
        assert_eq!(config.host, "/run/redis/redis.sock");
        assert_eq!(config.database, 2);

        assert!(ConnectionConfig::parse(&json!({"url": "::nope::"}), &json!({})).is_err());
    }

    #[test]
    fn prefixes_cascade_from_options_to_the_connection() {
        let options = json!({"prefix": "global:"});
        let config = ConnectionConfig::parse(&json!({}), &options).unwrap();
        assert_eq!(config.prefix, "global:");

        let config =
            ConnectionConfig::parse(&json!({"options": {"prefix": "own:"}}), &options).unwrap();
        assert_eq!(config.prefix, "own:");

        let config = ConnectionConfig::parse(
            &json!({"prefix": "direct:", "options": {"prefix": "own:"}}),
            &options,
        )
        .unwrap();
        assert_eq!(config.prefix, "direct:");

        let config = ConnectionConfig::parse(&json!({"prefix": ""}), &options).unwrap();
        assert_eq!(config.prefix, "");
    }

    #[test]
    fn unix_sockets_and_invalid_settings() {
        let config = parse(json!({"host": "/run/redis/redis.sock", "port": 0}));
        assert_eq!(config.scheme, "unix");
        assert_eq!(config.port, 0);
        assert!(config.connection_info().is_ok());

        let error = ConnectionConfig::parse(&json!({"port": 70000}), &json!({})).unwrap_err();
        assert_eq!(error.to_string(), "Invalid Redis port [70000].");
        let error = ConnectionConfig::parse(&json!({"database": "first"}), &json!({})).unwrap_err();
        assert_eq!(error.to_string(), "Invalid Redis database [first].");
        assert!(ConnectionConfig::parse(&json!("redis"), &json!({})).is_err());
    }

    #[test]
    fn it_builds_connection_info() {
        let info = parse(json!({"host": "127.0.0.1", "port": 6390, "database": 4, "username": "u", "password": "p"}))
            .connection_info()
            .unwrap();
        assert_eq!(info.addr().to_string(), "127.0.0.1:6390");
        assert_eq!(info.redis_settings().db(), 4);
        assert_eq!(info.redis_settings().username(), Some("u"));
        assert_eq!(info.redis_settings().password(), Some("p"));

        let error = parse(json!({"scheme": "tls"}))
            .connection_info()
            .unwrap_err();
        assert!(error.to_string().contains("not supported"));
    }
}
