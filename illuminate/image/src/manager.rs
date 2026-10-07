//! The image manager: resolves and caches the image drivers.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use illuminate_config::Repository as Config;
use illuminate_container::Container;
use illuminate_support::Result;
use illuminate_support::error::InvalidArgumentException;

use crate::driver::ImageDriver;
use crate::drivers::NativeDriver;
use crate::transformations::{AnyTransformationHandler, Transformation};

/// Creates a custom image driver.
pub type DriverCreator = Arc<dyn Fn(&Container) -> Arc<dyn ImageDriver> + Send + Sync>;

/// Resolves the image drivers — the built-in `image` driver (also known as
/// `gd` and `imagick`) and any registered with [`ImageManager::extend`] —
/// and the default one from `images.default`.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_image::ImageManager;
/// use illuminate_support::json;
///
/// let manager = ImageManager::new(Arc::new(Repository::new(json!({
///     "images": {"default": "gd"},
/// }))));
///
/// assert_eq!(manager.get_default_driver(), "gd");
///
/// // "gd" and "imagick" are aliases of the built-in driver...
/// let default = manager.driver(None).unwrap();
/// assert!(Arc::ptr_eq(&default, &manager.driver(Some("image")).unwrap()));
/// assert!(Arc::ptr_eq(&default, &manager.driver(Some("imagick")).unwrap()));
///
/// assert_eq!(
///     manager.driver(Some("vips")).err().unwrap().to_string(),
///     "Image driver [vips] is not supported.",
/// );
/// ```
pub struct ImageManager {
    config: Arc<Config>,
    drivers: RwLock<HashMap<String, Arc<dyn ImageDriver>>>,
    custom_creators: RwLock<HashMap<String, DriverCreator>>,
    handlers: RwLock<Vec<(String, AnyTransformationHandler)>>,
}

