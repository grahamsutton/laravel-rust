mod common;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use common::{app, jpeg, png, solid_png};
use illuminate_image::{Image, ImageException, ImageFormat, ImageOutputOptions};

fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

#[tokio::test]
async fn images_keep_their_original_format_by_default() {
    let _app = app();

    let jpeg = Image::from_bytes(jpeg(100, 100)).cover(50, 50);
    assert!(
        jpeg.to_bytes()
            .await
            .unwrap()
            .starts_with(&[0xFF, 0xD8, 0xFF])
    );
    assert_eq!(jpeg.mime_type().await.unwrap(), "image/jpeg");
    assert_eq!(jpeg.extension().await.unwrap(), "jpg");

    let png = Image::from_bytes(png(100, 100)).grayscale();
    assert!(
        png.to_bytes()
            .await
            .unwrap()
            .starts_with(b"\x89PNG\r\n\x1a\n")
    );
    assert_eq!(png.extension().await.unwrap(), "png");
}

#[tokio::test]
async fn images_without_changes_are_returned_untouched() {
    let _app = app();
    let contents = jpeg(100, 100);
    let image = Image::from_bytes(contents.clone());

    assert_eq!(image.to_bytes().await.unwrap(), contents);
    assert_eq!(
        image.to_bytes().await.unwrap(),
        image.to_bytes().await.unwrap()
    );
}

#[tokio::test]
async fn images_can_be_converted_to_every_writable_format() {
    let _app = app();
    let image = Image::from_bytes(png(30, 20));

    let webp = image.to_webp().to_bytes().await.unwrap();
    assert!(is_webp(&webp));

    for jpeg in [
        image.to_jpg(),
        image.to_jpeg(),
        image.to_format("jpeg").unwrap(),
    ] {
        assert!(
            jpeg.to_bytes()
                .await
                .unwrap()
                .starts_with(&[0xFF, 0xD8, 0xFF])
        );
    }

    let png = Image::from_bytes(jpeg(30, 20))
        .to_png()
        .to_bytes()
        .await
        .unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));

    let gif = image.to_gif().to_bytes().await.unwrap();
    assert!(gif.starts_with(b"GIF89a") || gif.starts_with(b"GIF87a"));

    let bmp = image.to_bmp().to_bytes().await.unwrap();
    assert!(bmp.starts_with(b"BM"));

    for (variant, mime_type, extension) in [
        (image.to_webp(), "image/webp", "webp"),
        (image.to_jpg(), "image/jpeg", "jpg"),
        (image.to_gif(), "image/gif", "gif"),
        (image.to_bmp(), "image/bmp", "bmp"),
    ] {
        assert_eq!(variant.mime_type().await.unwrap(), mime_type);
        assert_eq!(variant.extension().await.unwrap(), extension);
        // Conversion never changes the dimensions...
        assert_eq!(variant.dimensions().await.unwrap(), (30, 20));
    }
}

#[tokio::test]
async fn converted_images_can_be_read_again() {
    let _app = app();
    let webp = Image::from_bytes(png(40, 40))
        .to_webp()
        .to_bytes()
        .await
        .unwrap();
    let gif = Image::from_bytes(png(40, 40))
        .to_gif()
        .to_bytes()
        .await
        .unwrap();
    let bmp = Image::from_bytes(png(40, 40))
        .to_bmp()
        .to_bytes()
        .await
        .unwrap();

    for contents in [webp, gif, bmp] {
        let jpeg = Image::from_bytes(contents).cover(20, 10).to_jpg();
        assert_eq!(jpeg.dimensions().await.unwrap(), (20, 10));
        assert_eq!(jpeg.mime_type().await.unwrap(), "image/jpeg");
    }
}

#[tokio::test]
async fn avif_and_heic_fail_when_the_image_is_encoded() {
    let _app = app();
    let image = Image::from_bytes(png(10, 10));

    // Converting is fine — the error comes when the image is processed.
    let avif = image.to_avif();
    assert_eq!(avif.output().format, Some(ImageFormat::Avif));

    let error = avif.to_bytes().await.unwrap_err();
    let exception = error.downcast_ref::<ImageException>().unwrap();
    assert_eq!(
        exception.to_string(),
        "The [avif] format is not supported by the [image] image driver."
    );

    assert!(image.to_heic().to_bytes().await.is_err());
    assert_eq!(
        image.to_format("heif").unwrap().output().format,
        Some(ImageFormat::Heic)
    );
}

#[tokio::test]
async fn unsupported_formats_are_rejected() {
    let image = Image::from_bytes(Vec::new());

    for format in ["tiff", "jpge", "svg"] {
        let error = image.to_format(format).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("The [{format}] format is not supported.")
        );
        assert!(error.is::<ImageException>());
    }
    assert!(image.optimize_with("tiff", 50).is_err());
}

#[tokio::test]
async fn unsupported_inputs_fail_to_process() {
    let _app = app();
    let error = Image::from_bytes("not-an-image")
        .cover(10, 10)
        .to_bytes()
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "The image format [text/plain] is not supported."
    );

    let error = Image::from_bytes("not-an-image")
        .dimensions()
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Unable to determine the dimensions of the image."
    );
}

