//! Channel authorization: the callbacks registered in `routes/channels.rs`.
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_auth::AuthUser;
//! use illuminate_broadcasting::Broadcast;
//! use illuminate_container::Container;
//! use illuminate_support::json;
//!
//! # let container = Arc::new(Container::new());
//! # let _guard = Container::set_local_instance(container);
//! Broadcast::channel("orders.{order_id}", |user: AuthUser, order_id: u64| async move {
//!     user.id() == json!(order_id)
//! });
//!
//! Broadcast::channel("chat.{room_id}", |user: AuthUser, room_id: String| async move {
//!     json!({"id": user.id(), "room": room_id})
//! })
//! .guards(["web", "admin"]);
//!
//! assert!(Broadcast::channels().has("orders.{order_id}"));
//! ```

use std::fmt;
use std::future::Future;
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use regex::Regex;

use illuminate_auth::{AuthUser, FromAuthUser};
use illuminate_http::{HttpException, Request, with_request};
use illuminate_support::{Error, Result, Value, ValueExt};

use crate::contracts::Broadcaster;

/// A type-erased channel authorization callback: it receives the user and
/// the channel's wildcard values, and answers with Laravel's semantics —
/// `false` denies, a truthy value grants (and is the presence data), and any
/// other falsy value lets the next matching channel decide.
pub type ChannelAuthorizer =
    Arc<dyn Fn(AuthUser, Vec<String>) -> BoxFuture<'static, Result<Value>> + Send + Sync>;

/// Resolves the user payload for Pusher's user authentication
/// (`/broadcasting/user-auth`).
pub type UserAuthenticator =
    Arc<dyn Fn(Request) -> BoxFuture<'static, Option<Value>> + Send + Sync>;

// ----------------------------------------------------------------------
// Callback results
// ----------------------------------------------------------------------

/// Values a channel authorization callback may return: `bool`, a [`Value`]
/// (the presence channel's user data), `()`, or an `Option` / `Result` of
/// those.
///
/// ```
/// use illuminate_broadcasting::IntoChannelResult;
/// use illuminate_support::json;
///
/// assert_eq!(true.into_channel_result().unwrap(), json!(true));
/// assert_eq!(Some(json!({"id": 1})).into_channel_result().unwrap(), json!({"id": 1}));
/// assert_eq!(None::<bool>.into_channel_result().unwrap(), json!(null));
/// ```
pub trait IntoChannelResult: Send {
    /// Convert the callback's return value.
    fn into_channel_result(self) -> Result<Value>;
}

impl IntoChannelResult for bool {
    fn into_channel_result(self) -> Result<Value> {
        Ok(Value::Bool(self))
    }
}

impl IntoChannelResult for Value {
    fn into_channel_result(self) -> Result<Value> {
        Ok(self)
    }
}

impl IntoChannelResult for () {
    fn into_channel_result(self) -> Result<Value> {
        Ok(Value::Null)
    }
}

impl<T: IntoChannelResult> IntoChannelResult for Option<T> {
    fn into_channel_result(self) -> Result<Value> {
        match self {
            Some(value) => value.into_channel_result(),
            None => Ok(Value::Null),
        }
    }
}

impl<T: IntoChannelResult, E: Into<Error> + Send> IntoChannelResult for std::result::Result<T, E> {
    fn into_channel_result(self) -> Result<Value> {
        self.map_err(Into::into)?.into_channel_result()
    }
}

// ----------------------------------------------------------------------
// Channel parameters
// ----------------------------------------------------------------------

/// Types a channel wildcard (`{order}`) can be turned into.
///
/// Anything implementing [`FromStr`] works out of the box (`String`, `u64`,
/// ...); a value that doesn't parse denies access. Implement it for your
/// models to get Laravel's channel model binding:
///
/// ```
/// use illuminate_broadcasting::{FromChannelParameter, async_trait};
/// use illuminate_support::Result;
///
/// struct Order { id: u64, user_id: u64 }
///
/// #[async_trait]
/// impl FromChannelParameter for Order {
///     async fn from_channel_parameter(value: String) -> Result<Option<Self>> {
///         // Order::find(value).await
///         Ok(value.parse().ok().map(|id| Order { id, user_id: 1 }))
///     }
/// }
/// ```
#[async_trait]
pub trait FromChannelParameter: Sized + Send {
    /// Resolve the parameter, or `None` to deny access.
    async fn from_channel_parameter(value: String) -> Result<Option<Self>>;
}

