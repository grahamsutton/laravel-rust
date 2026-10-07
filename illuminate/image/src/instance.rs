//! The image: immutable, lazily loaded, and processed only when needed.

use std::fmt;
use std::io::Cursor;
use std::sync::{Arc, OnceLock};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;

use illuminate_container::Container;
use illuminate_filesystem::{FilesystemException, Storage, Visibility};
use illuminate_http::{IntoResponse, Response, UploadedFile, render_exception};
use illuminate_support::{Conditionable, Result, Str};

use crate::driver::ImageDriver;
use crate::exception::ImageException;
use crate::mime::{extension_for, mime_type_of};
use crate::pipeline::{ImageFormat, ImageOutputOptions, ImagePipeline};
use crate::source::Source;
use crate::transformations::{
    Blur, Contain, Cover, Crop, FlipHorizontally, FlipVertically, Grayscale, Orient, Resize,
    Rotate, Scale, Sharpen, Transformation,
};

/// An image, read from bytes, a file, a disk, an upload, or a URL.
///
/// Images are immutable: every transformation returns a new image with the
/// transformation appended to its [`ImagePipeline`], so one source can
/// produce many variants. Nothing is read until it's needed, and the
/// pipeline is applied — and the image encoded — only once, when you ask
/// for the result (`to_bytes`, `store`, `width`, ...).
///
/// ```
/// use illuminate_image::Image;
/// # use std::sync::Arc;
/// # use illuminate_container::Container;
/// # use illuminate_image::image::{DynamicImage, ImageFormat};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let _guard = Container::set_local_instance(Arc::new(Container::new()));
/// # let mut png = Vec::new();
/// # DynamicImage::new_rgb8(800, 600).write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png).unwrap();
/// let image = Image::from_bytes(png);
///
/// let avatar = image.cover(400, 400).to_webp().quality(80);
/// let thumbnail = image.scale_width(200);
///
/// assert_eq!(avatar.dimensions().await?, (400, 400));
/// assert_eq!(avatar.mime_type().await?, "image/webp");
/// assert_eq!(thumbnail.dimensions().await?, (200, 150));
/// assert_eq!(image.width().await?, 800);
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Clone)]
pub struct Image {
    source: Arc<Source>,
    pipeline: ImagePipeline,
    driver: Option<String>,
    file: Option<UploadedFile>,
    hash_name: OnceLock<String>,
    processed: OnceLock<Bytes>,
}

impl Image {
    pub(crate) fn with_source(source: Source, file: Option<UploadedFile>) -> Self {
        Self {
            source: Arc::new(source),
            pipeline: ImagePipeline::new(),
            driver: None,
            file,
            hash_name: OnceLock::new(),
            processed: OnceLock::new(),
        }
    }

    // ------------------------------------------------------------------
    // Resizing
    // ------------------------------------------------------------------

    /// Resize and crop the image to completely cover the given dimensions.
    pub fn cover(&self, width: u32, height: u32) -> Image {
        self.transform(Cover::new(width.max(1), height.max(1)))
    }

    /// Resize the image to fit within the given dimensions while preserving
    /// the entire image. Empty space is filled with white.
    pub fn contain(&self, width: u32, height: u32) -> Image {
        self.transform(Contain::new(width.max(1), height.max(1), None))
    }

    /// Resize the image to fit within the given dimensions, filling empty
    /// space with the given background: a color (`"#ffffff"`, `"rgb(0, 0, 0)"`,
    /// `"transparent"`, ...) or `"dominant"` for the image's dominant color.
    pub fn contain_with(&self, width: u32, height: u32, background: &str) -> Image {
        self.transform(Contain::new(
            width.max(1),
            height.max(1),
            Some(background.to_string()),
        ))
    }

    /// Crop the image to the given dimensions, starting at the top left corner.
    pub fn crop(&self, width: u32, height: u32) -> Image {
        self.crop_at(width, height, 0, 0)
    }

    /// Crop the image to the given dimensions, starting at the given `x` and
    /// `y` coordinates. Any area outside of the image is filled with white.
    pub fn crop_at(&self, width: u32, height: u32, x: i64, y: i64) -> Image {
        self.transform(Crop::new(width.max(1), height.max(1), x, y))
    }

