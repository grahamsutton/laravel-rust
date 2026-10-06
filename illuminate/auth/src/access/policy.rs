//! Policies: authorization logic organized around a model.

use std::any::{Any, TypeId, type_name};
use std::sync::Arc;

use illuminate_support::Str;

use super::response::AuthResponse;
use crate::user::{AuthUser, AuthUserRef};

/// A policy: the authorization rules for one model type.
///
/// Every method has a default that returns `None` — "this policy doesn't
/// handle that" — so a policy only implements the abilities it cares
/// about. Laravel's resource abilities have dedicated methods (abilities
/// are matched in any spelling: `viewAny`, `view_any`, or `view-any`);
/// any other ability goes to [`ability`](Policy::ability).
///
/// A `None` answer falls back to a gate defined with the same name, and is
/// otherwise a denial. Guests (and users of another type) are always
/// denied by policies.
///
/// ```
/// use illuminate_auth::{AuthResponse, Authenticatable, Policy};
/// use illuminate_support::{json, Value};
/// use serde::Serialize;
///
/// #[derive(Clone, Serialize)]
/// struct User { id: u64, admin: bool }
///
/// impl Authenticatable for User {
///     fn auth_identifier(&self) -> Value { json!(self.id) }
///     fn auth_password(&self) -> String { String::new() }
/// }
///
/// struct Post { user_id: u64 }
///
/// struct PostPolicy;
///
/// impl Policy<Post> for PostPolicy {
///     type User = User;
///
///     fn before(&self, user: &User, _ability: &str) -> Option<bool> {
///         user.admin.then_some(true)
///     }
///
///     fn update(&self, user: &User, post: &Post) -> Option<AuthResponse> {
///         Some(if user.id == post.user_id {
///             AuthResponse::allow()
///         } else {
///             AuthResponse::deny("You do not own this post.")
///         })
///     }
///
///     fn ability(&self, ability: &str, user: &User, post: Option<&Post>) -> Option<AuthResponse> {
///         match ability {
///             "publish" => Some(post.is_some_and(|post| post.user_id == user.id).into()),
///             _ => None,
///         }
///     }
/// }
/// ```
pub trait Policy<M: 'static>: Send + Sync + 'static {
    /// The user type the policy authorizes (`AuthUser` accepts any user).
    type User: AuthUserRef;

    /// Run before any other check: `Some(true)` authorizes everything,
    /// `Some(false)` denies everything, `None` falls through.
    fn before(&self, _user: &Self::User, _ability: &str) -> Option<bool> {
        None
    }

    /// Determine whether the user can view any models.
    fn view_any(&self, _user: &Self::User) -> Option<AuthResponse> {
        None
    }

    /// Determine whether the user can view the model.
    fn view(&self, _user: &Self::User, _model: &M) -> Option<AuthResponse> {
        None
    }

    /// Determine whether the user can create models.
    fn create(&self, _user: &Self::User) -> Option<AuthResponse> {
        None
    }

    /// Determine whether the user can update the model.
    fn update(&self, _user: &Self::User, _model: &M) -> Option<AuthResponse> {
        None
    }

    /// Determine whether the user can delete the model.
    fn delete(&self, _user: &Self::User, _model: &M) -> Option<AuthResponse> {
        None
    }

    /// Determine whether the user can restore the model.
    fn restore(&self, _user: &Self::User, _model: &M) -> Option<AuthResponse> {
        None
    }

    /// Determine whether the user can permanently delete the model.
    fn force_delete(&self, _user: &Self::User, _model: &M) -> Option<AuthResponse> {
        None
    }

    /// Any other ability. `model` is `None` when the check was made against
    /// the model type (`GateArgument::class::<Post>()`).
    fn ability(&self, _ability: &str, _user: &Self::User, _model: Option<&M>) -> Option<AuthResponse> {
        None
    }
}

/// The outcome of asking a policy about an ability.
pub(crate) enum PolicyOutcome {
    /// The policy answered (possibly with "no result").
    Handled(Option<AuthResponse>),
    /// The policy doesn't handle the ability; fall back to the gates.
    Unhandled,
}

type PolicyInvoker =
    Arc<dyn Fn(Option<&AuthUser>, &str, Option<&(dyn Any + Send + Sync)>) -> PolicyOutcome + Send + Sync>;

/// A registered policy, erased.
#[derive(Clone)]
pub(crate) struct PolicyEntry {
    pub(crate) model: TypeId,
    pub(crate) model_name: &'static str,
    pub(crate) instance: Arc<dyn Any + Send + Sync>,
    invoke: PolicyInvoker,
}

impl PolicyEntry {
    pub(crate) fn new<M: Any + Send + Sync, P: Policy<M>>(policy: P) -> Self {
        let policy = Arc::new(policy);
        let invoker = policy.clone();
        Self {
            model: TypeId::of::<M>(),
            model_name: type_name::<M>(),
            instance: policy,
            invoke: Arc::new(move |user, ability, model| {
                let Some(user) = user.and_then(P::User::from_auth_user_ref) else {
                    return PolicyOutcome::Unhandled;
                };
                if let Some(result) = invoker.before(user, ability) {
                    return PolicyOutcome::Handled(Some(AuthResponse::from(result)));
                }
                let model = model.and_then(|model| model.downcast_ref::<M>());
                let result = dispatch(invoker.as_ref(), user, ability, model);
                match result {
                    Some(result) => PolicyOutcome::Handled(Some(result)),
                    None => PolicyOutcome::Unhandled,
                }
            }),
        }
    }

    /// Ask the policy about an ability.
    pub(crate) fn invoke(
        &self,
        user: Option<&AuthUser>,
        ability: &str,
        model: Option<&(dyn Any + Send + Sync)>,
    ) -> PolicyOutcome {
        (self.invoke)(user, ability, model)
    }

    /// Determine if the policy is for a model with the given class name
    /// (its full type name, or its basename: `App\Models\Post` → `Post`).
    pub(crate) fn matches_name(&self, name: &str) -> bool {
        self.model_name == name || Str::class_basename(self.model_name) == Str::class_basename(name)
    }
}

/// Call the policy method for the ability.
fn dispatch<M: 'static, P: Policy<M>>(
    policy: &P,
    user: &P::User,
    ability: &str,
    model: Option<&M>,
) -> Option<AuthResponse> {
    match (normalize(ability).as_str(), model) {
        ("view_any", _) => policy.view_any(user),
        ("create", _) => policy.create(user),
        ("view", Some(model)) => policy.view(user, model),
        ("update", Some(model)) => policy.update(user, model),
        ("delete", Some(model)) => policy.delete(user, model),
        ("restore", Some(model)) => policy.restore(user, model),
        ("force_delete", Some(model)) => policy.force_delete(user, model),
        _ => policy.ability(ability, user, model),
    }
}

/// `viewAny`, `view-any` and `view_any` all name the same ability.
fn normalize(ability: &str) -> String {
    Str::snake(&ability.replace('-', "_"))
}
