//! Storage drivers: the low-level adapters a disk talks to.
//!
//! In Laravel, every disk is a `FilesystemAdapter` wrapping a Flysystem
//! adapter. [`Driver`] is that Flysystem adapter contract — implement it to
//! teach Laravel about a new kind of storage, then register it with
//! `Storage::extend`. Drivers always receive *normalized* paths, relative
//! to the disk's root: traversal protection happens before they are called.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use async_trait::async_trait;
use bytes::Bytes;

use illuminate_support::{Result, Value, ValueExt};

use crate::exceptions::FilesystemException;
use crate::filesystem::{blocking, set_mode};
use crate::path::detect_mime_type;

/// File visibility: whether a file should generally be accessible to others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Visibility {
    Public,
    Private,
}

impl Visibility {
    /// The visibility as Laravel spells it: `"public"` or `"private"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Private => "private",
        }
    }

    /// Parse a visibility string.
    ///
    /// ```
    /// use illuminate_filesystem::Visibility;
    ///
    /// assert_eq!(Visibility::parse("public").unwrap(), Visibility::Public);
    /// assert_eq!(
    ///     Visibility::parse("hidden").unwrap_err().to_string(),
    ///     "Unknown visibility: hidden."
    /// );
    /// ```
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "public" => Ok(Visibility::Public),
            "private" => Ok(Visibility::Private),
            other => Err(illuminate_support::error::InvalidArgumentException::new(format!(
                "Unknown visibility: {other}."
            ))
            .into()),
        }
    }
}

impl FromStr for Visibility {
    type Err = illuminate_support::Error;

    fn from_str(value: &str) -> Result<Self> {
        Visibility::parse(value)
    }
}

impl std::fmt::Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Options for a write: the visibility of the file and of any directories
/// created along the way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteOptions {
    pub visibility: Option<Visibility>,
    pub directory_visibility: Option<Visibility>,
}

/// An entry in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageAttributes {
    /// The path, relative to the disk's root.
    pub path: String,
    is_file: bool,
    /// The size in bytes (files only).
    pub file_size: Option<u64>,
    /// The last modification time as a UNIX timestamp.
    pub last_modified: Option<i64>,
}

impl StorageAttributes {
    /// Describe a file.
    pub fn file(path: impl Into<String>, file_size: Option<u64>, last_modified: Option<i64>) -> Self {
        Self { path: path.into(), is_file: true, file_size, last_modified }
    }

    /// Describe a directory.
    pub fn directory(path: impl Into<String>, last_modified: Option<i64>) -> Self {
        Self { path: path.into(), is_file: false, file_size: None, last_modified }
    }

    pub fn is_file(&self) -> bool {
        self.is_file
    }

    pub fn is_dir(&self) -> bool {
        !self.is_file
    }

    /// The same attributes with a different path.
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }
}

/// The contract every storage driver implements (Flysystem's adapter).
#[async_trait]
pub trait Driver: Send + Sync + 'static {
    /// Determine if a file exists.
    async fn file_exists(&self, path: &str) -> Result<bool>;

    /// Determine if a directory exists.
    async fn directory_exists(&self, path: &str) -> Result<bool>;

    /// Read a file's contents.
    async fn read(&self, path: &str) -> Result<Bytes>;

    /// Write a file, creating parent directories as needed.
    async fn write(&self, path: &str, contents: Bytes, options: WriteOptions) -> Result<()>;

    /// Delete a file. Deleting a file that doesn't exist is not an error.
    async fn delete(&self, path: &str) -> Result<()>;

    /// Delete a directory and everything in it.
    async fn delete_directory(&self, path: &str) -> Result<()>;

    /// Create a directory (and its parents).
    async fn create_directory(&self, path: &str, options: WriteOptions) -> Result<()>;

    /// Set the visibility of a file.
    async fn set_visibility(&self, path: &str, visibility: Visibility) -> Result<()>;

    /// Get the visibility of a file.
    async fn visibility(&self, path: &str) -> Result<Visibility>;

    /// Get the MIME type of a file.
    async fn mime_type(&self, path: &str) -> Result<String>;

    /// Get the last modification time of a file as a UNIX timestamp.
    async fn last_modified(&self, path: &str) -> Result<i64>;

    /// Get the size of a file in bytes.
    async fn file_size(&self, path: &str) -> Result<u64>;

    /// List the contents of a directory, optionally recursively.
    async fn list_contents(&self, path: &str, deep: bool) -> Result<Vec<StorageAttributes>>;

    /// Move a file.
    async fn move_(&self, from: &str, to: &str, options: WriteOptions) -> Result<()>;

    /// Copy a file.
    async fn copy(&self, from: &str, to: &str, options: WriteOptions) -> Result<()>;

    /// The URL of a file, if the driver knows how to build one itself.
    fn url(&self, _path: &str) -> Option<String> {
        None
    }

    /// Whether this driver stores files on the local filesystem.
    fn is_local(&self) -> bool {
        false
    }
}

