//! The `Broadcast` facade.

use std::future::Future;
use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, try_app};
use illuminate_events::Event;
use illuminate_http::{Request, Response};
use illuminate_routing::{GroupAttributes, RouteMiddleware, router};
use illuminate_support::{Result, Value};

use crate::anonymous::AnonymousEvent;
use crate::authorization::{ChannelCallback, ChannelRegistry, PendingChannel};
use crate::broadcasters::Pusher;
use crate::channel::IntoChannels;
use crate::contracts::{Broadcaster, ShouldBroadcast};
use crate::event::BroadcastEvent;
use crate::manager::BroadcastManager;
use crate::pending::PendingBroadcast;
use crate::registration::ensure_registered;
use crate::testing::BroadcastFake;

/// The names the CSRF middleware goes by; the authorization endpoints skip
/// it, like Laravel's `withoutMiddleware(PreventRequestForgery::class)`.
const CSRF_MIDDLEWARE: [&str; 2] = [
    "csrf",
    "illuminate_session::middleware::validate_csrf_token::ValidateCsrfToken",
];

/// The `Broadcast` facade.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_auth::AuthUser;
/// use illuminate_broadcasting::Broadcast;
/// use illuminate_container::Container;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container);
/// // routes/channels.rs
/// Broadcast::channel("orders.{order_id}", |user: AuthUser, order_id: u64| async move {
///     user.id() == json!(order_id)
/// });
///
/// // Anywhere in your application...
/// let fake = Broadcast::fake();
/// Broadcast::private("orders.1").as_("OrderShipped").send().await?;
/// fake.assert_broadcast_as("OrderShipped");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub struct Broadcast;

impl Broadcast {
    /// The broadcast manager behind the facade, registering one (configured
    /// from the container's `Repository`) if the application hasn't.
    pub fn manager() -> Arc<BroadcastManager> {
        if let Some(manager) = try_app::<BroadcastManager>() {
            return manager;
        }
        let container = Container::get_instance();
        container.singleton_if::<BroadcastManager>(make_manager);
        container.make::<BroadcastManager>()
    }

    // ------------------------------------------------------------------
    // Connections
    // ------------------------------------------------------------------

    /// Get a broadcaster by connection name.
    pub fn connection(name: &str) -> Result<Arc<dyn Broadcaster>> {
        Self::manager().connection(Some(name))
    }

    /// Get the default broadcaster.
    pub fn driver() -> Result<Arc<dyn Broadcaster>> {
        Self::manager().connection(None)
    }

    /// The default connection's name.
    pub fn get_default_driver() -> String {
        Self::manager().get_default_driver()
    }

    /// Set the default connection.
    pub fn set_default_driver(name: &str) {
        Self::manager().set_default_driver(name);
    }

    /// Forget a resolved connection (the default one when `None`).
    pub fn purge(name: Option<&str>) {
        Self::manager().purge(name);
    }

    /// Register a custom driver.
    pub fn extend(
        driver: &str,
        creator: impl Fn(&Value) -> Result<Arc<dyn Broadcaster>> + Send + Sync + 'static,
    ) {
        Self::manager().extend(driver, creator);
    }

    /// Get a Pusher client for the given connection configuration.
    pub fn pusher(config: &Value) -> Result<Pusher> {
        Self::manager().pusher(config)
    }

    // ------------------------------------------------------------------
    // Channels & authorization
    // ------------------------------------------------------------------

    /// Register a channel authorization callback — the heart of
    /// `routes/channels.rs`.
    ///
    /// The callback receives the authenticated user (as [`AuthUser`] or your
    /// own user type) and the channel's `{wildcards}`, parsed into whatever
    /// types you ask for. Return `true` to authorize the user, `false` to
    /// deny them, or — for presence channels — the user's data:
    ///
    /// ```ignore
    /// Broadcast::channel("orders.{order}", |user: User, order: Order| async move {
    ///     user.id == order.user_id
    /// });
    ///
    /// Broadcast::channel("chat.{room_id}", |user: User, room_id: u64| async move {
    ///     user.can_join_room(room_id).then(|| json!({"id": user.id, "name": user.name}))
    /// })
    /// .guards(["web", "admin"]);
    /// ```
    ///
    /// [`AuthUser`]: illuminate_auth::AuthUser
    pub fn channel<Args>(pattern: &str, callback: impl ChannelCallback<Args>) -> PendingChannel {
        Self::manager().channel(pattern, callback)
    }