impl ImageManager {
    /// The driver names that map to the built-in driver, so configuration
    /// written for Laravel keeps working.
    pub const ALIASES: [&'static str; 2] = ["gd", "imagick"];

    /// Create a new image manager reading the given configuration.
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            drivers: RwLock::new(HashMap::new()),
            custom_creators: RwLock::new(HashMap::new()),
            handlers: RwLock::new(Vec::new()),
        }
    }

    /// The default driver name (`images.default`, falling back to `image`).
    pub fn get_default_driver(&self) -> String {
        self.config.string_or("images.default", NativeDriver::NAME)
    }

    /// Get a driver by name — the default driver when `None`.
    ///
    /// Drivers are created once and cached. Unknown drivers fail with an
    /// `InvalidArgumentException` (`Image driver [name] is not supported.`).
    pub fn driver(&self, name: Option<&str>) -> Result<Arc<dyn ImageDriver>> {
        let name = name
            .map(str::to_string)
            .unwrap_or_else(|| self.get_default_driver());
        let key = self.canonical(&name);

        if let Some(driver) = self.drivers.read().unwrap().get(&key) {
            return Ok(driver.clone());
        }

        let driver = self.create_driver(&name, &key)?;
        Ok(self
            .drivers
            .write()
            .unwrap()
            .entry(key)
            .or_insert(driver)
            .clone())
    }

    /// Get the default driver.
    pub fn default_driver(&self) -> Result<Arc<dyn ImageDriver>> {
        self.driver(None)
    }

    /// Create an instance of the built-in driver.
    pub fn create_native_driver(&self) -> NativeDriver {
        NativeDriver::new()
    }

    /// Register a custom driver creator. The creator receives the container.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_config::Repository;
    /// use illuminate_image::{ImageManager, NativeDriver};
    ///
    /// let manager = ImageManager::new(Arc::new(Repository::empty()));
    /// manager.extend("vips", |_app| Arc::new(NativeDriver::new()));
    ///
    /// assert!(manager.driver(Some("vips")).is_ok());
    /// ```
    pub fn extend(
        &self,
        driver: impl Into<String>,
        creator: impl Fn(&Container) -> Arc<dyn ImageDriver> + Send + Sync + 'static,
    ) -> &Self {
        let driver = driver.into();
        self.drivers.write().unwrap().remove(&driver);
        self.custom_creators
            .write()
            .unwrap()
            .insert(driver, Arc::new(creator));
        self
    }

    /// Register a handler applying a custom transformation with the given
    /// driver. It's applied to the driver whether it has been resolved yet
    /// or not.
    pub fn transform_using<T, I>(
        &self,
        driver: &str,
        handler: impl Fn(I, &T) -> Result<I> + Send + Sync + 'static,
    ) -> &Self
    where
        T: Transformation,
        I: Send + 'static,
    {
        self.transform_using_handler(driver, AnyTransformationHandler::new(handler))
    }

    /// Register a type-erased transformation handler with the given driver.
    ///
    /// A handler the driver rejects (written for another image type, say)
    /// makes resolving the driver fail with the driver's error.
    pub fn transform_using_handler(
        &self,
        driver: &str,
        handler: AnyTransformationHandler,
    ) -> &Self {
        self.handlers
            .write()
            .unwrap()
            .push((driver.to_string(), handler.clone()));

        let key = self.canonical(driver);
        let resolved = self.drivers.read().unwrap().get(&key).cloned();
        if let Some(resolved) = resolved
            && resolved.transform_using(handler).is_err()
        {
            // Resolve it again — and report the problem — on next use.
            self.drivers.write().unwrap().remove(&key);
        }

        self
    }

    /// Forget every resolved driver instance.
    pub fn forget_drivers(&self) -> &Self {
        self.drivers.write().unwrap().clear();
        self
    }

    /// The names of the drivers resolved so far.
    pub fn get_drivers(&self) -> Vec<String> {
        let mut names: Vec<String> = self.drivers.read().unwrap().keys().cloned().collect();
        names.sort();
        names
    }

    /// The name a driver is cached under: aliases of the built-in driver
    /// share its instance, unless they've been extended themselves.
    fn canonical(&self, name: &str) -> String {
        if self.custom_creators.read().unwrap().contains_key(name) {
            return name.to_string();
        }
        if Self::ALIASES.contains(&name) {
            return NativeDriver::NAME.to_string();
        }
        name.to_string()
    }

    fn create_driver(&self, name: &str, key: &str) -> Result<Arc<dyn ImageDriver>> {
        let creator = self.custom_creators.read().unwrap().get(key).cloned();

        let driver: Arc<dyn ImageDriver> = match creator {
            Some(creator) => creator(&Container::get_instance()),
            None if key == NativeDriver::NAME => Arc::new(self.create_native_driver()),
            None => {
                return Err(InvalidArgumentException::new(format!(
                    "Image driver [{name}] is not supported."
                ))
                .into());
            }
        };

        self.apply_transformation_handlers(key, driver.as_ref())?;
        Ok(driver)
    }

    fn apply_transformation_handlers(&self, key: &str, driver: &dyn ImageDriver) -> Result<()> {
        let registered: Vec<(String, AnyTransformationHandler)> =
            self.handlers.read().unwrap().clone();

        for (name, handler) in registered {
            if self.canonical(&name) == key {
                driver.transform_using(handler)?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for ImageManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageManager")
            .field("default", &self.get_default_driver())
            .field("drivers", &self.get_drivers())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::pipeline::ImagePipeline;
    use illuminate_support::json;

    fn manager(config: illuminate_support::Value) -> ImageManager {
        ImageManager::new(Arc::new(Config::new(config)))
    }

    #[derive(Debug)]
    struct Marker;
    impl Transformation for Marker {}

    #[derive(Default)]
    struct RecordingDriver {
        handlers: Mutex<Vec<&'static str>>,
    }

    impl ImageDriver for RecordingDriver {
        fn process(&self, contents: &[u8], _: &ImagePipeline) -> Result<Vec<u8>> {
            Ok(contents.to_vec())
        }
        fn dimensions(&self, _: &[u8]) -> Result<(u32, u32)> {
            Ok((0, 0))
        }
        fn dominant_color(&self, _: &[u8]) -> Result<String> {
            Ok("#000000".into())
        }
        fn transform_using(&self, handler: AnyTransformationHandler) -> Result<()> {
            self.handlers
                .lock()
                .unwrap()
                .push(handler.transformation_name());
            Ok(())
        }
    }

    #[test]
    fn the_default_driver_comes_from_configuration() {
        assert_eq!(
            manager(json!({"images": {"default": "imagick"}})).get_default_driver(),
            "imagick"
        );
        assert_eq!(manager(json!({})).get_default_driver(), "image");
    }

    #[test]
    fn drivers_are_cached_and_aliases_share_the_built_in_driver() {
        let manager = manager(json!({}));
        let image = manager.driver(Some("image")).unwrap();

        assert!(Arc::ptr_eq(&image, &manager.driver(Some("gd")).unwrap()));
        assert!(Arc::ptr_eq(&image, &manager.default_driver().unwrap()));
        assert_eq!(manager.get_drivers(), vec!["image"]);

        manager.forget_drivers();
        assert!(manager.get_drivers().is_empty());
        assert!(!Arc::ptr_eq(
            &image,
            &manager.driver(Some("image")).unwrap()
        ));
    }

    #[test]
    fn unknown_drivers_are_not_supported() {
        let error = manager(json!({"images": {"default": "nonexistent"}}))
            .default_driver()
            .err()
            .unwrap();

        assert_eq!(
            error.to_string(),
            "Image driver [nonexistent] is not supported."
        );
        assert!(error.is::<InvalidArgumentException>());
    }

    #[test]
    fn custom_drivers_can_be_registered_and_replaced() {
        let manager = manager(json!({}));
        let first: Arc<dyn ImageDriver> = Arc::new(RecordingDriver::default());
        let second: Arc<dyn ImageDriver> = Arc::new(RecordingDriver::default());

        let driver = first.clone();
        manager.extend("custom", move |_| driver.clone());
        assert!(Arc::ptr_eq(
            &manager.driver(Some("custom")).unwrap(),
            &first
        ));

        let driver = second.clone();
        manager.extend("custom", move |_| driver.clone());
        assert!(Arc::ptr_eq(
            &manager.driver(Some("custom")).unwrap(),
            &second
        ));

        // Extending an alias takes it over...
        let driver = first.clone();
        manager.extend("gd", move |_| driver.clone());
        assert!(Arc::ptr_eq(&manager.driver(Some("gd")).unwrap(), &first));
        assert!(!Arc::ptr_eq(
            &manager.driver(Some("image")).unwrap(),
            &first
        ));
    }

    #[test]
    fn transformation_handlers_are_applied_to_new_and_resolved_drivers() {
        let manager = manager(json!({}));
        let created = Arc::new(RecordingDriver::default());
        let driver = created.clone();
        manager.extend("custom", move |_| driver.clone());

        manager.transform_using("custom", |image: Vec<u8>, _: &Marker| Ok(image));
        manager.driver(Some("custom")).unwrap();
        assert_eq!(created.handlers.lock().unwrap().len(), 1);

        manager.transform_using("custom", |image: Vec<u8>, _: &Marker| Ok(image));
        assert_eq!(created.handlers.lock().unwrap().len(), 2);
        assert!(created.handlers.lock().unwrap()[0].ends_with("Marker"));
    }

    #[test]
    fn handlers_for_aliases_reach_the_built_in_driver() {
        let manager = manager(json!({}));
        manager.driver(Some("image")).unwrap();

        // A handler for the wrong image type is rejected by the built-in driver...
        manager.transform_using("gd", |image: String, _: &Marker| Ok(image));
        let error = manager.driver(Some("image")).err().unwrap();
        assert!(
            error
                .to_string()
                .contains("transformation handler works with")
        );
    }
}
