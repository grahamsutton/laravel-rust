//! The `Gate` facade, the `authorize()` helper, and `user.can(...)`.

use std::any::Any;
use std::future::Future;
use std::sync::Arc;

use illuminate_container::{Container, try_app};

use super::arguments::{IntoAbilities, IntoGateArguments};
use super::callbacks::{Ability, AfterCallback, BeforeCallback};
use super::gate::{AccessGate, UserGate};
use super::policy::Policy;
use super::response::{AuthResponse, AuthorizationException, IntoMessage};
use crate::user::{AuthUser, Authenticatable};

/// Resolve the application's gate, registering one if the application
/// hasn't.
pub fn gate() -> Arc<AccessGate> {
    if let Some(gate) = try_app::<AccessGate>() {
        return gate;
    }
    let container = Container::get_instance();
    container.singleton_if::<AccessGate>(|_| Arc::new(AccessGate::new()));
    container.make::<AccessGate>()
}

/// The `Gate` facade: define abilities and policies, and authorize actions.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_auth::{Auth, AuthResponse, Gate, GenericUser};
/// use illuminate_container::Container;
/// use illuminate_support::json;
///
/// struct Post { user_id: i64 }
///
/// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// # runtime.block_on(async {
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container);
///
/// Gate::define("update-post", |user: &GenericUser, post: &Post| {
///     user.get("id") == json!(post.user_id)
/// });
///
/// Gate::define("edit-settings", |user: &GenericUser| {
///     if user.get("admin") == json!(true) {
///         AuthResponse::allow()
///     } else {
///         AuthResponse::deny("You must be an administrator.")
///     }
/// });
///
/// Auth::acting_as(&GenericUser::new(json!({"id": 1, "admin": false})), None);
///
/// assert!(Gate::allows("update-post", &Post { user_id: 1 }).await);
/// assert!(Gate::denies("update-post", &Post { user_id: 2 }).await);
///
/// let response = Gate::inspect("edit-settings", ()).await;
/// assert_eq!(response.message(), Some("You must be an administrator."));
///
/// let error = Gate::authorize("edit-settings", ()).await.unwrap_err();
/// assert_eq!(error.to_string(), "You must be an administrator.");
/// # });
/// ```
pub struct Gate;

impl Gate {
    /// The gate behind the facade.
    pub fn instance() -> Arc<AccessGate> {
        gate()
    }

    /// Define a new ability with a typed closure.
    pub fn define<M, F: Ability<M>>(ability: impl Into<String>, callback: F) {
        gate().define(ability, callback);
    }

    /// Register a policy for a model type: `Gate::policy::<Post, _>(PostPolicy)`.
    pub fn policy<M: Any + Send + Sync, P: Policy<M>>(policy: P) {
        gate().policy::<M, P>(policy);
    }

    /// Register a callback to run before all checks.
    pub fn before<M, F: BeforeCallback<M>>(callback: F) {
        gate().before(callback);
    }

    /// Register a callback to run after all checks.
    pub fn after<M, F: AfterCallback<M>>(callback: F) {
        gate().after(callback);
    }

    /// Set the response returned when a check is denied without one.
    pub fn default_denial_response(response: AuthResponse) {
        gate().default_denial_response(response);
    }

    /// Determine if an ability has been defined.
    pub fn has(ability: &str) -> bool {
        gate().has(ability)
    }

    /// Determine if every given ability has been defined.
    pub fn has_all(abilities: impl IntoAbilities) -> bool {
        gate().has_all(abilities)
    }

    /// The names of every defined ability.
    pub fn abilities() -> Vec<String> {
        gate().abilities()
    }