#[async_trait]
impl<T: FromStr + Send> FromChannelParameter for T {
    async fn from_channel_parameter(value: String) -> Result<Option<Self>> {
        Ok(value.parse().ok())
    }
}

// ----------------------------------------------------------------------
// Callbacks
// ----------------------------------------------------------------------

/// Anything that can authorize a channel: async closures and functions
/// taking the authenticated user (as [`AuthUser`] or your own user type)
/// followed by up to six channel wildcards.
///
/// ```
/// use illuminate_auth::AuthUser;
/// use illuminate_broadcasting::ChannelCallback;
///
/// struct OrderChannel;
///
/// impl OrderChannel {
///     /// Authenticate the user's access to the channel.
///     async fn join(user: AuthUser, order_id: u64) -> bool {
///         order_id > 0 && user.id() == 1
///     }
/// }
///
/// let authorizer = OrderChannel::join.into_authorizer();
/// ```
pub trait ChannelCallback<Args>: Send + Sync + Sized + 'static {
    /// Type-erase the callback.
    fn into_authorizer(self) -> ChannelAuthorizer;
}

impl ChannelCallback<ChannelAuthorizer> for ChannelAuthorizer {
    fn into_authorizer(self) -> ChannelAuthorizer {
        self
    }
}

macro_rules! impl_channel_callback {
    ($($param:ident),*) => {
        #[allow(non_snake_case, unused_mut, unused_variables)]
        impl<F, Fut, R, U, $($param,)*> ChannelCallback<(U, $($param,)*)> for F
        where
            F: Fn(U, $($param),*) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = R> + Send + 'static,
            R: IntoChannelResult,
            U: FromAuthUser + Send + 'static,
            $($param: FromChannelParameter + 'static,)*
        {
            fn into_authorizer(self) -> ChannelAuthorizer {
                let callback = Arc::new(self);
                Arc::new(move |user: AuthUser, parameters: Vec<String>| {
                    let callback = callback.clone();
                    Box::pin(async move {
                        let Some(user) = U::from_auth_user(&user) else {
                            return Ok(Value::Bool(false));
                        };
                        let mut parameters = parameters.into_iter();
                        $(
                            let value = parameters.next().unwrap_or_default();
                            let Some($param) = $param::from_channel_parameter(value).await? else {
                                return Ok(Value::Bool(false));
                            };
                        )*
                        callback(user, $($param),*).await.into_channel_result()
                    })
                })
            }
        }
    };
}

impl_channel_callback!();
impl_channel_callback!(P1);
impl_channel_callback!(P1, P2);
impl_channel_callback!(P1, P2, P3);
impl_channel_callback!(P1, P2, P3, P4);
impl_channel_callback!(P1, P2, P3, P4, P5);
impl_channel_callback!(P1, P2, P3, P4, P5, P6);

// ----------------------------------------------------------------------
// The registry
// ----------------------------------------------------------------------

/// Options for a registered channel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelOptions {
    /// The guards that may authenticate the user, in order (the default
    /// guard when `None`).
    pub guards: Option<Vec<String>>,
}

/// A registered channel.
struct ChannelDefinition {
    pattern: String,
    matcher: Regex,
    keys: Vec<String>,
    authorizer: ChannelAuthorizer,
    handler: String,
    options: ChannelOptions,
}

/// The application's registered channels and their authorization callbacks
/// (Laravel keeps these on the broadcaster; here every broadcaster shares
/// the manager's registry).
#[derive(Default)]
pub struct ChannelRegistry {
    channels: RwLock<IndexMap<String, Arc<ChannelDefinition>>>,
    user_authenticator: RwLock<Option<UserAuthenticator>>,
}

