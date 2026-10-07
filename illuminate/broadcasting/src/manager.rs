//! The broadcast manager: resolves broadcasters from the configuration and
//! sends events their way.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use illuminate_config::Repository;
use illuminate_http::{Request, Response, current_request, with_request};
use illuminate_support::error::{InvalidArgumentException, RuntimeException};
use illuminate_support::{Result, Value, ValueExt, json};

use crate::anonymous::AnonymousEvent;
use crate::authorization::{ChannelCallback, ChannelRegistry, PendingChannel, access_denied};
use crate::broadcasters::{
    AblyBroadcaster, LogBroadcaster, NullBroadcaster, Pusher, PusherBroadcaster,
};
use crate::channel::{Channel, IntoChannels};
use crate::contracts::{Broadcaster, ShouldBroadcast};
use crate::event::BroadcastEvent;
use crate::exceptions::report;
use crate::pending::{BroadcastOptions, PendingBroadcast};
use crate::testing::BroadcastFake;

/// Creates a broadcaster for a custom driver from the connection's
/// configuration.
pub type BroadcasterCreator = Arc<dyn Fn(&Value) -> Result<Arc<dyn Broadcaster>> + Send + Sync>;

/// The broadcast manager (Laravel's `BroadcastManager`): it resolves the
/// connections configured in `config/broadcasting.php`, queues events for
/// broadcasting, and authorizes channel subscriptions.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_broadcasting::BroadcastManager;
/// use illuminate_config::Repository;
/// use illuminate_support::json;
///
/// let manager = BroadcastManager::new(Arc::new(Repository::new(json!({
///     "broadcasting": {
///         "default": "log",
///         "connections": {"log": {"driver": "log"}, "null": {"driver": "null"}},
///     },
/// }))));
///
/// assert_eq!(manager.get_default_driver(), "log");
/// assert!(manager.connection(None).is_ok());
/// assert!(manager.connection(Some("pusher")).is_err());
/// ```
pub struct BroadcastManager {
    config: Arc<Repository>,
    channels: Arc<ChannelRegistry>,
    drivers: RwLock<HashMap<String, Arc<dyn Broadcaster>>>,
    creators: RwLock<HashMap<String, BroadcasterCreator>>,
    fake: RwLock<Option<Arc<BroadcastFake>>>,
}

impl BroadcastManager {
    /// Create a new manager reading the given configuration.
    pub fn new(config: Arc<Repository>) -> Self {
        Self {
            config,
            channels: Arc::new(ChannelRegistry::new()),
            drivers: RwLock::new(HashMap::new()),
            creators: RwLock::new(HashMap::new()),
            fake: RwLock::new(None),
        }
    }

    // ------------------------------------------------------------------
    // Connections
    // ------------------------------------------------------------------

    /// Get a broadcaster by connection name (the default connection when
    /// `None`).
    pub fn connection(&self, name: Option<&str>) -> Result<Arc<dyn Broadcaster>> {
        let name = name
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.get_default_driver());

        if let Some(broadcaster) = self.drivers.read().unwrap().get(&name) {
            return Ok(broadcaster.clone());
        }

