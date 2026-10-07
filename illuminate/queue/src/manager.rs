//! The queue manager: resolves the configured queue connections.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use illuminate_cache::Cache;
use illuminate_config::Repository as Config;
use illuminate_container::{Container, try_app};
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Carbon, Map, Result, Value, ValueExt, json};

use crate::contracts::{Queue, QueueConnector};
use crate::drivers::{
    ArrayQueue, BackgroundQueue, DatabaseQueue, DeferredQueue, FailoverQueue, NullQueue,
    RedisQueue, SyncQueue,
};
use crate::events::{
    JobExceptionOccurred, JobFailed, JobProcessed, JobProcessing, JobRetryRequested, Looping,
    QueueEvent, QueueEventType, QueueEvents, QueuePaused, QueueResumed, QueuesPaused,
    QueuesResumed, WorkerStarting, WorkerStopping,
};
use crate::failed::FailedJob;
use crate::payload::{PayloadHook, prepare_for_retry};
use crate::routes::QueueRoutes;
use crate::testing::QueueFake;

/// Resolves and caches the connections configured under
/// `queue.connections`.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_queue::QueueManager;
/// use illuminate_support::json;
///
/// let manager = QueueManager::new(Arc::new(Repository::new(json!({
///     "queue": {
///         "default": "array",
///         "connections": {
///             "array": {"driver": "array", "queue": "default"},
///             "sync": {"driver": "sync"},
///         },
///     },
/// }))));
///
/// assert_eq!(manager.get_default_driver(), "array");
/// assert_eq!(manager.connection(None).unwrap().connection_name(), "array");
/// assert!(manager.connected(Some("array")));
/// ```
pub struct QueueManager {
    config: Arc<Config>,
    connections: RwLock<HashMap<String, Arc<dyn Queue>>>,
    connectors: RwLock<HashMap<String, Arc<dyn QueueConnector>>>,
    routes: QueueRoutes,
    events: QueueEvents,
    payload_hooks: RwLock<Vec<PayloadHook>>,
    fake: RwLock<Option<Arc<QueueFake>>>,
    restartable: AtomicBool,
    pausable: AtomicBool,
}

impl QueueManager {
    /// Create a queue manager reading the given configuration, with the
    /// built-in connectors registered.
    pub fn new(config: Arc<Config>) -> Self {
        let manager = Self {
            config,
            connections: RwLock::new(HashMap::new()),
            connectors: RwLock::new(HashMap::new()),
            routes: QueueRoutes::new(),
            events: QueueEvents::new(),
            payload_hooks: RwLock::new(Vec::new()),
            fake: RwLock::new(None),
            restartable: AtomicBool::new(true),
            pausable: AtomicBool::new(true),
        };
        manager.register_connectors();
        manager
    }

    fn register_connectors(&self) {
        self.add_connector("null", |_: &Value, name: &str| {
            Ok(Arc::new(NullQueue::new(name)) as Arc<dyn Queue>)
        });
        self.add_connector("sync", |config: &Value, name: &str| {
            Ok(
                Arc::new(SyncQueue::new(name).with_after_commit(after_commit(config)))
                    as Arc<dyn Queue>,
            )
        });
        self.add_connector("array", |config: &Value, name: &str| {
            let queue = config
                .get("queue")
                .and_then(Value::as_str)
                .unwrap_or("default");
            Ok(Arc::new(
                ArrayQueue::new(name)
                    .with_default_queue(queue)
                    .with_after_commit(after_commit(config)),
            ) as Arc<dyn Queue>)
        });
        self.add_connector("database", |config: &Value, name: &str| {
            Ok(Arc::new(DatabaseQueue::from_config(config, name)) as Arc<dyn Queue>)
        });
        self.add_connector("redis", |config: &Value, name: &str| {
            Ok(Arc::new(RedisQueue::from_config(config, name)?) as Arc<dyn Queue>)
        });
        self.add_connector("deferred", |_: &Value, name: &str| {
            Ok(Arc::new(DeferredQueue::new(name)) as Arc<dyn Queue>)
        });
        self.add_connector("background", |_: &Value, name: &str| {
            Ok(Arc::new(BackgroundQueue::new(name)) as Arc<dyn Queue>)
        });
        self.add_connector("failover", |config: &Value, name: &str| {
            let connections = config
                .get("connections")
                .and_then(Value::as_array)
                .map(|connections| connections.iter().map(ValueExt::to_string_lossy).collect())
                .unwrap_or_default();
            Ok(Arc::new(FailoverQueue::new(name, connections)) as Arc<dyn Queue>)
        });
    }

