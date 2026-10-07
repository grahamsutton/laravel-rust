//! Authentication and authorization: middleware redirects, named routes,
//! and rendering `AuthenticationException` / `AuthorizationException`.

use illuminate_auth::{
    Auth, AuthenticationException, Authenticate, AuthorizationException, RedirectIfAuthenticated,
};
use illuminate_container::try_app;
use illuminate_http::current_request;

use crate::configuration::Middleware;
use crate::exceptions::{Handler, Prepared};

/// Wire the auth component into routing and the exception handler.
pub fn boot() {
    // `route('login')`, `route('verification.notice')`, ... resolve through the router.
    Auth::resolve_routes_using(|name| {
        illuminate_routing::Route::has(name)
            .then(|| illuminate_routing::route(name, ()).ok())
            .flatten()
    });

    // `$middleware->redirectGuestsTo('/login')` and `redirectUsersTo('/dashboard')`.
    if let Some(middleware) = try_app::<Middleware>() {
        if let Some(guests) = middleware.redirect_guests_to.clone() {
            let to = guests.clone();
            Authenticate::redirect_using(move |_| Some(to.clone()));
            AuthenticationException::redirect_using(move |_| Some(guests.clone()));
        }
        if let Some(users) = middleware.redirect_users_to.clone() {
            RedirectIfAuthenticated::redirect_using(move |_| users.clone());
        }
    }

    let Some(handler) = try_app::<Handler>() else {
        return;
    };
    handler.configure(|exceptions| {
        exceptions.prepare_using(|error| {
            error.downcast_ref::<AuthenticationException>().map(|e| Prepared::Unauthenticated {
                message: e.to_string(),
                redirect_to: current_request().and_then(|request| e.redirect_to(&request)),
            })
        });
        exceptions.prepare_using(|error| {
            error.downcast_ref::<AuthorizationException>().map(|e| {
                let http = e.to_http_exception();
                Prepared::Http {
                    status: http.status,
                    message: http.message(),
                    headers: http.headers,
                }
            })
        });
    });
}
