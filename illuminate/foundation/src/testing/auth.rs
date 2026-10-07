//! Authentication testing helpers.

use illuminate_auth::{Auth, AuthUser};
use illuminate_session::SessionManager;
use illuminate_support::{Value, ValueExt};

use super::TestApp;
use super::test_response::decrypt_cookie;

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

    /// Assert that the client is authenticated on the default guard — by
    /// `acting_as`, or by logging in during an earlier request.
    pub async fn assert_authenticated(&self) -> &Self {
        assert!(
            self.authenticated_id(None).await.is_some(),
            "The user is not authenticated."
        );
        self
    }

    /// Assert that the client is authenticated on the given guard.
    pub async fn assert_authenticated_on(&self, guard: &str) -> &Self {
        assert!(
            self.authenticated_id(Some(guard)).await.is_some(),
            "The user is not authenticated on the [{guard}] guard."
        );
        self
    }

    /// Assert that the given user is the one authenticated.
    pub async fn assert_authenticated_as(&self, user: impl Into<AuthUser>) -> &Self {
        let expected = user.into().auth_identifier();
        let actual = self.authenticated_id(None).await;
        assert!(actual.is_some(), "The current user is not authenticated.");
        assert!(
            actual.as_ref().map(ValueExt::to_string_lossy) == Some(expected.to_string_lossy()),
            "The currently authenticated user is not who was expected (expected [{}], got [{}]).",
            expected.to_string_lossy(),
            actual.map(|id| id.to_string_lossy()).unwrap_or_default()
        );
        self
    }

    /// Assert that the client is a guest.
    pub async fn assert_guest(&self) -> &Self {
        assert!(
            self.authenticated_id(None).await.is_none(),
            "The user is authenticated."
        );
        self
    }

    /// The identifier of the authenticated user: the `acting_as` user, or
    /// the one stored in the client's session by the session guard.
    async fn authenticated_id(&self, guard: Option<&str>) -> Option<Value> {
        if guard.is_none() && Auth::check().await {
            return Auth::id().await;
        }

        let manager = self.app.make::<SessionManager>();
        let cookie = manager.cookie_name();
        let id = self.cookies.get(&cookie).and_then(|value| decrypt_cookie(&cookie, value))?;
        let store = manager.driver().ok()?;
        store.set_id(Some(&id));
        store.start().await.ok()?;

        let guard = guard
            .map(str::to_string)
            .unwrap_or_else(|| self.app.config_repository().string_or("auth.defaults.guard", "web"));
        let prefix = format!("login_{guard}_");
        match store.all() {
            Value::Object(map) => map
                .into_iter()
                .find(|(key, value)| key.starts_with(&prefix) && !value.is_null())
                .map(|(_, value)| value),
            _ => None,
        }
    }
}