    // ------------------------------------------------------------------
    // Connections
    // ------------------------------------------------------------------

    /// Resolve a queue connection instance (the default one when `None`).
    pub fn connection(&self, name: Option<&str>) -> Result<Arc<dyn Queue>> {
        if let Some(fake) = self.fake.read().unwrap().clone() {
            return Ok(fake);
        }
        self.real_connection(name)
    }

    /// Resolve a connection, bypassing any fake.
    pub(crate) fn real_connection(&self, name: Option<&str>) -> Result<Arc<dyn Queue>> {
        let name = self.get_name(name);
        if let Some(connection) = self.connections.read().unwrap().get(&name) {
            return Ok(connection.clone());
        }
        let connection = self.resolve(&name)?;
        Ok(self
            .connections
            .write()
            .unwrap()
            .entry(name)
            .or_insert(connection)
            .clone())
    }

    /// Determine if the connection has been resolved.
    pub fn connected(&self, name: Option<&str>) -> bool {
        self.connections
            .read()
            .unwrap()
            .contains_key(&self.get_name(name))
    }

    /// Forget a resolved connection, so the next use rebuilds it.
    pub fn forget_connection(&self, name: Option<&str>) {
        self.connections
            .write()
            .unwrap()
            .remove(&self.get_name(name));
    }

    fn resolve(&self, name: &str) -> Result<Arc<dyn Queue>> {
        let config = self.get_config(name);
        if !config.is_object() {
            return Err(InvalidArgumentException::new(format!(
                "The [{name}] queue connection has not been configured."
            ))
            .into());
        }

        let driver = config
            .get("driver")
            .map(ValueExt::to_string_lossy)
            .unwrap_or_default();
        let connector = self.connectors.read().unwrap().get(&driver).cloned();
        match connector {
            Some(connector) => connector.connect(&config, name),
            None => {
                Err(InvalidArgumentException::new(format!("No connector for [{driver}].")).into())
            }
        }
    }

    /// The configuration of the named connection.
    ///
    /// The `null` connection needs no configuration, and neither does
    /// `sync` (so `dispatch_sync` always works).
    pub fn get_config(&self, name: &str) -> Value {
        if name.is_empty() || name == "null" {
            return json!({"driver": "null"});
        }
        let config = self.config.get(&format!("queue.connections.{name}"));
        if config.is_null() && name == "sync" {
            return json!({"driver": "sync"});
        }
        config
    }

    /// Register a custom connector for the given driver name.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_config::Repository;
    /// use illuminate_queue::{ArrayQueue, QueueManager};
    /// use illuminate_queue::contracts::Queue;
    /// use illuminate_support::{Value, json};
    ///
    /// let manager = QueueManager::new(Arc::new(Repository::new(json!({
    ///     "queue": {"default": "jobs", "connections": {"jobs": {"driver": "memory"}}},
    /// }))));
    ///
    /// manager.extend("memory", |_config: &Value, name: &str| {
    ///     Ok(Arc::new(ArrayQueue::new(name)) as Arc<dyn Queue>)
    /// });
    ///
    /// assert_eq!(manager.connection(None).unwrap().connection_name(), "jobs");
    /// ```
    pub fn extend(&self, driver: &str, connector: impl QueueConnector) {
        self.add_connector(driver, connector);
    }

    /// Register a connector for the given driver name.
    pub fn add_connector(&self, driver: &str, connector: impl QueueConnector) {
        self.connectors
            .write()
            .unwrap()
            .insert(driver.to_string(), Arc::new(connector));
    }

    /// The name of the default connection (`queue.default`, or `sync`).
    pub fn get_default_driver(&self) -> String {
        match self.config.get("queue.default") {
            Value::String(name) if !name.is_empty() => name,
            _ => "sync".to_string(),
        }
    }

    /// Set the name of the default connection.
    pub fn set_default_driver(&self, name: &str) {
        self.config.set("queue.default", name);
    }

    /// The full name of the given connection.
    pub fn get_name(&self, name: Option<&str>) -> String {
        match name {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => self.get_default_driver(),
        }
    }