impl ChannelRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a channel authenticator. Registering the same pattern again
    /// replaces its callback.
    pub fn channel<Args>(&self, pattern: &str, callback: impl ChannelCallback<Args>) {
        let handler = handler_name(std::any::type_name_of_val(&callback));
        self.channel_using(pattern, callback.into_authorizer(), handler);
    }

    /// Register an already type-erased channel authenticator.
    pub fn channel_using(&self, pattern: &str, authorizer: ChannelAuthorizer, handler: String) {
        let (matcher, keys) = compile_pattern(pattern);
        let options = self
            .channels
            .read()
            .unwrap()
            .get(pattern)
            .map(|existing| existing.options.clone())
            .unwrap_or_default();
        self.channels.write().unwrap().insert(
            pattern.to_string(),
            Arc::new(ChannelDefinition {
                pattern: pattern.to_string(),
                matcher,
                keys,
                authorizer,
                handler,
                options,
            }),
        );
    }

    /// Set the options of a registered channel.
    pub fn set_options(&self, pattern: &str, options: ChannelOptions) {
        let mut channels = self.channels.write().unwrap();
        if let Some(existing) = channels.get(pattern) {
            let updated = ChannelDefinition {
                pattern: existing.pattern.clone(),
                matcher: existing.matcher.clone(),
                keys: existing.keys.clone(),
                authorizer: existing.authorizer.clone(),
                handler: existing.handler.clone(),
                options,
            };
            channels.insert(pattern.to_string(), Arc::new(updated));
        }
    }

    /// Determine if a channel pattern is registered.
    pub fn has(&self, pattern: &str) -> bool {
        self.channels.read().unwrap().contains_key(pattern)
    }

    /// Forget every registered channel.
    pub fn flush(&self) {
        self.channels.write().unwrap().clear();
    }

    /// The registered channel patterns, in registration order.
    pub fn patterns(&self) -> Vec<String> {
        self.channels.read().unwrap().keys().cloned().collect()
    }

    /// Every registered channel pattern with the name of its handler
    /// (`Closure` or `OrderChannel@join`). Powers `channel:list`.
    pub fn get_channels(&self) -> Vec<(String, String)> {
        self.channels
            .read()
            .unwrap()
            .values()
            .map(|channel| (channel.pattern.clone(), channel.handler.clone()))
            .collect()
    }

    /// The options of the first channel pattern matching the channel name.
    pub fn retrieve_channel_options(&self, channel: &str) -> ChannelOptions {
        self.channels
            .read()
            .unwrap()
            .values()
            .find(|definition| definition.matcher.is_match(channel))
            .map(|definition| definition.options.clone())
            .unwrap_or_default()
    }

    /// Determine if a channel name matches a registered-style pattern:
    /// `{wildcards}` match anything but a dot.
    ///
    /// ```
    /// use illuminate_broadcasting::ChannelRegistry;
    ///
    /// assert!(ChannelRegistry::channel_name_matches_pattern("orders.1", "orders.{id}"));
    /// assert!(!ChannelRegistry::channel_name_matches_pattern("orders.1.items", "orders.{id}"));
    /// ```
    pub fn channel_name_matches_pattern(channel: &str, pattern: &str) -> bool {
        compile_pattern(pattern).0.is_match(channel)
    }

    /// Extract the wildcard values from a channel name, by wildcard name.
    ///
    /// ```
    /// use illuminate_broadcasting::ChannelRegistry;
    ///
    /// let keys = ChannelRegistry::extract_channel_keys("orders.{order}.{item}", "orders.1.7");
    /// assert_eq!(keys["order"], "1");
    /// assert_eq!(keys["item"], "7");
    /// ```
    pub fn extract_channel_keys(pattern: &str, channel: &str) -> IndexMap<String, String> {
        let (matcher, keys) = compile_pattern(pattern);
        extract_values(&matcher, &keys, channel)
            .map(|values| keys.into_iter().zip(values).collect())
            .unwrap_or_default()
    }

    /// Retrieve the authenticated user for the channel, using the channel's
    /// guards (or the default guard).
    pub async fn retrieve_user(&self, request: &Request, channel: &str) -> Option<AuthUser> {
        let guards = self.retrieve_channel_options(channel).guards;
        with_request(request.clone(), async move {
            let auth = illuminate_auth::manager();
            match guards {
                None => auth.resolve_user(None).await,
                Some(guards) => {
                    for guard in guards {
                        if let Some(user) = auth.resolve_user(Some(&guard)).await {
                            return Some(user);
                        }
                    }
                    None
                }
            }
        })
        .await
    }

    /// Authorize the request for the given (normalized) channel name.
    ///
    /// The callbacks of every matching pattern run in registration order:
    /// `false` denies access with a `403`, a truthy result is handed to the
    /// broadcaster's
    /// [`valid_authentication_response`](Broadcaster::valid_authentication_response),
    /// and any other result lets the next matching pattern decide. When no
    /// pattern grants access, the request is denied.
    pub async fn verify_user_can_access_channel(
        &self,
        request: &Request,
        channel: &str,
        broadcaster: &dyn Broadcaster,
    ) -> Result<Value> {
        let definitions: Vec<Arc<ChannelDefinition>> =
            self.channels.read().unwrap().values().cloned().collect();

        for definition in definitions {
            let Some(parameters) = extract_values(&definition.matcher, &definition.keys, channel)
            else {
                continue;
            };

            let Some(user) = self.retrieve_user(request, channel).await else {
                return Err(access_denied());
            };

            let authorizer = definition.authorizer.clone();
            let result = with_request(request.clone(), authorizer(user, parameters)).await?;

            if result == Value::Bool(false) {
                return Err(access_denied());
            }
            if result.truthy() {
                return broadcaster
                    .valid_authentication_response(request, result)
                    .await;
            }
        }

        Err(access_denied())
    }

    /// Register the callback used to resolve the user payload for Pusher's
    /// user authentication.
    pub fn resolve_authenticated_user_using<F, Fut>(&self, callback: F)
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<Value>> + Send + 'static,
    {
        let authenticator: UserAuthenticator = Arc::new(move |request| Box::pin(callback(request)));
        *self.user_authenticator.write().unwrap() = Some(authenticator);
    }

    /// Resolve the authenticated user payload for the request, if a
    /// resolver has been registered and it finds a user.
    pub async fn resolve_authenticated_user(&self, request: &Request) -> Option<Value> {
        let authenticator = self.user_authenticator.read().unwrap().clone()?;
        with_request(request.clone(), authenticator(request.clone())).await
    }
}

