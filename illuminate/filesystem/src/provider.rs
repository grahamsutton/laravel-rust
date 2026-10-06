//! The filesystem service provider.

use std::sync::Arc;

use illuminate_config::Repository as Config;
use illuminate_container::{Container, ServiceProvider};

use crate::filesystem::Filesystem;
use crate::manager::FilesystemManager;

/// Registers the local [`Filesystem`] (the `File` facade) and the
/// [`FilesystemManager`] (the `Storage` facade).
///
/// Configuration is read from the `filesystems` key of the configuration
/// repository each time a disk is first resolved.
pub struct FilesystemServiceProvider;

pub(crate) fn make_manager(container: &Container) -> Arc<FilesystemManager> {
    let config = container
        .try_make::<Config>()
        .unwrap_or_else(|_| Arc::new(Config::empty()));
    Arc::new(FilesystemManager::new(config))
}

impl ServiceProvider for FilesystemServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<Filesystem>(|_| Arc::new(Filesystem::new()));
        app.singleton::<FilesystemManager>(make_manager);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn it_registers_the_filesystem_services() {
        let container = Container::new();
        container.instance(Config::new(json!({"filesystems": {"default": "local", "disks": {"local": {"driver": "local", "root": "/tmp"}}}})));
        FilesystemServiceProvider.register(&container);

        assert!(container.bound::<Filesystem>());
        let manager = container.make::<FilesystemManager>();
        assert!(Arc::ptr_eq(
            &manager,
            &container.make::<FilesystemManager>()
        ));
        assert_eq!(manager.default_disk().unwrap().name(), "local");
        assert_eq!(
            FilesystemServiceProvider.name(),
            "FilesystemServiceProvider"
        );
    }
}
