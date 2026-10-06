//! A filesystem disk: Laravel's `FilesystemAdapter`.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use bytes::Bytes;

use illuminate_container::try_app;
use illuminate_http::{ExceptionHandler, Response, UploadedFile};
use illuminate_support::error::{InvalidArgumentException, RuntimeException};
use illuminate_support::{Carbon, Error, Result, Str, Value, ValueExt, json};

use crate::driver::{Driver, LocalDriver, Visibility, WriteOptions};
use crate::exceptions::{CorruptedPathDetected, FilesystemException, PathTraversalDetected};
use crate::filesystem::hash_bytes;
use crate::path::{IntoPaths, detect_mime_type, normalize_path, pathinfo};
use crate::serve::UrlSigner;

/// Builds temporary URLs for a disk: `(path, expiration, options) -> url`.
pub type TemporaryUrlCallback = Arc<dyn Fn(&str, Carbon, &Value) -> Result<String> + Send + Sync>;

/// A storage disk.
///
/// Every path you hand a disk is relative to its root, and is normalized
/// before it reaches the driver — a path that tries to climb out of the root
/// (`../../.env`) is rejected with [`PathTraversalDetected`].
///
/// Reading a file that can't be read is an error. Write operations
/// (`put`, `delete`, `copy`, ...) follow Laravel's `throw` option: by
/// default they report failure by returning `Ok(false)`; when the disk is
/// configured with `'throw' => true` they return the underlying error.
///
/// ```
/// use illuminate_filesystem::FilesystemAdapter;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let root = tempfile::tempdir().unwrap();
/// let disk = FilesystemAdapter::local(root.path());
///
/// disk.put("avatars/1.txt", "Taylor").await.unwrap();
///
/// assert!(disk.exists("avatars/1.txt").await.unwrap());
/// assert_eq!(disk.get("avatars/1.txt").await.unwrap(), "Taylor");
/// assert_eq!(disk.files("avatars").await.unwrap(), vec!["avatars/1.txt"]);
/// assert!(disk.get("../outside.txt").await.is_err());
/// # });
/// ```
pub struct FilesystemAdapter {
    driver: Arc<dyn Driver>,
    config: Value,
    disk: String,
    prefix: String,
    temporary_url_callback: RwLock<Option<TemporaryUrlCallback>>,
    _keep_alive: Option<Arc<tempfile::TempDir>>,
}

impl std::fmt::Debug for FilesystemAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesystemAdapter")
            .field("disk", &self.disk)
            .field("config", &self.config)
            .finish()
    }
}

impl FilesystemAdapter {
    /// Create a disk around the given driver and configuration.
    pub fn new(driver: Arc<dyn Driver>, config: Value) -> Self {
        let prefix = config
            .get("prefix")
            .and_then(Value::as_str)
            .map(|prefix| normalize_path(prefix).unwrap_or_default())
            .unwrap_or_default();
        Self {
            driver,
            config,
            disk: "ondemand".to_string(),
            prefix,
            temporary_url_callback: RwLock::new(None),
            _keep_alive: None,
        }
    }

    /// Create a local disk rooted at the given directory.
    pub fn local(root: impl Into<PathBuf>) -> Self {
        let root: PathBuf = root.into();
        let config = json!({"driver": "local", "root": root.to_string_lossy()});
        Self::new(Arc::new(LocalDriver::new(root)), config)
    }

    /// Set the name of the disk.
    pub fn with_name(mut self, disk: impl Into<String>) -> Self {
        self.disk = disk.into();
        self
    }

    /// Keep a temporary directory alive for as long as the disk lives.
    pub(crate) fn keep_alive(mut self, directory: tempfile::TempDir) -> Self {
        self._keep_alive = Some(Arc::new(directory));
        self
    }

    /// The name of the disk.
    pub fn name(&self) -> &str {
        &self.disk
    }

    /// The disk's configuration.
    pub fn get_config(&self) -> &Value {
        &self.config
    }

    /// The underlying driver.
    pub fn get_driver(&self) -> Arc<dyn Driver> {
        self.driver.clone()
    }

    // ------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------

    /// The driver location for a disk path: normalized, then prefixed.
    fn location(&self, path: &str) -> Result<String> {
        let normalized = normalize_path(path)?;
        Ok(match (self.prefix.is_empty(), normalized.is_empty()) {
            (true, _) => normalized,
            (false, true) => self.prefix.clone(),
            (false, false) => format!("{}/{}", self.prefix, normalized),
        })
    }

    /// Strip the disk's prefix from a driver location.
    fn strip_prefix(&self, location: String) -> String {
        if self.prefix.is_empty() {
            return location;
        }
        location
            .strip_prefix(&self.prefix)
            .map(|rest| rest.trim_start_matches('/').to_string())
            .unwrap_or(location)
    }

