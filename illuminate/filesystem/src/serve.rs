//! Serving files from local disks, and signed temporary URLs.
//!
//! A local disk configured with `'serve' => true` can hand out temporary,
//! signed URLs to its files. Signing URLs is the routing component's job, so
//! the filesystem asks for a [`UrlSigner`] in the container; the routing (or
//! foundation) component binds one:
//!
//! ```ignore
//! app.singleton::<dyn UrlSigner>(|_| Arc::new(MyRoutingUrlSigner));
//! ```
//!
//! The matching route — `GET {disk url path}/{path}` named `storage.{disk}`
//! — should call [`ServeFile::handle`]; [`FilesystemManager::served_disks`]
//! lists the disks (and URIs) that need one.
//!
//! [`FilesystemManager::served_disks`]: crate::FilesystemManager::served_disks

use illuminate_container::try_app;
use illuminate_http::{HttpException, Request, Response};
use illuminate_support::{Carbon, Result, Value, ValueExt};

use crate::Storage;
use crate::exceptions::PathTraversalDetected;

/// Signs and verifies URLs on behalf of the filesystem (implemented by the
/// routing component's URL generator).
pub trait UrlSigner: Send + Sync {
    /// Create a temporary signed URL to the named route.
    fn temporary_signed_route(
        &self,
        name: &str,
        expiration: Carbon,
        parameters: Value,
    ) -> Result<String>;

    /// Determine if the request has a valid signature for a relative URL.
    fn has_valid_relative_signature(&self, request: &Request) -> bool;
}

/// A local disk that should be served over HTTP.
#[derive(Debug, Clone, PartialEq)]
pub struct ServedDisk {
    /// The disk's name.
    pub disk: String,
    /// The URI prefix files are served from (`/storage` by default).
    pub uri: String,
    /// The disk's configuration.
    pub config: Value,
}

impl ServedDisk {
    /// The name of the route serving this disk (`storage.{disk}`).
    pub fn route_name(&self) -> String {
        format!("storage.{}", self.disk)
    }

    /// The route URI pattern, with a catch-all `{path}` parameter.
    pub fn route_uri(&self) -> String {
        format!("{}/{{path}}", self.uri)
    }

    /// The name of the route receiving uploads for this disk
    /// (`storage.{disk}.upload`): a `PUT` to [`route_uri`](Self::route_uri)
    /// handled by [`ReceiveFile`].
    pub fn upload_route_name(&self) -> String {
        format!("storage.{}.upload", self.disk)
    }
}

/// Serves a file from a local disk: Laravel's `ServeFile` route action.
#[derive(Debug, Clone)]
pub struct ServeFile {
    disk: String,
    config: Value,
    is_production: bool,
}

impl ServeFile {
    /// Create a new file server for the given disk.
    pub fn new(disk: impl Into<String>, config: Value, is_production: bool) -> Self {
        Self {
            disk: disk.into(),
            config,
            is_production,
        }
    }

    /// Handle the request for the given path.
    ///
    /// Requests without a valid signature are rejected (`403`, or `404` in
    /// production) unless the disk is public; missing files and traversal
    /// attempts are a `404`.
    pub async fn handle(&self, request: &Request, path: &str) -> Result<Response> {
        if !self.has_valid_signature(request) {
            return Err(HttpException::new(if self.is_production { 404 } else { 403 }).into());
        }
        match self.serve(request, path).await {
            Err(error) if error.is::<PathTraversalDetected>() => {
                Err(HttpException::new(404).into())
            }
            other => other,
        }
    }

    async fn serve(&self, request: &Request, path: &str) -> Result<Response> {
        let disk = Storage::disk(&self.disk)?;
        if !disk.exists(path).await? {
            return Err(HttpException::new(404).into());
        }
        let headers = [
            ("cache-control", "no-store, no-cache, must-revalidate, max-age=0"),
            (
                "content-security-policy",
                "default-src 'none'; style-src 'unsafe-inline'; sandbox",
            ),
        ];
        let mut response = disk.serve(request, path, None, &headers).await?;
        for (name, value) in headers {
            response.set_header(name, value);
        }
        Ok(response)
    }

    fn has_valid_signature(&self, request: &Request) -> bool {
        let upload = is_upload(request);
        let public = self.config.get("visibility").and_then(Value::as_str) == Some("public");
        !upload
            && (public
                || try_app::<dyn UrlSigner>()
                    .is_some_and(|signer| signer.has_valid_relative_signature(request)))
    }
}

/// Receives files uploaded to a local disk's temporary upload URLs:
/// Laravel's `ReceiveFile` route action, for the `PUT` route named
/// `storage.{disk}.upload`.
#[derive(Debug, Clone)]
pub struct ReceiveFile {
    disk: String,
    is_production: bool,
}

impl ReceiveFile {
    /// Create a new upload receiver for the given disk.
    pub fn new(disk: impl Into<String>, _config: Value, is_production: bool) -> Self {
        Self {
            disk: disk.into(),
            is_production,
        }
    }

    /// Store the request's body at the given path. Requests without a
    /// valid upload signature are rejected (`403`, or `404` in production),
    /// and traversal attempts are a `404`. Success is `204 No Content`.
    pub async fn handle(&self, request: &Request, path: &str) -> Result<Response> {
        if !self.has_valid_signature(request) {
            return Err(HttpException::new(if self.is_production { 404 } else { 403 }).into());
        }
        let disk = Storage::disk(&self.disk)?;
        match disk.put(path, request.body()).await {
            Ok(_) => Ok(Response::no_content()),
            Err(error) if error.is::<PathTraversalDetected>() => Err(HttpException::new(404).into()),
            Err(error) => Err(error),
        }
    }

