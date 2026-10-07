//! The image driver contract (Laravel's `Illuminate\Contracts\Image\Driver`).

use illuminate_support::Result;

use crate::exception::ImageException;
use crate::pipeline::ImagePipeline;
use crate::transformations::AnyTransformationHandler;

/// An image driver: decodes image contents, applies an [`ImagePipeline`],
/// and encodes the result.
///
/// The framework ships with [`NativeDriver`](crate::NativeDriver), a pure
/// Rust driver. You may register your own with
/// [`Image::extend`](crate::Image::extend):
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_image::{Image, ImageDriver, ImagePipeline};
/// use illuminate_support::Result;
///
/// struct VipsDriver;
///
/// impl ImageDriver for VipsDriver {
///     fn process(&self, contents: &[u8], pipeline: &ImagePipeline) -> Result<Vec<u8>> {
///         // Apply the pipeline's transformations and output options...
///         Ok(contents.to_vec())
///     }
///
///     fn dimensions(&self, contents: &[u8]) -> Result<(u32, u32)> {
///         // Read the image's width and height...
///         Ok((0, 0))
///     }
///
///     fn dominant_color(&self, contents: &[u8]) -> Result<String> {
///         // Calculate the image's average color...
///         Ok("#000000".into())
///     }
/// }
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container);
///
/// Image::extend("vips", |_app| Arc::new(VipsDriver));
///
/// assert!(Image::driver(Some("vips")).is_ok());
/// ```
pub trait ImageDriver: Send + Sync + 'static {
    /// Process the given image contents with the specified pipeline,
    /// returning the encoded image.
    fn process(&self, contents: &[u8], pipeline: &ImagePipeline) -> Result<Vec<u8>>;

    /// Get the dimensions (width, height) of the given image contents.
    fn dimensions(&self, contents: &[u8]) -> Result<(u32, u32)>;

    /// Get the dominant (average) color of the image as a hex string (`#rrggbb`).
    fn dominant_color(&self, contents: &[u8]) -> Result<String>;

    /// Register a handler for a custom transformation.
    ///
    /// Drivers that support custom transformations keep a
    /// [`TransformationHandlers`](crate::TransformationHandlers) registry
    /// for their native image type and register the handler with it. By
    /// default, drivers don't support custom transformations.
    fn transform_using(&self, handler: AnyTransformationHandler) -> Result<()> {
        Err(ImageException::new(format!(
            "The image driver does not support custom transformations such as [{}].",
            handler.transformation_name()
        ))
        .into())
    }
}
