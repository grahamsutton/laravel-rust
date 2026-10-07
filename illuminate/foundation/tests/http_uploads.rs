//! Uploading files from tests: `UploadedFile::fake()` and `TestApp::attach`.

use illuminate_filesystem::{Storage, UploadedFileExt};
use illuminate_foundation::Application;
use illuminate_foundation::testing::TestApp;
use illuminate_http::{Request, UploadedFile};
use illuminate_routing::Route;
use illuminate_support::{Error, json};
use illuminate_validation::{Validator, rules};

fn test_app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let builder = Application::configure_detached(dir.path()).with_routing(|routing| {
        routing.web(|| {
            Route::post("/avatar", |request: Request| async move {
                let file = request.file("avatar").expect("an avatar was uploaded");
                let path = file.store_on("avatars", "avatars").await?;
                Ok::<_, Error>(json!({
                    "path": path,
                    "name": request.input("name"),
                    "original": file.client_original_name(),
                    "mime": file.mime_type(),
                    "size": file.size(),
                    "dimensions": file.dimensions(),
                }))
            });
            Route::put("/documents/{id}", |request: Request| async move {
                Validator::make(request.all(), rules! {
                    "title" => "required",
                    "document" => "required|file|mimes:pdf|max:1024",
                })
                .with_files(request.all_files())
                .validate()
                .await?;
                Ok::<_, Error>(json!({
                    "method": request.method().as_str(),
                    "content_type": request.header("content-type"),
                    "title": request.input("title"),
                }))
            });
            Route::post("/photos", |request: Request| async move {
                let names: Vec<String> = request
                    .files("photos")
                    .iter()
                    .map(|file| file.client_original_name().to_string())
                    .collect();
                json!({
                    "names": names,
                    "second": request.file("photos.1").map(|f| f.client_original_name().to_string()),
                })
            });
            Route::post("/api/import", |request: Request| async move {
                json!({
                    "json": request.is_json(),
                    "rows": request.input("rows"),
                    "file": request.file("csv").map(|f| f.get()),
                })
            });
        });
    });
    (TestApp::new(builder), dir)
}

#[tokio::test]
async fn avatars_can_be_uploaded() {
    let (mut app, _dir) = test_app();
    let disk = Storage::fake("avatars").unwrap();

    let file = UploadedFile::fake().image("avatar.jpg", 200, 200);

    let response = app
        .attach("avatar", file.clone())
        .post("/avatar", json!({"name": "Taylor"}))
        .await;

    response.assert_ok().assert_json(json!({
        "path": format!("avatars/{}", file.hash_name()),
        "name": "Taylor",
        "original": "avatar.jpg",
        "mime": "image/jpeg",
        "dimensions": [200, 200],
    }));
    disk.assert_exists(format!("avatars/{}", file.hash_name()).as_str())
        .await;
    disk.assert_count("avatars", 1).await;

    // Files are only sent with the request they were attached to.
    let response = app.post("/api/import", json!({})).await;
    response.assert_ok().assert_json_path("file", json!(null));
}

#[tokio::test]
async fn uploads_are_validated_like_real_ones() {
    let (mut app, _dir) = test_app();

    let document =
        UploadedFile::fake().create_with_mime_type("contract.pdf", 512, "application/pdf");
    let response = app
        .attach("document", document)
        .put("/documents/1", json!({"title": "Contract"}))
        .await;
    response
        .assert_ok()
        .assert_json(json!({"method": "PUT", "title": "Contract"}));
    assert!(
        response
            .json_path("content_type")
            .as_str()
            .unwrap()
            .starts_with("multipart/form-data; boundary=")
    );

    // Too big: 2 MB reported for a 1 MB limit.
    let response = app
        .attach(
            "document",
            UploadedFile::fake().create("contract.pdf", 2048),
        )
        .put_json("/documents/1", json!({"title": "Contract"}))
        .await;
    response
        .assert_unprocessable()
        .assert_json_validation_errors(&["document"])
        .assert_json_path(
            "errors.document.0",
            "The document field must not be greater than 1024 kilobytes.",
        );

    // Spoofed methods work for multipart forms, too.
    let response = app
        .attach("document", UploadedFile::fake().create("contract.pdf", 10))
        .post(
            "/documents/1",
            json!({"_method": "PUT", "title": "Spoofed"}),
        )
        .await;
    response.assert_ok().assert_json_path("method", "PUT");
}

#[tokio::test]
async fn many_files_can_be_uploaded_under_one_name() {
    let (mut app, _dir) = test_app();

    let response = app
        .attach_many(
            "photos[]",
            [
                UploadedFile::fake().image("one.png", 10, 10),
                UploadedFile::fake().image("two.gif", 10, 10),
            ],
        )
        .post("/photos", json!({}))
        .await;

    response
        .assert_ok()
        .assert_json(json!({"names": ["one.png", "two.gif"], "second": "two.gif"}));
}

#[tokio::test]
async fn json_requests_carry_files_next_to_their_body() {
    let (mut app, _dir) = test_app();

    let response = app
        .attach(
            "csv",
            UploadedFile::fake().create_with_content("rows.csv", "id\n1\n"),
        )
        .post_json("/api/import", json!({"rows": 1}))
        .await;

    response
        .assert_ok()
        .assert_exact_json(json!({"json": true, "rows": 1, "file": "id\n1\n"}));
}
