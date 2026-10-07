use laravel::prelude::*;

use crate::{config, database, routes};

/// Create the application.
pub fn app() -> ApplicationBuilder {
    Application::configure(laravel::base_path!())
        .with_config(config::all())
        .with_providers(super::providers())
        .with_routing(|routing| {
            routing
                .web(routes::web)
                .commands(routes::console)
                .health("/up");
        })
        .with_middleware(|_middleware| {
            //
        })
        .with_exceptions(|exceptions| {
            exceptions.should_render_json_when(|request, _| request.is("api/*") || request.expects_json());
        })
        .with_commands(super::commands::all())
        .with_migrations(database::migrations::all())
        .with_seeders(database::seeders::register)
}
