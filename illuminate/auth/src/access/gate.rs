//! The gate: the application's authorization service.

use std::any::{Any, TypeId};
use std::future::Future;
use std::sync::{Arc, RwLock};

use indexmap::IndexMap;

use super::arguments::{GateArgument, GateArguments, IntoAbilities, IntoGateArguments};
use super::callbacks::{Ability, AbilityFn, AfterCallback, AfterFn, BeforeCallback, BeforeFn};
use super::policy::{Policy, PolicyEntry, PolicyOutcome};
use super::response::{AuthResponse, AuthorizationException, IntoMessage};
use crate::user::AuthUser;

/// Everything defined on the gate. Stored behind an `Arc` and copied on
/// write, so checks never hold a lock while running user callbacks.
#[derive(Clone, Default)]
struct Registry {
    abilities: IndexMap<String, AbilityFn>,
    policies: Vec<PolicyEntry>,
    before: Vec<BeforeFn>,
    after: Vec<AfterFn>,
    default_denial: Option<AuthResponse>,
}

impl Registry {
    fn policy_for(&self, argument: &GateArgument<'_>) -> Option<&PolicyEntry> {
        if let Some(type_id) = argument.model_type_id() {
            return self.policies.iter().find(|policy| policy.model == type_id);
        }
        match argument {
            GateArgument::Value(illuminate_support::Value::String(name)) => self
                .policies
                .iter()
                .find(|policy| policy.matches_name(name)),
            _ => None,
        }
    }

    /// Run the check: "before" hooks, the policy or gate, then "after" hooks.
    fn raw(
        &self,
        user: Option<&AuthUser>,
        ability: &str,
        arguments: &[GateArgument<'_>],
    ) -> Option<AuthResponse> {
        let mut result = self.before.iter().find_map(|before| before(user, ability));

        if result.is_none() {
            result = self.call_auth_callback(user, ability, arguments);
        }

        for after in &self.after {
            let after_result = after(
                user,
                ability,
                result.as_ref().map(AuthResponse::allowed),
                arguments,
            );
            if result.is_none() {
                result = after_result;
            }
        }

        result
    }

    fn call_auth_callback(
        &self,
        user: Option<&AuthUser>,
        ability: &str,
        arguments: &[GateArgument<'_>],
    ) -> Option<AuthResponse> {
        if let Some(first) = arguments.first()
            && let Some(policy) = self.policy_for(first)
            && let PolicyOutcome::Handled(result) = policy.invoke(user, ability, first.as_any())
        {
            return result;
        }
        self.abilities
            .get(ability)
            .and_then(|callback| callback(user, arguments))
    }

    fn inspect(
        &self,
        user: Option<&AuthUser>,
        ability: &str,
        arguments: &[GateArgument<'_>],
    ) -> AuthResponse {
        match self.raw(user, ability, arguments) {
            Some(response) if response.is_plain_denial() => {
                self.default_denial.clone().unwrap_or(response)
            }
            Some(response) => response,
            None => self
                .default_denial
                .clone()
                .unwrap_or_else(|| AuthResponse::deny(None)),
        }
    }
}

/// The gate — the service behind the `Gate` facade.
///
/// Abilities are defined with typed closures and policies; checks run
/// against the current request's user (asynchronously, since resolving the
/// user may hit the database), or against a specific user with
/// [`for_user`](AccessGate::for_user) (synchronously).
///
/// ```
/// use illuminate_auth::{AccessGate, AuthUser, GenericUser};
/// use illuminate_support::json;
///
/// struct Post { user_id: i64 }
///
/// let gate = AccessGate::new();
///
/// gate.define("update-post", |user: &GenericUser, post: &Post| {
///     user.get("id") == json!(post.user_id)
/// });
///
/// let user = AuthUser::new(GenericUser::new(json!({"id": 1})));
///
/// assert!(gate.for_user(&user).allows("update-post", &Post { user_id: 1 }));
/// assert!(gate.for_user(&user).denies("update-post", &Post { user_id: 2 }));
/// assert!(gate.for_guest().denies("update-post", &Post { user_id: 1 }));
/// ```
#[derive(Default)]
pub struct AccessGate {
    registry: RwLock<Arc<Registry>>,
}

impl AccessGate {
    /// Create an empty gate.
    pub fn new() -> Self {
        Self::default()
    }

    fn snapshot(&self) -> Arc<Registry> {
        self.registry.read().unwrap().clone()
    }

    fn change(&self, change: impl FnOnce(&mut Registry)) {
        let mut registry = self.registry.write().unwrap();
        change(Arc::make_mut(&mut registry));
    }

    // ------------------------------------------------------------------
    // Defining abilities
    // ------------------------------------------------------------------

    /// Define a new ability with a typed closure.
    ///
    /// The closure receives the user (`&User`, or `Option<&User>` to also
    /// run for guests) and up to two typed arguments, and returns a
    /// `bool`, an [`AuthResponse`], or an `Option` of either.
    pub fn define<M, F: Ability<M>>(&self, ability: impl Into<String>, callback: F) -> &Self {
        let callback = callback.into_ability();
        self.change(|registry| {
            registry.abilities.insert(ability.into(), callback);
        });
        self
    }

    /// Register a policy for a model type.
    pub fn policy<M: Any + Send + Sync, P: Policy<M>>(&self, policy: P) -> &Self {
        let entry = PolicyEntry::new::<M, P>(policy);
        self.change(|registry| {
            registry
                .policies
                .retain(|existing| existing.model != entry.model);
            registry.policies.push(entry);
        });
        self
    }

    /// Register a callback to run before all checks.
    pub fn before<M, F: BeforeCallback<M>>(&self, callback: F) -> &Self {
        let callback = callback.into_before();
        self.change(|registry| registry.before.push(callback));
        self
    }

    /// Register a callback to run after all checks.
    pub fn after<M, F: AfterCallback<M>>(&self, callback: F) -> &Self {
        let callback = callback.into_after();
        self.change(|registry| registry.after.push(callback));
        self
    }

    /// Set the response returned when a check is denied without one.
    pub fn default_denial_response(&self, response: AuthResponse) -> &Self {
        self.change(|registry| registry.default_denial = Some(response));
        self
    }

    /// Determine if an ability has been defined.
    pub fn has(&self, ability: &str) -> bool {
        self.snapshot().abilities.contains_key(ability)
    }

    /// Determine if every given ability has been defined.
    pub fn has_all(&self, abilities: impl IntoAbilities) -> bool {
        let registry = self.snapshot();
        abilities
            .into_abilities()
            .iter()
            .all(|ability| registry.abilities.contains_key(ability))
    }

    /// The names of every defined ability.
    pub fn abilities(&self) -> Vec<String> {
        self.snapshot().abilities.keys().cloned().collect()
    }

    /// The model types that have policies.
    pub fn policies(&self) -> Vec<&'static str> {
        self.snapshot()
            .policies
            .iter()
            .map(|policy| policy.model_name)
            .collect()
    }

    /// The policy registered for a model type (downcast it to the policy's
    /// type to call it directly).
    pub fn get_policy_for<M: 'static>(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        let type_id = TypeId::of::<M>();
        self.snapshot()
            .policies
            .iter()
            .find(|policy| policy.model == type_id)
            .map(|policy| policy.instance.clone())
    }

    /// Determine if a policy is registered for a model type.
    pub fn has_policy_for<M: 'static>(&self) -> bool {
        self.get_policy_for::<M>().is_some()
    }

