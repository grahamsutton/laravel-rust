//! File validation: uploads, MIME types, extensions, sizes and dimensions.

mod common;

use common::container;
use illuminate_http::UploadedFile;
use illuminate_support::{Value, json};
use illuminate_validation::{
    FailCallback, File, Rule, Rules, ValidationContext, ValidationRule, Validator, async_trait,
    rules,
};
use indexmap::IndexMap;

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
    bytes
}

fn png_file(name: &str, width: u32, height: u32) -> UploadedFile {
    UploadedFile::new(name, "image/png", png(width, height))
}

fn kilobytes(name: &str, mime: &str, kb: usize) -> UploadedFile {
    UploadedFile::new(name, mime, vec![0u8; kb * 1024])
}

async fn check(file: UploadedFile, rules: impl Into<Rules>) -> Validator {
    let mut validator = Validator::make(json!({}), rules).with_file("avatar", file);
    validator.passes().await;
    validator
}

async fn first_error(file: UploadedFile, rule: &str) -> Option<String> {
    let validator = check(file, [("avatar", rule)]).await;
    validator.errors().first("avatar").map(str::to_string)
}

#[tokio::test]
async fn file_rule() {
    let _c = container();
    assert_eq!(
        first_error(UploadedFile::fake().create("avatar.jpg", 10), "required|file").await,
        None
    );
    let mut validator = Validator::make(json!({"avatar": "not a file"}), [("avatar", "file")]);
    validator.passes().await;
    assert_eq!(
        validator.errors().first("avatar"),
        Some("The avatar field must be a file.")
    );
}

#[tokio::test]
async fn image_rule() {
    let _c = container();
    assert_eq!(
        first_error(UploadedFile::fake().create("avatar.jpg", 10), "image").await,
        None
    );
    assert_eq!(
        first_error(png_file("avatar.png", 10, 10), "image").await,
        None
    );
    assert_eq!(
        first_error(UploadedFile::fake().create("doc.pdf", 10), "image")
            .await
            .as_deref(),
        Some("The avatar field must be an image.")
    );
    let svg = UploadedFile::new(
        "logo.svg",
        "image/svg+xml",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".to_vec(),
    );
    assert!(first_error(svg.clone(), "image").await.is_some());
    assert_eq!(first_error(svg, "image:allow_svg").await, None);
}

#[tokio::test]
async fn mimes_and_mimetypes() {
    let _c = container();
    // The contents win over the client's claims.
    let disguised = UploadedFile::new("photo.txt", "text/plain", png(1, 1));
    assert_eq!(first_error(disguised, "mimes:jpg,png").await, None);
    assert_eq!(
        first_error(
            UploadedFile::new("notes.txt", "text/plain", b"hello".to_vec()),
            "mimes:jpg,png"
        )
        .await
        .as_deref(),
        Some("The avatar field must be a file of type: jpg, png.")
    );
    assert_eq!(
        first_error(UploadedFile::fake().create("photo.jpeg", 1), "mimes:jpg").await,
        None
    );
    assert_eq!(
        first_error(png_file("a.png", 1, 1), "mimetypes:image/*").await,
        None
    );
    assert_eq!(
        first_error(png_file("a.png", 1, 1), "mimetypes:image/png,image/gif").await,
        None
    );
    assert_eq!(
        first_error(png_file("a.png", 1, 1), "mimetypes:video/avi")
            .await
            .as_deref(),
        Some("The avatar field must be a file of type: video/avi.")
    );
    // PHP files are refused unless explicitly allowed.
    assert!(
        first_error(
            UploadedFile::new("shell.php", "image/png", png(1, 1)),
            "mimes:png"
        )
        .await
        .is_some()
    );
}

#[tokio::test]
async fn extensions_rule() {
    let _c = container();
    assert_eq!(
        first_error(png_file("photo.PNG", 1, 1), "extensions:jpg,png").await,
        None
    );
    assert_eq!(
        first_error(png_file("photo.gif", 1, 1), "extensions:jpg,png")
            .await
            .as_deref(),
        Some("The avatar field must have one of the following extensions: jpg, png.")
    );
}

