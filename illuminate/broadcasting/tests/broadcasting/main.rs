//! Integration tests for the broadcasting component: drivers talking to
//! faked HTTP APIs, events flowing through the dispatcher and the queue,
//! and channel authorization through the router.

mod common;
mod events;

mod authorization;
mod dispatching;
mod drivers;
mod pusher;
mod routes;