    fn config_visibility(&self, key: &str) -> Option<Visibility> {
        self.config
            .get(key)
            .and_then(Value::as_str)
            .and_then(|v| Visibility::parse(v).ok())
    }

    fn write_options(&self, visibility: Option<Visibility>) -> WriteOptions {
        WriteOptions {
            visibility: visibility.or_else(|| self.config_visibility("visibility")),
            directory_visibility: self.config_visibility("directory_visibility"),
        }
    }

    fn throws_exceptions(&self) -> bool {
        self.config.get("throw").is_some_and(ValueExt::truthy)
    }

    fn should_report(&self) -> bool {
        self.config.get("report").is_some_and(ValueExt::truthy)
    }

    fn is_read_only(&self) -> bool {
        self.config.get("read-only").is_some_and(ValueExt::truthy)
    }

    /// Handle a failed write: throw, or report and return `false`.
    fn failed(&self, error: Error) -> Result<bool> {
        if error.is::<PathTraversalDetected>()
            || error.is::<CorruptedPathDetected>()
            || self.throws_exceptions()
        {
            return Err(error);
        }
        if self.should_report()
            && let Some(handler) = try_app::<dyn ExceptionHandler>()
        {
            handler.report(&error);
        }
        Ok(false)
    }

    fn outcome(&self, result: Result<()>) -> Result<bool> {
        match result {
            Ok(()) => Ok(true),
            Err(error) => self.failed(error),
        }
    }

    fn read_only_failure(&self, location: &str) -> Result<bool> {
        self.failed(FilesystemException::write(location, "This is a readonly adapter.").into())
    }

    // ------------------------------------------------------------------
    // Existence
    // ------------------------------------------------------------------

    /// Determine if a file or directory exists.
    pub async fn exists(&self, path: &str) -> Result<bool> {
        let location = self.location(path)?;
        Ok(self.driver.file_exists(&location).await?
            || self.driver.directory_exists(&location).await?)
    }

    /// Determine if a file or directory is missing.
    pub async fn missing(&self, path: &str) -> Result<bool> {
        Ok(!self.exists(path).await?)
    }

    /// Determine if a file exists.
    pub async fn file_exists(&self, path: &str) -> Result<bool> {
        self.driver.file_exists(&self.location(path)?).await
    }

    /// Determine if a file is missing.
    pub async fn file_missing(&self, path: &str) -> Result<bool> {
        Ok(!self.file_exists(path).await?)
    }

    /// Determine if a directory exists.
    pub async fn directory_exists(&self, path: &str) -> Result<bool> {
        self.driver.directory_exists(&self.location(path)?).await
    }

    /// Determine if a directory is missing.
    pub async fn directory_missing(&self, path: &str) -> Result<bool> {
        Ok(!self.directory_exists(path).await?)
    }

    // ------------------------------------------------------------------
    // Reading
    // ------------------------------------------------------------------

    /// Get the full path to the file at the given disk path.
    ///
    /// ```
    /// use illuminate_filesystem::FilesystemAdapter;
    ///
    /// let disk = FilesystemAdapter::local("/var/www/storage/app");
    /// assert_eq!(disk.path("avatars/1.jpg").unwrap().to_str().unwrap(), "/var/www/storage/app/avatars/1.jpg");
    /// ```
    pub fn path(&self, path: &str) -> Result<PathBuf> {
        let root = self
            .config
            .get("root")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let location = self.location(path)?;
        Ok(if root.is_empty() {
            PathBuf::from(location)
        } else {
            PathBuf::from(root).join(location)
        })
    }

    /// Get the contents of a file as a string.
    pub async fn get(&self, path: &str) -> Result<String> {
        let bytes = self.bytes(path).await?;
        String::from_utf8(bytes.to_vec()).map_err(|_| {
            FilesystemException::read(
                path,
                "The file is not valid UTF-8; read it with `bytes` instead.",
            )
            .into()
        })
    }

    /// Get the raw contents of a file.
    pub async fn bytes(&self, path: &str) -> Result<Bytes> {
        self.driver.read(&self.location(path)?).await
    }

    /// Get the contents of a file, decoded from JSON.
    pub async fn json(&self, path: &str) -> Result<Value> {
        Ok(serde_json::from_slice(&self.bytes(path).await?)?)
    }

    /// Get the size of a file in bytes.
    pub async fn size(&self, path: &str) -> Result<u64> {
        self.driver.file_size(&self.location(path)?).await
    }

    /// Get the MD5 checksum of a file.
    pub async fn checksum(&self, path: &str) -> Result<String> {
        hash_bytes(&self.bytes(path).await?, "md5")
    }

    /// Get the MIME type of a file.
    pub async fn mime_type(&self, path: &str) -> Result<String> {
        self.driver.mime_type(&self.location(path)?).await
    }