impl fmt::Debug for ChannelRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChannelRegistry")
            .field("channels", &self.get_channels())
            .finish_non_exhaustive()
    }
}

/// A channel registration, returned by
/// [`Broadcast::channel`](crate::Broadcast::channel) so you may set its
/// options.
#[derive(Debug, Clone)]
pub struct PendingChannel {
    registry: Arc<ChannelRegistry>,
    pattern: String,
}

impl PendingChannel {
    pub(crate) fn new(registry: Arc<ChannelRegistry>, pattern: &str) -> Self {
        Self {
            registry,
            pattern: pattern.to_string(),
        }
    }

    /// Authenticate the user with these guards, in order, instead of the
    /// default guard.
    pub fn guards<I, S>(self, guards: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let options = ChannelOptions {
            guards: Some(guards.into_iter().map(Into::into).collect()),
        };
        self.registry.set_options(&self.pattern, options);
        self
    }

    /// Authenticate the user with the given guard.
    pub fn guard(self, guard: impl Into<String>) -> Self {
        self.guards([guard.into()])
    }

    /// The channel pattern.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }
}

/// The `channel:list` name of a callback type: `Closure` for closures,
/// `OrderChannel@join` for associated functions (even when the type is
/// defined inside a function body).
fn handler_name(type_name: &str) -> String {
    let path = type_name.split('<').next().unwrap_or(type_name);
    if path.ends_with('}') {
        return "Closure".to_string();
    }
    let path = path
        .split("::")
        .filter(|segment| !segment.starts_with('{'))
        .collect::<Vec<_>>()
        .join("::");
    illuminate_routing::handler::action_name_from_type(&path)
}

