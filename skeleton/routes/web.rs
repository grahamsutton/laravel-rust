use laravel::prelude::*;

/// The application's web routes.
pub fn web() {
    Route::get("/", || async { view("welcome", ()) });
}