    /// Get the file's last modification time as a UNIX timestamp.
    pub async fn last_modified(&self, path: &str) -> Result<i64> {
        self.driver.last_modified(&self.location(path)?).await
    }

    // ------------------------------------------------------------------
    // Writing
    // ------------------------------------------------------------------

    async fn write(
        &self,
        path: &str,
        contents: Bytes,
        visibility: Option<Visibility>,
    ) -> Result<bool> {
        let location = self.location(path)?;
        if self.is_read_only() {
            return self.read_only_failure(&location);
        }
        let result = self
            .driver
            .write(&location, contents, self.write_options(visibility))
            .await;
        self.outcome(result)
    }

    /// Write the contents of a file.
    pub async fn put(&self, path: &str, contents: impl AsRef<[u8]>) -> Result<bool> {
        self.write(path, Bytes::copy_from_slice(contents.as_ref()), None)
            .await
    }

    /// Write the contents of a file with the given visibility.
    pub async fn put_with_visibility(
        &self,
        path: &str,
        contents: impl AsRef<[u8]>,
        visibility: Visibility,
    ) -> Result<bool> {
        self.write(
            path,
            Bytes::copy_from_slice(contents.as_ref()),
            Some(visibility),
        )
        .await
    }

    /// Store an uploaded file in the given directory under a unique,
    /// generated name, returning the stored file's path.
    pub async fn put_file(&self, path: &str, file: &UploadedFile) -> Result<String> {
        self.store_file(path, file, &file.hash_name(), None).await
    }

    /// Store an uploaded file with the given visibility.
    pub async fn put_file_with_visibility(
        &self,
        path: &str,
        file: &UploadedFile,
        visibility: Visibility,
    ) -> Result<String> {
        self.store_file(path, file, &file.hash_name(), Some(visibility))
            .await
    }

    /// Store an uploaded file in the given directory under the given name.
    pub async fn put_file_as(&self, path: &str, file: &UploadedFile, name: &str) -> Result<String> {
        self.store_file(path, file, name, None).await
    }

    /// Store an uploaded file under the given name with the given visibility.
    pub async fn put_file_as_with_visibility(
        &self,
        path: &str,
        file: &UploadedFile,
        name: &str,
        visibility: Visibility,
    ) -> Result<String> {
        self.store_file(path, file, name, Some(visibility)).await
    }

    /// Store a file, returning its path. Unlike `put`, a failed write is
    /// always an error here: there is no path to hand back.
    pub(crate) async fn store_file(
        &self,
        path: &str,
        file: &UploadedFile,
        name: &str,
        visibility: Option<Visibility>,
    ) -> Result<String> {
        let path = format!("{path}/{name}").trim_matches('/').to_string();
        if self.write(&path, file.bytes().clone(), visibility).await? {
            Ok(path)
        } else {
            Err(FilesystemException::write(&path, "The file could not be stored.").into())
        }
    }

    /// Prepend to a file, separated by a newline.
    pub async fn prepend(&self, path: &str, data: impl AsRef<[u8]>) -> Result<bool> {
        self.prepend_with(path, data, "\n").await
    }

    /// Prepend to a file with a custom separator.
    pub async fn prepend_with(
        &self,
        path: &str,
        data: impl AsRef<[u8]>,
        separator: &str,
    ) -> Result<bool> {
        if self.file_exists(path).await? {
            let mut contents = data.as_ref().to_vec();
            contents.extend_from_slice(separator.as_bytes());
            contents.extend_from_slice(&self.bytes(path).await?);
            return self.put(path, contents).await;
        }
        self.put(path, data).await
    }

    /// Append to a file, separated by a newline.
    pub async fn append(&self, path: &str, data: impl AsRef<[u8]>) -> Result<bool> {
        self.append_with(path, data, "\n").await
    }

    /// Append to a file with a custom separator.
    pub async fn append_with(
        &self,
        path: &str,
        data: impl AsRef<[u8]>,
        separator: &str,
    ) -> Result<bool> {
        if self.file_exists(path).await? {
            let mut contents = self.bytes(path).await?.to_vec();
            contents.extend_from_slice(separator.as_bytes());
            contents.extend_from_slice(data.as_ref());
            return self.put(path, contents).await;
        }
        self.put(path, data).await
    }

    /// Delete the file (or files) at the given paths.
    pub async fn delete(&self, paths: impl IntoPaths) -> Result<bool> {
        let mut success = true;
        for path in paths.into_paths() {
            let location = self.location(&path)?;
            let result = if self.is_read_only() {
                Err(FilesystemException::delete(&location, "This is a readonly adapter.").into())
            } else {
                self.driver.delete(&location).await
            };
            if !self.outcome(result)? {
                success = false;
            }
        }
        Ok(success)
    }

