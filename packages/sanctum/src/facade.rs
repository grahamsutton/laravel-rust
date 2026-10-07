//! The `Sanctum` facade: Sanctum's hooks, helpers and testing support.

use std::any::TypeId;
use std::future::Future;
use std::sync::{Arc, RwLock};

use illuminate_auth::{AuthUser, Authenticatable};
use illuminate_container::{Container, try_app};
use illuminate_database::eloquent::{BoxFuture, Model};
use illuminate_http::Request;
use illuminate_support::{Result, Value, ValueExt};

use crate::access_token::{AccessToken, TokenBindings, TokenOwner, token_for};
use crate::has_api_tokens::HasApiTokens;
use crate::personal_access_token::PersonalAccessToken;
use crate::support::host_with_port;

/// Resolves the owner of a token from its `tokenable_type` and
/// `tokenable_id` (see [`Sanctum::resolve_tokenables_using`]).
pub type TokenableResolver =
    Arc<dyn Fn(String, Value) -> BoxFuture<'static, Result<Option<AuthUser>>> + Send + Sync>;

type TokenRetriever = Arc<dyn Fn(&Request) -> Option<String> + Send + Sync>;
type TokenAuthenticator = Arc<dyn Fn(&PersonalAccessToken, bool) -> bool + Send + Sync>;

/// A model type tokens may belong to.
#[derive(Clone, Copy)]
pub(crate) struct Tokenable {
    type_id: TypeId,
    class_name: fn() -> &'static str,
    morph_class: fn() -> String,
    find: fn(Value) -> BoxFuture<'static, Result<Option<AuthUser>>>,
}

impl Tokenable {
    fn of<M: Model + Authenticatable>() -> Self {
        Self {
            type_id: TypeId::of::<M>(),
            class_name: M::class_name,
            morph_class: M::morph_class,
            find: |id| Box::pin(async move { Ok(M::find(id).await?.map(AuthUser::new)) }),
        }
    }

    /// The model's class name (`User`).
    pub(crate) fn class_name(&self) -> &'static str {
        (self.class_name)()
    }

    /// Find the model with the given key, as an authenticated user.
    pub(crate) fn find(&self, id: Value) -> BoxFuture<'static, Result<Option<AuthUser>>> {
        (self.find)(id)
    }

    /// Whether a `tokenable_type` column refers to this model (by its morph
    /// alias or its class name).
    fn matches(&self, tokenable_type: &str) -> bool {
        (self.morph_class)() == tokenable_type || (self.class_name)() == tokenable_type
    }
}

/// Sanctum's per-application state: Laravel keeps it in static properties
/// of the `Sanctum` class; here it lives in the container, so every test
/// application gets its own.
#[derive(Default)]
pub(crate) struct SanctumState {
    tokenables: RwLock<Vec<Tokenable>>,
    resolver: RwLock<Option<TokenableResolver>>,
    retriever: RwLock<Option<TokenRetriever>>,
    authenticator: RwLock<Option<TokenAuthenticator>>,
    /// Tokens granted by `Sanctum::acting_as`, for every request.
    pub(crate) acting_as: TokenBindings,
    /// Tokens attached to users outside of a request.
    pub(crate) fallback: TokenBindings,
}

impl SanctumState {
    pub(crate) fn register_tokenable<M: Model + Authenticatable>(&self) {
        let id = TypeId::of::<M>();
        if self
            .tokenables
            .read()
            .unwrap()
            .iter()
            .any(|tokenable| tokenable.type_id == id)
        {
            return;
        }
        let mut tokenables = self.tokenables.write().unwrap();
        if !tokenables.iter().any(|tokenable| tokenable.type_id == id) {
            tokenables.push(Tokenable::of::<M>());
        }
    }

    /// The registered model a `tokenable_type` refers to.
    pub(crate) fn tokenable(&self, tokenable_type: &str) -> Option<Tokenable> {
        self.tokenables
            .read()
            .unwrap()
            .iter()
            .find(|tokenable| tokenable.matches(tokenable_type))
            .copied()
    }

    pub(crate) fn tokenable_resolver(&self) -> Option<TokenableResolver> {
        self.resolver.read().unwrap().clone()
    }

