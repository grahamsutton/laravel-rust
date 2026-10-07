//! The session-aware methods of the `Redirect` facade: `Redirect::guest()`
//! and `Redirect::intended()`.

use illuminate_http::{Response, current_request};
use illuminate_routing::Redirect;

use crate::helpers::try_session;
use crate::request::RequestSessionExt;

/// Adds Laravel's session-aware redirects to the routing component's
/// [`Redirect`] facade. Bring the trait into scope (it's in this crate's
/// prelude) and call them like any other facade method:
///
/// ```ignore
/// use illuminate_session::RedirectSessionExt;
///
/// return Redirect::guest("/login");        // remember where they were going
/// return Redirect::intended("/dashboard"); // ...and send them there after login
/// ```
pub trait RedirectSessionExt {
    /// Redirect to the given path (typically the login page), remembering
    /// the current URL — or the previous one, for non-`GET` or JSON
    /// requests — as the "intended" URL.
    fn guest(path: &str) -> Response;

    /// Redirect to the "intended" URL remembered by [`guest`](Self::guest),
    /// or to the default when there is none.
    fn intended(default: &str) -> Response;

    /// Remember the given URL as the "intended" one.
    fn set_intended_url(url: &str);

    /// The "intended" URL, if one is remembered.
    fn get_intended_url() -> Option<String>;
}

impl RedirectSessionExt for Redirect {
    fn guest(path: &str) -> Response {
        if let Some(request) = current_request() {
            let intended = if request.is_method("GET") && !request.expects_json() {
                Some(request.full_url())
            } else {
                request.header("referer").or_else(|| {
                    request
                        .try_session()
                        .and_then(|session| session.previous_url())
                })
            };
            if let (Some(intended), Some(session)) = (intended, request.try_session()) {
                session.put("url.intended", intended);
            }
        }
        Redirect::to(path)
    }

    fn intended(default: &str) -> Response {
        let intended = try_session()
            .map(|session| session.pull("url.intended"))
            .and_then(|url| url.as_str().map(str::to_string));
        Redirect::to(intended.as_deref().unwrap_or(default))
    }

    fn set_intended_url(url: &str) {
        if let Some(session) = try_session() {
            session.put("url.intended", url);
        }
    }

    fn get_intended_url() -> Option<String> {
        try_session().and_then(|session| session.get("url.intended").as_str().map(str::to_string))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use illuminate_container::Container;
    use illuminate_http::{Request, with_request};
    use illuminate_support::json;

    use super::*;
    use crate::handlers::NullSessionHandler;
    use crate::store::Store;

    fn request(uri: &str, method: &str) -> Request {
        let request = Request::create(uri, method);
        request.set_session(Arc::new(Store::new(
            "session",
            Arc::new(NullSessionHandler),
            None,
        )));
        request
    }

    #[tokio::test]
    async fn guests_are_redirected_and_sent_back_later() {
        let _guard = Container::set_local_instance(Arc::new(Container::new()));
        let request = request("http://localhost/dashboard?tab=1", "GET");
        let session = request.session();

        let response = with_request(request.clone(), async { Redirect::guest("/login") }).await;
        assert_eq!(response.target_url().unwrap(), "http://localhost/login");
        assert_eq!(
            session.get("url.intended"),
            json!("http://localhost/dashboard?tab=1")
        );

        let response = with_request(request.clone(), async {
            assert_eq!(
                Redirect::get_intended_url().as_deref(),
                Some("http://localhost/dashboard?tab=1")
            );
            Redirect::intended("/home")
        })
        .await;
        assert_eq!(
            response.target_url().unwrap(),
            "http://localhost/dashboard?tab=1"
        );

        // The intended URL is pulled from the session: next time it's the default.
        let response = with_request(request.clone(), async { Redirect::intended("/home") }).await;
        assert_eq!(response.target_url().unwrap(), "http://localhost/home");

        with_request(request.clone(), async {
            Redirect::set_intended_url("/billing")
        })
        .await;
        assert_eq!(session.get("url.intended"), json!("/billing"));
    }

    #[tokio::test]
    async fn non_get_requests_remember_the_previous_url() {
        let _guard = Container::set_local_instance(Arc::new(Container::new()));
        let request = request("http://localhost/comments", "POST");
        request.set_header("referer", "http://localhost/posts/1");
        with_request(request.clone(), async { Redirect::guest("/login") }).await;
        assert_eq!(
            request.session().get("url.intended"),
            json!("http://localhost/posts/1")
        );
    }
}