#[tokio::test]
async fn file_sizes_are_in_kilobytes() {
    let _c = container();
    assert_eq!(
        first_error(kilobytes("a.jpg", "image/jpeg", 2), "max:2").await,
        None
    );
    assert_eq!(
        first_error(kilobytes("a.jpg", "image/jpeg", 2), "max:1")
            .await
            .as_deref(),
        Some("The avatar field must not be greater than 1 kilobytes.")
    );
    assert_eq!(
        first_error(kilobytes("a.jpg", "image/jpeg", 2), "min:3")
            .await
            .as_deref(),
        Some("The avatar field must be at least 3 kilobytes.")
    );
    assert_eq!(
        first_error(kilobytes("a.jpg", "image/jpeg", 2), "size:2").await,
        None
    );
    assert_eq!(
        first_error(kilobytes("a.jpg", "image/jpeg", 2), "between:3,4")
            .await
            .as_deref(),
        Some("The avatar field must be between 3 and 4 kilobytes.")
    );
}

#[tokio::test]
async fn dimensions_rule() {
    let _c = container();
    let photo = || png_file("photo.png", 640, 480);
    assert_eq!(
        first_error(photo(), "dimensions:min_width=100,min_height=200").await,
        None
    );
    assert_eq!(
        first_error(photo(), "dimensions:max_width=100")
            .await
            .as_deref(),
        Some("The avatar field has invalid image dimensions.")
    );
    assert_eq!(
        first_error(photo(), "dimensions:width=640,height=480").await,
        None
    );
    assert_eq!(first_error(photo(), "dimensions:ratio=4/3").await, None);
    assert!(first_error(photo(), "dimensions:ratio=3/2").await.is_some());
    assert_eq!(
        first_error(photo(), "dimensions:min_ratio=1/2,max_ratio=3/2").await,
        None
    );
    assert!(
        first_error(photo(), "dimensions:min_ratio=3/2")
            .await
            .is_some()
    );

    let validator = check(photo(), rules! { "avatar" => [Rule::dimensions().max_width(1000).max_height(500).ratio_str("4/3")] }).await;
    assert!(validator.errors().is_empty());

    let svg = UploadedFile::new("logo.svg", "image/svg+xml", b"<svg></svg>".to_vec());
    assert_eq!(first_error(svg, "dimensions:max_width=1").await, None);
    assert!(
        first_error(UploadedFile::fake().create("doc.pdf", 1), "dimensions:max_width=1")
            .await
            .is_some()
    );
}

#[tokio::test]
async fn fluent_file_rules() {
    let _c = container();
    let validator = check(
        png_file("a.png", 1, 1),
        rules! { "avatar" => [File::types(["png", "jpg"]).max(1024)] },
    )
    .await;
    assert!(validator.errors().is_empty());

    let validator = check(
        png_file("a.png", 1, 1),
        rules! { "avatar" => [File::types(["mp3", "wav"])] },
    )
    .await;
    assert_eq!(
        validator.errors().first("avatar"),
        Some("The avatar field must be a file of type: mp3, wav.")
    );

    let validator = check(
        png_file("a.png", 2000, 10),
        rules! { "avatar" => [File::image(false).max("1mb").dimensions(Rule::dimensions().max_width(1000))] },
    )
    .await;
    assert_eq!(
        validator.errors().first("avatar"),
        Some("The avatar field has invalid image dimensions.")
    );

    let validator = check(
        kilobytes("a.jpg", "image/jpeg", 2),
        rules! { "avatar" => [Rule::file().min("3kb")] },
    )
    .await;
    assert_eq!(
        validator.errors().first("avatar"),
        Some("The avatar field must be at least 3 kilobytes.")
    );

    let validator = check(
        png_file("a.png", 1, 1),
        rules! { "avatar" => [Rule::image_file().extensions(["jpg"])] },
    )
    .await;
    assert_eq!(
        validator.errors().first("avatar"),
        Some("The avatar field must have one of the following extensions: jpg.")
    );
}

#[tokio::test]
async fn failed_uploads_are_reported() {
    let _c = container();
    let broken = UploadedFile::new("", "image/png", png(1, 1));
    let validator = check(broken, [("avatar", "required|image|max:10")]).await;
    assert_eq!(
        validator.errors().get("avatar"),
        vec!["The avatar failed to upload."]
    );
    assert!(validator.failed()["avatar"].contains_key("uploaded"));
}