#[tokio::test]
async fn quality_is_clamped() {
    let image = Image::from_bytes(Vec::new());

    assert_eq!(image.quality(0).output().quality, Some(1));
    assert_eq!(image.quality(-10).output().quality, Some(1));
    assert_eq!(image.quality(500).output().quality, Some(100));
    assert_eq!(image.quality(80).output().quality, Some(80));
    assert!(image.quality(80).pipeline().has_changes());
    assert_eq!(image.output().quality, None);
}

#[tokio::test]
async fn quality_affects_jpeg_file_size() {
    let _app = app();
    let image = Image::from_bytes(png(200, 200)).to_jpg();

    let low = image.quality(1).to_bytes().await.unwrap();
    let high = image.quality(100).to_bytes().await.unwrap();
    let default = image.to_bytes().await.unwrap();

    assert!(low.len() < default.len());
    assert!(default.len() < high.len());
    assert_eq!(image.quality(1).dimensions().await.unwrap(), (200, 200));

    // Quality alone re-encodes a JPEG in its own format...
    let reencoded = Image::from_bytes(jpeg(200, 200)).quality(1);
    assert_eq!(reencoded.mime_type().await.unwrap(), "image/jpeg");
    assert!(reencoded.to_bytes().await.unwrap().len() < jpeg(200, 200).len());
}

#[tokio::test]
async fn optimize_converts_to_webp_at_seventy_by_default() {
    let _app = app();
    let image = Image::from_bytes(png(20, 20));

    let optimized = image.optimize();
    assert_eq!(
        optimized.output(),
        &ImageOutputOptions {
            format: Some(ImageFormat::Webp),
            quality: Some(ImageOutputOptions::DEFAULT_QUALITY),
        }
    );
    assert!(is_webp(&optimized.to_bytes().await.unwrap()));

    let jpeg = image.optimize_with("jpg", 85).unwrap();
    assert_eq!(jpeg.output().format, Some(ImageFormat::Jpeg));
    assert_eq!(jpeg.output().quality, Some(85));
    assert_eq!(jpeg.mime_type().await.unwrap(), "image/jpeg");

    // Quality survives a later format conversion...
    assert_eq!(image.quality(40).to_png().output().quality, Some(40));
}

#[tokio::test]
async fn images_can_be_encoded_as_base64_and_data_uris() {
    let _app = app();
    let contents = png(10, 10);
    let image = Image::from_bytes(contents.clone());

    assert_eq!(image.to_base64().await.unwrap(), STANDARD.encode(&contents));
    assert_eq!(
        image.to_data_uri().await.unwrap(),
        format!("data:image/png;base64,{}", STANDARD.encode(&contents))
    );

    let webp = image.to_webp();
    let uri = webp.to_data_uri().await.unwrap();
    let encoded = uri.strip_prefix("data:image/webp;base64,").unwrap();
    assert_eq!(
        STANDARD.decode(encoded).unwrap(),
        webp.to_bytes().await.unwrap()
    );
}

#[tokio::test]
async fn images_display_as_data_uris() {
    let _app = app();
    let image = Image::from_bytes(jpeg(10, 10));

    assert_eq!(image.to_string(), image.to_data_uri().await.unwrap());
    assert!(
        image
            .cover(5, 5)
            .to_png()
            .to_string()
            .starts_with("data:image/png;base64,")
    );

    // Lazily loaded images display once they've been loaded...
    let lazy = Image::lazy(|| async { Ok(bytes::Bytes::from(png(4, 4))) });
    assert!(std::fmt::write(&mut String::new(), format_args!("{lazy}")).is_err());
    let loaded = lazy.cover(2, 2).load().await.unwrap();
    assert!(loaded.to_string().starts_with("data:image/png;base64,"));
}

#[tokio::test]
async fn dominant_color_is_the_average_color() {
    let _app = app();

    let image = Image::from_bytes(solid_png(20, 20, [0, 128, 255, 255]));
    assert_eq!(image.dominant_color().await.unwrap(), "#0080ff");

    let translucent = Image::from_bytes(solid_png(20, 20, [0, 128, 255, 128]));
    assert_eq!(translucent.dominant_color().await.unwrap(), "#0080ff");

    // It describes the processed image...
    let contained = image.contain_with(40, 20, "#ff0000");
    let color = contained.dominant_color().await.unwrap();
    assert_eq!(color, "#804080");
}

#[tokio::test]
async fn inspecting_describes_the_processed_image() {
    let _app = app();
    let image = Image::from_bytes(jpeg(320, 240));

    assert_eq!(image.width().await.unwrap(), 320);
    assert_eq!(image.height().await.unwrap(), 240);

    let covered = image.cover(400, 400).to_png();
    assert_eq!(covered.width().await.unwrap(), 400);
    assert_eq!(covered.height().await.unwrap(), 400);
    assert_eq!(covered.mime_type().await.unwrap(), "image/png");

    assert_eq!(
        Image::from_bytes(jpeg(1, 1)).dimensions().await.unwrap(),
        (1, 1)
    );
    assert_eq!(
        Image::from_bytes("plain text").extension().await.unwrap(),
        "bin"
    );
}
