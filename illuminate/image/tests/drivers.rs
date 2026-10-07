mod common;

use std::sync::{Arc, Mutex};

use common::{app, png};
use illuminate_container::Container;
use illuminate_image::image::{DynamicImage, GenericImageView, Rgba, imageops::FilterType};
use illuminate_image::{
    AnyTransformationHandler, Image, ImageDriver, ImageException, ImageManager, ImagePipeline,
    NativeDriver, Transformation, TransformationHandlers,
};
use illuminate_support::Result;
use illuminate_support::error::InvalidArgumentException;

/// A pretend driver that records what it was asked to do.
#[derive(Default)]
struct VipsDriver {
    processed: Mutex<Vec<usize>>,
    handlers: TransformationHandlers<Vec<String>>,
}

impl ImageDriver for VipsDriver {
    fn process(&self, contents: &[u8], pipeline: &ImagePipeline) -> Result<Vec<u8>> {
        self.processed
            .lock()
            .unwrap()
            .push(pipeline.transformations.len());

        let mut log = Vec::new();
        for transformation in &pipeline.transformations {
            match self.handlers.handler_for(transformation.as_ref()) {
                Some(handler) => log = handler.handle(log, transformation.as_ref())?,
                None => log.push(transformation.name().to_string()),
            }
        }
        if log.iter().any(|entry| entry == "explode") {
            return Err(ImageException::new("Vips exploded.").into());
        }
        Ok(contents.to_vec())
    }

    fn dimensions(&self, _: &[u8]) -> Result<(u32, u32)> {
        Ok((7, 7))
    }

    fn dominant_color(&self, _: &[u8]) -> Result<String> {
        Ok("#123456".into())
    }

    fn transform_using(&self, handler: AnyTransformationHandler) -> Result<()> {
        Ok(self.handlers.register(&handler)?)
    }
}

#[derive(Debug)]
struct Pixelate {
    size: u32,
}

impl Transformation for Pixelate {}

#[derive(Debug)]
struct Explode;

impl Transformation for Explode {}

#[tokio::test]
async fn the_default_driver_is_configurable() {
    let (container, _guard) = app();

    assert_eq!(Image::get_default_driver(), "gd");
    assert!(Arc::ptr_eq(
        &Image::driver(None).unwrap(),
        &container
            .make::<ImageManager>()
            .driver(Some("image"))
            .unwrap()
    ));
    assert_eq!(Image::manager().get_drivers(), vec!["image"]);
}

#[tokio::test]
async fn the_facade_works_without_a_service_provider() {
    let _guard = Container::set_local_instance(Arc::new(Container::new()));

    assert_eq!(Image::get_default_driver(), "image");
    assert_eq!(
        Image::from_bytes(png(4, 4))
            .cover(2, 2)
            .dimensions()
            .await
            .unwrap(),
        (2, 2)
    );
}

#[tokio::test]
async fn custom_drivers_can_be_registered() {
    let _app = app();
    let vips = Arc::new(VipsDriver::default());
    let driver = vips.clone();
    Image::extend("vips", move |_app| driver.clone());

    let image = Image::from_bytes(png(10, 10))
        .using("vips")
        .cover(5, 5)
        .blur(3);
    assert_eq!(image.driver_name(), Some("vips"));

    assert_eq!(image.to_bytes().await.unwrap(), png(10, 10));
    assert_eq!(image.dominant_color().await.unwrap(), "#123456");
    assert_eq!(*vips.processed.lock().unwrap(), vec![2]);

    // The native reader measures what it can, and the driver the rest...
    assert_eq!(image.dimensions().await.unwrap(), (10, 10));
    assert_eq!(
        Image::from_bytes("???")
            .using("vips")
            .to_avif()
            .dimensions()
            .await
            .unwrap(),
        (7, 7)
    );
}

#[tokio::test]
async fn custom_drivers_can_be_the_default() {
    let (container, _guard) = app();
    container.instance(illuminate_config::Repository::new(
        illuminate_support::json!({"images": {"default": "vips"}}),
    ));
    container.forget_instance::<ImageManager>();
    let vips = Arc::new(VipsDriver::default());
    let driver = vips.clone();
    Image::extend("vips", move |_app| driver.clone());

    Image::from_bytes(png(10, 10))
        .grayscale()
        .to_bytes()
        .await
        .unwrap();

    assert_eq!(Image::get_default_driver(), "vips");
    assert_eq!(*vips.processed.lock().unwrap(), vec![1]);
}

#[tokio::test]
async fn gd_and_imagick_are_aliases_of_the_built_in_driver() {
    let _app = app();
    let image = Image::from_bytes(png(20, 10));

    for variant in [
        image.using_gd(),
        image.using_imagick(),
        image.using("image"),
    ] {
        assert_eq!(variant.cover(5, 5).dimensions().await.unwrap(), (5, 5));
    }
    assert_eq!(image.using_gd().driver_name(), Some("gd"));
    assert_eq!(image.using_imagick().driver_name(), Some("imagick"));
    assert_eq!(image.driver_name(), None);
}

#[tokio::test]
async fn unknown_drivers_fail_when_the_image_is_processed() {
    let _app = app();
    let image = Image::from_bytes(png(10, 10)).using("nonexistent");

    // Without changes, there's nothing to process...
    assert!(image.to_bytes().await.is_ok());

    let error = image.cover(5, 5).to_bytes().await.unwrap_err();
    let exception = error.downcast_ref::<ImageException>().unwrap();
    assert_eq!(
        exception.to_string(),
        "Failed to process image: Image driver [nonexistent] is not supported."
    );
    assert_eq!(
        exception.previous().unwrap().to_string(),
        "Image driver [nonexistent] is not supported."
    );

    let error = Image::driver(Some("nonexistent")).err().unwrap();
    assert!(error.is::<InvalidArgumentException>());
}