    /// Resize the image to the given dimensions (ignoring its aspect ratio).
    pub fn resize(&self, width: u32, height: u32) -> Image {
        self.transform(Resize::new(Some(width.max(1)), Some(height.max(1))))
    }

    /// Resize the image to the given width, keeping its height.
    pub fn resize_width(&self, width: u32) -> Image {
        self.transform(Resize::new(Some(width.max(1)), None))
    }

    /// Resize the image to the given height, keeping its width.
    pub fn resize_height(&self, height: u32) -> Image {
        self.transform(Resize::new(None, Some(height.max(1))))
    }

    /// Resize the image to the given optional dimensions. At least one of
    /// them must be given.
    ///
    /// ```
    /// use illuminate_image::Image;
    ///
    /// let image = Image::from_bytes(Vec::new());
    ///
    /// assert!(image.resize_with(Some(800), None).is_ok());
    /// assert_eq!(
    ///     image.resize_with(None, None).unwrap_err().to_string(),
    ///     "At least one resize dimension must be specified.",
    /// );
    /// ```
    pub fn resize_with(&self, width: Option<u32>, height: Option<u32>) -> Result<Image> {
        if width.is_none() && height.is_none() {
            return Err(
                ImageException::new("At least one resize dimension must be specified.").into(),
            );
        }
        Ok(self.transform(Resize::new(
            width.map(|w| w.max(1)),
            height.map(|h| h.max(1)),
        )))
    }

    /// Proportionally scale the image down so it fits within the given
    /// dimensions. Scaling never increases the size of an image.
    pub fn scale(&self, width: u32, height: u32) -> Image {
        self.transform(Scale::new(Some(width.max(1)), Some(height.max(1))))
    }

    /// Proportionally scale the image down to the given width.
    pub fn scale_width(&self, width: u32) -> Image {
        self.transform(Scale::new(Some(width.max(1)), None))
    }

    /// Proportionally scale the image down to the given height.
    pub fn scale_height(&self, height: u32) -> Image {
        self.transform(Scale::new(None, Some(height.max(1))))
    }

    /// Scale the image down to the given optional dimensions. At least one
    /// of them must be given.
    pub fn scale_with(&self, width: Option<u32>, height: Option<u32>) -> Result<Image> {
        if width.is_none() && height.is_none() {
            return Err(
                ImageException::new("At least one scale dimension must be specified.").into(),
            );
        }
        Ok(self.transform(Scale::new(
            width.map(|w| w.max(1)),
            height.map(|h| h.max(1)),
        )))
    }

    // ------------------------------------------------------------------
    // Other transformations
    // ------------------------------------------------------------------

    /// Rotate the image according to its EXIF orientation data.
    pub fn orient(&self) -> Image {
        self.transform(Orient)
    }

    /// Rotate the image clockwise by the given angle, in degrees. Corners
    /// uncovered by the rotation are filled with white.
    pub fn rotate(&self, angle: impl Into<f64>) -> Image {
        self.transform(Rotate::new(angle.into(), None))
    }

    /// Rotate the image clockwise, filling uncovered corners with the given
    /// background color (or `"dominant"`).
    pub fn rotate_with(&self, angle: impl Into<f64>, background: &str) -> Image {
        self.transform(Rotate::new(angle.into(), Some(background.to_string())))
    }

    /// Blur the image. The amount is clamped between 0 and 100 (Laravel's
    /// default is 5).
    pub fn blur(&self, amount: i64) -> Image {
        self.transform(Blur::new(amount))
    }

    /// Convert the image to grayscale.
    pub fn grayscale(&self) -> Image {
        self.transform(Grayscale)
    }

    /// Sharpen the image. The amount is clamped between 0 and 100 (Laravel's
    /// default is 10).
    pub fn sharpen(&self, amount: i64) -> Image {
        self.transform(Sharpen::new(amount))
    }

    /// Flip the image vertically (top to bottom).
    pub fn flip_vertically(&self) -> Image {
        self.transform(FlipVertically)
    }

    /// Flip the image horizontally (left to right).
    pub fn flip_horizontally(&self) -> Image {
        self.transform(FlipHorizontally)
    }

    /// Alias of [`Image::flip_vertically`].
    pub fn flip(&self) -> Image {
        self.flip_vertically()
    }

    /// Alias of [`Image::flip_horizontally`].
    pub fn flop(&self) -> Image {
        self.flip_horizontally()
    }

