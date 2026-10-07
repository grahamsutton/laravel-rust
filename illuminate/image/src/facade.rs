//! The `Image` facade: reading images, and managing the image drivers.
//!
//! In Laravel, `Illuminate\Support\Facades\Image` and `Illuminate\Image\Image`
//! are two classes. Here the facade's static methods live right on the
//! [`Image`] type, so `Image::from_storage(...)` reads exactly like it does
//! in Laravel.

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt};

use illuminate_container::{Container, try_app};
use illuminate_filesystem::Storage;
use illuminate_http::UploadedFile;
use illuminate_http_client::Http;
use illuminate_support::Result;

use crate::driver::ImageDriver;
use crate::exception::ImageException;
use crate::instance::Image;
use crate::manager::ImageManager;
use crate::source::{ContentsFuture, Source};
use crate::transformations::Transformation;

/// Resolve the image manager, registering one if the application hasn't.
pub(crate) fn manager() -> Arc<ImageManager> {
    if let Some(manager) = try_app::<ImageManager>() {
        return manager;
    }
    let container = Container::get_instance();
    container.singleton_if::<ImageManager>(crate::provider::make_manager);
    container.make::<ImageManager>()
}

impl Image {
    // ------------------------------------------------------------------
    // Reading images
    // ------------------------------------------------------------------

    /// Create an image from raw bytes.
    pub fn from_bytes(contents: impl Into<Bytes>) -> Image {
        Image::with_source(Source::bytes(contents.into()), None)
    }

    /// Create an image from a base64 encoded string. The string is decoded
    /// when the image is first used, failing with "Invalid base64 image data."
    ///
    /// ```
    /// use illuminate_image::Image;
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// let image = Image::from_base64("aGVsbG8=");
    /// assert_eq!(image.to_bytes().await.unwrap(), "hello");
    ///
    /// let invalid = Image::from_base64("not base64!");
    /// assert_eq!(invalid.to_bytes().await.unwrap_err().to_string(), "Invalid base64 image data.");
    /// # });
    /// ```
    pub fn from_base64(base64: impl Into<String>) -> Image {
        Image::with_source(Source::base64(base64.into()), None)
    }

    /// Create an image from a local file path. The file is read lazily.
    pub fn from_path(path: impl Into<PathBuf>) -> Image {
        Image::with_source(Source::path(path.into()), None)
    }

    /// Create an image from a file on the given filesystem disk. The file
    /// is read lazily.
    ///
    /// ```
    /// use illuminate_image::Image;
    ///
    /// let image = Image::from_storage("avatars/photo.jpg", "public")
    ///     .cover(400, 400)
    ///     .to_webp();
    ///
    /// assert!(!image.is_loaded());
    /// ```
    pub fn from_storage(path: &str, disk: &str) -> Image {
        Self::storage_image(path, Some(disk))
    }

    /// Create an image from a file on the default filesystem disk.
    pub fn from_default_disk(path: &str) -> Image {
        Self::storage_image(path, None)
    }

    fn storage_image(path: &str, disk: Option<&str>) -> Image {
        let path = path.to_string();
        let disk = disk.map(str::to_string);
        let description = format!("storage ({}:{path})", disk.as_deref().unwrap_or("default"));

        Image::lazy_described(description, move || {
            let (path, disk) = (path.clone(), disk.clone());
            async move {
                let disk = match disk {
                    Some(disk) => Storage::disk(&disk)?,
                    None => Storage::default_disk()?,
                };
                disk.bytes(&path).await
            }
        })
    }

    /// Create an image from an uploaded file. The image remembers the file:
    /// see [`Image::file`].
    pub fn from_upload(file: &UploadedFile) -> Image {
        Image::with_source(Source::bytes(file.bytes().clone()), Some(file.clone()))
    }

    /// Create an image from a URL, fetched lazily with the `Http` client.
    /// Unsuccessful responses fail with a `RequestException`.
    pub fn from_url(url: impl Into<String>) -> Image {
        let url = url.into();

        Image::lazy_described(format!("url ({url})"), move || {
            let url = url.clone();
            async move {
                let response = Http::get(url).await?;
                response.throw()?;
                Ok(response.bytes().clone())
            }
        })
    }