    pub(crate) fn token_retriever(&self) -> Option<TokenRetriever> {
        self.retriever.read().unwrap().clone()
    }

    pub(crate) fn token_authenticator(&self) -> Option<TokenAuthenticator> {
        self.authenticator.read().unwrap().clone()
    }
}

/// Sanctum's state for the current application (registering it on first use).
pub(crate) fn state() -> Arc<SanctumState> {
    if let Some(state) = try_app::<SanctumState>() {
        return state;
    }
    let container = Container::get_instance();
    container.singleton_if::<SanctumState>(|_| Arc::new(SanctumState::default()));
    container.make::<SanctumState>()
}

/// The `Sanctum` facade.
///
/// ```ignore
/// use laravel_sanctum::Sanctum;
///
/// // In a test: authenticate as the user, with a token that may view tasks...
/// Sanctum::acting_as(user, &["view-tasks"], None);
///
/// // In a service provider: customize how tokens are found and validated...
/// Sanctum::get_access_token_from_request_using(|request| request.header("x-api-key"));
/// Sanctum::authenticate_access_tokens_using(|token, is_valid| is_valid && token.name != "legacy");
/// ```
pub struct Sanctum;

impl Sanctum {
    /// The name of the guard Sanctum registers (`auth:sanctum`).
    pub const GUARD: &'static str = "sanctum";

    /// The placeholder [`Sanctum::current_request_host`] puts in the
    /// stateful domains list, replaced by the request's host at runtime.
    pub const CURRENT_REQUEST_HOST_PLACEHOLDER: &'static str = "__SANCTUM_CURRENT_REQUEST_HOST__";