    // ------------------------------------------------------------------
    // Checking abilities
    // ------------------------------------------------------------------

    /// A gate that checks abilities for the given user.
    pub fn for_user(&self, user: impl Into<AuthUser>) -> UserGate {
        UserGate {
            registry: self.snapshot(),
            user: Some(user.into()),
        }
    }

    /// A gate that checks abilities for a guest.
    pub fn for_guest(&self) -> UserGate {
        UserGate {
            registry: self.snapshot(),
            user: None,
        }
    }

    /// A gate for an optional user.
    pub fn for_optional_user(&self, user: Option<AuthUser>) -> UserGate {
        UserGate {
            registry: self.snapshot(),
            user,
        }
    }

    /// A gate for the current request's user.
    pub async fn for_current_user(&self) -> UserGate {
        let user = crate::facade::manager().resolve_user(None).await;
        self.for_optional_user(user)
    }

    /// Determine if the ability should be granted for the current user.
    pub fn allows<'a>(
        &'a self,
        ability: &'a str,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = bool> + Send + 'a {
        let arguments = arguments.into_gate_arguments();
        async move { self.for_current_user().await.allows(ability, arguments) }
    }

    /// Determine if the ability should be denied for the current user.
    pub fn denies<'a>(
        &'a self,
        ability: &'a str,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = bool> + Send + 'a {
        let arguments = arguments.into_gate_arguments();
        async move { self.for_current_user().await.denies(ability, arguments) }
    }