    /// Add a transformation — one of your own, perhaps — to the image's pipeline.
    ///
    /// Custom transformations need a handler registered for the driver with
    /// [`Image::transform_using`].
    pub fn transform(&self, transformation: impl Transformation) -> Image {
        self.with_clone(|image| {
            image.pipeline.add(transformation);
        })
    }

    // ------------------------------------------------------------------
    // Encoding options
    // ------------------------------------------------------------------

    /// Convert the image to the given format and set its quality.
    ///
    /// ```
    /// use illuminate_image::{Image, ImageFormat};
    ///
    /// let image = Image::from_bytes(Vec::new()).optimize_with("jpg", 85).unwrap();
    ///
    /// assert_eq!(image.output().format, Some(ImageFormat::Jpeg));
    /// assert_eq!(image.output().quality, Some(85));
    /// ```
    pub fn optimize_with(&self, format: &str, quality: i64) -> Result<Image> {
        Ok(self.to_format(format)?.quality(quality))
    }

    /// Convert the image to WebP with a quality of 70.
    pub fn optimize(&self) -> Image {
        self.output_format(ImageFormat::Webp)
            .quality(i64::from(ImageOutputOptions::DEFAULT_QUALITY))
    }

    /// Set the output quality. The quality is clamped between 1 and 100.
    pub fn quality(&self, quality: i64) -> Image {
        let quality = quality.clamp(1, 100) as u8;
        self.with_clone(|image| image.pipeline.output.quality = Some(quality))
    }

    /// Convert the image to WebP.
    ///
    /// The built-in driver writes lossless WebP images, so it ignores the
    /// quality setting for this format.
    pub fn to_webp(&self) -> Image {
        self.output_format(ImageFormat::Webp)
    }

    /// Convert the image to JPEG.
    pub fn to_jpg(&self) -> Image {
        self.output_format(ImageFormat::Jpeg)
    }

    /// Alias of [`Image::to_jpg`].
    pub fn to_jpeg(&self) -> Image {
        self.to_jpg()
    }

    /// Convert the image to PNG.
    pub fn to_png(&self) -> Image {
        self.output_format(ImageFormat::Png)
    }

    /// Convert the image to GIF.
    pub fn to_gif(&self) -> Image {
        self.output_format(ImageFormat::Gif)
    }

    /// Convert the image to AVIF.
    ///
    /// The built-in driver can't encode AVIF images: processing an image
    /// converted to AVIF fails with an [`ImageException`]. Register a
    /// driver that can with [`Image::extend`].
    pub fn to_avif(&self) -> Image {
        self.output_format(ImageFormat::Avif)
    }

    /// Convert the image to HEIC. Like AVIF, this needs a driver that can
    /// encode HEIC images.
    pub fn to_heic(&self) -> Image {
        self.output_format(ImageFormat::Heic)
    }

    /// Convert the image to BMP.
    pub fn to_bmp(&self) -> Image {
        self.output_format(ImageFormat::Bmp)
    }

    /// Convert the image to the named format: `webp`, `jpg`, `jpeg`, `png`,
    /// `gif`, `avif`, `heic`, `heif` or `bmp`.
    pub fn to_format(&self, format: &str) -> Result<Image> {
        Ok(self.output_format(ImageFormat::parse(format)?))
    }

    /// Convert the image to the given format.
    pub fn output_format(&self, format: ImageFormat) -> Image {
        self.with_clone(|image| image.pipeline.output.format = Some(format))
    }

    // ------------------------------------------------------------------
    // Drivers
    // ------------------------------------------------------------------

    /// Process this image with the given driver instead of the default one.
    pub fn using(&self, driver: &str) -> Image {
        let mut clone = self.new_clone();
        clone.driver = Some(driver.to_string());
        clone
    }

    /// Process this image with the `gd` driver (an alias of the built-in driver).
    pub fn using_gd(&self) -> Image {
        self.using("gd")
    }

    /// Process this image with the `imagick` driver (an alias of the built-in driver).
    pub fn using_imagick(&self) -> Image {
        self.using("imagick")
    }

    /// The driver this image will be processed with, when not the default.
    pub fn driver_name(&self) -> Option<&str> {
        self.driver.as_deref()
    }

    fn resolve_driver(&self) -> Result<Arc<dyn ImageDriver>> {
        crate::facade::manager().driver(self.driver.as_deref())
    }

