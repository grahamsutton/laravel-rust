//! The mail manager: builds the mailers configured in `mail.mailers`.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use illuminate_config::Repository;
use illuminate_container::{Container, try_app};
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Map, Result, Value, ValueExt, json};

use crate::address::Address;
use crate::fake::MailFake;
use crate::mailable::{Mailable, MailableBuilder, SendOptions};
use crate::mailer::{Mailer, Shared};
use crate::message::SentMessage;
use crate::queue::{QueuedMessage, queue_hook};
use crate::transport::{
    ArrayTransport, DEFAULT_SENDMAIL_COMMAND, FailoverTransport, LogTransport, MailgunTransport,
    PostmarkTransport, ResendTransport, RoundRobinTransport, SendmailTransport, SesTransport,
    SmtpTransport, Transport,
};

/// Builds a custom transport from a mailer's configuration.
pub type TransportCreator = Arc<dyn Fn(&Value) -> Result<Arc<dyn Transport>> + Send + Sync>;

/// The transports that work without any configuration.
const BUILT_IN: &[&str] = &["smtp", "sendmail", "log", "array"];

/// The mail manager (Laravel's `MailManager`, the `Mail` facade's root).
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_mail::MailManager;
/// use illuminate_support::json;
///
/// let manager = MailManager::new(Arc::new(Repository::new(json!({
///     "mail": {
///         "default": "array",
///         "mailers": {"array": {"transport": "array"}},
///         "from": {"address": "hello@example.com", "name": "Example"},
///     },
/// }))));
///
/// let mailer = manager.mailer(None).unwrap();
/// assert_eq!(mailer.name(), "array");
/// assert_eq!(mailer.create_message().from[0].address, "hello@example.com");
/// assert!(Arc::ptr_eq(&mailer, &manager.mailer(Some("array")).unwrap()));
/// assert!(manager.mailer(Some("missing")).is_err());
/// ```
pub struct MailManager {
    config: Arc<Repository>,
    mailers: RwLock<HashMap<String, Arc<Mailer>>>,
    creators: RwLock<HashMap<String, TransportCreator>>,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for MailManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailManager")
            .field("default", &self.get_default_driver())
            .field(
                "mailers",
                &self.mailers.read().unwrap().keys().collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl MailManager {
    /// Create a manager reading the `mail` configuration from the repository.
    pub fn new(config: Arc<Repository>) -> Self {
        Self {
            config,
            mailers: RwLock::new(HashMap::new()),
            creators: RwLock::new(HashMap::new()),
            shared: Arc::new(Shared::default()),
        }
    }

    /// Resolve the manager from the container, registering one (configured
    /// from the container's configuration repository) if needed.
    pub fn resolve() -> Arc<MailManager> {
        if let Some(manager) = try_app::<MailManager>() {
            return manager;
        }
        let container = Container::get_instance();
        container.singleton_if::<MailManager>(|app| {
            let config = app
                .try_make::<Repository>()
                .unwrap_or_else(|_| Arc::new(Repository::empty()));
            Arc::new(MailManager::new(config))
        });
        container.make::<MailManager>()
    }

    /// The configuration repository.
    pub fn config(&self) -> &Arc<Repository> {
        &self.config
    }

    /// Get a mailer instance by name (the default mailer when `None`).
    pub fn mailer(&self, name: Option<&str>) -> Result<Arc<Mailer>> {
        let name = match name {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => self.get_default_driver(),
        };
        if let Some(mailer) = self.mailers.read().unwrap().get(&name) {
            return Ok(mailer.clone());
        }
        let mailer = self.resolve_mailer(&name)?;
        Ok(self
            .mailers
            .write()
            .unwrap()
            .entry(name)
            .or_insert(mailer)
            .clone())
    }

    /// The mailer a mailable asks for (with [`Mailable::mailer`] or in its
    /// `build` method), or the default mailer.
    pub fn mailer_for(&self, mailable: &dyn Mailable) -> Result<Arc<Mailer>> {
        let name = mailable
            .mailer()
            .or_else(|| MailableBuilder::for_mailable(mailable, &SendOptions::default()).mailer);
        self.mailer(name.as_deref())
    }

    /// Get a mailer driver instance (an alias of [`MailManager::mailer`]).
    pub fn driver(&self, name: Option<&str>) -> Result<Arc<Mailer>> {
        self.mailer(name)
    }

    fn resolve_mailer(&self, name: &str) -> Result<Arc<Mailer>> {
        let config = match self.get_config(name) {
            Some(config) => config,
            // While mail is faked nothing is delivered, so any mailer name works.
            None if self.is_fake() => json!({"transport": "array"}),
            None => {
                return Err(InvalidArgumentException::new(format!(
                    "Mailer [{name}] is not defined."
                ))
                .into());
            }
        };
        let mailer =
            Mailer::with_shared(name, self.create_transport(&config)?, self.shared.clone());
        for kind in ["from", "reply_to", "to", "return_path"] {
            let address = match config.get(kind) {
                Some(address) if address.is_object() => address.clone(),
                _ => self.config.get(&format!("mail.{kind}")),
            };
            let Some(email) = address.get("address").filter(|a| !a.is_blank()) else {
                continue;
            };
            let address = Address {
                address: email.to_string_lossy(),
                name: address
                    .get("name")
                    .filter(|n| !n.is_blank())
                    .map(ValueExt::to_string_lossy),
            };
            match kind {
                "from" => mailer.always_from(address),
                "reply_to" => mailer.always_reply_to(address),
                "to" => mailer.always_to(address),
                _ => mailer.always_return_path(address),
            }
        }
        Ok(Arc::new(mailer))
    }

    /// Build a new mailer instance from the given configuration (Laravel's
    /// `Mail::build([...])`, for on-demand mailers).
    pub fn build(&self, config: Value) -> Result<Arc<Mailer>> {
        let name = config
            .get("name")
            .map(ValueExt::to_string_lossy)
            .unwrap_or_else(|| "ondemand".to_string());
        let config = normalize_url(config);
        Ok(Arc::new(Mailer::with_shared(
            name,
            self.create_transport(&config)?,
            self.shared.clone(),
        )))
    }

    /// Create a transport from a mailer configuration.
    pub fn create_transport(&self, config: &Value) -> Result<Arc<dyn Transport>> {
        let transport = config
            .get("transport")
            .map(ValueExt::to_string_lossy)
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| self.config.string("mail.driver"));

        if let Some(creator) = self.creators.read().unwrap().get(&transport).cloned() {
            return creator(config);
        }

        Ok(match transport.as_str() {
            "smtp" => Arc::new(SmtpTransport::from_config(config)?),
            "sendmail" => {
                let path = config
                    .get("path")
                    .filter(|p| !p.is_blank())
                    .map(ValueExt::to_string_lossy)
                    .or_else(|| Some(self.config.string("mail.sendmail")).filter(|p| !p.is_empty()))
                    .unwrap_or_else(|| DEFAULT_SENDMAIL_COMMAND.to_string());
                Arc::new(SendmailTransport::new(path)?)
            }
            "log" => {
                let channel = config
                    .get("channel")
                    .filter(|c| !c.is_blank())
                    .map(ValueExt::to_string_lossy)
                    .or_else(|| {
                        Some(self.config.string("mail.log_channel")).filter(|c| !c.is_empty())
                    });
                Arc::new(LogTransport::new(channel))
            }
            "array" => Arc::new(ArrayTransport::new()),
            "postmark" => Arc::new(PostmarkTransport::from_config(
                config,
                &self.config.get("services.postmark"),
            )?),
            "resend" => Arc::new(ResendTransport::from_config(
                config,
                &self.config.get("services.resend"),
            )?),
            "mailgun" => Arc::new(MailgunTransport::from_config(
                config,
                &self.config.get("services.mailgun"),
            )?),
            "ses" | "ses-v2" => Arc::new(SesTransport::from_config(
                config,
                &self.config.get("services.ses"),
            )?),
            "failover" | "roundrobin" => {
                let mut transports = Vec::new();
                for name in config
                    .get("mailers")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let name = name.to_string_lossy();
                    let mailer_config = self.get_config(&name).ok_or_else(|| {
                        InvalidArgumentException::new(format!("Mailer [{name}] is not defined."))
                    })?;
                    transports.push(self.create_transport(&mailer_config)?);
                }
                let retry_after = Duration::from_secs(
                    config
                        .get("retry_after")
                        .and_then(ValueExt::to_i64_lossy)
                        .unwrap_or(60)
                        .max(0) as u64,
                );
                if transport == "failover" {
                    Arc::new(FailoverTransport::new(transports, retry_after)?)
                } else {
                    Arc::new(RoundRobinTransport::new(transports, retry_after)?)
                }
            }
            other => {
                return Err(InvalidArgumentException::new(format!(
                    "Unsupported mail transport [{other}]."
                ))
                .into());
            }
        })
    }