    /// Authenticate as the given user for the rest of the test, with a token
    /// granted the given abilities (`["*"]` grants them all). The guard
    /// defaults to `sanctum`, and becomes the default guard.
    ///
    /// ```ignore
    /// Sanctum::acting_as(User::factory().create().await?, &["view-tasks"], None);
    ///
    /// let response = test.get("/api/task").await;
    /// response.assert_ok();
    /// ```
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use illuminate_auth::{Auth, AuthServiceProvider, Authenticatable};
    /// # use illuminate_config::Repository;
    /// # use illuminate_container::{Container, ServiceProvider};
    /// # use illuminate_database::eloquent::*;
    /// use laravel_sanctum::{HasApiTokens, Sanctum, SanctumServiceProvider};
    ///
    /// # #[derive(Debug, Clone, Default, Model)]
    /// # pub struct User {
    /// #     pub id: u64,
    /// # }
    /// # impl Authenticatable for User {
    /// #     fn auth_identifier(&self) -> Value { json!(self.id) }
    /// #     fn auth_password(&self) -> String { String::new() }
    /// # }
    /// # fn main() {
    /// # let container = Arc::new(Container::new());
    /// # let _guard = Container::set_local_instance(container.clone());
    /// # container.instance(Repository::new(json!({})));
    /// # AuthServiceProvider.register(&container);
    /// # SanctumServiceProvider.register(&container);
    /// let user = Sanctum::acting_as(User { id: 1 }, &["view-tasks"], None);
    ///
    /// assert!(user.token_can("view-tasks"));
    /// assert!(user.token_cant("delete-tasks"));
    /// assert_eq!(Auth::get_default_driver(), "sanctum");
    /// # }
    /// ```
    pub fn acting_as<'a, M: HasApiTokens>(
        user: M,
        abilities: &[&str],
        guard: impl Into<Option<&'a str>>,
    ) -> M {
        let guard = guard.into().unwrap_or(Self::GUARD);
        Self::use_tokenable_model::<M>();

        let token = PersonalAccessToken {
            tokenable_type: M::morph_class(),
            tokenable_id: user.get_key(),
            abilities: abilities
                .iter()
                .map(|ability| ability.to_string())
                .collect(),
            ..Default::default()
        };
        state()
            .acting_as
            .set(TokenOwner::of(&user), AccessToken::from(token));

        illuminate_auth::manager().acting_as(AuthUser::new(user.clone()), Some(guard));
        user
    }

    /// Let tokens belong to the given model: Sanctum can then find the
    /// owner of a token whose `tokenable_type` is the model's morph class.
    ///
    /// Models that issue tokens in this process are registered
    /// automatically; the framework registers every authenticatable model
    /// through [`Sanctum::resolve_tokenables_using`].
    ///
    /// ```ignore
    /// Sanctum::use_tokenable_model::<User>();
    /// ```
    pub fn use_tokenable_model<M: Model + Authenticatable>() {
        state().register_tokenable::<M>();
    }

    /// Resolve the owners of tokens whose model wasn't registered with
    /// [`Sanctum::use_tokenable_model`]: the callback receives the token's
    /// `tokenable_type` and `tokenable_id`.
    ///
    /// ```ignore
    /// Sanctum::resolve_tokenables_using(|tokenable_type, id| async move {
    ///     match tokenable_type.as_str() {
    ///         "Admin" => Ok(Admin::find(id).await?.map(AuthUser::new)),
    ///         _ => Ok(None),
    ///     }
    /// });
    /// ```
    pub fn resolve_tokenables_using<F, Fut>(resolver: F)
    where
        F: Fn(String, Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<AuthUser>>> + Send + 'static,
    {
        let resolver: TokenableResolver =
            Arc::new(move |tokenable_type, id| Box::pin(resolver(tokenable_type, id)));
        *state().resolver.write().unwrap() = Some(resolver);
    }

    /// Specify how the access token is read from the request (by default,
    /// the `Authorization: Bearer` header).
    ///
    /// ```ignore
    /// Sanctum::get_access_token_from_request_using(|request| request.header("x-api-key"));
    /// ```
    pub fn get_access_token_from_request_using(
        callback: impl Fn(&Request) -> Option<String> + Send + Sync + 'static,
    ) {
        *state().retriever.write().unwrap() = Some(Arc::new(callback));
    }

    /// Decide whether a token may authenticate the request: the callback
    /// receives the token and whether Sanctum considers it valid.
    ///
    /// ```ignore
    /// Sanctum::authenticate_access_tokens_using(|token, is_valid| {
    ///     is_valid && token.last_used_at.is_none_or(|used| used.gt(&Carbon::now().sub_days(30)))
    /// });
    /// ```
    pub fn authenticate_access_tokens_using(
        callback: impl Fn(&PersonalAccessToken, bool) -> bool + Send + Sync + 'static,
    ) {
        *state().authenticator.write().unwrap() = Some(Arc::new(callback));
    }

    /// The host (and port) of `app.url`, prefixed with a comma — ready to be
    /// appended to a comma-separated stateful domains list, like Laravel's
    /// `config/sanctum.php` does. Empty when there's no application URL.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_config::Repository;
    /// use illuminate_container::Container;
    /// use illuminate_support::json;
    /// use laravel_sanctum::Sanctum;
    ///
    /// let container = Arc::new(Container::new());
    /// let _guard = Container::set_local_instance(container.clone());
    /// container.instance(Repository::new(json!({"app": {"url": "http://laravel.test:8000"}})));
    ///
    /// let stateful = format!("localhost,127.0.0.1{}", Sanctum::current_application_url_with_port());
    /// assert_eq!(stateful, "localhost,127.0.0.1,laravel.test:8000");
    /// ```
    pub fn current_application_url_with_port() -> String {
        try_app::<illuminate_config::Repository>()
            .map(|config| config.get("app.url"))
            .filter(|url| !url.is_null())
            .and_then(|url| host_with_port(&url.to_string_lossy()))
            .map(|host| format!(",{host}"))
            .unwrap_or_default()
    }

    /// A placeholder (prefixed with a comma) that makes every request to the
    /// application's own host stateful.
    pub fn current_request_host() -> String {
        format!(",{}", Self::CURRENT_REQUEST_HOST_PLACEHOLDER)
    }

    /// The token that authenticated the given user, if any.
    pub fn access_token_for(user: &AuthUser) -> Option<AccessToken> {
        token_for(&TokenOwner::of_user(user))
    }

    /// Forget every token granted by [`Sanctum::acting_as`].
    pub fn forget_acting_as() {
        state().acting_as.clear();
    }
}
