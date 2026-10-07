use laravel::prelude::*;

/// The application's service providers.
pub fn providers() -> Vec<Box<dyn ServiceProvider>> {
    vec![
        Box::new(crate::app::providers::AppServiceProvider),
    ]
}
