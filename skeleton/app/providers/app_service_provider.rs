use laravel::prelude::*;

pub struct AppServiceProvider;

impl ServiceProvider for AppServiceProvider {
    /// Register any application services.
    fn register(&self, _app: &Container) {
        //
    }

    /// Bootstrap any application services.
    fn boot(&self, _app: &Container) {
        //
    }
}