    /// Copy a file to a new location.
    pub async fn copy(&self, from: &str, to: &str) -> Result<bool> {
        let (from, to) = (self.location(from)?, self.location(to)?);
        if self.is_read_only() {
            return self.read_only_failure(&to);
        }
        let result = self.driver.copy(&from, &to, self.write_options(None)).await;
        self.outcome(result)
    }

    /// Move a file to a new location.
    pub async fn move_(&self, from: &str, to: &str) -> Result<bool> {
        let (from, to) = (self.location(from)?, self.location(to)?);
        if self.is_read_only() {
            return self.read_only_failure(&to);
        }
        let result = self
            .driver
            .move_(&from, &to, self.write_options(None))
            .await;
        self.outcome(result)
    }

    /// Copy a file to another disk, keeping its path unless `to` is given.
    pub async fn copy_to_disk(&self, disk: &str, from: &str, to: Option<&str>) -> Result<bool> {
        let destination = crate::Storage::disk(disk)?;
        let to = to.unwrap_or(from);
        if std::ptr::eq(destination.as_ref(), self) && to == from {
            return Err(InvalidArgumentException::new(
                "Cannot copy a file to the same disk and path.",
            )
            .into());
        }
        let contents = self.bytes(from).await?;
        destination.write(to, contents, None).await
    }

    /// Move a file to another disk, keeping its path unless `to` is given.
    pub async fn move_to_disk(&self, disk: &str, from: &str, to: Option<&str>) -> Result<bool> {
        Ok(self.copy_to_disk(disk, from, to).await? && self.delete(from).await?)
    }

    // ------------------------------------------------------------------
    // Visibility
    // ------------------------------------------------------------------

    /// Get the visibility of a file.
    pub async fn get_visibility(&self, path: &str) -> Result<Visibility> {
        self.driver.visibility(&self.location(path)?).await
    }

    /// Set the visibility of a file.
    pub async fn set_visibility(&self, path: &str, visibility: Visibility) -> Result<bool> {
        let location = self.location(path)?;
        let result = self.driver.set_visibility(&location, visibility).await;
        self.outcome(result)
    }

    // ------------------------------------------------------------------
    // Directories
    // ------------------------------------------------------------------

    async fn listing(&self, directory: &str, recursive: bool, files: bool) -> Result<Vec<String>> {
        let location = self.location(directory)?;
        let mut paths: Vec<String> = self
            .driver
            .list_contents(&location, recursive)
            .await?
            .into_iter()
            .filter(|attributes| attributes.is_file() == files)
            .map(|attributes| self.strip_prefix(attributes.path))
            .collect();
        paths.sort();
        Ok(paths)
    }

    /// Get the files in a directory (use `""` for the root), sorted by path.
    pub async fn files(&self, directory: &str) -> Result<Vec<String>> {
        self.listing(directory, false, true).await
    }

    /// Get all of the files in a directory, recursively.
    pub async fn all_files(&self, directory: &str) -> Result<Vec<String>> {
        self.listing(directory, true, true).await
    }

    /// Get the directories within a directory.
    pub async fn directories(&self, directory: &str) -> Result<Vec<String>> {
        self.listing(directory, false, false).await
    }

    /// Get all of the directories within a directory, recursively.
    pub async fn all_directories(&self, directory: &str) -> Result<Vec<String>> {
        self.listing(directory, true, false).await
    }

    /// Create a directory, including any needed subdirectories.
    pub async fn make_directory(&self, path: &str) -> Result<bool> {
        let location = self.location(path)?;
        if self.is_read_only() {
            return self.failed(
                FilesystemException::create_directory(&location, "This is a readonly adapter.")
                    .into(),
            );
        }
        let result = self
            .driver
            .create_directory(&location, self.write_options(None))
            .await;
        self.outcome(result)
    }

    /// Recursively delete a directory.
    pub async fn delete_directory(&self, directory: &str) -> Result<bool> {
        let location = self.location(directory)?;
        if self.is_read_only() {
            return self.failed(
                FilesystemException::delete_directory(&location, "This is a readonly adapter.")
                    .into(),
            );
        }
        let result = self.driver.delete_directory(&location).await;
        self.outcome(result)
    }

    // ------------------------------------------------------------------
    // URLs
    // ------------------------------------------------------------------

    fn config_url(&self) -> Option<&str> {
        self.config.get("url").and_then(Value::as_str)
    }

