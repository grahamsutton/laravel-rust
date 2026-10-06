//! The view service provider.

use std::path::PathBuf;
use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};

use crate::compiler::BladeCompiler;
use crate::factory::Factory;

/// Registers the view [`Factory`] (configured from `view.paths`) and its
/// [`BladeCompiler`].
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_support::json;
/// use illuminate_view::{Factory, ViewServiceProvider};
///
/// let dir = tempfile::tempdir().unwrap();
/// let app = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(app.clone());
/// app.instance(Repository::new(json!({"view": {"paths": [dir.path()]}})));
///
/// ViewServiceProvider.register(&app);
///
/// assert_eq!(app.make::<Factory>().finder().paths().len(), 1);
/// ```
pub struct ViewServiceProvider;

impl ServiceProvider for ViewServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<Factory>(|app| {
            let factory = match app.try_make::<Repository>() {
                Ok(config) => Factory::from_config(&config),
                Err(_) => Factory::new(Vec::<PathBuf>::new()),
            };
            Arc::new(factory)
        });
        app.bind::<BladeCompiler>(|app| Arc::new(app.make::<Factory>().blade().clone()));
    }
}
