//! # Illuminate Image
//!
//! Laravel's fluent image manipulation API: resize, crop, encode, and store
//! images with the same expressive conventions found throughout the
//! framework.
//!
//! ```
//! use illuminate_image::Image;
//! # use std::sync::Arc;
//! # use illuminate_config::Repository;
//! # use illuminate_container::Container;
//! # use illuminate_filesystem::Storage;
//! # use illuminate_image::image::{DynamicImage, ImageFormat};
//! # use illuminate_support::json;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! # let container = Arc::new(Container::new());
//! # let _guard = Container::set_local_instance(container.clone());
//! # container.instance(Repository::new(json!({"filesystems": {"default": "local"}})));
//! # let disk = Storage::fake("public")?;
//! # let mut jpeg = Vec::new();
//! # DynamicImage::new_rgb8(640, 480).write_to(&mut std::io::Cursor::new(&mut jpeg), ImageFormat::Jpeg).unwrap();
//! # disk.put("avatars/photo.jpg", &jpeg).await?;
//! let path = Image::from_storage("avatars/photo.jpg", "public")
//!     .cover(400, 400)
//!     .to_webp()
//!     .quality(80)
//!     .store_publicly_on("avatars", "public")
//!     .await?;
//!
//! assert!(path.starts_with("avatars/") && path.ends_with(".webp"));
//! # Ok::<(), illuminate_support::Error>(())
//! # }).unwrap();
//! ```
//!
//! ## Reading images
//!
//! Images may be read from raw bytes ([`Image::from_bytes`]), base64
//! ([`Image::from_base64`]), local paths ([`Image::from_path`]),
//! filesystem disks ([`Image::from_storage`], `disk.image(path)` with
//! [`FilesystemImageExt`], or `Storage::image(path)` with
//! [`StorageImageExt`]), remote URLs ([`Image::from_url`]), streams
//! ([`Image::from_stream`]) and uploaded files ([`Image::from_upload`],
//! `request.image("avatar")` with [`RequestImageExt`], or `file.image()`
//! with [`UploadedFileImageExt`]). Contents are loaded lazily: nothing is
//! read until the image is processed or its bytes are requested.
//!
//! ## Manipulating images
//!
//! Images are immutable. Every transformation returns a new image with the
//! transformation appended to its pipeline, and the pipeline is applied —
//! and the image encoded — just once, at the end:
//!
//! ```
//! # use illuminate_image::Image;
//! # let image = Image::from_bytes(Vec::new());
//! let image = image.orient().cover(400, 400).sharpen(10);
//!
//! let image = image.resize(800, 600);         // resize(width: 800, height: 600)
//! let image = image.resize_width(800);        // resize(width: 800)
//! let image = image.scale_height(600);        // scale(height: 600)
//! let image = image.contain_with(400, 400, "dominant");
//! let image = image.crop_at(300, 200, 50, 25);
//! let image = image.rotate_with(90, "#ffffff");
//! let image = image.blur(5).grayscale().flip_vertically().flip_horizontally();
//! ```
//!
//! Laravel's optional arguments become separate methods: `resize(w, h)`,
//! `resize_width(w)`, `resize_height(h)` (and `resize_with(Option, Option)`);
//! the same for `scale`. `contain_with` and `rotate_with` take a background
//! color, and `crop_at` takes the crop's position.
//!
//! Images use [`Conditionable`](illuminate_support::Conditionable), so
//! `when` and `unless` work too.
//!
//! ## Encoding, inspecting, and storing
//!
//! Convert with `to_webp`, `to_jpg`, `to_png`, `to_gif`, `to_bmp`,
//! `to_avif` or `to_format("...")`, set the `quality` (clamped to 1..=100),
//! or `optimize()` (WebP at 70). Read the result with `to_bytes`,
//! `to_base64`, `to_data_uri`, `mime_type`, `extension`, `dimensions`,
//! `width`, `height` and `dominant_color`, and store it with `store`,
//! `store_as`, `store_publicly` and `store_publicly_as` (each with an
//! `_on` variant naming the disk). Images are responses, too.
//!
//! ## Drivers
//!
//! The built-in [`NativeDriver`] is pure Rust and needs no system
//! libraries. It's registered as `image`, and `gd` and `imagick` are
//! aliases of it so Laravel configuration keeps working. Register your own
//! drivers with [`Image::extend`] and custom transformations with
//! [`Image::transform_using`].

mod color;
pub mod driver;
pub mod drivers;
pub mod exception;
mod extensions;
mod facade;
mod instance;
pub mod manager;
mod mime;
pub mod pipeline;
pub mod provider;
mod source;
pub mod transformations;

pub use driver::ImageDriver;
pub use drivers::NativeDriver;
pub use exception::ImageException;
pub use extensions::{FilesystemImageExt, RequestImageExt, StorageImageExt, UploadedFileImageExt};
pub use instance::Image;
pub use manager::{DriverCreator, ImageManager};
pub use mime::{extension_for, mime_type_of};
pub use pipeline::{ImageFormat, ImageOutputOptions, ImagePipeline};
pub use provider::ImageServiceProvider;
pub use transformations::{
    AnyTransformationHandler, Transformation, TransformationHandler, TransformationHandlers,
};

/// The `image` crate powering the built-in driver, for writing custom
/// transformations (`image::DynamicImage`, `image::imageops`, ...).
pub use ::image;

/// The facades provided by this component.
pub mod facades {
    pub use crate::instance::Image;
}

/// Everything you need in one import.
pub mod prelude {
    pub use crate::{
        FilesystemImageExt, Image, RequestImageExt, StorageImageExt, UploadedFileImageExt,
    };
}

/// The default `images` configuration (`config/images.php`).
///
/// ```
/// use illuminate_support::json;
///
/// assert_eq!(illuminate_image::config(), json!({"default": "image"}));
/// ```
pub fn config() -> illuminate_support::Value {
    illuminate_support::json!({
        /*
        |--------------------------------------------------------------------------
        | Default Image Driver
        |--------------------------------------------------------------------------
        |
        | This option controls the default image processing driver that will be
        | used when manipulating or converting images. This driver is always
        | utilized unless another driver is explicitly specified instead.
        |
        | Supported: "image" (pure Rust; "gd" and "imagick" are aliases)
        |
        */

        "default": illuminate_support::env("IMAGE_DRIVER", "image"),
    })
}