    /// The configuration repository.
    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }

    // ------------------------------------------------------------------
    // Routing
    // ------------------------------------------------------------------

    /// The queue routes.
    pub fn routes(&self) -> &QueueRoutes {
        &self.routes
    }

    /// Route the job registered under `job_name` to a queue and/or
    /// connection.
    pub fn route(&self, job_name: &str, queue: Option<&str>, connection: Option<&str>) {
        self.routes.set(job_name, queue, connection);
    }

    /// Forward jobs pushed onto `queue` to another queue and/or connection.
    pub fn forward(&self, queue: &str, to: Option<&str>, connection: Option<&str>) {
        self.routes.forward(queue, to, connection);
    }

    // ------------------------------------------------------------------
    // Events
    // ------------------------------------------------------------------

    /// The queue's event listeners.
    pub fn events(&self) -> &QueueEvents {
        &self.events
    }

    /// Listen for queue events of type `E`.
    pub fn listen<E: QueueEventType>(&self, callback: impl Fn(&E) + Send + Sync + 'static) {
        self.events.listen(callback);
    }

    /// Listen for every queue event.
    pub fn listen_all(&self, callback: impl Fn(&QueueEvent) + Send + Sync + 'static) {
        self.events.listen_all(callback);
    }

    /// Run the callback before each job is processed.
    pub fn before(&self, callback: impl Fn(&JobProcessing) + Send + Sync + 'static) {
        self.events.listen(callback);
    }

    /// Run the callback after each job is processed.
    pub fn after(&self, callback: impl Fn(&JobProcessed) + Send + Sync + 'static) {
        self.events.listen(callback);
    }

    /// Run the callback when a job throws an exception.
    pub fn exception_occurred(
        &self,
        callback: impl Fn(&JobExceptionOccurred) + Send + Sync + 'static,
    ) {
        self.events.listen(callback);
    }

    /// Run the callback before a worker looks for a job.
    pub fn looping(&self, callback: impl Fn(&Looping) + Send + Sync + 'static) {
        self.events.listen(callback);
    }

    /// Run the callback when a job fails.
    pub fn failing(&self, callback: impl Fn(&JobFailed) + Send + Sync + 'static) {
        self.events.listen(callback);
    }

    /// Run the callback when a worker starts.
    pub fn starting(&self, callback: impl Fn(&WorkerStarting) + Send + Sync + 'static) {
        self.events.listen(callback);
    }

    /// Run the callback when a worker stops.
    pub fn stopping(&self, callback: impl Fn(&WorkerStopping) + Send + Sync + 'static) {
        self.events.listen(callback);
    }

    // ------------------------------------------------------------------
    // Payloads
    // ------------------------------------------------------------------

    /// Add keys to every payload: the callback receives the connection,
    /// the queue and the payload, and returns the keys to merge in.
    pub fn create_payload_using(
        &self,
        callback: impl Fn(&str, &str, &Value) -> Map<String, Value> + Send + Sync + 'static,
    ) {
        self.payload_hooks.write().unwrap().push(Arc::new(callback));
    }

    /// Forget every payload hook.
    pub fn flush_payload_hooks(&self) {
        self.payload_hooks.write().unwrap().clear();
    }

    pub(crate) fn payload_hooks(&self) -> Vec<PayloadHook> {
        self.payload_hooks.read().unwrap().clone()
    }

    // ------------------------------------------------------------------
    // Pausing, resuming and restarting workers
    // ------------------------------------------------------------------

    /// Pause a queue: workers stop picking up its jobs.
    pub async fn pause(&self, connection: &str, queue: &str) -> Result<()> {
        Cache::default_store()?
            .forever(&paused_key(connection, queue), true)
            .await?;
        self.events.dispatch(QueuePaused {
            connection_name: connection.to_string(),
            queue: queue.to_string(),
            ttl: None,
        });
        Ok(())
    }

    /// Pause a queue for the given number of seconds.
    pub async fn pause_for(&self, connection: &str, queue: &str, seconds: u64) -> Result<()> {
        Cache::default_store()?
            .put(&paused_key(connection, queue), true, seconds)
            .await?;
        self.events.dispatch(QueuePaused {
            connection_name: connection.to_string(),
            queue: queue.to_string(),
            ttl: Some(seconds),
        });
        Ok(())
    }

    /// Pause every queue.
    pub async fn pause_all(&self) -> Result<()> {
        Cache::default_store()?
            .forever("illuminate:queues:paused", true)
            .await?;
        self.events.dispatch(QueuesPaused);
        Ok(())
    }

    /// Resume a paused queue.
    pub async fn resume(&self, connection: &str, queue: &str) -> Result<()> {
        Cache::default_store()?
            .forget(&paused_key(connection, queue))
            .await?;
        self.events.dispatch(QueueResumed {
            connection_name: connection.to_string(),
            queue: queue.to_string(),
        });
        Ok(())
    }

    /// Resume every queue (queues paused individually stay paused).
    pub async fn resume_all(&self) -> Result<()> {
        Cache::default_store()?
            .forget("illuminate:queues:paused")
            .await?;
        self.events.dispatch(QueuesResumed);
        Ok(())
    }

    /// Determine if the given queue is paused.
    pub async fn is_paused(&self, connection: &str, queue: &str) -> Result<bool> {
        let cache = Cache::default_store()?;
        Ok(flag(&cache, "illuminate:queues:paused").await?
            || flag(&cache, &paused_key(connection, queue)).await?)
    }

    /// The paused queues among the given ones.
    pub async fn get_paused_queues(
        &self,
        connection: &str,
        queues: &[String],
    ) -> Result<Vec<String>> {
        let cache = Cache::default_store()?;
        if flag(&cache, "illuminate:queues:paused").await? {
            return Ok(queues.to_vec());
        }
        let mut paused = Vec::new();
        for queue in queues {
            if flag(&cache, &paused_key(connection, queue)).await? {
                paused.push(queue.clone());
            }
        }
        Ok(paused)
    }

    /// Signal every worker to restart after its current job
    /// (`queue:restart`).
    pub async fn restart(&self) -> Result<()> {
        Cache::default_store()?
            .forever("illuminate:queue:restart", Carbon::now().timestamp())
            .await?;
        Ok(())
    }

    /// Stop workers from polling the cache for restart and pause signals.
    pub fn without_interruption_polling(&self) {
        self.restartable.store(false, Ordering::SeqCst);
        self.pausable.store(false, Ordering::SeqCst);
    }

    /// Whether workers poll for restart signals.
    pub fn is_restartable(&self) -> bool {
        self.restartable.load(Ordering::SeqCst)
    }

    /// Whether workers poll for pause signals.
    pub fn is_pausable(&self) -> bool {
        self.pausable.load(Ordering::SeqCst)
    }

    /// Set whether workers poll for restart signals.
    pub fn set_restartable(&self, restartable: bool) {
        self.restartable.store(restartable, Ordering::SeqCst);
    }

    /// Set whether workers poll for pause signals.
    pub fn set_pausable(&self, pausable: bool) {
        self.pausable.store(pausable, Ordering::SeqCst);
    }

    // ------------------------------------------------------------------
    // Failed jobs
    // ------------------------------------------------------------------

    /// Push a failed job back onto its connection and queue
    /// (`queue:retry`), with its attempts reset.
    pub async fn retry(&self, failed: &FailedJob) -> Result<Option<String>> {
        self.events.dispatch(JobRetryRequested {
            job: failed.clone(),
        });
        let payload = prepare_for_retry(&failed.payload)?;
        self.connection(Some(&failed.connection))?
            .push_raw(payload, Some(&failed.queue), None)
            .await
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    pub(crate) fn set_fake(&self, fake: Option<Arc<QueueFake>>) {
        *self.fake.write().unwrap() = fake;
    }

    /// The fake swapped in by `Queue::fake()`, if any.
    pub fn fake_instance(&self) -> Option<Arc<QueueFake>> {
        self.fake.read().unwrap().clone()
    }
}

fn after_commit(config: &Value) -> bool {
    config.get("after_commit").is_some_and(ValueExt::truthy)
}

async fn flag(cache: &illuminate_cache::Repository, key: &str) -> Result<bool> {
    Ok(cache.get(key).await?.is_some_and(|value| value.truthy()))
}

fn paused_key(connection: &str, queue: &str) -> String {
    format!("illuminate:queue:paused:{connection}:{queue}")
}

/// Build a queue manager from the container's configuration.
pub(crate) fn make_manager(container: &Container) -> Arc<QueueManager> {
    let config = container
        .try_make::<Config>()
        .unwrap_or_else(|_| Arc::new(Config::empty()));
    Arc::new(QueueManager::new(config))
}

/// The queue manager bound in the container (registered on first use).
pub fn queue_manager() -> Arc<QueueManager> {
    if let Some(manager) = try_app::<QueueManager>() {
        return manager;
    }
    let container = Container::get_instance();
    container.singleton_if::<QueueManager>(make_manager);
    container.make::<QueueManager>()
}
