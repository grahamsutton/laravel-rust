//! Sanctum's events.

use std::sync::Arc;

use illuminate_container::try_app;
use illuminate_events::Dispatcher;
use illuminate_support::Result;

use crate::personal_access_token::PersonalAccessToken;

/// Dispatched when a personal access token authenticates a request.
///
/// ```ignore
/// Event::listen(|event: Arc<TokenAuthenticated>| async move {
///     Log::info(format!("Token [{}] was used.", event.token.name));
///     Ok(())
/// });
/// ```
#[derive(Clone, Debug)]
pub struct TokenAuthenticated {
    /// The token that authenticated the request.
    pub token: PersonalAccessToken,
}

impl TokenAuthenticated {
    /// Create the event.
    pub fn new(token: PersonalAccessToken) -> Self {
        Self { token }
    }
}

/// Dispatch an event through the application's dispatcher, when it has one.
pub(crate) async fn dispatch<E: Send + Sync + 'static>(event: E) -> Result<()> {
    let dispatcher: Option<Arc<Dispatcher>> = try_app::<Dispatcher>();
    match dispatcher {
        Some(dispatcher) => dispatcher.dispatch(event).await,
        None => Ok(()),
    }
}
