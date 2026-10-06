//! The filesystem manager and the `Storage` facade.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use illuminate_config::Repository as Config;
use illuminate_container::{Container, try_app};
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Carbon, Map, Result, Value, ValueExt, json};

use crate::adapter::FilesystemAdapter;
use crate::driver::{LocalDriver, Visibility};
use crate::path::IntoPaths;
use crate::serve::ServedDisk;

/// Creates a disk for a custom driver from the disk's configuration.
pub type DiskCreator = Arc<dyn Fn(&Container, &Value) -> Result<FilesystemAdapter> + Send + Sync>;

/// Resolves and caches the application's disks from `filesystems.disks`.
pub struct FilesystemManager {
    config: Arc<Config>,
    disks: RwLock<HashMap<String, Arc<FilesystemAdapter>>>,
    custom_creators: RwLock<HashMap<String, DiskCreator>>,
}

impl FilesystemManager {
    /// Create a new filesystem manager reading the given configuration.
    pub fn new(config: Arc<Config>) -> Self {
        Self { config, disks: RwLock::new(HashMap::new()), custom_creators: RwLock::new(HashMap::new()) }
    }

    /// Get a disk by name.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_config::Repository;
    /// use illuminate_filesystem::FilesystemManager;
    /// use illuminate_support::json;
    ///
    /// let manager = FilesystemManager::new(Arc::new(Repository::new(json!({
    ///     "filesystems": {"disks": {"public": {"driver": "local", "root": "/tmp/public", "url": "/storage"}}},
    /// }))));
    ///
    /// let disk = manager.disk("public").unwrap();
    /// assert_eq!(disk.url("avatars/1.jpg").unwrap(), "/storage/avatars/1.jpg");
    /// assert!(manager.disk("s4").is_err());
    /// ```
    pub fn disk(&self, name: &str) -> Result<Arc<FilesystemAdapter>> {
        if let Some(disk) = self.disks.read().unwrap().get(name) {
            return Ok(disk.clone());
        }
        let disk = Arc::new(self.resolve(name, None)?);
        Ok(self.disks.write().unwrap().entry(name.to_string()).or_insert(disk).clone())
    }

    /// Alias of [`FilesystemManager::disk`].
    pub fn drive(&self, name: &str) -> Result<Arc<FilesystemAdapter>> {
        self.disk(name)
    }

    /// Get the default disk (`filesystems.default`).
    pub fn default_disk(&self) -> Result<Arc<FilesystemAdapter>> {
        self.disk(&self.get_default_driver())
    }

    /// Get the default cloud disk (`filesystems.cloud`).
    pub fn cloud(&self) -> Result<Arc<FilesystemAdapter>> {
        self.disk(&self.get_default_cloud_driver())
    }

    /// Build an on-demand disk from a configuration — or, given a string,
    /// a local disk rooted at that path.
    pub fn build(&self, config: impl Into<Value>) -> Result<Arc<FilesystemAdapter>> {
        let config = match config.into() {
            Value::String(root) => json!({"driver": "local", "root": root}),
            other => other,
        };
        Ok(Arc::new(self.resolve("ondemand", Some(config))?))
    }

    fn get_config(&self, name: &str) -> Value {
        match self.config.get(&format!("filesystems.disks.{name}")) {
            Value::Object(map) => Value::Object(map),
            _ => Value::Object(Map::new()),
        }
    }

    fn resolve(&self, name: &str, config: Option<Value>) -> Result<FilesystemAdapter> {
        let config = config.unwrap_or_else(|| self.get_config(name));
        let driver = config.get("driver").and_then(Value::as_str).unwrap_or_default().to_string();
        if driver.is_empty() {
            return Err(InvalidArgumentException::new(format!("Disk [{name}] does not have a configured driver.")).into());
        }

        let creator = self.custom_creators.read().unwrap().get(&driver).cloned();
        if let Some(creator) = creator {
            return Ok(creator(&Container::get_instance(), &config)?.with_name(name));
        }

        match driver.as_str() {
            "local" => self.create_local_driver(&config, name),
            "scoped" => self.create_scoped_driver(&config, name),
            _ => Err(InvalidArgumentException::new(format!("Driver [{driver}] is not supported.")).into()),
        }
    }