        let broadcaster = self.resolve(&name)?;
        Ok(self
            .drivers
            .write()
            .unwrap()
            .entry(name)
            .or_insert(broadcaster)
            .clone())
    }

    /// Get a broadcaster by driver (connection) name. Alias of
    /// [`connection`](BroadcastManager::connection).
    pub fn driver(&self, name: Option<&str>) -> Result<Arc<dyn Broadcaster>> {
        self.connection(name)
    }

    fn resolve(&self, name: &str) -> Result<Arc<dyn Broadcaster>> {
        let config = self.get_config(name);
        if !config.is_object() {
            return Err(InvalidArgumentException::new(format!(
                "Broadcast connection [{name}] is not defined."
            ))
            .into());
        }
        let driver = config
            .get("driver")
            .map(ValueExt::to_string_lossy)
            .unwrap_or_default();

        let creator = self.creators.read().unwrap().get(&driver).cloned();
        if let Some(creator) = creator {
            return creator(&config);
        }

        let created: Result<Arc<dyn Broadcaster>> = match driver.as_str() {
            "pusher" | "reverb" => self.create_pusher_driver(&config),
            "ably" => AblyBroadcaster::from_config(&config, self.channels.clone())
                .map(|broadcaster| Arc::new(broadcaster) as Arc<dyn Broadcaster>),
            "log" => Ok(Arc::new(
                match config.get("channel").filter(|channel| !channel.is_blank()) {
                    Some(channel) => LogBroadcaster::on_channel(channel.to_string_lossy()),
                    None => LogBroadcaster::new(),
                },
            )),
            "null" => Ok(Arc::new(NullBroadcaster)),
            _ => {
                return Err(InvalidArgumentException::new(format!(
                    "Driver [{driver}] is not supported."
                ))
                .into());
            }
        };

        created.map_err(|error| {
            RuntimeException::new(format!(
                "Failed to create broadcaster for connection \"{name}\" with error: {}.",
                error.to_string().trim_end_matches('.')
            ))
            .into()
        })
    }

    fn create_pusher_driver(&self, config: &Value) -> Result<Arc<dyn Broadcaster>> {
        let broadcaster = PusherBroadcaster::new(self.pusher(config)?, self.channels.clone())
            .allow_jsonp(config.get("jsonp").is_some_and(ValueExt::truthy));
        Ok(Arc::new(broadcaster))
    }

    /// Get a Pusher client for the given connection configuration.
    pub fn pusher(&self, config: &Value) -> Result<Pusher> {
        Pusher::from_config(config)
    }

    /// The configuration of a connection (the `null` connection needs none).
    fn get_config(&self, name: &str) -> Value {
        if name == "null" {
            let config = self.config.get(&format!("broadcasting.connections.{name}"));
            return if config.is_object() {
                config
            } else {
                json!({"driver": "null"})
            };
        }
        self.config.get(&format!("broadcasting.connections.{name}"))
    }

    /// The default connection's name (`broadcasting.default`, or `null`).
    pub fn get_default_driver(&self) -> String {
        let default = self.config.get("broadcasting.default");
        if default.is_blank() {
            "null".to_string()
        } else {
            default.to_string_lossy()
        }
    }

    /// Set the default connection.
    pub fn set_default_driver(&self, name: &str) {
        self.config.set("broadcasting.default", name);
    }

    /// Forget a resolved connection (the default one when `None`), so it's
    /// created again the next time it's used.
    pub fn purge(&self, name: Option<&str>) {
        let name = name
            .map(str::to_string)
            .unwrap_or_else(|| self.get_default_driver());
        self.drivers.write().unwrap().remove(&name);
    }

    /// Forget every resolved connection.
    pub fn forget_drivers(&self) -> &Self {
        self.drivers.write().unwrap().clear();
        self
    }

    /// Register a custom driver.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_broadcasting::{BroadcastManager, NullBroadcaster};
    /// use illuminate_config::Repository;
    /// use illuminate_support::json;
    ///
    /// let manager = BroadcastManager::new(Arc::new(Repository::new(json!({
    ///     "broadcasting": {"default": "carrier-pigeon", "connections": {"carrier-pigeon": {"driver": "pigeon"}}},
    /// }))));
    ///
    /// manager.extend("pigeon", |_config| Ok(Arc::new(NullBroadcaster)));
    ///
    /// assert!(manager.connection(None).is_ok());
    /// ```
    pub fn extend(
        &self,
        driver: &str,
        creator: impl Fn(&Value) -> Result<Arc<dyn Broadcaster>> + Send + Sync + 'static,
    ) -> &Self {
        self.creators
            .write()
            .unwrap()
            .insert(driver.to_string(), Arc::new(creator));
        self
    }

    // ------------------------------------------------------------------
    // Channels
    // ------------------------------------------------------------------

    /// The registered channels, shared by every connection.
    pub fn channels(&self) -> Arc<ChannelRegistry> {
        self.channels.clone()
    }

    /// Register a channel authorization callback.
    pub fn channel<Args>(
        &self,
        pattern: &str,
        callback: impl ChannelCallback<Args>,
    ) -> PendingChannel {
        self.channels.channel(pattern, callback);
        PendingChannel::new(self.channels.clone(), pattern)
    }

    /// Register the callback resolving the user payload for Pusher's user
    /// authentication.
    pub fn resolve_authenticated_user_using<F, Fut>(&self, callback: F)
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Option<Value>> + Send + 'static,
    {
        self.channels.resolve_authenticated_user_using(callback);
    }

    /// Authorize the request's channel subscription with the default
    /// connection, and turn the answer into a response.
    pub async fn auth(&self, request: &Request) -> Result<Response> {
        let broadcaster = self.connection(None)?;
        let result = {
            let broadcaster = broadcaster.clone();
            let request = request.clone();
            with_request(
                request.clone(),
                async move { broadcaster.auth(&request).await },
            )
            .await?
        };
        Ok(auth_response(broadcaster.as_ref(), request, result))
    }

    /// Authenticate the request's user for the connection (Pusher's user
    /// authentication), or deny it with a `403`.
    pub async fn authenticate_user(&self, request: &Request) -> Result<Response> {
        match self.resolve_authenticated_user(request).await? {
            Some(user) => Ok(Response::json(&user)),
            None => Err(access_denied()),
        }
    }

    /// Resolve the authenticated user payload for the request with the
    /// default connection.
    pub async fn resolve_authenticated_user(&self, request: &Request) -> Result<Option<Value>> {
        let broadcaster = self.connection(None)?;
        let request = request.clone();
        with_request(request.clone(), async move {
            broadcaster.resolve_authenticated_user(&request).await
        })
        .await
    }

    /// The socket ID of the given request (or the current one): its
    /// `X-Socket-ID` header.
    pub fn socket(&self, request: Option<&Request>) -> Option<String> {
        let request = match request {
            Some(request) => request.clone(),
            None => current_request()?,
        };
        request
            .header("X-Socket-ID")
            .filter(|socket| !socket.is_empty())
    }

    // ------------------------------------------------------------------
    // Broadcasting
    // ------------------------------------------------------------------

    /// Begin sending an anonymous broadcast to the given channels.
    pub fn on(&self, channels: impl IntoChannels) -> AnonymousEvent {
        AnonymousEvent::new(channels)
    }

    /// Begin sending an anonymous broadcast to the given private channel.
    pub fn private(&self, channel: &str) -> AnonymousEvent {
        AnonymousEvent::new(Channel::private(channel))
    }

    /// Begin sending an anonymous broadcast to the given presence channel.
    pub fn presence(&self, channel: &str) -> AnonymousEvent {
        AnonymousEvent::new(Channel::presence(channel))
    }

    /// Begin broadcasting an event.
    pub fn event<E: ShouldBroadcast>(&self, event: E) -> PendingBroadcast<E> {
        PendingBroadcast::new(event)
    }

    /// Queue the given event for broadcast, without dispatching it to its
    /// listeners.
    pub async fn queue<E: ShouldBroadcast>(&self, event: &E) -> Result<()> {
        self.queue_with(event, BroadcastOptions::default()).await
    }

    /// Queue the given event for broadcast with the given options.
    ///
    /// Events that [should broadcast now](ShouldBroadcast::should_broadcast_now)
    /// are broadcast immediately; the others are pushed onto the queue as a
    /// [`BroadcastEvent`] job. Events that
    /// [should be rescued](ShouldBroadcast::should_rescue) report failures
    /// instead of returning them.
    pub async fn queue_with<E: ShouldBroadcast>(
        &self,
        event: &E,
        options: BroadcastOptions,
    ) -> Result<()> {
        match self.push(event, options).await {
            Err(error) if event.should_rescue() => {
                report(&error);
                Ok(())
            }
            result => result,
        }
    }

    async fn push<E: ShouldBroadcast>(&self, event: &E, options: BroadcastOptions) -> Result<()> {
        let socket = options.socket.or_else(|| {
            event
                .dont_broadcast_to_current_user()
                .then(|| self.socket(None))
                .flatten()
        });
        let job = BroadcastEvent::capture(event, socket, options.connections)?;

        let fake = self.fake.read().unwrap().clone();
        if let Some(fake) = fake {
            fake.record(job);
            return Ok(());
        }

        if event.should_broadcast_now() {
            return self.broadcast_event(&job).await;
        }

        let mut pending = illuminate_queue::dispatch(job);
        if let Some(connection) = event.queue_connection() {
            pending = pending.on_connection(connection);
        }
        if let Some(queue) = event.broadcast_queue() {
            pending = pending.on_queue(queue);
        }
        pending = match event.after_commit() {
            Some(true) => pending.after_commit(),
            Some(false) => pending.before_commit(),
            None => pending,
        };
        pending.await
    }

    /// Broadcast a captured event on each of its connections: what the
    /// queued [`BroadcastEvent`] job does.
    pub async fn broadcast_event(&self, job: &BroadcastEvent) -> Result<()> {
        if job.channels.is_empty() {
            return Ok(());
        }
        for connection in job.connection_names() {
            self.connection(connection.as_deref())?
                .broadcast(&job.channels, &job.name, job.payload.clone())
                .await?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    /// Record broadcasts instead of queueing and sending them.
    pub fn fake(&self) -> Arc<BroadcastFake> {
        let fake = Arc::new(BroadcastFake::new());
        *self.fake.write().unwrap() = Some(fake.clone());
        fake
    }

    /// Stop faking broadcasts.
    pub fn forget_fake(&self) {
        *self.fake.write().unwrap() = None;
    }

    /// The current fake, if broadcasts are being faked.
    pub fn fake_instance(&self) -> Option<Arc<BroadcastFake>> {
        self.fake.read().unwrap().clone()
    }
}

impl fmt::Debug for BroadcastManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut drivers: Vec<String> = self.drivers.read().unwrap().keys().cloned().collect();
        drivers.sort();
        f.debug_struct("BroadcastManager")
            .field("default", &self.get_default_driver())
            .field("resolved", &drivers)
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

/// Turn an authorization answer into a response: JSON (JSONP when the
/// broadcaster allows it and the request asks for it), or an empty response
/// for drivers that answer nothing.
fn auth_response(broadcaster: &dyn Broadcaster, request: &Request, result: Value) -> Response {
    if result.is_null() {
        return Response::new("");
    }
    let callback = request.string("callback");
    if broadcaster.allows_jsonp() && !callback.is_empty() {
        return Response::jsonp(&callback, &result);
    }
    Response::json(&result)
}