/// The permissions used to express visibility on the local filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub file_public: u32,
    pub file_private: u32,
    pub dir_public: u32,
    pub dir_private: u32,
}

impl Default for Permissions {
    fn default() -> Self {
        Self { file_public: 0o644, file_private: 0o600, dir_public: 0o755, dir_private: 0o700 }
    }
}

impl Permissions {
    /// Read the `permissions` option of a disk's configuration.
    pub fn from_config(config: &Value) -> Self {
        let defaults = Self::default();
        let mode = |key: &str, default: u32| {
            config
                .dot(key)
                .and_then(|value| match value {
                    Value::String(s) => u32::from_str_radix(s.trim_start_matches("0o"), 8).ok(),
                    other => other.to_i64_lossy().map(|n| n as u32),
                })
                .unwrap_or(default)
        };
        Self {
            file_public: mode("file.public", defaults.file_public),
            file_private: mode("file.private", defaults.file_private),
            dir_public: mode("dir.public", defaults.dir_public),
            dir_private: mode("dir.private", defaults.dir_private),
        }
    }

    fn for_file(&self, visibility: Visibility) -> u32 {
        match visibility {
            Visibility::Public => self.file_public,
            Visibility::Private => self.file_private,
        }
    }

    fn for_directory(&self, visibility: Visibility) -> u32 {
        match visibility {
            Visibility::Public => self.dir_public,
            Visibility::Private => self.dir_private,
        }
    }
}

/// The `local` driver: files on this machine, under a root directory.
#[derive(Debug, Clone)]
pub struct LocalDriver {
    root: PathBuf,
    permissions: Permissions,
    directory_visibility: Visibility,
}