    /// Get the URL for the file at the given path.
    ///
    /// Local disks use their `url` option as the base, falling back to
    /// `/storage`:
    ///
    /// ```
    /// use illuminate_filesystem::FilesystemAdapter;
    ///
    /// let disk = FilesystemAdapter::local("/tmp/public");
    /// assert_eq!(disk.url("avatars/1.jpg").unwrap(), "/storage/avatars/1.jpg");
    /// ```
    pub fn url(&self, path: &str) -> Result<String> {
        let path = if self.prefix.is_empty() {
            path.to_string()
        } else {
            concat_path_to_url(&self.prefix, path)
        };
        if let Some(url) = self.driver.url(&path) {
            return Ok(url);
        }
        if self.driver.is_local() {
            return Ok(self.local_url(&path));
        }
        if let Some(url) = self.config_url() {
            return Ok(concat_path_to_url(url, &path));
        }
        Err(RuntimeException::new("This driver does not support retrieving URLs.").into())
    }

    fn local_url(&self, path: &str) -> String {
        if let Some(url) = self.config_url() {
            return concat_path_to_url(url, path);
        }
        let path = format!("/storage/{path}");
        // Using the default disk to build a "public" path most likely meant
        // the "public" disk, so drop the extra segment.
        if path.contains("/storage/public/") {
            return Str::replace_first("/public/", "/", &path);
        }
        path
    }

    /// Determine if temporary URLs can be generated for this disk.
    pub fn provides_temporary_urls(&self) -> bool {
        self.temporary_url_callback.read().unwrap().is_some()
            || (self.serves_signed_urls() && try_app::<dyn UrlSigner>().is_some())
    }

    fn serves_signed_urls(&self) -> bool {
        self.driver.is_local() && self.config.get("serve").is_some_and(ValueExt::truthy)
    }

    /// Get a temporary URL for the file at the given path.
    ///
    /// Local disks with `'serve' => true` sign a URL to the `storage.{disk}`
    /// route through the bound [`UrlSigner`]; any disk can customize this
    /// with [`FilesystemAdapter::build_temporary_urls_using`].
    pub fn temporary_url(&self, path: &str, expiration: Carbon) -> Result<String> {
        self.temporary_url_with(path, expiration, &json!({}))
    }

    /// Get a temporary URL, passing driver specific options along.
    pub fn temporary_url_with(
        &self,
        path: &str,
        expiration: Carbon,
        options: &Value,
    ) -> Result<String> {
        let callback = self.temporary_url_callback.read().unwrap().clone();
        if let Some(callback) = callback {
            return callback(path, expiration, options);
        }
        if self.serves_signed_urls()
            && let Some(signer) = try_app::<dyn UrlSigner>()
        {
            return signer.temporary_signed_route(
                &format!("storage.{}", self.disk),
                expiration,
                json!({"path": path}),
            );
        }
        Err(RuntimeException::new("This driver does not support creating temporary URLs.").into())
    }

    /// Define a custom temporary URL builder for this disk.
    pub fn build_temporary_urls_using(
        &self,
        callback: impl Fn(&str, Carbon, &Value) -> Result<String> + Send + Sync + 'static,
    ) {
        *self.temporary_url_callback.write().unwrap() = Some(Arc::new(callback));
    }

    // ------------------------------------------------------------------
    // Responses
    // ------------------------------------------------------------------

    /// Create a response that displays the file inline in the browser.
    pub async fn response(&self, path: &str, name: Option<&str>) -> Result<Response> {
        self.build_response(path, name, "inline").await
    }

    /// Create a response that forces the browser to download the file.
    pub async fn download(&self, path: &str, name: Option<&str>) -> Result<Response> {
        self.build_response(path, name, "attachment").await
    }

    async fn build_response(
        &self,
        path: &str,
        name: Option<&str>,
        disposition: &str,
    ) -> Result<Response> {
        let contents = self.bytes(path).await?;
        let mime = match self.mime_type(path).await {
            Ok(mime) => mime,
            Err(_) => detect_mime_type(path, Some(&contents))
                .unwrap_or_else(|| "application/octet-stream".into()),
        };
        let filename = name
            .map(str::to_string)
            .unwrap_or_else(|| pathinfo(path).basename);
        let disposition = make_disposition(disposition, &filename, &fallback_name(&filename))?;
        let length = contents.len().to_string();
        Ok(Response::new(contents)
            .with_header("content-type", &mime)
            .with_header("content-length", &length)
            .with_header("content-disposition", &disposition))
    }

    // ------------------------------------------------------------------
    // Testing assertions
    // ------------------------------------------------------------------

    /// Assert that the given file or directory exists.
    ///
    /// # Panics
    ///
    /// Panics when a path is missing.
    pub async fn assert_exists(&self, paths: impl IntoPaths) -> &Self {
        for path in paths.into_paths() {
            assert!(
                self.exists(&path).await.unwrap_or(false),
                "Unable to find a file or directory at path [{path}]."
            );
        }
        self
    }

    /// Assert that the given file exists and has the given contents.
    pub async fn assert_exists_with_content(&self, path: &str, content: &str) -> &Self {
        self.assert_exists(path).await;
        let actual = self.get(path).await.unwrap_or_default();
        assert!(
            actual == content,
            "File or directory [{path}] was found, but content [{actual}] does not match [{content}]."
        );
        self
    }