#[tokio::test]
async fn image_exceptions_from_drivers_are_not_wrapped() {
    let _app = app();
    Image::extend("vips", |_app| Arc::new(VipsDriver::default()));
    Image::transform_using("vips", |mut log: Vec<String>, _: &Explode| {
        log.push("explode".into());
        Ok(log)
    });

    let error = Image::from_bytes(png(2, 2))
        .using("vips")
        .transform(Explode)
        .to_bytes()
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "Vips exploded.");
}

#[tokio::test]
async fn custom_transformations_run_on_the_built_in_driver() {
    let _app = app();
    let received = Arc::new(Mutex::new(None));
    let seen = received.clone();

    Image::transform_using("gd", move |image: DynamicImage, pixelate: &Pixelate| {
        *seen.lock().unwrap() = Some(pixelate.size);
        let (width, height) = image.dimensions();
        Ok(image
            .resize_exact(
                width / pixelate.size,
                height / pixelate.size,
                FilterType::Nearest,
            )
            .resize_exact(width, height, FilterType::Nearest))
    });

    let image = Image::from_bytes(png(48, 48)).transform(Pixelate { size: 12 });
    let pixelated =
        illuminate_image::image::load_from_memory(&image.to_bytes().await.unwrap()).unwrap();

    assert_eq!(*received.lock().unwrap(), Some(12));
    assert_eq!(pixelated.dimensions(), (48, 48));
    // Each 12x12 block is a single color now...
    assert_eq!(pixelated.get_pixel(0, 0), pixelated.get_pixel(11, 11));
    assert_ne!(pixelated.get_pixel(0, 0), pixelated.get_pixel(47, 47));
}

#[tokio::test]
async fn custom_transformations_registered_after_resolution_still_apply() {
    let _app = app();
    Image::driver(None).unwrap();

    Image::transform_using("image", |_: DynamicImage, _: &Pixelate| {
        Ok(DynamicImage::ImageRgba8(
            illuminate_image::image::RgbaImage::from_pixel(3, 3, Rgba([1, 2, 3, 255])),
        ))
    });

    let image = Image::from_bytes(png(10, 10)).transform(Pixelate { size: 2 });
    assert_eq!(image.dimensions().await.unwrap(), (3, 3));
    assert_eq!(image.dominant_color().await.unwrap(), "#010203");
}

#[tokio::test]
async fn custom_transformations_without_a_handler_are_not_supported() {
    let _app = app();
    let error = Image::from_bytes(png(10, 10))
        .transform(Pixelate { size: 2 })
        .to_bytes()
        .await
        .unwrap_err();

    assert!(error.is::<ImageException>());
    assert!(error.to_string().starts_with("The image transformation ["));
    assert!(error.to_string().ends_with("Pixelate] is not supported."));
}

#[tokio::test]
async fn custom_transformations_run_on_custom_drivers() {
    let _app = app();
    let vips = Arc::new(VipsDriver::default());
    let driver = vips.clone();
    Image::extend("vips", move |_app| driver.clone());
    Image::transform_using("vips", |mut log: Vec<String>, pixelate: &Pixelate| {
        log.push(format!("pixelate:{}", pixelate.size));
        Ok(log)
    });

    Image::from_bytes(png(2, 2))
        .using("vips")
        .transform(Pixelate { size: 4 })
        .to_bytes()
        .await
        .unwrap();

    assert!(vips.handlers.has::<Pixelate>());
    assert!(!vips.handlers.has::<Explode>());
}

#[tokio::test]
async fn handlers_for_the_wrong_image_type_are_reported() {
    let _app = app();
    Image::transform_using("image", |log: Vec<String>, _: &Pixelate| Ok(log));

    let error = Image::from_bytes(png(2, 2))
        .blur(1)
        .to_bytes()
        .await
        .unwrap_err();
    assert!(
        error.to_string().starts_with("Failed to process image:")
            || error.to_string().contains("works with")
    );
}

#[tokio::test]
async fn drivers_without_transformation_support_reject_handlers() {
    struct Minimal;
    impl ImageDriver for Minimal {
        fn process(&self, contents: &[u8], _: &ImagePipeline) -> Result<Vec<u8>> {
            Ok(contents.to_vec())
        }
        fn dimensions(&self, _: &[u8]) -> Result<(u32, u32)> {
            Ok((1, 1))
        }
        fn dominant_color(&self, _: &[u8]) -> Result<String> {
            Ok("#000000".into())
        }
    }

    let error = Minimal
        .transform_using(AnyTransformationHandler::new(
            |image: Vec<u8>, _: &Pixelate| Ok(image),
        ))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("The image driver does not support custom transformations")
    );

    let native = NativeDriver::new();
    assert!(
        native
            .transform_using(AnyTransformationHandler::new(
                |image: DynamicImage, _: &Pixelate| Ok(image)
            ))
            .is_ok()
    );
}

#[tokio::test]
async fn the_default_configuration_matches_laravel() {
    let config = illuminate_image::config();

    assert!(config["default"].is_string());
    assert_eq!(config.as_object().unwrap().len(), 1);
}
