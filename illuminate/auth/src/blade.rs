//! Synchronous helpers for templates: `@auth`, `@guest`, `@can`, and
//! `Auth::user()` in Blade.
//!
//! Templates render synchronously, so they can't look a user up in the
//! database. These helpers answer from what the current request already
//! knows: the user resolved by the `auth` middleware (or any earlier
//! `Auth::user().await`), a user set with `Auth::acting_as`, and — for
//! session guards that haven't resolved their user yet — whether the
//! session holds a login. The view layer registers them as its
//! `auth_check` and `gate_check` functions.
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_auth::{Auth, Gate, GenericUser, blade};
//! use illuminate_container::Container;
//! use illuminate_support::json;
//!
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container);
//!
//! assert!(!blade::auth_check(None));
//!
//! Gate::define("view-dashboard", |user: &GenericUser| user.get("admin") == json!(true));
//! Auth::acting_as(&GenericUser::new(json!({"id": 1, "admin": true})), None);
//!
//! assert!(blade::auth_check(None));
//! assert_eq!(blade::auth_user(None).unwrap().id(), json!(1));
//! assert!(blade::gate_check("view-dashboard", ()));
//! ```

use illuminate_http::current_request;
use illuminate_session::RequestSessionExt;

use crate::access::{IntoGateArguments, gate};
use crate::facade::manager;
use crate::guards::SessionGuard;
use crate::state::AuthState;
use crate::user::AuthUser;

/// The user the current request knows about for the guard (`None` for the
/// default guard), without resolving anything.
pub fn auth_user(guard: Option<&str>) -> Option<AuthUser> {
    manager().resolved_user(&AuthState::current(), guard)
}

/// Determine if the current user is authenticated (`@auth`), without
/// resolving anything.
///
/// A session guard that hasn't resolved its user yet counts as
/// authenticated when the session holds a login.
pub fn auth_check(guard: Option<&str>) -> bool {
    if auth_user(guard).is_some() {
        return true;
    }
    let manager = manager();
    let state = AuthState::current();
    let name = guard
        .map(str::to_string)
        .unwrap_or_else(|| manager.get_default_driver());
    let snapshot = state.guard(&name);
    if snapshot.resolved || snapshot.logged_out {
        return false;
    }
    let Ok(resolved) = manager.guard(Some(&name)) else {
        return false;
    };
    let Some(session_guard) = resolved.downcast_ref::<SessionGuard>() else {
        return false;
    };
    current_request()
        .and_then(|request| request.try_session())
        .is_some_and(|session| !session.get(&session_guard.get_name()).is_null())
}

/// Determine if the current user is a guest (`@guest`).
pub fn guest_check(guard: Option<&str>) -> bool {
    !auth_check(guard)
}

/// Determine if the current user has the ability (`@can`), using the user
/// the request already knows about.
pub fn gate_check<'a>(ability: &str, arguments: impl IntoGateArguments<'a>) -> bool {
    gate()
        .for_optional_user(auth_user(None))
        .allows(ability, arguments)
}

/// Determine if the current user has any of the abilities (`@canany`).
pub fn gate_any<'a>(
    abilities: impl crate::access::IntoAbilities,
    arguments: impl IntoGateArguments<'a>,
) -> bool {
    gate()
        .for_optional_user(auth_user(None))
        .any(abilities, arguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Auth;
    use crate::tests::{app, in_request};
    use illuminate_support::json;

    #[tokio::test]
    async fn templates_see_the_session_login_before_the_user_is_resolved() {
        let app = app();
        let key = "login_web_59ba36addc2b2f9401580f014c7f58ea4e30989d";
        in_request(|request| async move {
            assert!(!auth_check(None));
            assert!(guest_check(None));

            request.session().put(key, 1);
            assert!(auth_check(None), "the session holds a login");
            assert!(auth_user(None).is_none(), "but the user isn't resolved yet");

            Auth::user::<AuthUser>().await;
            assert_eq!(auth_user(None).unwrap().id(), json!(1));

            Auth::logout().await.unwrap();
            assert!(!auth_check(None));
            assert!(!auth_check(Some("admin")));
        })
        .await;
        drop(app);
    }

    #[tokio::test]
    async fn templates_check_abilities() {
        let app = app();
        crate::Gate::define("admin", |user: &crate::tests::User| user.admin);
        let taylor = app.user(1);
        in_request(|_| async move {
            assert!(!gate_check("admin", ()));
            Auth::login(&taylor, false).await.unwrap();
            assert!(gate_check("admin", ()));
            assert!(gate_any(["nope", "admin"], ()));
        })
        .await;
    }
}
