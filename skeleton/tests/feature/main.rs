//! Feature tests: make requests to your application and assert on the
//! responses. Create one with `cargo artisan make:test UserTest`.

mod example_test;

use laravel::testing::TestApp;

/// Create the application for a test.
pub fn app() -> TestApp {
    TestApp::new(example_app::bootstrap::app())
}