    /// Determine if all of the abilities should be granted for the current user.
    pub fn check<'a>(
        &'a self,
        abilities: impl IntoAbilities,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = bool> + Send + 'a {
        let abilities = abilities.into_abilities();
        let arguments = arguments.into_gate_arguments();
        async move { self.for_current_user().await.check(abilities, arguments) }
    }

    /// Determine if any one of the abilities should be granted for the current user.
    pub fn any<'a>(
        &'a self,
        abilities: impl IntoAbilities,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = bool> + Send + 'a {
        let abilities = abilities.into_abilities();
        let arguments = arguments.into_gate_arguments();
        async move { self.for_current_user().await.any(abilities, arguments) }
    }

    /// Determine if none of the abilities should be granted for the current user.
    pub fn none<'a>(
        &'a self,
        abilities: impl IntoAbilities,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = bool> + Send + 'a {
        let abilities = abilities.into_abilities();
        let arguments = arguments.into_gate_arguments();
        async move { self.for_current_user().await.none(abilities, arguments) }
    }

    /// Authorize the ability for the current user, or fail with an
    /// [`AuthorizationException`].
    pub fn authorize<'a>(
        &'a self,
        ability: &'a str,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = Result<AuthResponse, AuthorizationException>> + Send + 'a {
        let arguments = arguments.into_gate_arguments();
        async move { self.for_current_user().await.authorize(ability, arguments) }
    }

    /// Inspect the full response of a check for the current user.
    pub fn inspect<'a>(
        &'a self,
        ability: &'a str,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = AuthResponse> + Send + 'a {
        let arguments = arguments.into_gate_arguments();
        async move { self.for_current_user().await.inspect(ability, arguments) }
    }

    /// The raw result of a check for the current user (`None` when nothing
    /// had an opinion).
    pub fn raw<'a>(
        &'a self,
        ability: &'a str,
        arguments: impl IntoGateArguments<'a>,
    ) -> impl Future<Output = Option<AuthResponse>> + Send + 'a {
        let arguments = arguments.into_gate_arguments();
        async move { self.for_current_user().await.raw(ability, arguments) }
    }

    /// Authorize inline: allowed when the condition is true.
    pub fn allow_if(
        &self,
        condition: bool,
        message: impl IntoMessage,
    ) -> Result<AuthResponse, AuthorizationException> {
        AuthResponse::new(condition, message).authorize()
    }

    /// Authorize inline: denied when the condition is true.
    pub fn deny_if(
        &self,
        condition: bool,
        message: impl IntoMessage,
    ) -> Result<AuthResponse, AuthorizationException> {
        AuthResponse::new(!condition, message).authorize()
    }
}

/// A gate bound to a specific user (or a guest), returned by
/// [`AccessGate::for_user`]. Its checks are synchronous.
#[derive(Clone)]
pub struct UserGate {
    registry: Arc<Registry>,
    user: Option<AuthUser>,
}

impl UserGate {
    /// The user the gate checks abilities for.
    pub fn user(&self) -> Option<&AuthUser> {
        self.user.as_ref()
    }

    /// The raw result of a check (`None` when nothing had an opinion).
    pub fn raw<'a>(
        &self,
        ability: &str,
        arguments: impl IntoGateArguments<'a>,
    ) -> Option<AuthResponse> {
        self.registry.raw(
            self.user.as_ref(),
            ability,
            &arguments.into_gate_arguments(),
        )
    }

    /// Inspect the full response of a check.
    pub fn inspect<'a>(
        &self,
        ability: &str,
        arguments: impl IntoGateArguments<'a>,
    ) -> AuthResponse {
        self.registry.inspect(
            self.user.as_ref(),
            ability,
            &arguments.into_gate_arguments(),
        )
    }

    /// Determine if the ability should be granted.
    pub fn allows<'a>(&self, ability: &str, arguments: impl IntoGateArguments<'a>) -> bool {
        self.inspect(ability, arguments).allowed()
    }

    /// Determine if the ability should be denied.
    pub fn denies<'a>(&self, ability: &str, arguments: impl IntoGateArguments<'a>) -> bool {
        !self.allows(ability, arguments)
    }

    /// Determine if all of the abilities should be granted.
    pub fn check<'a>(
        &self,
        abilities: impl IntoAbilities,
        arguments: impl IntoGateArguments<'a>,
    ) -> bool {
        let arguments: GateArguments<'a> = arguments.into_gate_arguments();
        abilities.into_abilities().iter().all(|ability| {
            self.registry
                .inspect(self.user.as_ref(), ability, &arguments)
                .allowed()
        })
    }

    /// Determine if any one of the abilities should be granted.
    pub fn any<'a>(
        &self,
        abilities: impl IntoAbilities,
        arguments: impl IntoGateArguments<'a>,
    ) -> bool {
        let arguments: GateArguments<'a> = arguments.into_gate_arguments();
        abilities.into_abilities().iter().any(|ability| {
            self.registry
                .inspect(self.user.as_ref(), ability, &arguments)
                .allowed()
        })
    }

    /// Determine if none of the abilities should be granted.
    pub fn none<'a>(
        &self,
        abilities: impl IntoAbilities,
        arguments: impl IntoGateArguments<'a>,
    ) -> bool {
        !self.any(abilities, arguments)
    }

    /// Authorize the ability, or fail with an [`AuthorizationException`].
    pub fn authorize<'a>(
        &self,
        ability: &str,
        arguments: impl IntoGateArguments<'a>,
    ) -> Result<AuthResponse, AuthorizationException> {
        self.inspect(ability, arguments).authorize()
    }
}

impl std::fmt::Debug for AccessGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessGate")
            .field("abilities", &self.abilities())
            .field("policies", &self.policies())
            .finish()
    }
}
