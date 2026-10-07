//! Authentication testing helpers.

use illuminate_auth::{Auth, AuthUser};

use super::TestApp;

impl TestApp {
    /// Authenticate the given user for the following requests
    /// (`$this->actingAs($user)`).
    pub fn acting_as(&mut self, user: impl Into<AuthUser>) -> &mut Self {
        Auth::acting_as(user, None);
        self
    }

    /// Authenticate the given user on a specific guard.
    pub fn acting_as_guard(&mut self, user: impl Into<AuthUser>, guard: &str) -> &mut Self {
        Auth::acting_as(user, Some(guard));
        self
    }

    /// Stop acting as a user.
    pub fn as_guest(&mut self) -> &mut Self {
        Auth::forget_acting_as();
        self
    }
}
