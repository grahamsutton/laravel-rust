//! Typed gate callbacks.
//!
//! Gates are defined with ordinary, typed closures —
//! `|user: &User, post: &Post| user.id == post.user_id` — and stored
//! type-erased. When a check runs, the authenticated user and the arguments
//! are downcast back to the types the closure asked for. If they don't fit
//! (a guest, a user of another type, or an argument of another type), the
//! closure isn't called and the check is denied, just like Laravel.

use std::any::Any;
use std::sync::Arc;

use super::arguments::GateArgument;
use super::response::AuthResponse;
use crate::user::{AuthUser, AuthUserRef};

/// An erased ability callback: `(user, arguments) -> result` (`None` when
/// the callback doesn't apply).
pub type AbilityFn =
    Arc<dyn Fn(Option<&AuthUser>, &[GateArgument<'_>]) -> Option<AuthResponse> + Send + Sync>;

/// An erased "before" callback: `(user, ability) -> result`.
pub type BeforeFn = Arc<dyn Fn(Option<&AuthUser>, &str) -> Option<AuthResponse> + Send + Sync>;

/// An erased "after" callback: `(user, ability, result, arguments) -> result`.
pub type AfterFn = Arc<
    dyn Fn(Option<&AuthUser>, &str, Option<bool>, &[GateArgument<'_>]) -> Option<AuthResponse>
        + Send
        + Sync,
>;

/// What a gate, policy, or hook may return: `bool`, an [`AuthResponse`],
/// or either wrapped in `Option` (where `None` means "no opinion").
pub trait IntoGateResult {
    /// Convert into the gate's result.
    fn into_gate_result(self) -> Option<AuthResponse>;
}

impl IntoGateResult for bool {
    fn into_gate_result(self) -> Option<AuthResponse> {
        Some(AuthResponse::from(self))
    }
}

impl IntoGateResult for Option<bool> {
    fn into_gate_result(self) -> Option<AuthResponse> {
        self.map(AuthResponse::from)
    }
}

impl IntoGateResult for AuthResponse {
    fn into_gate_result(self) -> Option<AuthResponse> {
        Some(self)
    }
}

impl IntoGateResult for Option<AuthResponse> {
    fn into_gate_result(self) -> Option<AuthResponse> {
        self
    }
}

/// Resolve the user parameter of a guest-aware callback: `Some(None)` for
/// a guest, `Some(Some(user))` for a user of the right type, `None` when
/// the user is of another type.
fn optional_user<U: AuthUserRef>(user: Option<&AuthUser>) -> Option<Option<&U>> {
    match user {
        None => Some(None),
        Some(user) => U::from_auth_user_ref(user).map(Some),
    }
}

fn argument<'b, A: Any>(arguments: &'b [GateArgument<'_>], index: usize) -> Option<&'b A> {
    arguments.get(index)?.downcast_ref::<A>()
}

/// Closures that can define an ability.
///
/// Implemented for closures taking the user (`&User`, or `Option<&User>`
/// to allow guests) and up to two typed arguments, returning anything that
/// implements [`IntoGateResult`].
pub trait Ability<Marker>: Send + Sync + 'static {
    /// Erase the closure.
    fn into_ability(self) -> AbilityFn;
}

impl<F, U, R> Ability<(U,)> for F
where
    F: Fn(&U) -> R + Send + Sync + 'static,
    U: AuthUserRef,
    R: IntoGateResult,
{
    fn into_ability(self) -> AbilityFn {
        Arc::new(move |user, _| self(U::from_auth_user_ref(user?)?).into_gate_result())
    }
}

impl<F, U, R> Ability<(Option<U>,)> for F
where
    F: Fn(Option<&U>) -> R + Send + Sync + 'static,
    U: AuthUserRef,
    R: IntoGateResult,
{
    fn into_ability(self) -> AbilityFn {
        Arc::new(move |user, _| self(optional_user::<U>(user)?).into_gate_result())
    }
}

impl<F, U, A, R> Ability<(U, A)> for F
where
    F: Fn(&U, &A) -> R + Send + Sync + 'static,
    U: AuthUserRef,
    A: Any,
    R: IntoGateResult,
{
    fn into_ability(self) -> AbilityFn {
        Arc::new(move |user, arguments| {
            let user = U::from_auth_user_ref(user?)?;
            self(user, argument::<A>(arguments, 0)?).into_gate_result()
        })
    }
}

impl<F, U, A, R> Ability<(Option<U>, A)> for F
where
    F: Fn(Option<&U>, &A) -> R + Send + Sync + 'static,
    U: AuthUserRef,
    A: Any,
    R: IntoGateResult,
{
    fn into_ability(self) -> AbilityFn {
        Arc::new(move |user, arguments| {
            let user = optional_user::<U>(user)?;
            self(user, argument::<A>(arguments, 0)?).into_gate_result()
        })
    }
}

impl<F, U, A, B, R> Ability<(U, A, B)> for F
where
    F: Fn(&U, &A, &B) -> R + Send + Sync + 'static,
    U: AuthUserRef,
    A: Any,
    B: Any,
    R: IntoGateResult,
{
    fn into_ability(self) -> AbilityFn {
        Arc::new(move |user, arguments| {
            let user = U::from_auth_user_ref(user?)?;
            self(user, argument::<A>(arguments, 0)?, argument::<B>(arguments, 1)?).into_gate_result()
        })
    }
}

impl<F, U, A, B, R> Ability<(Option<U>, A, B)> for F
where
    F: Fn(Option<&U>, &A, &B) -> R + Send + Sync + 'static,
    U: AuthUserRef,
    A: Any,
    B: Any,
    R: IntoGateResult,
{
    fn into_ability(self) -> AbilityFn {
        Arc::new(move |user, arguments| {
            let user = optional_user::<U>(user)?;
            self(user, argument::<A>(arguments, 0)?, argument::<B>(arguments, 1)?).into_gate_result()
        })
    }
}

/// Closures that can run before every check: `|user: &User, ability: &str|`
/// (or `Option<&User>` to include guests). Returning a result short-circuits
/// the check.
pub trait BeforeCallback<Marker>: Send + Sync + 'static {
    /// Erase the closure.
    fn into_before(self) -> BeforeFn;
}

impl<F, U, R> BeforeCallback<(U,)> for F
where
    F: Fn(&U, &str) -> R + Send + Sync + 'static,
    U: AuthUserRef,
    R: IntoGateResult,
{
    fn into_before(self) -> BeforeFn {
        Arc::new(move |user, ability| self(U::from_auth_user_ref(user?)?, ability).into_gate_result())
    }
}

impl<F, U, R> BeforeCallback<(Option<U>,)> for F
where
    F: Fn(Option<&U>, &str) -> R + Send + Sync + 'static,
    U: AuthUserRef,
    R: IntoGateResult,
{
    fn into_before(self) -> BeforeFn {
        Arc::new(move |user, ability| self(optional_user::<U>(user)?, ability).into_gate_result())
    }
}

/// Closures that run after every check:
/// `|user: &User, ability: &str, result: Option<bool>|`, optionally taking
/// the arguments as a fourth parameter. Their result is only used when the
/// check itself had no result.
pub trait AfterCallback<Marker>: Send + Sync + 'static {
    /// Erase the closure.
    fn into_after(self) -> AfterFn;
}

impl<F, U, R> AfterCallback<(U,)> for F
where
    F: Fn(&U, &str, Option<bool>) -> R + Send + Sync + 'static,
    U: AuthUserRef,
    R: IntoGateResult,
{
    fn into_after(self) -> AfterFn {
        Arc::new(move |user, ability, result, _| {
            self(U::from_auth_user_ref(user?)?, ability, result).into_gate_result()
        })
    }
}

impl<F, U, R> AfterCallback<(U, GateArgument<'static>)> for F
where
    F: Fn(&U, &str, Option<bool>, &[GateArgument<'_>]) -> R + Send + Sync + 'static,
    U: AuthUserRef,
    R: IntoGateResult,
{
    fn into_after(self) -> AfterFn {
        Arc::new(move |user, ability, result, arguments| {
            self(U::from_auth_user_ref(user?)?, ability, result, arguments).into_gate_result()
        })
    }
}