impl LocalDriver {
    /// Create a local driver rooted at the given directory. Directories it
    /// creates are private unless configured otherwise.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), permissions: Permissions::default(), directory_visibility: Visibility::Private }
    }

    /// Create a local driver from a disk configuration (`root`,
    /// `permissions`, `visibility`, `directory_visibility`).
    pub fn from_config(config: &Value) -> Result<Self> {
        let visibility = |key: &str| -> Result<Option<Visibility>> {
            match config.get(key).and_then(Value::as_str) {
                Some(value) => Visibility::parse(value).map(Some),
                None => Ok(None),
            }
        };
        let directory_visibility = visibility("directory_visibility")?
            .or(visibility("visibility")?)
            .unwrap_or(Visibility::Private);
        Ok(Self {
            root: PathBuf::from(config.get("root").and_then(Value::as_str).unwrap_or_default()),
            permissions: Permissions::from_config(config.get("permissions").unwrap_or(&Value::Null)),
            directory_visibility,
        })
    }

    /// The root directory of the driver.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The absolute location of a (normalized) path.
    pub fn prefix_path(&self, path: &str) -> PathBuf {
        if path.is_empty() { self.root.clone() } else { self.root.join(path) }
    }

    fn strip_prefix(&self, location: &Path) -> String {
        location
            .strip_prefix(&self.root)
            .unwrap_or(location)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn ensure_directory_exists(&self, directory: &Path, visibility: Visibility) -> std::io::Result<()> {
        if directory.is_dir() {
            return Ok(());
        }
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(self.permissions.for_directory(visibility));
        }
        #[cfg(not(unix))]
        let _ = visibility;
        builder.create(directory)
    }

    fn write_sync(&self, path: &str, contents: &[u8], options: WriteOptions) -> Result<()> {
        let location = self.prefix_path(path);
        let directory_visibility = options.directory_visibility.unwrap_or(self.directory_visibility);
        let parent = location.parent().unwrap_or(Path::new("."));
        self.ensure_directory_exists(parent, directory_visibility)
            .map_err(|error| FilesystemException::write(path, error))?;
        fs::write(&location, contents).map_err(|error| FilesystemException::write(path, error))?;
        if let Some(visibility) = options.visibility {
            set_mode(&location, self.permissions.for_file(visibility))
                .map_err(|error| FilesystemException::write(path, error))?;
        }
        Ok(())
    }

    fn list_sync(&self, path: &str, deep: bool) -> Result<Vec<StorageAttributes>> {
        let location = self.prefix_path(path);
        let mut results = Vec::new();
        if location.is_dir() {
            self.walk(&location, deep, &mut results);
        }
        Ok(results)
    }

    fn walk(&self, directory: &Path, deep: bool, results: &mut Vec<StorageAttributes>) {
        let Ok(entries) = fs::read_dir(directory) else { return };
        for entry in entries.flatten() {
            let location = entry.path();
            let Ok(metadata) = fs::metadata(&location) else { continue };
            let is_symlink = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            let modified = modified_timestamp(&metadata);
            let path = self.strip_prefix(&location);
            if metadata.is_dir() {
                results.push(StorageAttributes::directory(path, modified));
                if deep && !is_symlink {
                    self.walk(&location, deep, results);
                }
            } else {
                results.push(StorageAttributes::file(path, Some(metadata.len()), modified));
            }
        }
    }
}

fn modified_timestamp(metadata: &fs::Metadata) -> Option<i64> {
    let modified = metadata.modified().ok()?;
    Some(match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(error) => -(error.duration().as_secs() as i64),
    })
}

fn file_mode(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        if metadata.permissions().readonly() { 0o444 } else { 0o644 }
    }
}

#[async_trait]
impl Driver for LocalDriver {
    async fn file_exists(&self, path: &str) -> Result<bool> {
        Ok(tokio::fs::metadata(self.prefix_path(path)).await.map(|m| m.is_file()).unwrap_or(false))
    }

    async fn directory_exists(&self, path: &str) -> Result<bool> {
        Ok(tokio::fs::metadata(self.prefix_path(path)).await.map(|m| m.is_dir()).unwrap_or(false))
    }

    async fn read(&self, path: &str) -> Result<Bytes> {
        tokio::fs::read(self.prefix_path(path))
            .await
            .map(Bytes::from)
            .map_err(|error| FilesystemException::read(path, error).into())
    }

    async fn write(&self, path: &str, contents: Bytes, options: WriteOptions) -> Result<()> {
        let driver = self.clone();
        let path = path.to_string();
        blocking(move || driver.write_sync(&path, &contents, options)).await
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let location = self.prefix_path(path);
        if tokio::fs::symlink_metadata(&location).await.is_err() {
            return Ok(());
        }
        tokio::fs::remove_file(&location)
            .await
            .map_err(|error| FilesystemException::delete(path, error).into())
    }

    async fn delete_directory(&self, path: &str) -> Result<()> {
        let location = self.prefix_path(path);
        if !tokio::fs::metadata(&location).await.map(|m| m.is_dir()).unwrap_or(false) {
            return Ok(());
        }
        tokio::fs::remove_dir_all(&location)
            .await
            .map_err(|error| FilesystemException::delete_directory(path, error).into())
    }

