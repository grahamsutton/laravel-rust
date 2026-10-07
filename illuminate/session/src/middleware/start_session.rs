use std::sync::Arc;

use illuminate_cache::Cache;
use illuminate_config::Repository;
use illuminate_container::try_app;
use illuminate_cookie::CookieQueue;
use illuminate_http::{
    Cookie, Middleware, Next, Request, Response, async_trait, current_request, with_request,
};
use illuminate_routing::CurrentRoute;
use illuminate_support::{Map, Result, Value, to_value};

use crate::manager::{SessionConfig, SessionManager};
use crate::request::RequestSessionExt;
use crate::store::Store;

/// Starts the session for the request, and saves it once the response is ready.
///
/// On the way in, the session is loaded using the ID from the session cookie,
/// attached to the request (`request.session()`), and its previous URL is
/// exposed as the `_previous_url` request attribute. On the way out, the
/// response's flash data (`with`, `with_input`, `with_errors`) is written to
/// the session, the current URL is remembered, the session is saved, and the
/// session cookie is attached.
#[derive(Clone, Default)]
pub struct StartSession {
    manager: Option<Arc<SessionManager>>,
}

impl StartSession {
    /// Create the middleware, using the application's session manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create the middleware with a specific session manager.
    pub fn with_manager(manager: Arc<SessionManager>) -> Self {
        Self {
            manager: Some(manager),
        }
    }

    fn manager(&self) -> Arc<SessionManager> {
        if let Some(manager) = &self.manager {
            return manager.clone();
        }
        try_app::<SessionManager>().unwrap_or_else(|| {
            let config = try_app::<Repository>().unwrap_or_else(|| Arc::new(Repository::empty()));
            Arc::new(SessionManager::new(config))
        })
    }

    /// Get the session for the request, using the ID from its session cookie.
    pub fn get_session(&self, manager: &SessionManager, request: &Request) -> Result<Arc<Store>> {
        let session = manager.driver()?;
        session.set_id(request.cookie(&session.name()).as_deref());
        Ok(session)
    }

    /// Handle the request while holding the session's lock, so other
    /// requests using the same session wait their turn (`Route::block`).
    async fn handle_request_while_blocking(
        &self,
        manager: &SessionManager,
        request: Request,
        session: Arc<Store>,
        next: Next,
    ) -> Result<Response> {
        // Only matched routes can block; anything else runs as usual.
        let Some(route) = request.extension::<CurrentRoute>() else {
            return self.handle_stateful_request(manager, request, session, next).await;
        };
        let lock_for = route
            .route()
            .locks_for()
            .filter(|seconds| *seconds > 0)
            .unwrap_or_else(|| manager.default_route_block_lock_seconds());
        let wait_for = route
            .route()
            .waits_for()
            .unwrap_or_else(|| manager.default_route_block_wait_seconds());

        let lock = Cache::manager()?
            .driver(manager.block_driver().as_deref())?
            .lock(&format!("session:{}", session.id()), lock_for)
            .between_blocked_attempts_sleep_for(50);

        let result = match lock.block(wait_for).await {
            Ok(_) => self.handle_stateful_request(manager, request, session, next).await,
            Err(error) => Err(error),
        };
        // Releasing a lock we never acquired is a no-op.
        let _ = lock.release().await;
        result
    }

    async fn handle_stateful_request(
        &self,
        manager: &SessionManager,
        request: Request,
        session: Arc<Store>,
        next: Next,
    ) -> Result<Response> {
        let config = manager.get_session_config();

        // Start the session so its data is ready for the application.
        session.set_request_on_handler(&request);
        session.start().await?;
        request.set_session(session.clone());
        if let Some(previous) = session.previous_url() {
            request.set_attribute("_previous_url", previous);
        }

        self.collect_garbage(&session, &config).await;

        let mut response = if current_request().is_some() {
            next.run(request.clone()).await
        } else {
            with_request(request.clone(), next.run(request.clone())).await
        };

        apply_response_session_data(&session, &mut response);
        self.store_current_url(&request, &response, &session);
        self.add_cookie_to_response(&request, &mut response, &session, &config);

        if !is_precognitive(&request) {
            session.save().await?;
        }

        // The cookie driver queues the session payload as a cookie while saving.
        if config.driver.as_deref() == Some("cookie") {
            for cookie in CookieQueue::for_request(&request).all() {
                response.add_cookie(cookie);
            }
        }

        Ok(response)
    }

    /// Sweep expired sessions if this request wins the lottery.
    async fn collect_garbage(&self, session: &Store, config: &SessionConfig) {
        if config_hits_lottery(config.lottery) {
            // Garbage collection is best effort; a failure must not fail the request.
            let _ = session.get_handler().gc(config.lifetime_seconds()).await;
        }
    }

