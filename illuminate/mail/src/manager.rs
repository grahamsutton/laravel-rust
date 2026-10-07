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
    ArrayTransport, DEFAULT_SENDMAIL_COMMAND, FailoverTransport, LogTransport, RoundRobinTransport,
    SendmailTransport, SmtpTransport, Transport,
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

/// Merge a mailer's `url` (`smtp://user:pass@host:port`) into its configuration.
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
    let mut parts = Map::new();
    let scheme = parsed.scheme().to_string();
    let (transport, scheme) = match scheme.as_str() {
        "smtps" => ("smtp".to_string(), Some("smtps".to_string())),
        "smtp" => ("smtp".to_string(), Some("smtp".to_string())),
        other => (other.to_string(), None),
    };
    parts.insert("transport".into(), Value::String(transport));
    if let Some(scheme) = scheme {
        parts.insert("scheme".into(), Value::String(scheme));
    }
    if let Some(host) = parsed.host_str() {
        parts.insert("host".into(), Value::String(decode(host)));
    }
    if let Some(port) = parsed.port() {
        parts.insert("port".into(), Value::from(port));
    }
    if !parsed.username().is_empty() {
        parts.insert("username".into(), Value::String(decode(parsed.username())));
    }
    if let Some(password) = parsed.password() {
        parts.insert("password".into(), Value::String(decode(password)));
    }
    for (key, value) in parsed.query_pairs() {
        parts.insert(key.into_owned(), Value::String(value.into_owned()));
    }
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
                "ses": {"transport": "ses"},
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
                .mailer(Some("ses"))
                .unwrap_err()
                .to_string()
                .contains("Unsupported mail transport [ses].")
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