/// Laravel's `AccessDeniedHttpException`.
pub(crate) fn access_denied() -> Error {
    HttpException::new(403).into()
}

/// Compile a channel pattern: dots and other characters are literal,
/// `{wildcards}` match anything but a dot.
fn compile_pattern(pattern: &str) -> (Regex, Vec<String>) {
    static WILDCARD: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"\{(.*?)\}").expect("valid regex"));

    let mut expression = String::from("^");
    let mut keys = Vec::new();
    let mut last = 0;
    for capture in WILDCARD.captures_iter(pattern) {
        let whole = capture.get(0).expect("the whole match");
        expression.push_str(&regex::escape(&pattern[last..whole.start()]));
        expression.push_str(r"([^.]+)");
        keys.push(capture[1].to_string());
        last = whole.end();
    }
    expression.push_str(&regex::escape(&pattern[last..]));
    expression.push('$');

    let matcher = Regex::new(&expression).unwrap_or_else(|_| {
        Regex::new(&format!("^{}$", regex::escape(pattern))).expect("an escaped pattern is valid")
    });
    (matcher, keys)
}

/// The wildcard values of a channel name, or `None` when it doesn't match.
fn extract_values(matcher: &Regex, keys: &[String], channel: &str) -> Option<Vec<String>> {
    let captures = matcher.captures(channel)?;
    Some(
        (1..=keys.len())
            .map(|index| {
                captures
                    .get(index)
                    .map(|value| value.as_str().to_string())
                    .unwrap_or_default()
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_names_match_patterns_like_laravel() {
        let cases = [
            ("something", "something", true),
            ("something.23", "something.{id}", true),
            ("something.23.test", "something.{id}.test", true),
            ("something.23.test.42", "something.{id}.test.{id2}", true),
            ("something-23:test-42", "something-{id}:test-{id2}", true),
            ("something..test.42", "something.{id}.test.{id2}", false),
            ("23:string:test", "{id}:string:{text}", true),
            ("something.23", "something", false),
            ("something.23.test.42", "something.test.{id}", false),
            ("something-23-test-42", "something-{id}-test", false),
            ("23:test", "{id}:test:abcd", false),
            ("customer.order.1", "order.{id}", false),
            ("customerorder.1", "order.{id}", false),
            ("TestChannel", "Test.{id}", false),
            ("orders+1", "orders+{id}", true),
            ("ordersX1", "orders.{id}", false),
        ];
        for (channel, pattern, expected) in cases {
            assert_eq!(
                ChannelRegistry::channel_name_matches_pattern(channel, pattern),
                expected,
                "{channel} vs {pattern}"
            );
        }
    }

    #[test]
    fn wildcard_values_are_extracted_in_order() {
        let keys =
            ChannelRegistry::extract_channel_keys("asd.{model}.{nonModel}", "asd.1.something");
        assert_eq!(
            keys.into_iter().collect::<Vec<_>>(),
            vec![
                ("model".to_string(), "1".to_string()),
                ("nonModel".to_string(), "something".to_string())
            ]
        );
        assert!(ChannelRegistry::extract_channel_keys("asd", "asd").is_empty());
        assert!(ChannelRegistry::extract_channel_keys("asd.{id}", "other.1").is_empty());
    }

    #[test]
    fn channels_are_registered_with_options() {
        let registry = ChannelRegistry::new();
        registry.channel("orders.{id}", |_user: AuthUser, _id: u64| async { true });
        registry.channel("chat", |_user: AuthUser| async { false });

        assert!(registry.has("orders.{id}"));
        assert_eq!(registry.patterns(), vec!["orders.{id}", "chat"]);
        assert_eq!(
            registry.get_channels(),
            vec![
                ("orders.{id}".to_string(), "Closure".to_string()),
                ("chat".to_string(), "Closure".to_string())
            ]
        );

        assert_eq!(
            registry.retrieve_channel_options("orders.1"),
            ChannelOptions::default()
        );
        PendingChannel::new(Arc::new(ChannelRegistry::new()), "missing").guard("web");

        registry.set_options(
            "orders.{id}",
            ChannelOptions {
                guards: Some(vec!["admin".into()]),
            },
        );
        assert_eq!(
            registry.retrieve_channel_options("orders.1").guards,
            Some(vec!["admin".to_string()])
        );
        assert_eq!(
            registry.retrieve_channel_options("unknown"),
            ChannelOptions::default()
        );

        // Re-registering keeps the options.
        registry.channel("orders.{id}", |_user: AuthUser, _id: u64| async { false });
        assert_eq!(
            registry.retrieve_channel_options("orders.1").guards,
            Some(vec!["admin".to_string()])
        );
        assert_eq!(registry.patterns().len(), 2);

        registry.flush();
        assert!(registry.patterns().is_empty());
    }

    #[test]
    fn handler_names_ignore_enclosing_closures() {
        assert_eq!(
            handler_name("app::routes::channels::{{closure}}"),
            "Closure"
        );
        assert_eq!(
            handler_name("app::tests::my_test::{{closure}}::OrderChannel::join"),
            "OrderChannel@join"
        );
        assert_eq!(
            handler_name("app::broadcasting::authorize"),
            "app::broadcasting::authorize"
        );
    }

    #[test]
    fn named_handlers_are_listed_like_controllers() {
        struct OrderChannel;
        impl OrderChannel {
            async fn join(_user: AuthUser, _order: u64) -> bool {
                true
            }
        }

        let registry = ChannelRegistry::new();
        registry.channel("orders.{order}", OrderChannel::join);
        assert_eq!(registry.get_channels()[0].1, "OrderChannel@join");
    }

    #[tokio::test]
    async fn callbacks_convert_users_parameters_and_results() {
        let user = AuthUser::new(illuminate_auth::GenericUser::new(
            illuminate_support::json!({"id": 1}),
        ));

        let authorizer = (|user: AuthUser, order: u64, slug: String| async move {
            illuminate_support::json!({"id": user.id(), "order": order, "slug": slug})
        })
        .into_authorizer();
        let result = authorizer(user.clone(), vec!["7".into(), "hello".into()])
            .await
            .unwrap();
        assert_eq!(
            result,
            illuminate_support::json!({"id": 1, "order": 7, "slug": "hello"})
        );

        // Parameters that don't parse deny access.
        let denied = authorizer(user.clone(), vec!["seven".into(), "hello".into()])
            .await
            .unwrap();
        assert_eq!(denied, Value::Bool(false));

        // So do users of another type.
        #[derive(Clone, serde::Serialize)]
        struct Admin;
        impl illuminate_auth::Authenticatable for Admin {
            fn auth_identifier(&self) -> Value {
                Value::from(1)
            }
            fn auth_password(&self) -> String {
                String::new()
            }
        }
        let admins_only = (|_admin: Admin| async { true }).into_authorizer();
        assert_eq!(
            admins_only(user.clone(), Vec::new()).await.unwrap(),
            Value::Bool(false)
        );

        // Errors propagate.
        let failing = (|_user: AuthUser| async {
            Err::<bool, _>(illuminate_support::error::RuntimeException::new("Boom"))
        })
        .into_authorizer();
        assert_eq!(
            failing(user.clone(), Vec::new())
                .await
                .unwrap_err()
                .to_string(),
            "Boom"
        );

        // Unit results let the next channel decide.
        let undecided = (|_user: AuthUser| async {}).into_authorizer();
        assert_eq!(undecided(user, Vec::new()).await.unwrap(), Value::Null);
    }
}