    // ------------------------------------------------------------------
    // Accessors
    // ------------------------------------------------------------------

    /// The image's processing pipeline.
    pub fn pipeline(&self) -> &ImagePipeline {
        &self.pipeline
    }

    /// The image's output options.
    pub fn output(&self) -> &ImageOutputOptions {
        &self.pipeline.output
    }

    /// The uploaded file the image was created from, if any.
    pub fn file(&self) -> Option<&UploadedFile> {
        self.file.as_ref()
    }

    /// Whether the image's source contents have been read yet.
    pub fn is_loaded(&self) -> bool {
        self.source.is_loaded()
    }

    // ------------------------------------------------------------------
    // Processing
    // ------------------------------------------------------------------

    /// Process the image and return its raw bytes.
    ///
    /// Without any transformations or output changes, the original contents
    /// are returned untouched. The processed result is remembered, so asking
    /// again is free.
    pub async fn to_bytes(&self) -> Result<Bytes> {
        if let Some(processed) = self.processed.get() {
            return Ok(processed.clone());
        }
        if !self.pipeline.has_changes() {
            return self.source.contents().await;
        }

        let contents = self.source.contents().await.map_err(ImageException::wrap)?;
        let driver = self.resolve_driver().map_err(ImageException::wrap)?;
        let pipeline = self.pipeline.clone();
        let processed = blocking(move || driver.process(&contents, &pipeline))
            .await
            .map_err(ImageException::wrap)?;

        Ok(self.remember(processed))
    }

    /// Process the image and return it as a base64 encoded string.
    pub async fn to_base64(&self) -> Result<String> {
        Ok(STANDARD.encode(self.to_bytes().await?))
    }

    /// Process the image and return it as a `data:` URI.
    pub async fn to_data_uri(&self) -> Result<String> {
        Ok(data_uri(&self.to_bytes().await?))
    }

    /// Load and process the image now, so that synchronous conversions —
    /// [`Display`](fmt::Display) (`to_string()`) and [`IntoResponse`] —
    /// can use the result.
    ///
    /// Images read from bytes, base64, uploads, or local paths don't need
    /// this; images read from disks and URLs do.
    pub async fn load(&self) -> Result<Image> {
        self.to_bytes().await?;
        Ok(self.clone())
    }

    /// Create an HTTP response containing the processed image.
    pub async fn to_response(&self) -> Result<Response> {
        Ok(image_response(self.to_bytes().await?))
    }

    /// Process the image without waiting on I/O, when its contents are at hand.
    fn to_bytes_now(&self) -> Result<Bytes> {
        if let Some(processed) = self.processed.get() {
            return Ok(processed.clone());
        }

        let contents = match self.source.contents_now() {
            Some(contents) => contents,
            None => {
                return Err(ImageException::new(
                    "The image has not been loaded yet. Await `load()` or `to_bytes()` first.",
                )
                .into());
            }
        };
        if !self.pipeline.has_changes() {
            return contents;
        }

        let contents = contents.map_err(ImageException::wrap)?;
        let driver = self.resolve_driver().map_err(ImageException::wrap)?;
        let processed = driver
            .process(&contents, &self.pipeline)
            .map_err(ImageException::wrap)?;

        Ok(self.remember(processed))
    }

    fn remember(&self, processed: Vec<u8>) -> Bytes {
        self.processed
            .get_or_init(|| Bytes::from(processed))
            .clone()
    }

    // ------------------------------------------------------------------
    // Inspecting
    // ------------------------------------------------------------------

    /// The MIME type of the processed image.
    pub async fn mime_type(&self) -> Result<String> {
        Ok(mime_type_of(&self.to_bytes().await?).to_string())
    }

    /// The file extension of the processed image (`jpg`, `png`, `webp`, ...,
    /// or `bin` when unknown).
    pub async fn extension(&self) -> Result<String> {
        Ok(extension_for(&self.mime_type().await?).to_string())
    }

    /// The `(width, height)` of the processed image.
    pub async fn dimensions(&self) -> Result<(u32, u32)> {
        let contents = self.to_bytes().await?;

        if let Some(dimensions) = read_dimensions(&contents) {
            return Ok(dimensions);
        }
        // Let the driver have a go at formats we can't read ourselves...
        if let Ok(dimensions) = self
            .resolve_driver()
            .and_then(|driver| driver.dimensions(&contents))
        {
            return Ok(dimensions);
        }

        Err(ImageException::new("Unable to determine the dimensions of the image.").into())
    }

