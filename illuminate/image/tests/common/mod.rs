//! Shared helpers: an application container and small images made in memory.

#![allow(dead_code)]

use std::io::Cursor;
use std::sync::Arc;

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageEncoder, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};

use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_filesystem::FilesystemServiceProvider;
use illuminate_image::ImageServiceProvider;
use illuminate_support::json;

/// A fresh container with configuration, the filesystem, and the image manager.
pub fn app() -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({
        "app": {"url": "http://localhost"},
        "filesystems": {
            "default": "local",
            "disks": {
                "local": {"driver": "local", "root": "/nonexistent"},
                "public": {"driver": "local", "root": "/nonexistent", "visibility": "public"},
            },
        },
        "images": {"default": "gd"},
    })));
    FilesystemServiceProvider.register(&container);
    ImageServiceProvider.register(&container);
    (container, guard)
}

/// Encode an image in the given format.
pub fn encode(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), format)
        .unwrap();
    bytes
}

/// A colorful gradient, so transformations have something to work with.
pub fn gradient(width: u32, height: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
        Rgb([
            (x * 255 / width.max(1)) as u8,
            (y * 255 / height.max(1)) as u8,
            128,
        ])
    }))
}

/// A PNG gradient of the given size.
pub fn png(width: u32, height: u32) -> Vec<u8> {
    encode(&gradient(width, height), ImageFormat::Png)
}

/// A JPEG gradient of the given size.
pub fn jpeg(width: u32, height: u32) -> Vec<u8> {
    encode(&gradient(width, height), ImageFormat::Jpeg)
}

/// A PNG filled with a single color.
pub fn solid_png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    encode(
        &DynamicImage::ImageRgba8(RgbaImage::from_pixel(width, height, Rgba(color))),
        ImageFormat::Png,
    )
}

/// A JPEG whose EXIF data carries the given orientation (6 = rotate 90° clockwise).
pub fn jpeg_with_orientation(width: u32, height: u32, orientation: u16) -> Vec<u8> {
    let image = gradient(width, height).to_rgb8();
    let [high, low] = orientation.to_be_bytes();
    let exif = vec![
        b'M', b'M', 0, 42, // big endian TIFF header
        0, 0, 0, 8, // the first IFD's offset
        0, 1, // one entry
        0x01, 0x12, // Orientation
        0, 3, // SHORT
        0, 0, 0, 1, // one value
        high, low, 0, 0, // the value
        0, 0, 0, 0, // no next IFD
    ];

    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, 90);
    encoder.set_exif_metadata(exif).unwrap();
    encoder
        .write_image(
            image.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
    bytes
}

/// Decode image bytes.
pub fn decode(bytes: &[u8]) -> DynamicImage {
    image::load_from_memory(bytes).unwrap()
}