    async fn create_directory(&self, path: &str, options: WriteOptions) -> Result<()> {
        let driver = self.clone();
        let path = path.to_string();
        blocking(move || {
            let location = driver.prefix_path(&path);
            let visibility = options
                .visibility
                .or(options.directory_visibility)
                .unwrap_or(driver.directory_visibility);
            if location.is_dir() {
                return set_mode(&location, driver.permissions.for_directory(visibility))
                    .map_err(|error| FilesystemException::create_directory(&path, error).into());
            }
            driver
                .ensure_directory_exists(&location, visibility)
                .map_err(|error| FilesystemException::create_directory(&path, error).into())
        })
        .await
    }

    async fn set_visibility(&self, path: &str, visibility: Visibility) -> Result<()> {
        let location = self.prefix_path(path);
        let metadata = tokio::fs::metadata(&location)
            .await
            .map_err(|error| FilesystemException::visibility(path, error))?;
        let mode = if metadata.is_dir() {
            self.permissions.for_directory(visibility)
        } else {
            self.permissions.for_file(visibility)
        };
        set_mode(&location, mode).map_err(|error| FilesystemException::visibility(path, error).into())
    }

    async fn visibility(&self, path: &str) -> Result<Visibility> {
        let metadata = tokio::fs::metadata(self.prefix_path(path))
            .await
            .map_err(|error| FilesystemException::metadata("visibility", path, error))?;
        let mode = file_mode(&metadata);
        let private = if metadata.is_dir() { self.permissions.dir_private } else { self.permissions.file_private };
        Ok(if mode == private { Visibility::Private } else { Visibility::Public })
    }

    async fn mime_type(&self, path: &str) -> Result<String> {
        let location = self.prefix_path(path);
        let path = path.to_string();
        blocking(move || {
            let mut sample = vec![0u8; 512];
            let read = fs::File::open(&location)
                .and_then(|mut file| {
                    if file.metadata()?.is_dir() {
                        return Err(std::io::Error::other("The path is a directory."));
                    }
                    file.read(&mut sample)
                })
                .map_err(|error| FilesystemException::metadata("mime_type", &path, error))?;
            sample.truncate(read);
            detect_mime_type(&path, Some(&sample))
                .ok_or_else(|| FilesystemException::metadata("mime_type", &path, "").into())
        })
        .await
    }

    async fn last_modified(&self, path: &str) -> Result<i64> {
        let metadata = tokio::fs::metadata(self.prefix_path(path))
            .await
            .map_err(|error| FilesystemException::metadata("last_modified", path, error))?;
        modified_timestamp(&metadata)
            .ok_or_else(|| FilesystemException::metadata("last_modified", path, "").into())
    }

    async fn file_size(&self, path: &str) -> Result<u64> {
        match tokio::fs::metadata(self.prefix_path(path)).await {
            Ok(metadata) if metadata.is_file() => Ok(metadata.len()),
            Ok(_) => Err(FilesystemException::metadata("file_size", path, "").into()),
            Err(error) => Err(FilesystemException::metadata("file_size", path, error).into()),
        }
    }

    async fn list_contents(&self, path: &str, deep: bool) -> Result<Vec<StorageAttributes>> {
        let driver = self.clone();
        let path = path.to_string();
        blocking(move || driver.list_sync(&path, deep)).await
    }

    async fn move_(&self, from: &str, to: &str, options: WriteOptions) -> Result<()> {
        if from == to {
            return Ok(());
        }
        let driver = self.clone();
        let (from, to) = (from.to_string(), to.to_string());
        blocking(move || {
            let (source, destination) = (driver.prefix_path(&from), driver.prefix_path(&to));
            let visibility = options.directory_visibility.unwrap_or(driver.directory_visibility);
            let parent = destination.parent().unwrap_or(Path::new("."));
            driver
                .ensure_directory_exists(parent, visibility)
                .and_then(|_| fs::rename(&source, &destination))
                .map_err(|error| FilesystemException::move_(&from, &to, error).into())
        })
        .await
    }

