mod common;

use common::{app, jpeg, png};
use illuminate_filesystem::{FileNotFoundException, Storage, Visibility};
use illuminate_image::{FilesystemImageExt, Image, ImageException};

#[tokio::test]
async fn images_can_be_read_from_storage_lazily() {
    let _app = app();
    let disk = Storage::fake("public").unwrap();
    disk.put("avatars/photo.jpg", jpeg(320, 240)).await.unwrap();

    let image = Image::from_storage("avatars/photo.jpg", "public");
    assert!(!image.is_loaded());

    let avatar = image.cover(100, 100);
    assert_eq!(avatar.dimensions().await.unwrap(), (100, 100));
    assert!(image.is_loaded(), "variants share their source");
    assert_eq!(image.dimensions().await.unwrap(), (320, 240));

    let default = Storage::fake("local").unwrap();
    default.put("photo.png", png(10, 10)).await.unwrap();
    assert_eq!(
        Image::from_default_disk("photo.png").width().await.unwrap(),
        10
    );
}

#[tokio::test]
async fn images_can_be_read_from_a_disk() {
    let _app = app();
    let disk = Storage::fake("public").unwrap();
    disk.put("photo.png", png(30, 20)).await.unwrap();

    let image = Storage::disk("public").unwrap().image("photo.png");
    assert!(!image.is_loaded());
    assert_eq!(image.dimensions().await.unwrap(), (30, 20));
}

#[tokio::test]
async fn missing_storage_files_fail_when_used() {
    let _app = app();
    Storage::fake("public").unwrap();

    let image = Image::from_storage("missing.jpg", "public");
    assert!(image.to_bytes().await.is_err());

    // Processing failures are wrapped in an image exception...
    let error = image.cover(10, 10).to_bytes().await.unwrap_err();
    let exception = error.downcast_ref::<ImageException>().unwrap();
    assert!(
        exception
            .to_string()
            .starts_with("Failed to process image:")
    );
    assert!(exception.previous().is_some());
}

#[tokio::test]
async fn images_can_be_read_from_local_paths() {
    let _app = app();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("photo.png");
    std::fs::write(&path, png(40, 30)).unwrap();

    let image = Image::from_path(&path);
    assert!(!image.is_loaded());
    assert_eq!(image.dimensions().await.unwrap(), (40, 30));

    // Local files can be displayed without awaiting anything...
    assert!(
        Image::from_path(&path)
            .to_string()
            .starts_with("data:image/png;base64,")
    );

    let error = Image::from_path(directory.path().join("missing.png"))
        .to_bytes()
        .await
        .unwrap_err();
    assert!(error.is::<FileNotFoundException>());
}

#[tokio::test]
async fn images_are_stored_under_a_unique_name() {
    let _app = app();
    let local = Storage::fake("local").unwrap();
    let image = Image::from_bytes(jpeg(100, 100)).cover(50, 50);

    let path = image.store("avatars").await.unwrap();
    assert!(path.starts_with("avatars/"));
    assert!(path.ends_with(".jpg"));
    assert_eq!(path.len(), "avatars/".len() + 40 + ".jpg".len());
    local.assert_exists(path.as_str()).await;

    let stored = Image::from_default_disk(&path);
    assert_eq!(stored.dimensions().await.unwrap(), (50, 50));

    // The generated name is remembered...
    assert_eq!(
        image.hash_name().await.unwrap(),
        path.trim_start_matches("avatars/")
    );
    assert_eq!(image.store("avatars").await.unwrap(), path);
}

#[tokio::test]
async fn hash_names_use_the_processed_extension() {
    let _app = app();
    let image = Image::from_bytes(jpeg(10, 10));

    let jpg = image.hash_name().await.unwrap();
    let webp = image.to_webp().hash_name().await.unwrap();

    assert!(jpg.ends_with(".jpg"));
    assert!(webp.ends_with(".webp"));
    assert_eq!(jpg.trim_end_matches(".jpg"), webp.trim_end_matches(".webp"));
    assert_ne!(
        Image::from_bytes(jpeg(10, 10)).hash_name().await.unwrap(),
        jpg,
        "different images get different names"
    );
}

#[tokio::test]
async fn images_can_be_stored_on_a_given_disk() {
    let _app = app();
    let public = Storage::fake("public").unwrap();
    let image = Image::from_bytes(png(20, 20)).to_webp();

    let path = image.store_on("images", "public").await.unwrap();
    assert!(path.starts_with("images/") && path.ends_with(".webp"));
    public.assert_exists(path.as_str()).await;
    public.assert_count("images", 1).await;
}

#[tokio::test]
async fn images_can_be_stored_with_a_name() {
    let _app = app();
    let local = Storage::fake("local").unwrap();
    let public = Storage::fake("public").unwrap();
    let image = Image::from_bytes(png(20, 20));

    assert_eq!(
        image.store_as("avatars", "avatar.png").await.unwrap(),
        "avatars/avatar.png"
    );
    local.assert_exists("avatars/avatar.png").await;

    assert_eq!(
        image.store_as("", "avatar.png").await.unwrap(),
        "avatar.png"
    );
    local.assert_exists("avatar.png").await;

    assert_eq!(
        image
            .store_as_on("/avatars/", "1.png", "public")
            .await
            .unwrap(),
        "avatars/1.png"
    );
    public.assert_exists("avatars/1.png").await;

    let stored = public.bytes("avatars/1.png").await.unwrap();
    assert_eq!(stored, image.to_bytes().await.unwrap());

    assert!(image.store_as("../escape", "x.png").await.is_err());
}

#[tokio::test]
async fn images_can_be_stored_publicly() {
    let _app = app();
    let local = Storage::fake("local").unwrap();
    let public = Storage::fake("public").unwrap();
    let image = Image::from_bytes(png(20, 20)).to_webp();

    let path = image.store_publicly("images").await.unwrap();
    local.assert_exists(path.as_str()).await;

    let path = image.store_publicly_on("images", "public").await.unwrap();
    public.assert_exists(path.as_str()).await;

    let path = image
        .store_publicly_as("images", "public-avatar.webp")
        .await
        .unwrap();
    assert_eq!(path, "images/public-avatar.webp");

    let path = image
        .store_publicly_as_on("images", "avatar.webp", "public")
        .await
        .unwrap();
    assert_eq!(path, "images/avatar.webp");

    #[cfg(unix)]
    {
        assert_eq!(
            local
                .get_visibility("images/public-avatar.webp")
                .await
                .unwrap(),
            Visibility::Public
        );
        assert_eq!(
            public.get_visibility("images/avatar.webp").await.unwrap(),
            Visibility::Public
        );
    }
}

#[tokio::test]
async fn storing_on_an_unknown_disk_fails() {
    let _app = app();
    let error = Image::from_bytes(png(2, 2))
        .store_on("images", "s4")
        .await
        .unwrap_err();

    assert!(error.to_string().contains("s4"));
}
