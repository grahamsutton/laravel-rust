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
    fn temporary_signed_route(&self, name: &str, expiration: Carbon, parameters: Value) -> Result<String>;

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
        Self { disk: disk.into(), config, is_production }
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
        match self.serve(path).await {
            Err(error) if error.is::<PathTraversalDetected>() => Err(HttpException::new(404).into()),
            other => other,
        }
    }

    async fn serve(&self, path: &str) -> Result<Response> {
        let disk = Storage::disk(&self.disk)?;
        if !disk.exists(path).await? {
            return Err(HttpException::new(404).into());
        }
        let mut response = disk.response(path, None).await?;
        response.set_header("cache-control", "no-store, no-cache, must-revalidate, max-age=0");
        response.set_header("content-security-policy", "default-src 'none'; style-src 'unsafe-inline'; sandbox");
        Ok(response)
    }

    fn has_valid_signature(&self, request: &Request) -> bool {
        let upload = match request.query("upload") {
            Value::String(value) => matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"),
            other => other.truthy(),
        };
        let public = self.config.get("visibility").and_then(Value::as_str) == Some("public");
        !upload
            && (public || try_app::<dyn UrlSigner>().is_some_and(|signer| signer.has_valid_relative_signature(request)))
    }
}