    async fn copy(&self, from: &str, to: &str, options: WriteOptions) -> Result<()> {
        if from == to {
            return Ok(());
        }
        let driver = self.clone();
        let (from, to) = (from.to_string(), to.to_string());
        blocking(move || {
            let (source, destination) = (driver.prefix_path(&from), driver.prefix_path(&to));
            let visibility = options.directory_visibility.unwrap_or(driver.directory_visibility);
            let parent = destination.parent().unwrap_or(Path::new("."));
            driver
                .ensure_directory_exists(parent, visibility)
                .and_then(|_| fs::copy(&source, &destination))
                .map_err(|error| FilesystemException::copy(&from, &to, error))?;
            if let Some(visibility) = options.visibility {
                set_mode(&destination, driver.permissions.for_file(visibility))?;
            }
            Ok(())
        })
        .await
    }

    fn is_local(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_local_driver_round_trips_files() {
        let dir = tempfile::tempdir().unwrap();
        let driver = LocalDriver::new(dir.path());

        driver.write("a/b/c.txt", Bytes::from_static(b"hi"), WriteOptions::default()).await.unwrap();
        assert!(driver.file_exists("a/b/c.txt").await.unwrap());
        assert!(driver.directory_exists("a/b").await.unwrap());
        assert!(!driver.file_exists("a/b").await.unwrap());
        assert_eq!(driver.read("a/b/c.txt").await.unwrap(), Bytes::from_static(b"hi"));
        assert_eq!(driver.file_size("a/b/c.txt").await.unwrap(), 2);
        assert_eq!(driver.mime_type("a/b/c.txt").await.unwrap(), "text/plain");

        let listing = driver.list_contents("", true).await.unwrap();
        let mut paths: Vec<_> = listing.iter().map(|a| (a.path.clone(), a.is_file())).collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![("a".into(), false), ("a/b".into(), false), ("a/b/c.txt".into(), true)]
        );

        driver.copy("a/b/c.txt", "d.txt", WriteOptions::default()).await.unwrap();
        driver.move_("d.txt", "e/f.txt", WriteOptions::default()).await.unwrap();
        assert!(driver.file_exists("e/f.txt").await.unwrap());
        driver.delete("e/f.txt").await.unwrap();
        driver.delete("e/f.txt").await.unwrap();
        driver.delete_directory("a").await.unwrap();
        assert!(!driver.directory_exists("a").await.unwrap());

        let error = driver.read("missing.txt").await.unwrap_err();
        assert!(error.to_string().starts_with("Unable to read file from location: missing.txt."));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn visibility_maps_to_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let driver = LocalDriver::from_config(&illuminate_support::json!({
            "root": dir.path().to_string_lossy(),
            "visibility": "public",
        }))
        .unwrap();

        let options = WriteOptions { visibility: Some(Visibility::Private), directory_visibility: None };
        driver.write("dir/secret.txt", Bytes::from_static(b"x"), options).await.unwrap();
        assert_eq!(driver.visibility("dir/secret.txt").await.unwrap(), Visibility::Private);

        driver.set_visibility("dir/secret.txt", Visibility::Public).await.unwrap();
        assert_eq!(driver.visibility("dir/secret.txt").await.unwrap(), Visibility::Public);
        assert_eq!(driver.visibility("dir").await.unwrap(), Visibility::Public);
    }

    #[test]
    fn permissions_can_be_configured() {
        let permissions = Permissions::from_config(&illuminate_support::json!({
            "file": {"public": 0o664, "private": "0640"},
        }));
        assert_eq!(permissions.file_public, 0o664);
        assert_eq!(permissions.file_private, 0o640);
        assert_eq!(permissions.dir_public, 0o755);
    }
}
