//! The image service provider.

use std::sync::Arc;

use illuminate_config::Repository as Config;
use illuminate_container::{Container, ServiceProvider};

use crate::manager::ImageManager;

/// Registers the [`ImageManager`] behind the `Image` facade.
///
/// The default driver is read from the `images.default` configuration
/// option whenever a driver is resolved.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_image::{Image, ImageManager, ImageServiceProvider};
/// use illuminate_support::json;
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({"images": {"default": "gd"}})));
///
/// ImageServiceProvider.register(&container);
///
/// assert!(container.bound::<ImageManager>());
/// assert_eq!(Image::get_default_driver(), "gd");
/// ```
pub struct ImageServiceProvider;

pub(crate) fn make_manager(container: &Container) -> Arc<ImageManager> {
    let config = container
        .try_make::<Config>()
        .unwrap_or_else(|_| Arc::new(Config::empty()));
    Arc::new(ImageManager::new(config))
}

impl ServiceProvider for ImageServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<ImageManager>(make_manager);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn it_registers_a_shared_image_manager() {
        let container = Container::new();
        container.instance(Config::new(json!({"images": {"default": "imagick"}})));
        ImageServiceProvider.register(&container);

        let manager = container.make::<ImageManager>();
        assert!(Arc::ptr_eq(&manager, &container.make::<ImageManager>()));
        assert_eq!(manager.get_default_driver(), "imagick");
        assert_eq!(ImageServiceProvider.name(), "ImageServiceProvider");
    }
}