    /// Create an instance of the local driver.
    pub fn create_local_driver(&self, config: &Value, name: &str) -> Result<FilesystemAdapter> {
        let driver = LocalDriver::from_config(config)?;
        Ok(FilesystemAdapter::new(Arc::new(driver), config.clone()).with_name(name))
    }

    /// Create a scoped disk: another disk's configuration with a path prefix.
    fn create_scoped_driver(&self, config: &Value, name: &str) -> Result<FilesystemAdapter> {
        let parent = match config.get("disk") {
            Some(Value::String(disk)) if !disk.is_empty() => self.get_config(disk),
            Some(Value::Object(map)) => Value::Object(map.clone()),
            _ => return Err(InvalidArgumentException::new("Scoped disk is missing \"disk\" configuration option.").into()),
        };
        let prefix = config.get("prefix").and_then(Value::as_str).unwrap_or_default();
        if prefix.is_empty() {
            return Err(InvalidArgumentException::new("Scoped disk is missing \"prefix\" configuration option.").into());
        }

        let mut parent = parent;
        let combined = match parent.get("prefix").and_then(Value::as_str).filter(|p| !p.is_empty()) {
            Some(existing) => format!("{}/{}", existing.trim_end_matches('/'), prefix.trim_start_matches('/')),
            None => prefix.to_string(),
        };
        parent["prefix"] = json!(combined);
        for key in ["visibility", "throw"] {
            if let Some(value) = config.get(key) {
                parent[key] = value.clone();
            }
        }
        self.resolve(name, Some(parent))
    }

    /// Set the given disk instance.
    pub fn set(&self, name: &str, disk: impl Into<Arc<FilesystemAdapter>>) -> &Self {
        self.disks.write().unwrap().insert(name.to_string(), disk.into());
        self
    }

    /// The default disk's name.
    pub fn get_default_driver(&self) -> String {
        self.config.string_or("filesystems.default", "local")
    }

    /// The default cloud disk's name.
    pub fn get_default_cloud_driver(&self) -> String {
        self.config.string_or("filesystems.cloud", "s3")
    }

    /// Unset the given disk instances, so they are resolved fresh next time.
    pub fn forget_disk(&self, disks: impl IntoPaths) -> &Self {
        let mut resolved = self.disks.write().unwrap();
        for disk in disks.into_paths() {
            resolved.remove(&disk);
        }
        self
    }

    /// Disconnect the given disk (the default disk when `None`).
    pub fn purge(&self, name: Option<&str>) {
        let name = name.map(str::to_string).unwrap_or_else(|| self.get_default_driver());
        self.disks.write().unwrap().remove(&name);
    }