    /// The registered channels.
    pub fn channels() -> Arc<ChannelRegistry> {
        Self::manager().channels()
    }

    /// Every registered channel pattern with its handler's name.
    pub fn get_channels() -> Vec<(String, String)> {
        Self::channels().get_channels()
    }

    /// Authorize the request's channel subscription (`channel_name` and
    /// `socket_id`) — for your own authorization endpoint.
    pub async fn auth(request: &Request) -> Result<Response> {
        Self::manager().auth(request).await
    }

    /// Resolve the request's user payload for Pusher's user authentication.
    pub async fn resolve_authenticated_user(request: &Request) -> Result<Option<Value>> {
        Self::manager().resolve_authenticated_user(request).await
    }

    /// Register the callback resolving the user payload for Pusher's user
    /// authentication (`/broadcasting/user-auth`). The payload must have an
    /// `id`.
    ///
    /// ```ignore
    /// Broadcast::resolve_authenticated_user_using(|request: Request| async move {
    ///     let user: User = request.user()?;
    ///     Some(json!({"id": user.id, "user_info": {"name": user.name}}))
    /// });
    /// ```
    pub fn resolve_authenticated_user_using<F, Fut>(callback: F)
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<Value>> + Send + 'static,
    {
        Self::manager().resolve_authenticated_user_using(callback);
    }

    /// Get the socket ID of the given request (or of the current request):
    /// its `X-Socket-ID` header.
    pub fn socket(request: Option<&Request>) -> Option<String> {
        Self::manager().socket(request)
    }

    // ------------------------------------------------------------------
    // Routes
    // ------------------------------------------------------------------

    /// Register the `GET|POST /broadcasting/auth` route that authorizes
    /// channel subscriptions. The routes get the `web` middleware group
    /// unless you give other attributes.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_broadcasting::Broadcast;
    /// use illuminate_container::Container;
    /// use illuminate_routing::Route;
    ///
    /// # let container = Arc::new(Container::new());
    /// # let _guard = Container::set_local_instance(container);
    /// Broadcast::routes(None);
    ///
    /// let route = &Route::get_routes()[0];
    /// assert_eq!(route.uri(), "broadcasting/auth");
    /// assert_eq!(route.middleware_names(), vec!["web"]);
    /// ```
    pub fn routes(attributes: Option<GroupAttributes>) {
        register_route(attributes, "/broadcasting/auth", authenticate);
    }

    /// Register the `GET|POST /broadcasting/user-auth` route for Pusher's
    /// user authentication.
    pub fn user_routes(attributes: Option<GroupAttributes>) {
        register_route(attributes, "/broadcasting/user-auth", authenticate_user);
    }

    /// Register the channel authorization routes. Alias of
    /// [`routes`](Broadcast::routes).
    pub fn channel_routes(attributes: Option<GroupAttributes>) {
        Self::routes(attributes);
    }

    // ------------------------------------------------------------------
    // Broadcasting
    // ------------------------------------------------------------------

    /// Begin broadcasting an event (see [`broadcast`](crate::broadcast)).
    pub fn event<E: ShouldBroadcast>(event: E) -> PendingBroadcast<E> {
        PendingBroadcast::new(event)
    }

    /// Queue the given event for broadcast, without dispatching it to its
    /// listeners.
    pub async fn queue<E: ShouldBroadcast>(event: &E) -> Result<()> {
        Self::manager().queue(event).await
    }

    /// Broadcast a captured event right away.
    pub async fn broadcast_event(job: &BroadcastEvent) -> Result<()> {
        Self::manager().broadcast_event(job).await
    }

    /// Begin sending an anonymous broadcast to the given channels.
    pub fn on(channels: impl IntoChannels) -> AnonymousEvent {
        Self::manager().on(channels)
    }