    fn has_valid_signature(&self, request: &Request) -> bool {
        is_upload(request)
            && try_app::<dyn UrlSigner>()
                .is_some_and(|signer| signer.has_valid_relative_signature(request))
    }
}

/// Whether the request's `upload` query parameter is truthy (PHP's
/// `FILTER_VALIDATE_BOOLEAN`).
fn is_upload(request: &Request) -> bool {
    match request.query("upload") {
        Value::String(value) => matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        ),
        other => other.truthy(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use illuminate_config::Repository as Config;
    use illuminate_container::{Container, ServiceProvider};
    use illuminate_support::json;

    use super::*;
    use crate::FilesystemServiceProvider;

    struct Signer;

    impl UrlSigner for Signer {
        fn temporary_signed_route(
            &self,
            name: &str,
            expiration: Carbon,
            parameters: Value,
        ) -> Result<String> {
            Ok(format!(
                "/signed/{name}/{}?expires={}&signature=valid",
                parameters["path"].as_str().unwrap_or_default(),
                expiration.timestamp()
            ))
        }

        fn has_valid_relative_signature(&self, request: &Request) -> bool {
            request.query("signature") == "valid"
        }
    }

    fn setup() -> (
        Arc<Container>,
        illuminate_container::LocalInstanceGuard,
        tempfile::TempDir,
    ) {
        let root = tempfile::tempdir().unwrap();
        let container = Arc::new(Container::new());
        let guard = Container::set_local_instance(container.clone());
        container.instance(Config::new(json!({
            "filesystems": {"default": "local", "disks": {
                "local": {"driver": "local", "root": root.path().join("private").to_string_lossy(), "serve": true},
                "public": {"driver": "local", "root": root.path().join("public").to_string_lossy(), "visibility": "public"},
            }},
        })));
        FilesystemServiceProvider.register(&container);
        (container, guard, root)
    }

    fn status(result: Result<Response>) -> u16 {
        match result {
            Ok(response) => response.status_code(),
            Err(error) => error
                .downcast_ref::<HttpException>()
                .map(|e| e.status)
                .unwrap_or(500),
        }
    }

    #[tokio::test]
    async fn private_files_need_a_valid_signature() {
        let (container, _guard, _root) = setup();
        Storage::disk("local")
            .unwrap()
            .put("invoice.txt", "paid")
            .await
            .unwrap();
        let config = Storage::disk("local").unwrap().get_config().clone();
        let serve = ServeFile::new("local", config.clone(), false);

        let request = Request::create("/storage/invoice.txt?signature=valid", "GET");
        assert_eq!(status(serve.handle(&request, "invoice.txt").await), 403);
        let production = ServeFile::new("local", config, true);
        assert_eq!(
            status(production.handle(&request, "invoice.txt").await),
            404
        );

        container.instance_arc::<dyn UrlSigner>(Arc::new(Signer));
        let response = serve.handle(&request, "invoice.txt").await.unwrap();
        assert_eq!(response.content_string(), "paid");
        assert_eq!(
            response.header("cache-control").unwrap(),
            "no-store, no-cache, must-revalidate, max-age=0"
        );
        assert_eq!(
            response.header("content-security-policy").unwrap(),
            "default-src 'none'; style-src 'unsafe-inline'; sandbox"
        );
        assert_eq!(
            response.header("content-disposition").unwrap(),
            "inline; filename=invoice.txt"
        );

        let unsigned = Request::create("/storage/invoice.txt?signature=forged", "GET");
        assert_eq!(status(serve.handle(&unsigned, "invoice.txt").await), 403);
        let upload = Request::create("/storage/invoice.txt?signature=valid&upload=1", "GET");
        assert_eq!(status(serve.handle(&upload, "invoice.txt").await), 403);
        assert_eq!(status(serve.handle(&request, "missing.txt").await), 404);
        assert_eq!(
            status(serve.handle(&request, "../public/secret.txt").await),
            404
        );
    }

    #[tokio::test]
    async fn public_disks_are_served_without_a_signature() {
        let (_container, _guard, _root) = setup();
        let disk = Storage::disk("public").unwrap();
        disk.put("logo.txt", "logo").await.unwrap();
        let serve = ServeFile::new("public", disk.get_config().clone(), true);
        let response = serve
            .handle(&Request::create("/storage/logo.txt", "GET"), "logo.txt")
            .await
            .unwrap();
        assert_eq!(response.content_string(), "logo");
    }

    #[tokio::test]
    async fn served_disks_sign_temporary_urls() {
        let (container, _guard, _root) = setup();
        let disk = Storage::disk("local").unwrap();
        let expiration = Carbon::from_timestamp(1_800_000_000);
        assert!(!disk.provides_temporary_urls());
        assert!(disk.temporary_url("a.txt", expiration).is_err());

        container.instance_arc::<dyn UrlSigner>(Arc::new(Signer));
        assert!(disk.provides_temporary_urls());
        assert_eq!(
            disk.temporary_url("a.txt", expiration).unwrap(),
            "/signed/storage.local/a.txt?expires=1800000000&signature=valid"
        );
        // Disks that aren't served can't sign URLs.
        assert!(
            Storage::disk("public")
                .unwrap()
                .temporary_url("a.txt", expiration)
                .is_err()
        );
    }
}
