//! Laravel Sanctum, end to end: tokens issued by Eloquent users, requests
//! authenticated through the router with `auth:sanctum`, SPA sessions, the
//! ability middleware, `Sanctum::acting_as`, and `sanctum:prune-expired`.

mod support;

mod abilities;
mod console;
mod guard;
mod provider;
mod stateful;
mod testing;
mod tokens;