    /// The width of the processed image.
    pub async fn width(&self) -> Result<u32> {
        Ok(self.dimensions().await?.0)
    }

    /// The height of the processed image.
    pub async fn height(&self) -> Result<u32> {
        Ok(self.dimensions().await?.1)
    }

    /// The dominant (average) color of the processed image, as a hex string
    /// such as `#0080ff`.
    pub async fn dominant_color(&self) -> Result<String> {
        let contents = self.to_bytes().await?;
        let driver = self.resolve_driver()?;
        blocking(move || driver.dominant_color(&contents)).await
    }

    // ------------------------------------------------------------------
    // Storing
    // ------------------------------------------------------------------

    /// A unique, random file name with the processed image's extension.
    /// The random part is generated once per image.
    pub async fn hash_name(&self) -> Result<String> {
        let name = self.hash_name.get_or_init(|| Str::random(40)).clone();
        Ok(format!("{name}.{}", self.extension().await?))
    }

    /// Store the processed image in the given directory of the default
    /// disk, under a unique generated name. Returns the stored path.
    pub async fn store(&self, path: &str) -> Result<String> {
        self.store_image(path, None, None, None).await
    }

    /// Store the processed image on the given disk, under a unique generated name.
    pub async fn store_on(&self, path: &str, disk: &str) -> Result<String> {
        self.store_image(path, None, Some(disk), None).await
    }

    /// Store the processed image under the given name on the default disk.
    pub async fn store_as(&self, path: &str, name: &str) -> Result<String> {
        self.store_image(path, Some(name), None, None).await
    }

    /// Store the processed image under the given name on the given disk.
    pub async fn store_as_on(&self, path: &str, name: &str, disk: &str) -> Result<String> {
        self.store_image(path, Some(name), Some(disk), None).await
    }

    /// Store the processed image with public visibility on the default disk.
    pub async fn store_publicly(&self, path: &str) -> Result<String> {
        self.store_image(path, None, None, Some(Visibility::Public))
            .await
    }

    /// Store the processed image with public visibility on the given disk.
    pub async fn store_publicly_on(&self, path: &str, disk: &str) -> Result<String> {
        self.store_image(path, None, Some(disk), Some(Visibility::Public))
            .await
    }

    /// Store the processed image under the given name, with public visibility.
    pub async fn store_publicly_as(&self, path: &str, name: &str) -> Result<String> {
        self.store_image(path, Some(name), None, Some(Visibility::Public))
            .await
    }

    /// Store the processed image under the given name, with public
    /// visibility, on the given disk.
    pub async fn store_publicly_as_on(&self, path: &str, name: &str, disk: &str) -> Result<String> {
        self.store_image(path, Some(name), Some(disk), Some(Visibility::Public))
            .await
    }

    async fn store_image(
        &self,
        path: &str,
        name: Option<&str>,
        disk: Option<&str>,
        visibility: Option<Visibility>,
    ) -> Result<String> {
        let name = match name {
            Some(name) => name.to_string(),
            None => self.hash_name().await?,
        };
        let path = [path.trim_matches('/'), name.trim_matches('/')]
            .into_iter()
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        let contents = self.to_bytes().await?;

        let disk = match disk {
            Some(disk) => Storage::disk(disk)?,
            None => Storage::default_disk()?,
        };
        let stored = match visibility {
            Some(visibility) => {
                disk.put_with_visibility(&path, &contents, visibility)
                    .await?
            }
            None => disk.put(&path, &contents).await?,
        };

        if stored {
            Ok(path)
        } else {
            Err(FilesystemException::UnableToWriteFile {
                location: path,
                reason: "The image could not be stored.".to_string(),
            }
            .into())
        }
    }

    // ------------------------------------------------------------------
    // Cloning
    // ------------------------------------------------------------------

    /// An independent copy of the image, forgetting any processed result.
    fn new_clone(&self) -> Image {
        Image {
            processed: OnceLock::new(),
            ..self.clone()
        }
    }

    fn with_clone(&self, callback: impl FnOnce(&mut Image)) -> Image {
        let mut clone = self.new_clone();
        callback(&mut clone);
        clone
    }
}