    /// Get the configuration of a mailer.
    pub fn get_config(&self, name: &str) -> Option<Value> {
        let config = self.config.get(&format!("mail.mailers.{name}"));
        let config = match config {
            Value::Object(_) => config,
            _ if BUILT_IN.contains(&name) => json!({"transport": name}),
            _ => return None,
        };
        Some(normalize_url(config))
    }

    /// Get the default mail driver name (`mail.default`, `log` when unset).
    pub fn get_default_driver(&self) -> String {
        let driver = self.config.string("mail.driver");
        if !driver.is_empty() {
            return driver;
        }
        self.config.string_or("mail.default", "log")
    }

    /// Set the default mail driver name.
    pub fn set_default_driver(&self, name: &str) {
        self.config.set("mail.default", name);
    }

    /// Disconnect the given mailer and remove it from the local cache.
    pub fn purge(&self, name: Option<&str>) {
        let name = name.map_or_else(|| self.get_default_driver(), str::to_string);
        self.mailers.write().unwrap().remove(&name);
    }

    /// Forget all of the resolved mailers.
    pub fn forget_mailers(&self) {
        self.mailers.write().unwrap().clear();
    }

    /// Register a custom transport creator.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_config::Repository;
    /// use illuminate_mail::{ArrayTransport, MailManager, Transport};
    /// use illuminate_support::json;
    ///
    /// let manager = MailManager::new(Arc::new(Repository::new(json!({
    ///     "mail": {"default": "custom", "mailers": {"custom": {"transport": "acme"}}},
    /// }))));
    ///
    /// manager.extend("acme", |_config| Ok(Arc::new(ArrayTransport::new()) as Arc<dyn Transport>));
    ///
    /// assert_eq!(manager.mailer(None).unwrap().transport().name(), "array");
    /// ```
    pub fn extend(
        &self,
        driver: &str,
        creator: impl Fn(&Value) -> Result<Arc<dyn Transport>> + Send + Sync + 'static,
    ) -> &Self {
        self.creators
            .write()
            .unwrap()
            .insert(driver.to_string(), Arc::new(creator));
        self
    }