    /// The model types that have policies.
    pub fn policies() -> Vec<&'static str> {
        gate().policies()
    }

    /// The policy registered for a model type.
    pub fn get_policy_for<M: 'static>() -> Option<Arc<dyn Any + Send + Sync>> {
        gate().get_policy_for::<M>()
    }

    /// A gate that checks abilities for the given user.
    pub fn for_user(user: impl Into<AuthUser>) -> UserGate {
        gate().for_user(user)
    }

    /// A gate that checks abilities for a guest.
    pub fn for_guest() -> UserGate {
        gate().for_guest()
    }

    /// Determine if the ability should be granted for the current user.
    pub fn allows<'a>(
        ability: &'a str,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = bool> + Send + 'a {
        let arguments = arguments.into_gate_arguments();
        async move { gate().for_current_user().await.allows(ability, arguments) }
    }

    /// Determine if the ability should be denied for the current user.
    pub fn denies<'a>(
        ability: &'a str,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = bool> + Send + 'a {
        let arguments = arguments.into_gate_arguments();
        async move { gate().for_current_user().await.denies(ability, arguments) }
    }

    /// Determine if all of the abilities should be granted for the current user.
    pub fn check<'a>(
        abilities: impl IntoAbilities,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = bool> + Send + 'a {
        let abilities = abilities.into_abilities();
        let arguments = arguments.into_gate_arguments();
        async move { gate().for_current_user().await.check(abilities, arguments) }
    }

    /// Determine if any one of the abilities should be granted for the current user.
    pub fn any<'a>(
        abilities: impl IntoAbilities,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = bool> + Send + 'a {
        let abilities = abilities.into_abilities();
        let arguments = arguments.into_gate_arguments();
        async move { gate().for_current_user().await.any(abilities, arguments) }
    }

    /// Determine if none of the abilities should be granted for the current user.
    pub fn none<'a>(
        abilities: impl IntoAbilities,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = bool> + Send + 'a {
        let abilities = abilities.into_abilities();
        let arguments = arguments.into_gate_arguments();
        async move { gate().for_current_user().await.none(abilities, arguments) }
    }

    /// Authorize the ability for the current user, or fail with an
    /// [`AuthorizationException`].
    pub fn authorize<'a>(
        ability: &'a str,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = Result<AuthResponse, AuthorizationException>> + Send + 'a {
        authorize(ability, arguments)
    }

    /// Inspect the full response of a check for the current user.
    pub fn inspect<'a>(
        ability: &'a str,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = AuthResponse> + Send + 'a {
        let arguments = arguments.into_gate_arguments();
        async move { gate().for_current_user().await.inspect(ability, arguments) }
    }

    /// The raw result of a check for the current user.
    pub fn raw<'a>(
        ability: &'a str,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = Option<AuthResponse>> + Send + 'a {
        let arguments = arguments.into_gate_arguments();
        async move { gate().for_current_user().await.raw(ability, arguments) }
    }

    /// Authorize inline: allowed when the condition is true.
    pub fn allow_if(
        condition: bool,
        message: impl IntoMessage,
    ) -> Result<AuthResponse, AuthorizationException> {
        gate().allow_if(condition, message)
    }

    /// Authorize inline: denied when the condition is true.
    pub fn deny_if(
        condition: bool,
        message: impl IntoMessage,
    ) -> Result<AuthResponse, AuthorizationException> {
        gate().deny_if(condition, message)
    }
}

/// Authorize an action for the current user, failing with an
/// [`AuthorizationException`] (a `403`) when it's denied — the controller
/// helper Laravel calls `$this->authorize()`.
///
/// ```ignore
/// async fn update(post: Post) -> Result<Response> {
///     authorize("update", &post).await?;
///     // The current user can update the blog post...
/// }
/// ```
pub fn authorize<'a>(
    ability: &'a str,
    arguments: impl IntoGateArguments<'a>,
) -> impl Future<Output = Result<AuthResponse, AuthorizationException>> + Send + 'a {
    let arguments = arguments.into_gate_arguments();
    async move {
        gate()
            .for_current_user()
            .await
            .authorize(ability, arguments)
    }
}

/// Authorization checks right on the user: `user.can("update", &post)`.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_auth::{Authorizable, Gate, GenericUser};
/// use illuminate_container::Container;
/// use illuminate_support::json;
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container);
///
/// Gate::define("view-dashboard", |user: &GenericUser| user.get("admin") == json!(true));
///
/// let admin = GenericUser::new(json!({"id": 1, "admin": true}));
/// assert!(admin.can("view-dashboard", ()));
/// assert!(admin.can_any(["view-dashboard", "delete-everything"], ()));
///
/// let guest = GenericUser::new(json!({"id": 2}));
/// assert!(guest.cannot("view-dashboard", ()));
/// ```
pub trait Authorizable {
    /// The user as an [`AuthUser`].
    fn as_auth_user(&self) -> AuthUser;

    /// Determine if the user has all of the given abilities.
    fn can<'a>(
        &self,
        abilities: impl IntoAbilities,
        arguments: impl IntoGateArguments<'a>,
    ) -> bool {
        gate()
            .for_user(self.as_auth_user())
            .check(abilities, arguments)
    }

    /// Determine if the user has any of the given abilities.
    fn can_any<'a>(
        &self,
        abilities: impl IntoAbilities,
        arguments: impl IntoGateArguments<'a>,
    ) -> bool {
        gate()
            .for_user(self.as_auth_user())
            .any(abilities, arguments)
    }

    /// Determine if the user lacks the given abilities.
    fn cannot<'a>(
        &self,
        abilities: impl IntoAbilities,
        arguments: impl IntoGateArguments<'a>,
    ) -> bool {
        !self.can(abilities, arguments)
    }

    /// Alias of [`cannot`](Authorizable::cannot).
    fn cant<'a>(
        &self,
        abilities: impl IntoAbilities,
        arguments: impl IntoGateArguments<'a>,
    ) -> bool {
        self.cannot(abilities, arguments)
    }
}

impl<U: Authenticatable> Authorizable for U {
    fn as_auth_user(&self) -> AuthUser {
        AuthUser::new(self.clone())
    }
}

impl Authorizable for AuthUser {
    fn as_auth_user(&self) -> AuthUser {
        self.clone()
    }
}