    /// Register a custom driver creator.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_config::Repository;
    /// use illuminate_filesystem::{FilesystemAdapter, FilesystemManager, LocalDriver};
    /// use illuminate_support::json;
    ///
    /// let manager = FilesystemManager::new(Arc::new(Repository::new(json!({
    ///     "filesystems": {"disks": {"backups": {"driver": "vault", "root": "/tmp/vault"}}},
    /// }))));
    ///
    /// manager.extend("vault", |_app, config| {
    ///     let root = config["root"].as_str().unwrap_or_default();
    ///     Ok(FilesystemAdapter::new(Arc::new(LocalDriver::new(root)), config.clone()))
    /// });
    ///
    /// assert_eq!(manager.disk("backups").unwrap().name(), "backups");
    /// ```
    pub fn extend(
        &self,
        driver: &str,
        creator: impl Fn(&Container, &Value) -> Result<FilesystemAdapter> + Send + Sync + 'static,
    ) -> &Self {
        self.custom_creators.write().unwrap().insert(driver.to_string(), Arc::new(creator));
        self
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    /// Replace the given disk with a local, temporary disk for testing.
    ///
    /// The fake disk lives in a fresh directory under the system's temp
    /// directory, which is removed once the disk is dropped.
    pub fn fake(&self, disk: &str) -> Result<Arc<FilesystemAdapter>> {
        self.fake_with(disk, json!({}))
    }

    /// Replace the given disk with a fake, merging in extra configuration.
    pub fn fake_with(&self, disk: &str, config: Value) -> Result<Arc<FilesystemAdapter>> {
        let base = testing_root();
        std::fs::create_dir_all(&base)?;
        let directory = tempfile::Builder::new().prefix(&format!("{disk}-")).tempdir_in(&base)?;
        let root = directory.path().to_path_buf();

        let fake = self.create_local_driver(&self.fake_configuration(disk, config, &root), disk)?;
        let fake = fake.keep_alive(directory);

        let url = self.config.string_or("app.url", "http://localhost");
        fake.build_temporary_urls_using(move |path, expiration: Carbon, _| {
            Ok(format!(
                "{}/{}?expiration={}",
                url.trim_end_matches('/'),
                path.trim_start_matches('/'),
                expiration.timestamp()
            ))
        });

        let fake = Arc::new(fake);
        self.set(disk, fake.clone());
        Ok(fake)
    }

    /// Replace the given disk with a local testing disk that keeps its
    /// files between test runs.
    pub fn persistent_fake(&self, disk: &str) -> Result<Arc<FilesystemAdapter>> {
        let root = testing_root().join(disk);
        let fake = self.create_local_driver(&self.fake_configuration(disk, json!({}), &root), disk)?;
        let fake = Arc::new(fake);
        self.set(disk, fake.clone());
        Ok(fake)
    }

    fn fake_configuration(&self, disk: &str, config: Value, root: &std::path::Path) -> Value {
        let original = self.get_config(disk);
        let mut merged = Map::new();
        merged.insert("throw".into(), json!(original.get("throw").is_some_and(ValueExt::truthy)));
        if let Value::Object(extra) = config {
            merged.extend(extra);
        }
        merged.insert("driver".into(), json!("local"));
        merged.insert("root".into(), json!(root.to_string_lossy()));
        Value::Object(merged)
    }

    // ------------------------------------------------------------------
    // Serving files
    // ------------------------------------------------------------------

    /// The local disks configured with `'serve' => true`, and the URI each
    /// should be served from. Routing registers a `GET {uri}/{path}` route
    /// named `storage.{disk}` for each, handled by [`crate::ServeFile`].
    pub fn served_disks(&self) -> Result<Vec<ServedDisk>> {
        let mut served: Vec<ServedDisk> = Vec::new();
        let Value::Object(disks) = self.config.get("filesystems.disks") else {
            return Ok(served);
        };
        for (disk, config) in disks {
            let is_local = config.get("driver").and_then(Value::as_str) == Some("local");
            if !is_local || !config.get("serve").is_some_and(ValueExt::truthy) {
                continue;
            }
            let uri = match config.get("url").and_then(Value::as_str) {
                Some(url) => url_path(url).trim_end_matches('/').to_string(),
                None => "/storage".to_string(),
            };
            if let Some(existing) = served.iter().find(|s| s.uri == uri) {
                return Err(InvalidArgumentException::new(format!(
                    "The [{disk}] disk conflicts with the [{}] disk at [{uri}]. Each served disk must have a unique URL.",
                    existing.disk
                ))
                .into());
            }
            served.push(ServedDisk { disk: disk.clone(), uri, config: config.clone() });
        }
        Ok(served)
    }
}

/// The directory fake disks live in.
fn testing_root() -> PathBuf {
    std::env::temp_dir().join("illuminate-testing").join("disks")
}

/// The path portion of a URL (`https://example.com/storage` → `/storage`).
fn url_path(url: &str) -> String {
    let without_query = url.split(['?', '#']).next().unwrap_or_default();
    match without_query.split_once("://") {
        Some((_, rest)) => rest.find('/').map(|index| rest[index..].to_string()).unwrap_or_default(),
        None => without_query.to_string(),
    }
}

// ----------------------------------------------------------------------
// The `Storage` facade
// ----------------------------------------------------------------------

/// The `Storage` facade: your application's disks.
///
/// Methods called directly on the facade use the default disk:
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::Container;
/// use illuminate_filesystem::{FilesystemServiceProvider, Storage};
/// use illuminate_container::ServiceProvider;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({"filesystems": {"default": "local"}})));
/// FilesystemServiceProvider.register(&container);
///
/// Storage::fake("local").unwrap();
///
/// Storage::put("avatars/1.txt", "Taylor").await.unwrap();
///
/// assert_eq!(Storage::get("avatars/1.txt").await.unwrap(), "Taylor");
/// Storage::disk("local").unwrap().assert_exists("avatars/1.txt").await;
/// # });
/// ```
pub struct Storage;

fn manager() -> Result<Arc<FilesystemManager>> {
    if let Some(manager) = try_app::<FilesystemManager>() {
        return Ok(manager);
    }
    let container = Container::get_instance();
    container.singleton_if::<FilesystemManager>(crate::provider::make_manager);
    Ok(container.try_make::<FilesystemManager>()?)
}

impl Storage {
    /// Get the filesystem manager.
    pub fn manager() -> Result<Arc<FilesystemManager>> {
        manager()
    }