    // ------------------------------------------------------------------
    // Queueing
    // ------------------------------------------------------------------

    /// Install the hook that pushes queued mail onto the queue.
    pub fn queue_using<F, Fut>(&self, hook: F)
    where
        F: Fn(QueuedMessage) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        *self.shared.queue.write().unwrap() = Some(queue_hook(hook));
    }

    /// Remove the queue hook (queued mail will be sent immediately).
    pub fn forget_queue_hook(&self) {
        *self.shared.queue.write().unwrap() = None;
    }

    /// Determine if a queue hook is installed.
    pub fn has_queue_hook(&self) -> bool {
        self.shared.queue.read().unwrap().is_some()
    }

    /// Deliver a message taken off the queue, using the mailer it names.
    pub async fn send_queued(&self, queued: QueuedMessage) -> Result<Option<SentMessage>> {
        let mailer = self.mailer(Some(&queued.mailer))?;
        mailer.send_queued(queued).await
    }

    // ------------------------------------------------------------------
    // Faking
    // ------------------------------------------------------------------

    /// Replace sending with a [`MailFake`] that records mailables instead.
    pub fn fake(&self) -> Arc<MailFake> {
        let fake = Arc::new(MailFake::new());
        *self.shared.fake.write().unwrap() = Some(fake.clone());
        fake
    }

    /// Stop faking mail.
    pub fn unfake(&self) {
        *self.shared.fake.write().unwrap() = None;
        // Forget the stand-in mailers created while faking.
        let names: Vec<String> = self.mailers.read().unwrap().keys().cloned().collect();
        for name in names {
            if self.get_config(&name).is_none() {
                self.mailers.write().unwrap().remove(&name);
            }
        }
    }

    /// Determine if mail is being faked.
    pub fn is_fake(&self) -> bool {
        self.shared.fake.read().unwrap().is_some()
    }

    /// The fake, when mail is being faked.
    pub fn get_fake(&self) -> Option<Arc<MailFake>> {
        self.shared.fake()
    }
}

