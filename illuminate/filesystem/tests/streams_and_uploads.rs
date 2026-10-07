//! Streams, temporary upload URLs, and custom file serving.

use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_filesystem::{
    FilesystemAdapter, FilesystemServiceProvider, ReceiveFile, ServeFile, Storage, UrlSigner,
    Visibility,
};
use illuminate_http::{HeaderMap, HttpException, Method, Request, Response, Version};
use illuminate_support::{Carbon, Error, Result, Value, json};

struct Signer;

impl UrlSigner for Signer {
    fn temporary_signed_route(
        &self,
        name: &str,
        expiration: Carbon,
        parameters: Value,
    ) -> Result<String> {
        let upload = if parameters["upload"] == json!(true) {
            "&upload=1"
        } else {
            ""
        };
        Ok(format!(
            "/{name}/{}?expires={}{upload}&signature=valid",
            parameters["path"].as_str().unwrap_or_default(),
            expiration.timestamp()
        ))
    }

    fn has_valid_relative_signature(&self, request: &Request) -> bool {
        request.query("signature") == "valid"
    }
}

fn app(root: &std::path::Path) -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(json!({
        "app": {"url": "https://example.com"},
        "filesystems": {"default": "local", "disks": {
            "local": {"driver": "local", "root": root.join("private").to_string_lossy(), "serve": true},
            "public": {"driver": "local", "root": root.join("public").to_string_lossy(), "visibility": "public"},
        }},
    })));
    FilesystemServiceProvider.register(&container);
    (container, guard)
}

fn status(result: Result<Response>) -> u16 {
    match result {
        Ok(response) => response.status_code(),
        Err(error) => error
            .downcast_ref::<HttpException>()
            .map_or(500, |e| e.status),
    }
}