    /// Assert that the given file or directory does not exist.
    pub async fn assert_missing(&self, paths: impl IntoPaths) -> &Self {
        for path in paths.into_paths() {
            assert!(
                !self.exists(&path).await.unwrap_or(false),
                "Found unexpected file or directory at path [{path}]."
            );
        }
        self
    }

    /// Assert that the given directory contains exactly `count` files.
    pub async fn assert_count(&self, path: &str, count: usize) -> &Self {
        let actual = self.files(path).await.map(|f| f.len()).unwrap_or(0);
        assert!(
            actual == count,
            "Expected [{count}] files at [{path}], but found [{actual}]."
        );
        self
    }

    /// Assert that the given directory contains exactly `count` files, recursively.
    pub async fn assert_count_recursive(&self, path: &str, count: usize) -> &Self {
        let actual = self.all_files(path).await.map(|f| f.len()).unwrap_or(0);
        assert!(
            actual == count,
            "Expected [{count}] files at [{path}], but found [{actual}]."
        );
        self
    }

    /// Assert that the given directory is empty.
    pub async fn assert_directory_empty(&self, path: &str) -> &Self {
        let files = self.all_files(path).await.unwrap_or_default();
        assert!(files.is_empty(), "Directory [{path}] is not empty.");
        self
    }

    /// Assert that the disk contains no files.
    pub async fn assert_empty(&self) -> &Self {
        let files = self.all_files("").await.unwrap_or_default();
        assert!(files.is_empty(), "Disk is not empty.");
        self
    }
}