    /// Begin sending an anonymous broadcast to the given private channel.
    pub fn private(channel: &str) -> AnonymousEvent {
        Self::manager().private(channel)
    }

    /// Begin sending an anonymous broadcast to the given presence channel.
    pub fn presence(channel: &str) -> AnonymousEvent {
        Self::manager().presence(channel)
    }

    /// Broadcast the event `E` whenever it's dispatched through the event
    /// dispatcher (see [`register_broadcast!`](crate::register_broadcast)).
    pub fn register<E: ShouldBroadcast>() {
        ensure_registered::<E>(&Event::dispatcher());
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    /// Record broadcasts instead of queueing and sending them. Returns the
    /// fake so you may make assertions against it.
    pub fn fake() -> Arc<BroadcastFake> {
        Self::manager().fake()
    }

    /// The current fake.
    ///
    /// # Panics
    ///
    /// Panics when broadcasts aren't being faked.
    #[track_caller]
    pub fn fake_instance() -> Arc<BroadcastFake> {
        Self::manager()
            .fake_instance()
            .expect("Broadcasts are not being faked. Call `Broadcast::fake()` first.")
    }

    /// Assert that the event `E` was broadcast.
    #[track_caller]
    pub fn assert_broadcast<E: ShouldBroadcast>() {
        Self::fake_instance().assert_broadcast::<E>();
    }

    /// Assert that a broadcast of the event `E` passing the truth test was
    /// recorded.
    #[track_caller]
    pub fn assert_broadcast_with<E: ShouldBroadcast>(callback: impl Fn(&BroadcastEvent) -> bool) {
        Self::fake_instance().assert_broadcast_with::<E>(callback);
    }

    /// Assert that the event `E` was broadcast on the given channel.
    #[track_caller]
    pub fn assert_broadcast_on<E: ShouldBroadcast>(channel: impl AsRef<str>) {
        Self::fake_instance().assert_broadcast_on::<E>(channel);
    }

    /// Assert that the event `E` was broadcast exactly `times` times.
    #[track_caller]
    pub fn assert_broadcast_times<E: ShouldBroadcast>(times: usize) {
        Self::fake_instance().assert_broadcast_times::<E>(times);
    }

    /// Assert that an event was broadcast with the given broadcast name.
    #[track_caller]
    pub fn assert_broadcast_as(name: &str) {
        Self::fake_instance().assert_broadcast_as(name);
    }

    /// Assert that the event `E` was not broadcast.
    #[track_caller]
    pub fn assert_not_broadcast<E: ShouldBroadcast>() {
        Self::fake_instance().assert_not_broadcast::<E>();
    }

    /// Assert that nothing was broadcast.
    #[track_caller]
    pub fn assert_nothing_broadcast() {
        Self::fake_instance().assert_nothing_broadcast();
    }
}

/// Build the manager from the container's configuration.
pub(crate) fn make_manager(app: &Container) -> Arc<BroadcastManager> {
    let config = app
        .try_make::<Repository>()
        .unwrap_or_else(|_| Arc::new(Repository::empty()));
    Arc::new(BroadcastManager::new(config))
}

fn register_route<H, T>(attributes: Option<GroupAttributes>, uri: &str, handler: H)
where
    H: illuminate_routing::Handler<T>,
    T: 'static,
{
    let attributes = attributes.unwrap_or_else(|| GroupAttributes {
        middleware: vec![RouteMiddleware::named("web")],
        ..GroupAttributes::default()
    });
    let router = router();
    router.group(attributes, || {
        router
            .match_(&["GET", "POST"], uri, handler)
            .without_middleware(CSRF_MIDDLEWARE.to_vec());
    });
}

/// `/broadcasting/auth`: authorize the request's channel subscription.
async fn authenticate(request: Request) -> Result<Response> {
    Broadcast::manager().auth(&request).await
}

/// `/broadcasting/user-auth`: authenticate the connection's user.
async fn authenticate_user(request: Request) -> Result<Response> {
    Broadcast::manager().authenticate_user(&request).await
}
