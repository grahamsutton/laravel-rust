mod common;

use std::sync::Arc;

use common::{app, jpeg, png};
use illuminate_filesystem::Storage;
use illuminate_http::{IntoResponse, Request, UploadedFile};
use illuminate_http_client::{HeaderMap, Http, RequestException, Response as HttpResponse};
use illuminate_image::{Image, ImageException, RequestImageExt, UploadedFileImageExt};

fn request_with_avatar(contents: Vec<u8>) -> Request {
    let request = Request::create("/avatar", "POST");
    request.attach_file(
        "avatar",
        UploadedFile::new("avatar.jpg", "image/jpeg", contents),
    );
    request
}

#[tokio::test]
async fn requests_hand_out_uploaded_images() {
    let _app = app();
    let local = Storage::fake("local").unwrap();
    let request = request_with_avatar(jpeg(640, 480));

    let image = request.image("avatar").unwrap();
    assert_eq!(image.file().unwrap().client_original_name(), "avatar.jpg");
    assert!(request.image("missing").is_none());

    let path = request
        .image("avatar")
        .unwrap()
        .cover(400, 400)
        .to_webp()
        .store_publicly("avatars")
        .await
        .unwrap();

    assert!(path.starts_with("avatars/") && path.ends_with(".webp"));
    local.assert_exists(path.as_str()).await;
    assert_eq!(
        Image::from_default_disk(&path).dimensions().await.unwrap(),
        (400, 400)
    );
}

#[tokio::test]
async fn two_variants_can_be_made_from_one_upload() {
    let _app = app();
    let public = Storage::fake("public").unwrap();
    let request = request_with_avatar(jpeg(300, 300));

    let image = request.image("avatar").unwrap();
    let large = image
        .cover(200, 200)
        .store_on("avatars", "public")
        .await
        .unwrap();
    let small = image
        .cover(50, 50)
        .to_png()
        .store_on("avatars", "public")
        .await
        .unwrap();

    assert_ne!(large, small);
    public.assert_count("avatars", 2).await;
    assert_eq!(
        Image::from_storage(&large, "public").width().await.unwrap(),
        200
    );
    assert_eq!(
        Image::from_storage(&small, "public").width().await.unwrap(),
        50
    );
    assert!(small.ends_with(".png"));
}

#[tokio::test]
async fn uploaded_files_become_images() {
    let _app = app();
    let file = UploadedFile::new("photo.png", "image/png", png(64, 32));

    let image = file.image();
    assert_eq!(image.file(), Some(&file));
    assert_eq!(image.dimensions().await.unwrap(), (64, 32));

    let from_upload = Image::from_upload(&file).scale_width(16);
    assert_eq!(from_upload.dimensions().await.unwrap(), (16, 8));
    assert_eq!(from_upload.file(), Some(&file), "variants keep the upload");

    assert!(Image::from_bytes(png(1, 1)).file().is_none());
}

#[tokio::test]
async fn images_are_responses() {
    let _app = app();
    let image = Image::from_bytes(png(40, 40)).cover(20, 20).to_jpg();
    let bytes = image.to_bytes().await.unwrap();

    let response = image.clone().into_response();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.header("content-type").unwrap(), "image/jpeg");
    assert_eq!(response.content(), &bytes[..]);

    let response = image.to_response().await.unwrap();
    assert_eq!(response.header("content-type").unwrap(), "image/jpeg");
    assert_eq!(response.content(), &bytes[..]);

    // Unprocessable images render through the exception handler...
    let broken = Image::from_bytes("not-an-image")
        .cover(1, 1)
        .into_response();
    assert_eq!(broken.status().as_u16(), 500);

    // Lazily loaded images respond once loaded...
    Storage::fake("public")
        .unwrap()
        .put("photo.png", png(8, 8))
        .await
        .unwrap();
    let lazy = Image::from_storage("photo.png", "public");
    assert_eq!(lazy.clone().into_response().status().as_u16(), 500);
    let response = lazy.load().await.unwrap().into_response();
    assert_eq!(response.header("content-type").unwrap(), "image/png");
}

#[tokio::test]
async fn images_can_be_read_from_urls() {
    let _app = app();
    let mut headers = HeaderMap::new();
    headers.insert("content-type", "image/png".parse().unwrap());
    Http::fake_urls([
        (
            "example.com/photo.png",
            HttpResponse::new(200, headers, png(30, 10)),
        ),
        (
            "example.com/missing.png",
            HttpResponse::new(404, HeaderMap::new(), "Not Found"),
        ),
    ]);

    let image = Image::from_url("https://example.com/photo.png");
    Http::assert_sent_count(0);

    let variant = image.cover(10, 10);
    assert_eq!(image.dimensions().await.unwrap(), (30, 10));
    assert_eq!(variant.dimensions().await.unwrap(), (10, 10));
    Http::assert_sent_count(1);

    let error = Image::from_url("https://example.com/missing.png")
        .to_bytes()
        .await
        .unwrap_err();
    assert!(error.is::<RequestException>());
}

#[tokio::test]
async fn images_can_be_read_from_base64() {
    let _app = app();
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(png(12, 6));

    let image = Image::from_base64(encoded);
    assert_eq!(image.dimensions().await.unwrap(), (12, 6));
    assert!(image.to_string().starts_with("data:image/png;base64,"));

    let error = Image::from_base64("%%%").to_bytes().await.unwrap_err();
    assert_eq!(error.to_string(), "Invalid base64 image data.");
    assert!(error.is::<ImageException>());
}

#[tokio::test]
async fn images_can_be_read_from_streams() {
    let _app = app();
    let image = Image::from_stream(std::io::Cursor::new(png(9, 3)));

    assert!(!image.is_loaded());
    let variants = [image.clone(), image.resize(3, 3), image.to_webp()];
    assert_eq!(variants[0].dimensions().await.unwrap(), (9, 3));
    assert_eq!(variants[1].dimensions().await.unwrap(), (3, 3));
    assert_eq!(variants[2].mime_type().await.unwrap(), "image/webp");

    let empty = Image::from_stream(std::io::Cursor::new(Vec::new()));
    assert_eq!(
        empty.to_bytes().await.unwrap_err().to_string(),
        "Invalid stream image data."
    );
}

#[tokio::test]
async fn lazy_contents_are_loaded_once_and_shared() {
    let _app = app();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let image = Image::lazy(move || {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        async { Ok(bytes::Bytes::from(png(8, 8))) }
    });

    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    image.cover(4, 4).to_bytes().await.unwrap();
    image.blur(1).to_bytes().await.unwrap();
    image.to_bytes().await.unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

    let other = Image::lazy(|| async { Ok(bytes::Bytes::from(png(2, 2))) });
    assert_eq!(other.dimensions().await.unwrap(), (2, 2));
}