/// Join a base URL and a path with exactly one slash.
fn concat_path_to_url(url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// An ASCII-only fallback for a download file name.
fn fallback_name(name: &str) -> String {
    let fallback: String = Str::ascii(name)
        .chars()
        .filter(|c| *c != '%')
        .map(|c| if (' '..='~').contains(&c) { c } else { '_' })
        .collect();
    if fallback.is_empty() {
        "_".repeat(name.chars().count())
    } else {
        fallback
    }
}

/// Build a `Content-Disposition` header value, like Symfony's `HeaderUtils::makeDisposition`.
pub(crate) fn make_disposition(
    disposition: &str,
    filename: &str,
    fallback: &str,
) -> Result<String> {
    if filename.contains(['/', '\\']) || fallback.contains(['/', '\\']) {
        return Err(InvalidArgumentException::new(
            "The filename and the fallback cannot contain the \"/\" and \"\\\" characters.",
        )
        .into());
    }
    let mut header = format!("{disposition}; filename={}", quote_header_value(fallback));
    if filename != fallback {
        let encoded =
            percent_encoding::utf8_percent_encode(filename, percent_encoding::NON_ALPHANUMERIC)
                .to_string()
                .replace("%2D", "-")
                .replace("%2E", ".")
                .replace("%5F", "_")
                .replace("%7E", "~");
        header.push_str(&format!("; filename*=utf-8''{encoded}"));
    }
    Ok(header)
}

fn quote_header_value(value: &str) -> String {
    let token = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*.^_`|~-".contains(c));
    if token {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk_with(config: Value) -> (tempfile::TempDir, FilesystemAdapter) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config;
        config["root"] = json!(dir.path().to_string_lossy());
        let driver = LocalDriver::from_config(&config).unwrap();
        (
            dir,
            FilesystemAdapter::new(Arc::new(driver), config).with_name("local"),
        )
    }

    #[tokio::test]
    async fn it_stores_and_reads_files() {
        let (_dir, disk) = disk_with(json!({}));
        assert!(disk.put("docs/readme.txt", "Hello").await.unwrap());
        assert!(disk.exists("docs/readme.txt").await.unwrap());
        assert!(disk.exists("docs").await.unwrap());
        assert!(disk.directory_exists("docs").await.unwrap());
        assert!(disk.file_missing("docs").await.unwrap());
        assert!(disk.missing("nope.txt").await.unwrap());
        assert_eq!(disk.get("docs/readme.txt").await.unwrap(), "Hello");
        assert_eq!(disk.size("docs/readme.txt").await.unwrap(), 5);
        assert_eq!(
            disk.mime_type("docs/readme.txt").await.unwrap(),
            "text/plain"
        );
        assert_eq!(
            disk.checksum("docs/readme.txt").await.unwrap(),
            "8b1a9953c4611296a827abf8c47804d7"
        );
        assert!(disk.last_modified("docs/readme.txt").await.unwrap() > 0);

        disk.put("orders.json", r#"{"total": 42}"#).await.unwrap();
        assert_eq!(disk.json("orders.json").await.unwrap()["total"], 42);

        assert!(disk.get("missing.txt").await.is_err());
    }

    #[tokio::test]
    async fn it_prepends_appends_copies_moves_and_deletes() {
        let (_dir, disk) = disk_with(json!({}));
        disk.append("log.txt", "first").await.unwrap();
        disk.append("log.txt", "second").await.unwrap();
        disk.prepend("log.txt", "zeroth").await.unwrap();
        assert_eq!(disk.get("log.txt").await.unwrap(), "zeroth\nfirst\nsecond");

        assert!(disk.copy("log.txt", "backup/log.txt").await.unwrap());
        assert!(
            disk.move_("backup/log.txt", "archive/log.txt")
                .await
                .unwrap()
        );
        assert!(disk.missing("backup/log.txt").await.unwrap());
        assert!(disk.exists("archive/log.txt").await.unwrap());

        assert!(disk.delete(["log.txt", "archive/log.txt"]).await.unwrap());
        assert!(disk.missing("log.txt").await.unwrap());

        // Failed writes return false unless the disk throws.
        assert!(!disk.copy("missing.txt", "other.txt").await.unwrap());
        assert!(!disk.move_("missing.txt", "other.txt").await.unwrap());
    }

    #[tokio::test]
    async fn throwing_disks_return_errors() {
        let (_dir, disk) = disk_with(json!({"throw": true}));
        let error = disk.copy("missing.txt", "other.txt").await.unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Unable to copy file from missing.txt to other.txt")
        );
        assert!(matches!(
            error.downcast_ref::<FilesystemException>(),
            Some(FilesystemException::UnableToCopyFile { .. })
        ));
    }

    #[tokio::test]
    async fn path_traversal_is_always_rejected() {
        let (_dir, disk) = disk_with(json!({}));
        for result in [
            disk.put("../escape.txt", "x").await.map(|_| ()),
            disk.get("../../etc/passwd").await.map(|_| ()),
            disk.exists("a/../../b").await.map(|_| ()),
            disk.delete("../x").await.map(|_| ()),
            disk.path("../x").map(|_| ()),
        ] {
            assert!(
                result
                    .unwrap_err()
                    .downcast_ref::<PathTraversalDetected>()
                    .is_some()
            );
        }
    }

    #[tokio::test]
    async fn it_lists_files_and_directories() {
        let (_dir, disk) = disk_with(json!({}));
        disk.put("b.txt", "").await.unwrap();
        disk.put("a.txt", "").await.unwrap();
        disk.put("photos/2024/beach.jpg", "").await.unwrap();
        disk.put("photos/cover.jpg", "").await.unwrap();
        disk.make_directory("empty").await.unwrap();

        assert_eq!(disk.files("").await.unwrap(), vec!["a.txt", "b.txt"]);
        assert_eq!(
            disk.files("photos").await.unwrap(),
            vec!["photos/cover.jpg"]
        );
        assert_eq!(
            disk.all_files("").await.unwrap(),
            vec![
                "a.txt",
                "b.txt",
                "photos/2024/beach.jpg",
                "photos/cover.jpg"
            ]
        );
        assert_eq!(disk.directories("").await.unwrap(), vec!["empty", "photos"]);
        assert_eq!(
            disk.all_directories("").await.unwrap(),
            vec!["empty", "photos", "photos/2024"]
        );

        assert!(disk.delete_directory("photos").await.unwrap());
        assert!(disk.missing("photos/cover.jpg").await.unwrap());
    }

    #[tokio::test]
    async fn prefixed_disks_are_scoped() {
        let (dir, disk) = disk_with(json!({"prefix": "tenant-1"}));
        disk.put("avatar.jpg", "x").await.unwrap();
        assert!(dir.path().join("tenant-1/avatar.jpg").exists());
        assert_eq!(disk.files("").await.unwrap(), vec!["avatar.jpg"]);
        assert_eq!(
            disk.path("avatar.jpg").unwrap(),
            dir.path().join("tenant-1/avatar.jpg")
        );
        assert_eq!(
            disk.url("avatar.jpg").unwrap(),
            "/storage/tenant-1/avatar.jpg"
        );
    }

    #[tokio::test]
    async fn read_only_disks_refuse_writes() {
        let (_dir, disk) = disk_with(json!({"read-only": true}));
        assert!(!disk.put("a.txt", "x").await.unwrap());
        let (_dir, disk) = disk_with(json!({"read-only": true, "throw": true}));
        let error = disk.put("a.txt", "x").await.unwrap_err();
        assert!(error.to_string().contains("This is a readonly adapter."));
    }

    #[test]
    fn it_builds_urls() {
        let disk = FilesystemAdapter::new(
            Arc::new(LocalDriver::new("/tmp")),
            json!({"driver": "local", "root": "/tmp", "url": "http://localhost/storage/"}),
        );
        assert_eq!(
            disk.url("/avatars/1.jpg").unwrap(),
            "http://localhost/storage/avatars/1.jpg"
        );

        let disk = FilesystemAdapter::local("/tmp");
        assert_eq!(
            disk.url("public/avatars/1.jpg").unwrap(),
            "/storage/avatars/1.jpg"
        );
    }

    #[test]
    fn temporary_urls_need_a_builder() {
        let disk = FilesystemAdapter::local("/tmp");
        let expiration = Carbon::from_timestamp(1_700_000_000);
        assert!(!disk.provides_temporary_urls());
        assert_eq!(
            disk.temporary_url("a.txt", expiration)
                .unwrap_err()
                .to_string(),
            "This driver does not support creating temporary URLs."
        );

        disk.build_temporary_urls_using(|path, expiration, _| {
            Ok(format!("/files/{path}?expires={}", expiration.timestamp()))
        });
        assert!(disk.provides_temporary_urls());
        assert_eq!(
            disk.temporary_url("a.txt", expiration).unwrap(),
            "/files/a.txt?expires=1700000000"
        );
    }

    #[tokio::test]
    async fn it_builds_download_and_inline_responses() {
        let (_dir, disk) = disk_with(json!({}));
        disk.put("reports/q1 report.txt", "numbers").await.unwrap();

        let response = disk.download("reports/q1 report.txt", None).await.unwrap();
        assert_eq!(response.header("content-type").unwrap(), "text/plain");
        assert_eq!(response.header("content-length").unwrap(), "7");
        assert_eq!(
            response.header("content-disposition").unwrap(),
            "attachment; filename=\"q1 report.txt\""
        );
        assert_eq!(response.content_string(), "numbers");

        let response = disk
            .response("reports/q1 report.txt", Some("report.txt"))
            .await
            .unwrap();
        assert_eq!(
            response.header("content-disposition").unwrap(),
            "inline; filename=report.txt"
        );

        let response = disk
            .download("reports/q1 report.txt", Some("résumé.txt"))
            .await
            .unwrap();
        assert_eq!(
            response.header("content-disposition").unwrap(),
            "attachment; filename=resume.txt; filename*=utf-8''r%C3%A9sum%C3%A9.txt"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn visibility_can_be_read_and_changed() {
        let (_dir, disk) = disk_with(json!({}));
        disk.put_with_visibility("secret.txt", "x", Visibility::Private)
            .await
            .unwrap();
        assert_eq!(
            disk.get_visibility("secret.txt").await.unwrap(),
            Visibility::Private
        );
        assert!(
            disk.set_visibility("secret.txt", Visibility::Public)
                .await
                .unwrap()
        );
        assert_eq!(
            disk.get_visibility("secret.txt").await.unwrap(),
            Visibility::Public
        );

        let (_dir, public) = disk_with(json!({"visibility": "public"}));
        public.put("open.txt", "x").await.unwrap();
        assert_eq!(
            public.get_visibility("open.txt").await.unwrap(),
            Visibility::Public
        );
    }

    #[tokio::test]
    async fn it_stores_uploaded_files() {
        let (_dir, disk) = disk_with(json!({}));
        let file = UploadedFile::new("avatar.png", "image/png", b"\x89PNG\r\n\x1a\n".to_vec());

        let path = disk.put_file("avatars", &file).await.unwrap();
        assert!(path.starts_with("avatars/"));
        assert!(path.ends_with(".png"));
        assert_eq!(path.len(), "avatars/".len() + 40 + ".png".len());
        disk.assert_exists(path.as_str()).await;

        let path = disk.put_file_as("avatars", &file, "1.png").await.unwrap();
        assert_eq!(path, "avatars/1.png");
        let path = disk.put_file_as("", &file, "root.png").await.unwrap();
        assert_eq!(path, "root.png");
    }

    #[tokio::test]
    async fn assertions_pass_and_fail() {
        let (_dir, disk) = disk_with(json!({}));
        disk.put("photos/a.jpg", "a").await.unwrap();
        disk.put("photos/b.jpg", "b").await.unwrap();
        disk.make_directory("wallpapers").await.unwrap();

        disk.assert_exists(["photos/a.jpg", "photos/b.jpg"]).await;
        disk.assert_exists_with_content("photos/a.jpg", "a").await;
        disk.assert_missing("photos/c.jpg").await;
        disk.assert_count("photos", 2).await;
        disk.assert_count_recursive("", 2).await;
        disk.assert_directory_empty("wallpapers").await;

        let result = tokio::spawn(async move {
            disk.assert_exists("missing.jpg").await;
        })
        .await;
        let panic = result.unwrap_err().into_panic();
        let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
        assert_eq!(
            message,
            "Unable to find a file or directory at path [missing.jpg]."
        );
    }
}