/// Merge a mailer's `url` into its configuration.
///
/// SMTP URLs look like `smtp://user:pass@host:port`; the API transports use
/// Symfony's DSNs, where the host is `default` unless you need another
/// endpoint:
///
/// - `postmark+api://TOKEN@default?message_stream=broadcasts`
/// - `resend://KEY@default`
/// - `mailgun+api://KEY:DOMAIN@default?region=eu`
/// - `ses+api://ACCESS_KEY:SECRET_KEY@default?region=eu-west-1&session_token=TOKEN`
///
/// Any other query parameters are merged into the configuration as-is.
fn normalize_url(mut config: Value) -> Value {
    let Some(url) = config
        .get("url")
        .filter(|u| !u.is_blank())
        .map(ValueExt::to_string_lossy)
    else {
        return config;
    };
    let Ok(parsed) = url::Url::parse(&url) else {
        return config;
    };
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8_lossy()
            .into_owned()
    };
    let user = Some(decode(parsed.username())).filter(|user| !user.is_empty());
    let password = parsed.password().map(decode);
    let mut query: Map<String, Value> = parsed
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), Value::String(value.into_owned())))
        .collect();
    let mut take = |key: &str| query.remove(key).map(|value| value.to_string_lossy());
    // The API DSNs use the `default` host for the provider's own endpoint.
    let endpoint = parsed
        .host_str()
        .map(decode)
        .filter(|host| host != "default")
        .map(|host| match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host,
        });

    let mut parts = Map::new();
    let mut set = |key: &str, value: Option<String>| {
        if let Some(value) = value {
            parts.insert(key.to_string(), Value::String(value));
        }
    };
    let transport = match parsed.scheme() {
        "postmark" | "postmark+api" | "postmark+https" => {
            set("token", user);
            set("message_stream_id", take("message_stream"));
            "postmark"
        }
        "resend" | "resend+api" => {
            set("key", user);
            "resend"
        }
        "mailgun" | "mailgun+api" | "mailgun+https" => {
            let region = take("region").filter(|region| !region.is_empty() && region != "us");
            set("secret", user);
            set("domain", password);
            set(
                "endpoint",
                endpoint.or_else(|| region.map(|region| format!("api.{region}.mailgun.net"))),
            );
            "mailgun"
        }
        "ses" | "ses+api" | "ses+https" => {
            set("key", user);
            set("secret", password);
            set("region", take("region"));
            set("token", take("session_token"));
            set("endpoint", endpoint);
            "ses"
        }
        scheme => {
            set("host", parsed.host_str().map(decode));
            set("username", user);
            set("password", password);
            match scheme {
                "smtp" | "smtps" => {
                    set("scheme", Some(scheme.to_string()));
                    "smtp"
                }
                other => other,
            }
        }
    };
    // The API DSNs fold the port into their endpoint; everything else keeps it.
    let api = matches!(transport, "postmark" | "resend" | "mailgun" | "ses");
    if let Some(port) = parsed.port().filter(|_| !api) {
        parts.insert("port".into(), Value::from(port));
    }
    parts.insert("transport".into(), Value::String(transport.to_string()));
    parts.extend(query);
    if let Value::Object(map) = &mut config {
        map.extend(parts);
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager(mail: Value) -> MailManager {
        MailManager::new(Arc::new(Repository::new(json!({"mail": mail}))))
    }

    #[test]
    fn mailers_are_resolved_from_configuration() {
        let manager = manager(json!({
            "default": "array",
            "mailers": {
                "array": {"transport": "array", "reply_to": {"address": "reply@example.com", "name": null}},
                "log": {"transport": "log", "channel": "mail"},
                "smtp": {"transport": "smtp", "host": "mail.example.com", "port": 2525},
                "sendmail": {"transport": "sendmail", "path": "/usr/sbin/sendmail -t -i"},
                "failover": {"transport": "failover", "mailers": ["smtp", "log"], "retry_after": 30},
                "roundrobin": {"transport": "roundrobin", "mailers": ["array", "log"]},
                "broken": {"transport": "failover", "mailers": ["missing"]},
                "pigeon": {"transport": "carrier-pigeon"},
            },
            "from": {"address": "hello@example.com", "name": "Example"},
            "to": {"address": "dev@example.com"},
        }));

        let array = manager.mailer(None).unwrap();
        let message = array.create_message();
        assert_eq!(
            message.from[0],
            Address::new("hello@example.com", "Example")
        );
        assert_eq!(message.reply_to[0], Address::email("reply@example.com"));

        assert_eq!(
            manager.mailer(Some("log")).unwrap().transport().name(),
            "log"
        );
        assert_eq!(
            manager.mailer(Some("smtp")).unwrap().transport().name(),
            "smtp://mail.example.com:2525"
        );
        assert_eq!(
            manager.mailer(Some("sendmail")).unwrap().transport().name(),
            "sendmail://default"
        );
        assert_eq!(
            manager.mailer(Some("failover")).unwrap().transport().name(),
            "failover(smtp://mail.example.com:2525 log)"
        );
        assert!(
            manager
                .mailer(Some("roundrobin"))
                .unwrap()
                .transport()
                .name()
                .starts_with("roundrobin(")
        );
        assert!(
            manager
                .mailer(Some("broken"))
                .unwrap_err()
                .to_string()
                .contains("Mailer [missing] is not defined.")
        );
        assert!(
            manager
                .mailer(Some("pigeon"))
                .unwrap_err()
                .to_string()
                .contains("Unsupported mail transport [carrier-pigeon].")
        );
        assert!(
            manager
                .mailer(Some("nope"))
                .unwrap_err()
                .to_string()
                .contains("Mailer [nope] is not defined.")
        );

        // Mailers are cached until purged.
        let first = manager.mailer(Some("log")).unwrap();
        assert!(Arc::ptr_eq(&first, &manager.mailer(Some("log")).unwrap()));
        manager.purge(Some("log"));
        assert!(!Arc::ptr_eq(&first, &manager.mailer(Some("log")).unwrap()));
        manager.forget_mailers();

        manager.set_default_driver("log");
        assert_eq!(manager.get_default_driver(), "log");
        assert!(format!("{manager:?}").contains("log"));
    }

    #[test]
    fn built_in_transports_work_without_configuration() {
        let manager = manager(json!({}));
        assert_eq!(manager.get_default_driver(), "log");
        assert_eq!(
            manager.mailer(Some("array")).unwrap().transport().name(),
            "array"
        );
        assert_eq!(manager.mailer(None).unwrap().transport().name(), "log");
    }

    #[test]
    fn urls_configure_smtp_mailers() {
        let manager = manager(json!({
            "mailers": {"smtp": {"transport": "smtp", "url": "smtps://user%40example.com:secret@smtp.example.com:465?local_domain=example.com"}},
        }));
        let config = manager.get_config("smtp").unwrap();
        assert_eq!(config["host"], "smtp.example.com");
        assert_eq!(config["port"], 465);
        assert_eq!(config["username"], "user@example.com");
        assert_eq!(config["password"], "secret");
        assert_eq!(config["scheme"], "smtps");
        assert_eq!(config["local_domain"], "example.com");
        assert_eq!(
            manager.mailer(Some("smtp")).unwrap().transport().name(),
            "smtps://smtp.example.com:465"
        );
    }

    fn config_of(manager: &MailManager, name: &str) -> Value {
        let mut config = manager.get_config(name).unwrap();
        config.as_object_mut().unwrap().remove("url");
        config
    }

    #[test]
    fn api_transports_are_resolved_from_the_services_configuration() {
        let manager = MailManager::new(Arc::new(Repository::new(json!({
            "mail": {
                "mailers": {
                    "postmark": {"transport": "postmark", "message_stream_id": "outbound"},
                    "resend": {"transport": "resend"},
                    "mailgun": {"transport": "mailgun"},
                    "ses": {"transport": "ses"},
                    "ses-v2": {"transport": "ses-v2", "region": "eu-central-1"},
                    "failover": {"transport": "failover", "mailers": ["postmark", "ses"]},
                },
            },
            "services": {
                "postmark": {"key": "postmark-key"},
                "resend": {"key": "re_123"},
                "mailgun": {"domain": "mg.example.com", "secret": "key-secret", "endpoint": "api.eu.mailgun.net"},
                "ses": {"key": "AKIDEXAMPLE", "secret": "secret", "region": "eu-west-1"},
            },
        }))));

        let name = |mailer: &str| manager.mailer(Some(mailer)).unwrap().transport().name();
        assert_eq!(
            name("postmark"),
            "postmark+api://api.postmarkapp.com?message_stream=outbound"
        );
        assert_eq!(name("resend"), "resend");
        assert_eq!(
            name("mailgun"),
            "mailgun+https://api.eu.mailgun.net?domain=mg.example.com"
        );
        assert_eq!(name("ses"), "ses");
        assert_eq!(name("ses-v2"), "ses-v2");
        assert_eq!(
            name("failover"),
            "failover(postmark+api://api.postmarkapp.com?message_stream=outbound ses)"
        );

        let ses = crate::downcast_transport::<SesTransport>(
            &manager.mailer(Some("ses-v2")).unwrap().transport(),
        )
        .unwrap();
        assert_eq!(ses.region(), "eu-central-1");
        assert_eq!(ses.credentials().unwrap().key, "AKIDEXAMPLE");

        // Without credentials, the API mailers can't be created.
        let manager = manager_without_services();
        for (mailer, error) in [
            ("postmark", "requires a server token"),
            ("resend", "requires an API key"),
            ("mailgun", "requires a [secret]"),
        ] {
            let message = manager.mailer(Some(mailer)).unwrap_err().to_string();
            assert!(message.contains(error), "{mailer}: {message}");
        }
    }

    fn manager_without_services() -> MailManager {
        manager(json!({
            "mailers": {
                "postmark": {"transport": "postmark"},
                "resend": {"transport": "resend"},
                "mailgun": {"transport": "mailgun"},
            },
        }))
    }

    #[test]
    fn api_transports_may_be_configured_with_symfony_dsns() {
        let manager = manager(json!({
            "mailers": {
                "postmark": {"url": "postmark+api://server%2Ftoken@default?message_stream=broadcasts"},
                "resend": {"url": "resend://re_123@default"},
                "resend-api": {"url": "resend+api://re_456@default"},
                "mailgun": {"url": "mailgun+api://key-secret:mg.example.com@default?region=eu"},
                "mailgun-us": {"url": "mailgun+https://key-secret:mg.example.com@default?region=us"},
                "mailgun-host": {"url": "mailgun://key-secret:mg.example.com@localhost:8025"},
                "ses": {"url": "ses+api://AKIDEXAMPLE:se%2Fcret@default?region=eu-west-1&session_token=token"},
                "ses-local": {"transport": "ses-v2", "url": "ses+https://key:secret@localhost:4566?region=us-west-2"},
            },
        }));

        assert_eq!(
            config_of(&manager, "postmark"),
            json!({"transport": "postmark", "token": "server/token", "message_stream_id": "broadcasts"})
        );
        assert_eq!(
            config_of(&manager, "resend"),
            json!({"transport": "resend", "key": "re_123"})
        );
        assert_eq!(
            config_of(&manager, "resend-api"),
            json!({"transport": "resend", "key": "re_456"})
        );
        assert_eq!(
            config_of(&manager, "mailgun"),
            json!({"transport": "mailgun", "secret": "key-secret", "domain": "mg.example.com", "endpoint": "api.eu.mailgun.net"})
        );
        assert_eq!(
            config_of(&manager, "mailgun-us"),
            json!({"transport": "mailgun", "secret": "key-secret", "domain": "mg.example.com"})
        );
        assert_eq!(
            config_of(&manager, "mailgun-host")["endpoint"],
            "localhost:8025"
        );
        assert_eq!(
            config_of(&manager, "ses"),
            json!({"transport": "ses", "key": "AKIDEXAMPLE", "secret": "se/cret", "region": "eu-west-1", "token": "token"})
        );
        assert_eq!(
            config_of(&manager, "ses-local"),
            json!({"transport": "ses", "key": "key", "secret": "secret", "region": "us-west-2", "endpoint": "localhost:4566"})
        );

        let transport = |mailer: &str| manager.mailer(Some(mailer)).unwrap().transport();
        assert_eq!(
            transport("postmark").name(),
            "postmark+api://api.postmarkapp.com?message_stream=broadcasts"
        );
        assert_eq!(transport("resend").name(), "resend");
        assert_eq!(
            transport("mailgun").name(),
            "mailgun+https://api.eu.mailgun.net?domain=mg.example.com"
        );
        let ses = crate::downcast_transport::<SesTransport>(&transport("ses")).unwrap();
        assert_eq!(ses.credentials().unwrap().token.as_deref(), Some("token"));
        assert_eq!(
            ses.url(),
            "https://email.eu-west-1.amazonaws.com/v2/email/outbound-emails"
        );
        let local = crate::downcast_transport::<SesTransport>(&transport("ses-local")).unwrap();
        assert_eq!(
            local.url(),
            "https://localhost:4566/v2/email/outbound-emails"
        );

        // On-demand mailers accept DSNs too.
        let mailer = manager
            .build(json!({"url": "resend://re_789@default"}))
            .unwrap();
        assert_eq!(mailer.transport().name(), "resend");
    }

    #[test]
    fn on_demand_mailers_can_be_built() {
        let manager = manager(json!({}));
        let mailer = manager.build(json!({"transport": "array"})).unwrap();
        assert_eq!(mailer.name(), "ondemand");
        let mailer = manager
            .build(json!({"name": "custom", "url": "smtp://localhost:1025"}))
            .unwrap();
        assert_eq!(mailer.name(), "custom");
        assert_eq!(mailer.transport().name(), "smtp://localhost:1025");
    }

    #[test]
    fn faked_managers_accept_any_mailer() {
        let manager = manager(json!({}));
        assert!(manager.mailer(Some("postmark")).is_err());
        let fake = manager.fake();
        assert!(manager.is_fake());
        assert!(Arc::ptr_eq(&fake, &manager.get_fake().unwrap()));
        assert_eq!(manager.mailer(Some("postmark")).unwrap().name(), "postmark");
        manager.unfake();
        assert!(!manager.is_fake());
        assert!(manager.mailer(Some("postmark")).is_err());
    }
}