#[tokio::test]
async fn multiple_files_are_validated_with_wildcards() {
    let _c = container();
    let mut files = IndexMap::new();
    files.insert(
        "photos".to_string(),
        vec![png_file("a.png", 1, 1), UploadedFile::fake().create("b.pdf", 1)],
    );
    let mut validator = Validator::make(
        json!({}),
        rules! { "photos" => "required|array|max:2", "photos.*" => "image" },
    )
    .with_files(files);
    assert!(validator.fails().await);
    assert_eq!(validator.errors().keys(), vec!["photos.1"]);
    assert_eq!(
        validator.errors().first("photos.1"),
        Some("The photos.1 field must be an image.")
    );

    // A single file sent as `photos[]` is still a list.
    let mut files = IndexMap::new();
    files.insert("photos[]".to_string(), vec![png_file("a.png", 1, 1)]);
    let mut validator = Validator::make(
        json!({}),
        rules! { "photos" => "array", "photos.*" => "image" },
    )
    .with_files(files);
    assert!(validator.passes().await);

    // ...and so is a lone file addressed with a wildcard.
    let mut validator = Validator::make(json!({}), rules! { "photos.*" => "image" })
        .with_file("photos", UploadedFile::fake().create("x.pdf", 1));
    assert!(validator.fails().await);
    assert!(validator.errors().has("photos.0"));
}

#[tokio::test]
async fn nested_file_inputs_use_dot_notation() {
    let _c = container();
    let mut validator = Validator::make(
        json!({"user": {"name": "Taylor"}}),
        rules! { "user.name" => "required", "user.avatar" => "required|image" },
    )
    .with_file("user[avatar]", png_file("me.png", 1, 1));
    assert!(validator.passes().await);
    assert_eq!(
        validator.validated().unwrap(),
        json!({"user": {"name": "Taylor"}})
    );
    let files = validator.validated_files();
    assert_eq!(files["user.avatar"].client_original_name(), "me.png");
    assert_eq!(
        validator
            .safe()
            .unwrap()
            .file("user.avatar")
            .unwrap()
            .client_original_name(),
        "me.png"
    );
}

#[tokio::test]
async fn files_are_required_and_present() {
    let _c = container();
    let validator = check(png_file("a.png", 1, 1), [("avatar", "required")]).await;
    assert!(validator.errors().is_empty());
    let mut validator = Validator::make(json!({}), [("avatar", "required|file")]);
    validator.passes().await;
    assert_eq!(
        validator.errors().first("avatar"),
        Some("The avatar field is required.")
    );
}

#[tokio::test]
async fn file_encoding() {
    let _c = container();
    let good = UploadedFile::new("a.csv", "text/csv", "naïve,café".as_bytes().to_vec());
    assert_eq!(first_error(good, "encoding:utf-8").await, None);
    let bad = UploadedFile::new("a.csv", "text/csv", vec![0xff, 0xfe, 0x41]);
    assert_eq!(
        first_error(bad, "encoding:utf-8").await.as_deref(),
        Some("The avatar field must be encoded in utf-8.")
    );
}

struct SmallerThan(usize);

#[async_trait]
impl ValidationRule for SmallerThan {
    async fn validate_with(
        &self,
        _: &str,
        value: &Value,
        context: &ValidationContext<'_>,
        fail: &mut FailCallback<'_>,
    ) {
        match context.uploaded_file(value) {
            Some(file) if file.size() < self.0 => {}
            Some(_) => fail("The :attribute is too big."),
            None => fail("The :attribute must be a file."),
        }
    }
}

#[tokio::test]
async fn rule_objects_can_inspect_files() {
    let _c = container();
    let validator = check(
        UploadedFile::new("a.txt", "text/plain", vec![1, 2, 3]),
        rules! { "avatar" => [SmallerThan(10)] },
    )
    .await;
    assert!(validator.errors().is_empty());
    let validator = check(
        UploadedFile::new("a.txt", "text/plain", vec![0; 20]),
        rules! { "avatar" => [SmallerThan(10)] },
    )
    .await;
    assert_eq!(
        validator.errors().first("avatar"),
        Some("The avatar is too big.")
    );
}