    /// Get a disk by name.
    pub fn disk(name: &str) -> Result<Arc<FilesystemAdapter>> {
        manager()?.disk(name)
    }

    /// Alias of [`Storage::disk`].
    pub fn drive(name: &str) -> Result<Arc<FilesystemAdapter>> {
        manager()?.disk(name)
    }

    /// Get the default disk.
    pub fn default_disk() -> Result<Arc<FilesystemAdapter>> {
        manager()?.default_disk()
    }

    /// Get the default cloud disk.
    pub fn cloud() -> Result<Arc<FilesystemAdapter>> {
        manager()?.cloud()
    }

    /// Build an on-demand disk.
    pub fn build(config: impl Into<Value>) -> Result<Arc<FilesystemAdapter>> {
        manager()?.build(config)
    }

    /// Register a custom driver creator.
    pub fn extend(
        driver: &str,
        creator: impl Fn(&Container, &Value) -> Result<FilesystemAdapter> + Send + Sync + 'static,
    ) -> Result<()> {
        manager()?.extend(driver, creator);
        Ok(())
    }

    /// Replace the given disk with a temporary local disk for testing.
    pub fn fake(disk: &str) -> Result<Arc<FilesystemAdapter>> {
        manager()?.fake(disk)
    }

    /// Replace the given disk with a fake, merging in extra configuration.
    pub fn fake_with(disk: &str, config: Value) -> Result<Arc<FilesystemAdapter>> {
        manager()?.fake_with(disk, config)
    }

    /// Replace the given disk with a persistent local disk for testing.
    pub fn persistent_fake(disk: &str) -> Result<Arc<FilesystemAdapter>> {
        manager()?.persistent_fake(disk)
    }

    /// Set the given disk instance.
    pub fn set(name: &str, disk: impl Into<Arc<FilesystemAdapter>>) -> Result<()> {
        manager()?.set(name, disk);
        Ok(())
    }

    pub async fn exists(path: &str) -> Result<bool> {
        Self::default_disk()?.exists(path).await
    }

    pub async fn missing(path: &str) -> Result<bool> {
        Self::default_disk()?.missing(path).await
    }

    pub async fn file_exists(path: &str) -> Result<bool> {
        Self::default_disk()?.file_exists(path).await
    }

    pub async fn directory_exists(path: &str) -> Result<bool> {
        Self::default_disk()?.directory_exists(path).await
    }

    pub fn path(path: &str) -> Result<PathBuf> {
        Self::default_disk()?.path(path)
    }

    pub async fn get(path: &str) -> Result<String> {
        Self::default_disk()?.get(path).await
    }

    pub async fn bytes(path: &str) -> Result<bytes::Bytes> {
        Self::default_disk()?.bytes(path).await
    }

    pub async fn json(path: &str) -> Result<Value> {
        Self::default_disk()?.json(path).await
    }

    pub async fn put(path: &str, contents: impl AsRef<[u8]>) -> Result<bool> {
        Self::default_disk()?.put(path, contents).await
    }

    pub async fn put_with_visibility(path: &str, contents: impl AsRef<[u8]>, visibility: Visibility) -> Result<bool> {
        Self::default_disk()?.put_with_visibility(path, contents, visibility).await
    }

    pub async fn put_file(path: &str, file: &illuminate_http::UploadedFile) -> Result<String> {
        Self::default_disk()?.put_file(path, file).await
    }