impl Conditionable for Image {}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("source", &self.source)
            .field("pipeline", &self.pipeline)
            .field("driver", &self.driver)
            .field(
                "file",
                &self.file.as_ref().map(UploadedFile::client_original_name),
            )
            .field("processed", &self.processed.get().is_some())
            .finish()
    }
}

/// An image displays as its `data:` URI.
///
/// Displaying needs the image's contents without waiting on I/O: images
/// read from bytes, base64, uploads, and local paths always have them, and
/// images read from disks or URLs have them once loaded (see
/// [`Image::load`]). Otherwise — or when the image can't be processed —
/// formatting fails, so prefer [`Image::to_data_uri`] in async code.
impl fmt::Display for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_bytes_now() {
            Ok(contents) => f.write_str(&data_uri(&contents)),
            Err(_) => Err(fmt::Error),
        }
    }
}

/// Images are responses: the processed bytes with their `Content-Type`.
///
/// Like [`Display`](fmt::Display), this needs the image's contents at
/// hand: return `image.load().await?` (or use [`Image::to_response`]) for
/// images read from disks or URLs. Failures are rendered by the exception
/// handler.
impl IntoResponse for Image {
    fn into_response(self) -> Response {
        match self.to_bytes_now() {
            Ok(contents) => image_response(contents),
            Err(error) => render_exception(error),
        }
    }
}

fn image_response(contents: Bytes) -> Response {
    let mime_type = mime_type_of(&contents);
    Response::new(contents).with_header("content-type", mime_type)
}

fn data_uri(contents: &[u8]) -> String {
    format!(
        "data:{};base64,{}",
        mime_type_of(contents),
        STANDARD.encode(contents)
    )
}

/// Read an image's dimensions from its header.
fn read_dimensions(contents: &[u8]) -> Option<(u32, u32)> {
    ::image::ImageReader::new(Cursor::new(contents))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Run CPU-heavy work on Tokio's blocking pool (keeping the current
/// container), or inline outside of a runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return work();
    };

    let container = Container::get_instance();
    handle
        .spawn_blocking(move || {
            let _guard = Container::set_local_instance(container);
            work()
        })
        .await
        .map_err(|error| ImageException::new(format!("The image driver failed: {error}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}
    fn assert_send<T: Send>(_: &T) {}

    #[test]
    fn images_and_their_futures_can_cross_threads() {
        assert_send_sync::<Image>();

        let image = Image::from_bytes(Vec::new());
        assert_send(&image.to_bytes());
        assert_send(&image.dimensions());
        assert_send(&image.dominant_color());
        assert_send(&image.store_publicly_as_on("a", "b", "c"));
        assert_send(&image.to_response());
        assert_send(&image.load());
    }

    #[test]
    fn hash_names_are_kept_by_clones_made_afterwards() {
        let image = Image::from_bytes(Vec::new());
        let before = image.blur(1);
        let name = image.hash_name.get_or_init(|| Str::random(40)).clone();

        assert_eq!(image.blur(1).hash_name.get(), Some(&name));
        assert_eq!(before.hash_name.get(), None);
    }

    #[test]
    fn transformations_forget_the_processed_result() {
        let image = Image::from_bytes(Vec::new()).grayscale();
        image.remember(b"processed".to_vec());

        assert_eq!(image.clone().processed.get().unwrap(), "processed");
        assert!(image.blur(1).processed.get().is_none());
        assert!(image.using("gd").processed.get().is_none());
        assert_eq!(image.to_bytes_now().unwrap(), "processed");
    }

    #[test]
    fn images_describe_themselves() {
        let file = UploadedFile::new("me.png", "image/png", b"png".to_vec());
        let image = Image::from_upload(&file).cover(1, 1).using("gd");
        let debug = format!("{image:?}");

        assert!(debug.contains("Cover"));
        assert!(debug.contains("Some(\"gd\")"));
        assert!(debug.contains("me.png"));
        assert!(debug.contains("bytes (3 bytes)"));
    }

    #[test]
    fn lazy_images_cannot_be_displayed_before_they_are_loaded() {
        let image = Image::lazy(|| async { Ok(Bytes::from_static(b"lazy")) });

        let error = image.to_bytes_now().unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("The image has not been loaded yet.")
        );
        assert!(std::fmt::write(&mut String::new(), format_args!("{image}")).is_err());
    }
}
