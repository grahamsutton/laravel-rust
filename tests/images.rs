//! Image manipulation through the `laravel` crate: images read from a
//! disk, transformed, and returned as responses.

use laravel::image::image::{ImageFormat, Rgb, RgbImage};
use laravel::prelude::*;
use laravel::testing::TestApp;

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    RgbImage::from_pixel(width, height, Rgb([0, 128, 255]))
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn app() -> TestApp {
    let dir = tempfile::tempdir().unwrap().keep();
    TestApp::new(Application::configure_detached(&dir).with_routing(|routing| {
        routing.web(|| {
            Route::get("/avatars/{name}", |Path(name): Path<String>| async move {
                Image::from_storage(&format!("avatars/{name}"), "public")
                    .cover(64, 64)
                    .to_webp()
                    .to_response()
                    .await
            });
        });
    }))
}

#[tokio::test]
async fn images_are_transformed_and_served() {
    let mut app = app();
    let disk = Storage::fake("public").unwrap();
    disk.put("avatars/taylor.png", png(400, 200)).await.unwrap();

    let response = app.get("/avatars/taylor.png").await;
    response.assert_ok().assert_header("Content-Type", Some("image/webp"));

    let laravel::http::Body::Bytes(bytes) = response.response().body() else {
        panic!("the image is buffered");
    };
    let served = Image::from_bytes(bytes.clone());
    assert_eq!(served.dimensions().await.unwrap(), (64, 64));
    assert_eq!(served.dominant_color().await.unwrap(), "#0080ff");
}

#[tokio::test]
async fn processed_images_are_stored() {
    let _app = app();
    Storage::fake("public").unwrap();
    Storage::fake("local").unwrap();

    let path = Image::from_bytes(png(300, 300))
        .scale(100, 100)
        .to_jpg()
        .quality(80)
        .store_publicly_on("thumbnails", "public")
        .await
        .unwrap();

    assert!(path.starts_with("thumbnails/") && path.ends_with(".jpg"), "{path}");
    let stored = Storage::disk("public").unwrap().image(&path);
    assert_eq!(stored.dimensions().await.unwrap(), (100, 100));
    assert_eq!(stored.mime_type().await.unwrap(), "image/jpeg");
}