    pub async fn put_file_as(path: &str, file: &illuminate_http::UploadedFile, name: &str) -> Result<String> {
        Self::default_disk()?.put_file_as(path, file, name).await
    }

    pub async fn prepend(path: &str, data: impl AsRef<[u8]>) -> Result<bool> {
        Self::default_disk()?.prepend(path, data).await
    }

    pub async fn append(path: &str, data: impl AsRef<[u8]>) -> Result<bool> {
        Self::default_disk()?.append(path, data).await
    }

    pub async fn delete(paths: impl IntoPaths) -> Result<bool> {
        Self::default_disk()?.delete(paths).await
    }

    pub async fn copy(from: &str, to: &str) -> Result<bool> {
        Self::default_disk()?.copy(from, to).await
    }

    pub async fn move_(from: &str, to: &str) -> Result<bool> {
        Self::default_disk()?.move_(from, to).await
    }

    pub async fn size(path: &str) -> Result<u64> {
        Self::default_disk()?.size(path).await
    }

    pub async fn mime_type(path: &str) -> Result<String> {
        Self::default_disk()?.mime_type(path).await
    }

    pub async fn last_modified(path: &str) -> Result<i64> {
        Self::default_disk()?.last_modified(path).await
    }

    pub async fn checksum(path: &str) -> Result<String> {
        Self::default_disk()?.checksum(path).await
    }

    pub async fn get_visibility(path: &str) -> Result<Visibility> {
        Self::default_disk()?.get_visibility(path).await
    }

    pub async fn set_visibility(path: &str, visibility: Visibility) -> Result<bool> {
        Self::default_disk()?.set_visibility(path, visibility).await
    }

    pub fn url(path: &str) -> Result<String> {
        Self::default_disk()?.url(path)
    }

    pub fn temporary_url(path: &str, expiration: Carbon) -> Result<String> {
        Self::default_disk()?.temporary_url(path, expiration)
    }

    pub async fn files(directory: &str) -> Result<Vec<String>> {
        Self::default_disk()?.files(directory).await
    }

    pub async fn all_files(directory: &str) -> Result<Vec<String>> {
        Self::default_disk()?.all_files(directory).await
    }

    pub async fn directories(directory: &str) -> Result<Vec<String>> {
        Self::default_disk()?.directories(directory).await
    }

    pub async fn all_directories(directory: &str) -> Result<Vec<String>> {
        Self::default_disk()?.all_directories(directory).await
    }

    pub async fn make_directory(path: &str) -> Result<bool> {
        Self::default_disk()?.make_directory(path).await
    }

    pub async fn delete_directory(directory: &str) -> Result<bool> {
        Self::default_disk()?.delete_directory(directory).await
    }

    pub async fn download(path: &str, name: Option<&str>) -> Result<illuminate_http::Response> {
        Self::default_disk()?.download(path, name).await
    }