    /// Remember the current URL as the "previous" URL for GET, non-AJAX,
    /// non-prefetch requests.
    ///
    /// Laravel only remembers URLs that matched a route. The session can't
    /// see the router, so responses that are `404 Not Found` (which is what
    /// an unmatched route produces) are skipped instead.
    fn store_current_url(&self, request: &Request, response: &Response, session: &Store) {
        if request.is_method("GET")
            && !response.is_not_found()
            && !request.ajax()
            && !request.prefetch()
            && !is_precognitive(request)
        {
            session.set_previous_url(request.full_url());
            session.set_previous_route(request.route_name().as_deref());
        }
    }

    /// Attach the session cookie to the response.
    fn add_cookie_to_response(
        &self,
        request: &Request,
        response: &mut Response,
        session: &Store,
        config: &SessionConfig,
    ) {
        let mut cookie = Cookie::new(session.name(), session.id())
            .path(config.path.clone())
            .secure(config.secure.unwrap_or_else(|| request.secure()))
            .http_only(config.http_only)
            .same_site(config.same_site)
            .partitioned(config.partitioned);
        if let Some(domain) = &config.domain {
            cookie = cookie.domain(domain.clone());
        }
        if !config.expire_on_close {
            cookie = cookie.minutes(config.lifetime);
        }
        response.add_cookie(cookie);
    }
}

#[async_trait]
impl Middleware for StartSession {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let manager = self.manager();
        if !manager.session_configured() {
            return Ok(next.run(request).await);
        }
        let session = self.get_session(&manager, &request)?;
        let route_blocks = request
            .extension::<CurrentRoute>()
            .is_some_and(|route| route.route().locks_for().is_some());
        if manager.should_block() || route_blocks {
            return self
                .handle_request_while_blocking(&manager, request, session, next)
                .await;
        }
        self.handle_stateful_request(&manager, request, session, next).await
    }
}

/// Write the data a response asked the session to remember: flashed
/// key / value pairs (`with`), flashed input (`with_input`, stored as
/// `_old_input`), and errors (`with_errors`).
///
/// Errors are flashed under the `errors` key as `{bag: {field: [messages]}}`,
/// merged with any bags already flashed (just like Laravel's `ViewErrorBag`).
pub fn apply_response_session_data(session: &Store, response: &mut Response) {
    let (flash, input, errors) = response.take_session_data();

    for (key, value) in flash {
        session.flash(key, value);
    }

    if let Some(input) = input {
        session.flash_input(input);
    }

    if let Some((bag, messages)) = errors {
        let mut bags = match session.get("errors") {
            Value::Object(bags) => bags,
            _ => Map::new(),
        };
        bags.insert(bag, to_value(messages.messages()));
        session.flash("errors", Value::Object(bags));
    }
}

/// Determine if the configuration odds hit the lottery.
fn config_hits_lottery((wins, out_of): (u32, u32)) -> bool {
    rand::random_range(1..=out_of.max(1)) <= wins
}

/// Precognitive requests (Laravel Precognition) never touch the session:
/// the `precognitive` middleware marks them while they're handled.
fn is_precognitive(request: &Request) -> bool {
    request.is_precognitive()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::NullSessionHandler;
    use illuminate_support::{MessageBag, json};

    fn store() -> Store {
        Store::new("session", Arc::new(NullSessionHandler), None)
    }

    #[test]
    fn response_data_is_flashed_to_the_session() {
        let session = store();
        let mut response = Response::redirect("/form")
            .with("status", "Saved!")
            .with_input(json!({"name": "Taylor"}))
            .with_errors(MessageBag::from([(
                "email",
                "The email field is required.",
            )]));

        apply_response_session_data(&session, &mut response);

        assert_eq!(session.get("status"), json!("Saved!"));
        assert_eq!(session.get_old_input("name"), json!("Taylor"));
        assert_eq!(
            session.get("errors"),
            json!({"default": {"email": ["The email field is required."]}})
        );
        assert_eq!(
            session.get("_flash.new"),
            json!(["status", "_old_input", "errors"])
        );
        assert!(
            response.flashed().is_empty(),
            "the data was taken from the response"
        );
    }

    #[test]
    fn error_bags_are_merged() {
        let session = store();
        session.flash("errors", json!({"login": {"email": ["Wrong."]}}));
        let mut response = Response::redirect("/")
            .with_errors_in(MessageBag::from([("name", "Taken.")]), "register");

        apply_response_session_data(&session, &mut response);

        assert_eq!(
            session.get("errors"),
            json!({"login": {"email": ["Wrong."]}, "register": {"name": ["Taken."]}})
        );
    }

    #[test]
    fn the_lottery_respects_the_odds() {
        assert!(config_hits_lottery((1, 1)));
        assert!(!config_hits_lottery((0, 100)));
        assert!(!config_hits_lottery((0, 0)));
    }

    #[test]
    fn precognitive_requests_are_detected() {
        let request = Request::create("/", "POST");
        request.set_header("precognition", "true");
        // The header alone doesn't make the request precognitive: the
        // route's `precognitive` middleware does.
        assert!(!is_precognitive(&request));
        request.set_attribute("precognitive", true);
        assert!(is_precognitive(&request));
    }
}