    /// Create an image from a stream, read to the end when the image is
    /// first used. Empty streams fail with "Invalid stream image data."
    pub fn from_stream(stream: impl AsyncRead + Send + Unpin + 'static) -> Image {
        let stream: Arc<Mutex<Option<Box<dyn AsyncRead + Send + Unpin>>>> =
            Arc::new(Mutex::new(Some(Box::new(stream))));

        Image::lazy_described("stream", move || {
            let stream = stream.clone();
            async move {
                let taken = stream.lock().unwrap().take();
                let invalid = || ImageException::new("Invalid stream image data.");
                let mut stream = taken.ok_or_else(invalid)?;

                let mut contents = Vec::new();
                stream.read_to_end(&mut contents).await?;
                if contents.is_empty() {
                    return Err(invalid().into());
                }
                Ok(Bytes::from(contents))
            }
        })
    }

    /// Create an image whose contents are loaded by the given callback, the
    /// first time they're needed. The callback runs at most once, even
    /// across images derived from this one.
    ///
    /// ```
    /// use illuminate_image::Image;
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// let image = Image::lazy(|| async { Ok(bytes::Bytes::from_static(b"contents")) });
    ///
    /// assert!(!image.is_loaded());
    /// assert_eq!(image.to_bytes().await.unwrap(), "contents");
    /// assert!(image.is_loaded());
    /// # });
    /// ```
    pub fn lazy<F, Fut>(loader: F) -> Image
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Bytes>> + Send + 'static,
    {
        Image::lazy_described("callback", loader)
    }

    fn lazy_described<F, Fut>(description: impl Into<String>, loader: F) -> Image
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Bytes>> + Send + 'static,
    {
        let loader = Arc::new(move || -> ContentsFuture { Box::pin(loader()) });
        Image::with_source(Source::lazy(description, loader), None)
    }

    // ------------------------------------------------------------------
    // Drivers
    // ------------------------------------------------------------------

    /// The image manager behind the facade.
    pub fn manager() -> Arc<ImageManager> {
        manager()
    }

    /// Get an image driver by name — the default one when `None`.
    pub fn driver(name: Option<&str>) -> Result<Arc<dyn ImageDriver>> {
        manager().driver(name)
    }

    /// The default image driver's name (`images.default`).
    pub fn get_default_driver() -> String {
        manager().get_default_driver()
    }

    /// Register a custom image driver.
    ///
    /// Typically, you'll do this in the `boot` method of a service provider.
    /// See [`ImageDriver`] for an example.
    pub fn extend(
        driver: impl Into<String>,
        creator: impl Fn(&Container) -> Arc<dyn ImageDriver> + Send + Sync + 'static,
    ) {
        manager().extend(driver, creator);
    }

    /// Register a handler applying a custom transformation with the given
    /// driver. The handler receives the driver's native image type — a
    /// [`DynamicImage`](::image::DynamicImage) for the built-in driver.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_container::Container;
    /// use illuminate_image::{Image, Transformation};
    /// use illuminate_image::image::{DynamicImage, imageops::FilterType};
    ///
    /// #[derive(Debug)]
    /// pub struct Pixelate {
    ///     pub size: u32,
    /// }
    ///
    /// impl Transformation for Pixelate {}
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// # let _guard = Container::set_local_instance(Arc::new(Container::new()));
    /// Image::transform_using("image", |image: DynamicImage, pixelate: &Pixelate| {
    ///     let (width, height) = (image.width(), image.height());
    ///     Ok(image
    ///         .resize_exact((width / pixelate.size).max(1), (height / pixelate.size).max(1), FilterType::Nearest)
    ///         .resize_exact(width, height, FilterType::Nearest))
    /// });
    ///
    /// # let mut png = Vec::new();
    /// # DynamicImage::new_rgb8(48, 48).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    /// let image = Image::from_bytes(png).transform(Pixelate { size: 12 });
    ///
    /// assert_eq!(image.dimensions().await?, (48, 48));
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn transform_using<T, I>(
        driver: &str,
        handler: impl Fn(I, &T) -> Result<I> + Send + Sync + 'static,
    ) where
        T: Transformation,
        I: Send + 'static,
    {
        manager().transform_using(driver, handler);
    }

    /// Forget every resolved driver instance.
    pub fn forget_drivers() {
        manager().forget_drivers();
    }
}