fn chunks(parts: Vec<&'static str>) -> impl futures::Stream<Item = Result<Bytes>> + Send + 'static {
    futures::stream::iter(parts.into_iter().map(|part| Ok(Bytes::from(part))))
}

#[tokio::test]
async fn files_can_be_read_and_written_as_streams() {
    let root = tempfile::tempdir().unwrap();
    let disk = FilesystemAdapter::local(root.path());

    // A file bigger than one chunk arrives in several.
    let big = vec![b'x'; 150 * 1024];
    let parts: Vec<Result<Bytes>> = big
        .chunks(10_000)
        .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
        .collect();
    assert!(
        disk.write_stream("nested/dir/big.bin", futures::stream::iter(parts))
            .await
            .unwrap()
    );
    assert_eq!(disk.size("nested/dir/big.bin").await.unwrap(), 150 * 1024);

    let mut stream = disk.read_stream("nested/dir/big.bin").await.unwrap();
    let mut received = Vec::new();
    let mut count = 0;
    while let Some(chunk) = stream.next().await {
        received.extend_from_slice(&chunk.unwrap());
        count += 1;
    }
    assert_eq!(received, big);
    assert_eq!(count, 3);

    // Streams pipe straight into responses.
    let response = Response::stream(disk.read_stream("nested/dir/big.bin").await.unwrap());
    assert_eq!(response.into_bytes().await.unwrap().len(), 150 * 1024);

    assert!(disk.read_stream("missing.txt").await.is_err());
    assert!(disk.read_stream("nested").await.is_err());
    assert!(disk.read_stream("../escape.txt").await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn streamed_writes_respect_visibility() {
    let root = tempfile::tempdir().unwrap();
    let disk = FilesystemAdapter::local(root.path());
    assert!(
        disk.write_stream_with_visibility("a.txt", chunks(vec!["public"]), Visibility::Public)
            .await
            .unwrap()
    );
    assert_eq!(
        disk.get_visibility("a.txt").await.unwrap(),
        Visibility::Public
    );
}

#[tokio::test]
async fn failing_streams_follow_the_throw_option() {
    let root = tempfile::tempdir().unwrap();
    let disk = FilesystemAdapter::local(root.path());
    let failing = || {
        futures::stream::iter(vec![
            Ok(Bytes::from("partial")),
            Err(Error::msg("connection lost")),
        ])
    };
    assert!(!disk.write_stream("a.txt", failing()).await.unwrap());

    let throwing = FilesystemAdapter::new(
        disk.get_driver(),
        json!({"driver": "local", "root": root.path().to_string_lossy(), "throw": true}),
    );
    let error = throwing.write_stream("a.txt", failing()).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Unable to write file at location: a.txt"),
        "{error}"
    );

    let read_only = FilesystemAdapter::new(
        disk.get_driver(),
        json!({"driver": "local", "root": root.path().to_string_lossy(), "read-only": true}),
    );
    assert!(
        !read_only
            .write_stream("b.txt", chunks(vec!["x"]))
            .await
            .unwrap()
    );
    assert!(!disk.exists("b.txt").await.unwrap());
}

#[tokio::test]
async fn the_storage_facade_streams_through_the_default_disk() {
    let root = tempfile::tempdir().unwrap();
    let (_container, _guard) = app(root.path());
    assert!(
        Storage::write_stream("ab.txt", chunks(vec!["a", "b"]))
            .await
            .unwrap()
    );
    let collected: Vec<Bytes> = Storage::read_stream("ab.txt")
        .await
        .unwrap()
        .map(|chunk| chunk.unwrap())
        .collect()
        .await;
    assert_eq!(collected.concat(), b"ab");
}

#[tokio::test]
async fn served_local_disks_sign_temporary_upload_urls() {
    let root = tempfile::tempdir().unwrap();
    let (container, _guard) = app(root.path());

    let local = Storage::disk("local").unwrap();
    assert!(!local.provides_temporary_upload_urls());
    let error = local
        .temporary_upload_url("a.txt", Carbon::from_timestamp(100))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "This driver does not support creating temporary upload URLs."
    );

    container.instance_arc::<dyn UrlSigner>(Arc::new(Signer));
    assert!(local.provides_temporary_upload_urls());
    assert!(Storage::provides_temporary_upload_urls().unwrap());
    let upload = Storage::temporary_upload_url("docs/a.txt", Carbon::from_timestamp(100)).unwrap();
    assert_eq!(
        upload.url,
        "/storage.local.upload/docs/a.txt?expires=100&upload=1&signature=valid"
    );
    assert!(upload.headers.is_empty());

    // Disks that aren't served can't sign upload URLs.
    assert!(
        !Storage::disk("public")
            .unwrap()
            .provides_temporary_upload_urls()
    );
}

fn upload(uri: &str, body: &'static str) -> Request {
    Request::from_parts(
        Method::PUT,
        uri.parse().unwrap(),
        Version::HTTP_11,
        HeaderMap::new(),
        Bytes::from_static(body.as_bytes()),
        None,
    )
}

#[tokio::test]
async fn uploads_are_received_with_a_valid_signature() {
    let root = tempfile::tempdir().unwrap();
    let (container, _guard) = app(root.path());
    container.instance_arc::<dyn UrlSigner>(Arc::new(Signer));
    let config = Storage::disk("local").unwrap().get_config().clone();
    let receive = ReceiveFile::new("local", config.clone(), false);

    let request = upload("/storage/docs/a.txt?upload=1&signature=valid", "Hello");
    assert_eq!(status(receive.handle(&request, "docs/a.txt").await), 204);
    assert_eq!(Storage::get("docs/a.txt").await.unwrap(), "Hello");

    // Without the upload flag, or a valid signature, uploads are refused.
    let request = upload("/storage/docs/b.txt?signature=valid", "Nope");
    assert_eq!(status(receive.handle(&request, "docs/b.txt").await), 403);
    let request = upload("/storage/docs/b.txt?upload=1&signature=forged", "Nope");
    assert_eq!(status(receive.handle(&request, "docs/b.txt").await), 403);
    let production = ReceiveFile::new("local", config.clone(), true);
    assert_eq!(status(production.handle(&request, "docs/b.txt").await), 404);
    assert!(!Storage::exists("docs/b.txt").await.unwrap());

    // Traversal is a 404.
    let request = upload("/storage/x?upload=1&signature=valid", "Nope");
    assert_eq!(status(receive.handle(&request, "../escape.txt").await), 404);

    // Upload URLs can't be used to read files.
    let serve = ServeFile::new("local", config, false);
    let request = Request::create("/storage/docs/a.txt?upload=1&signature=valid", "GET");
    assert_eq!(status(serve.handle(&request, "docs/a.txt").await), 403);
}

#[tokio::test]
async fn disks_can_serve_files_their_own_way() {
    let root = tempfile::tempdir().unwrap();
    let (container, _guard) = app(root.path());
    container.instance_arc::<dyn UrlSigner>(Arc::new(Signer));
    let disk = Storage::disk("local").unwrap();
    disk.put("photo.jpg", "jpeg").await.unwrap();

    disk.serve_using(|request, path, headers| async move {
        let mut response = Response::new(format!("{} {path} {}", request.path(), headers.len()));
        response.set_header("x-served-by", "cdn");
        Ok(response)
    });

    let serve = ServeFile::new("local", disk.get_config().clone(), false);
    let request = Request::create("/storage/photo.jpg?signature=valid", "GET");
    let response = serve.handle(&request, "photo.jpg").await.unwrap();
    assert_eq!(response.content_string(), "storage/photo.jpg photo.jpg 2");
    assert_eq!(response.header("x-served-by").unwrap(), "cdn");
    // The security headers are always applied.
    assert_eq!(
        response.header("content-security-policy").unwrap(),
        "default-src 'none'; style-src 'unsafe-inline'; sandbox"
    );

    // Without a callback, the file is served inline with the given headers.
    let plain = Storage::disk("public").unwrap();
    plain.put("a.txt", "A").await.unwrap();
    let response = plain
        .serve(
            &Request::create("/", "GET"),
            "a.txt",
            None,
            &[("x-extra", "1")],
        )
        .await
        .unwrap();
    assert_eq!(response.content_string(), "A");
    assert_eq!(response.header("x-extra").unwrap(), "1");
    assert_eq!(
        response.header("content-disposition").unwrap(),
        "inline; filename=a.txt"
    );
}

#[tokio::test]
async fn fake_disks_build_temporary_upload_urls() {
    let root = tempfile::tempdir().unwrap();
    let (_container, _guard) = app(root.path());
    let fake = Storage::fake("s3").unwrap();
    assert!(fake.provides_temporary_upload_urls());
    let upload = fake
        .temporary_upload_url("photos/1.jpg", Carbon::from_timestamp(1_700_000_000))
        .unwrap();
    assert_eq!(
        upload.url,
        "https://example.com/photos/1.jpg?expiration=1700000000"
    );
    assert!(upload.headers.is_empty());
    assert_eq!(
        fake.temporary_url("photos/1.jpg", Carbon::from_timestamp(5))
            .unwrap(),
        "https://example.com/photos/1.jpg?expiration=5"
    );
}