    pub async fn response(path: &str, name: Option<&str>) -> Result<illuminate_http::Response> {
        Self::default_disk()?.response(path, name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_container::ServiceProvider;

    fn manager_with(config: Value) -> FilesystemManager {
        FilesystemManager::new(Arc::new(Config::new(config)))
    }

    #[test]
    fn it_resolves_and_caches_disks() {
        let manager = manager_with(json!({
            "filesystems": {
                "default": "local",
                "disks": {
                    "local": {"driver": "local", "root": "/tmp/app/private"},
                    "public": {"driver": "local", "root": "/tmp/app/public", "url": "http://localhost/storage", "visibility": "public"},
                    "broken": {"root": "/tmp"},
                    "ftp": {"driver": "ftp"},
                },
            },
        }));

        let local = manager.default_disk().unwrap();
        assert!(Arc::ptr_eq(&local, &manager.disk("local").unwrap()));
        assert_eq!(local.name(), "local");
        assert_eq!(manager.disk("public").unwrap().url("a.jpg").unwrap(), "http://localhost/storage/a.jpg");

        assert_eq!(
            manager.disk("broken").unwrap_err().to_string(),
            "Disk [broken] does not have a configured driver."
        );
        assert_eq!(manager.disk("ftp").unwrap_err().to_string(), "Driver [ftp] is not supported.");
        assert_eq!(
            manager.disk("missing").unwrap_err().to_string(),
            "Disk [missing] does not have a configured driver."
        );

        manager.forget_disk("local");
        assert!(!Arc::ptr_eq(&local, &manager.disk("local").unwrap()));
    }

    #[tokio::test]
    async fn on_demand_and_scoped_disks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().to_string();
        let manager = manager_with(json!({
            "filesystems": {"disks": {
                "local": {"driver": "local", "root": root},
                "tenant": {"driver": "scoped", "disk": "local", "prefix": "tenants/1"},
            }},
        }));

        let disk = manager.build(json!({"driver": "local", "root": root})).unwrap();
        disk.put("on-demand.txt", "x").await.unwrap();
        assert!(dir.path().join("on-demand.txt").exists());

        let disk = manager.build(root.clone()).unwrap();
        assert!(disk.exists("on-demand.txt").await.unwrap());

        let tenant = manager.disk("tenant").unwrap();
        tenant.put("invoice.pdf", "pdf").await.unwrap();
        assert!(dir.path().join("tenants/1/invoice.pdf").exists());
        assert_eq!(tenant.files("").await.unwrap(), vec!["invoice.pdf"]);
    }

    #[tokio::test]
    async fn fakes_swap_disks_for_temporary_directories() {
        let manager = manager_with(json!({
            "app": {"url": "https://laravel.com"},
            "filesystems": {"disks": {"photos": {"driver": "local", "root": "/nonexistent/photos", "throw": true}}},
        }));
        let fake = manager.fake("photos").unwrap();
        assert!(Arc::ptr_eq(&fake, &manager.disk("photos").unwrap()));
        assert_eq!(fake.get_config()["throw"], json!(true));

        let root = fake.path("").unwrap();
        assert!(root.starts_with(std::env::temp_dir()));

        fake.put("photo1.jpg", "x").await.unwrap();
        fake.assert_exists("photo1.jpg").await.assert_missing("photo2.jpg").await;
        assert_eq!(
            fake.temporary_url("photo1.jpg", Carbon::from_timestamp(1_000)).unwrap(),
            "https://laravel.com/photo1.jpg?expiration=1000"
        );

        // A second fake starts empty.
        let second = manager.fake("photos").unwrap();
        second.assert_empty().await;

        drop(fake);
        assert!(!root.exists());
    }

    #[test]
    fn served_disks_are_discovered() {
        let manager = manager_with(json!({
            "filesystems": {"disks": {
                "local": {"driver": "local", "root": "/tmp/private", "serve": true},
                "public": {"driver": "local", "root": "/tmp/public", "url": "http://localhost/files/"},
                "assets": {"driver": "local", "root": "/tmp/assets", "url": "https://cdn.test/assets", "serve": true},
            }},
        }));
        let served = manager.served_disks().unwrap();
        assert_eq!(served.len(), 2);
        assert_eq!(served[0].disk, "local");
        assert_eq!(served[0].uri, "/storage");
        assert_eq!(served[0].route_name(), "storage.local");
        assert_eq!(served[0].route_uri(), "/storage/{path}");
        assert_eq!(served[1].uri, "/assets");

        let manager = manager_with(json!({
            "filesystems": {"disks": {
                "a": {"driver": "local", "root": "/tmp/a", "serve": true},
                "b": {"driver": "local", "root": "/tmp/b", "serve": true},
            }},
        }));
        assert_eq!(
            manager.served_disks().unwrap_err().to_string(),
            "The [b] disk conflicts with the [a] disk at [/storage]. Each served disk must have a unique URL."
        );
    }

    #[tokio::test]
    async fn the_facade_resolves_disks_from_the_container() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container.clone());
        container.instance(Config::new(json!({"filesystems": {"default": "public"}})));
        crate::FilesystemServiceProvider.register(&container);

        let fake = Storage::fake("public").unwrap();
        assert!(Storage::put("a.txt", "a").await.unwrap());
        assert!(Storage::exists("a.txt").await.unwrap());
        assert_eq!(Storage::files("").await.unwrap(), vec!["a.txt"]);
        assert_eq!(Storage::url("a.txt").unwrap(), "/storage/a.txt");
        assert!(Storage::delete("a.txt").await.unwrap());
        fake.assert_missing("a.txt").await;
    }
}
