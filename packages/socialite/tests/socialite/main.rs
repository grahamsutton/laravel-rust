//! Laravel Socialite, end to end: redirecting to every provider, checking
//! the state, exchanging codes for tokens, mapping users, custom drivers,
//! and the testing fakes.

mod support;

mod auth_urls;
mod drivers;
mod fakes;
mod routing;
mod state;
mod tokens;
mod users;
