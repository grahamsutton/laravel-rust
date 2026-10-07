//! Testing helpers: make requests to your application and assert on the
//! responses, just like Laravel's `TestCase`.

mod auth;
mod content;
mod database;
pub mod fluent;
pub mod json;
mod test_app;
pub mod test_response;
mod test_view;
mod time;
mod views;

pub use fluent::{AssertableJson, JsonTypes};
pub use test_app::{TestApp, encrypt_cookie};
pub use test_response::{TestResponse, decrypt_cookie};
pub use test_view::{TestComponent, TestView};
