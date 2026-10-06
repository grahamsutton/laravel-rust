//! Per-request authentication state.
//!
//! Guards are long-lived and shared by every request the application
//! handles concurrently, so they never hold on to "the current user"
//! themselves. Instead, everything a guard learns while handling a request
//! — the resolved user, whether they logged out, whether they were
//! remembered — lives in an [`AuthState`] attached to that request.
//! Outside of a request (in a console command or a test), the auth
//! manager's own fallback state is used.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use illuminate_container::try_app;
use illuminate_http::{Request, current_request};

use crate::manager::AuthManager;
use crate::user::AuthUser;

/// What a single guard knows about the request being handled.
#[derive(Clone, Debug, Default)]
pub struct GuardState {
    /// The authenticated user, once resolved.
    pub user: Option<AuthUser>,
    /// Whether the guard already tried to resolve the user.
    pub resolved: bool,
    /// Whether the user logged out during this request.
    pub logged_out: bool,
    /// Whether the user was authenticated via the "remember me" cookie.
    pub via_remember: bool,
    /// Whether the "remember me" cookie was already tried.
    pub recall_attempted: bool,
    /// The user most recently retrieved by `attempt` / `validate`.
    pub last_attempted: Option<AuthUser>,
}

#[derive(Debug, Default)]
struct Inner {
    guards: HashMap<String, GuardState>,
    default_guard: Option<String>,
}

/// The authentication state of a request (or of the application, outside
/// of one).
///
/// Custom guards use it to remember what they resolved, so the user is
/// available through `request.user()`, gates, and the `_auth_id` request
/// attribute:
///
/// ```
/// use illuminate_auth::{AuthState, AuthUser, GenericUser};
/// use illuminate_http::Request;
/// use illuminate_support::json;
///
/// let request = Request::create("/", "GET");
/// let state = AuthState::for_request(&request);
///
/// state.update("api", |guard| {
///     guard.user = Some(AuthUser::new(GenericUser::new(json!({"id": 1}))));
///     guard.resolved = true;
/// });
///
/// assert_eq!(AuthState::for_request(&request).user("api").unwrap().id(), json!(1));
/// ```
#[derive(Debug, Default)]
pub struct AuthState {
    inner: Mutex<Inner>,
}

impl AuthState {
    /// Create an empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// The state attached to the given request (attaching a fresh one if
    /// the request doesn't have one yet).
    pub fn for_request(request: &Request) -> Arc<AuthState> {
        if let Some(state) = request.extension::<AuthState>() {
            return state;
        }
        let state = Arc::new(AuthState::new());
        request.set_extension(state.clone());
        state
    }

    /// The state for the current request, or the auth manager's fallback
    /// state when no request is being handled.
    pub fn current() -> Arc<AuthState> {
        match current_request() {
            Some(request) => Self::for_request(&request),
            None => match try_app::<AuthManager>() {
                Some(manager) => manager.fallback_state(),
                None => Arc::new(AuthState::new()),
            },
        }
    }

    /// A snapshot of the given guard's state.
    pub fn guard(&self, guard: &str) -> GuardState {
        self.inner
            .lock()
            .unwrap()
            .guards
            .get(guard)
            .cloned()
            .unwrap_or_default()
    }

    /// Change the given guard's state.
    pub fn update<R>(&self, guard: &str, change: impl FnOnce(&mut GuardState) -> R) -> R {
        let mut inner = self.inner.lock().unwrap();
        change(inner.guards.entry(guard.to_string()).or_default())
    }

    /// The user the given guard resolved (unless they logged out).
    pub fn user(&self, guard: &str) -> Option<AuthUser> {
        let state = self.guard(guard);
        if state.logged_out { None } else { state.user }
    }

    /// The guard this request uses by default (set by `Auth::should_use`).
    pub fn default_guard(&self) -> Option<String> {
        self.inner.lock().unwrap().default_guard.clone()
    }

    /// Set the guard this request uses by default.
    pub fn set_default_guard(&self, guard: Option<String>) {
        self.inner.lock().unwrap().default_guard = guard;
    }

    /// The names of the guards that hold state.
    pub fn guards(&self) -> Vec<String> {
        self.inner.lock().unwrap().guards.keys().cloned().collect()
    }

    /// Forget everything the guards learned.
    pub fn flush(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.guards.clear();
        inner.default_guard = None;
    }
}
