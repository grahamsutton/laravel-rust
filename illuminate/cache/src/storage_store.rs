//! The `storage` store: cache items as files on a filesystem disk.

use std::sync::Arc;

use async_trait::async_trait;
use illuminate_filesystem::FilesystemAdapter;
use illuminate_support::{Carbon, Result, Value};
use sha1::{Digest, Sha1};

use crate::repository::FLEXIBLE_CREATED_KEY_PREFIX;
use crate::store::{Store, int_value};

/// The expiration written for items that never expire.
const FOREVER: i64 = 9_999_999_999;

/// Stores cache items as files on one of the application's filesystem
/// disks — local or in the cloud — laid out like the `file` store's.
///
/// ```json
/// "storage": {"driver": "storage", "disk": "s3", "path": "framework/cache/data"}
/// ```
pub struct StorageStore {
    disk: Arc<FilesystemAdapter>,
    directory: String,
    prefix: String,
}

impl StorageStore {
    /// Create a store keeping its files under `directory` on the disk.
    pub fn new(disk: Arc<FilesystemAdapter>, directory: &str, prefix: impl Into<String>) -> Self {
        Self {
            disk,
            directory: directory.trim_matches('/').to_string(),
            prefix: prefix.into(),
        }
    }

    /// The disk the items are stored on.
    pub fn disk(&self) -> &Arc<FilesystemAdapter> {
        &self.disk
    }

    /// The directory the items are stored in.
    pub fn directory(&self) -> &str {
        &self.directory
    }

    /// The path of the file holding the key's item.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use illuminate_cache::StorageStore;
    /// # use illuminate_filesystem::FilesystemAdapter;
    /// let disk = Arc::new(FilesystemAdapter::local(std::env::temp_dir()));
    /// let store = StorageStore::new(disk, "framework/cache/data", "");
    ///
    /// assert_eq!(
    ///     store.path("foo"),
    ///     "framework/cache/data/0b/ee/0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33"
    /// );
    /// ```
    pub fn path(&self, key: &str) -> String {
        let hash = hex::encode(Sha1::digest(format!("{}{key}", self.prefix).as_bytes()));
        format!("{}/{}/{}/{hash}", self.directory, &hash[0..2], &hash[2..4])
            .trim_matches('/')
            .to_string()
    }

    fn expiration(seconds: u64) -> i64 {
        let time = Carbon::now()
            .timestamp()
            .saturating_add(i64::try_from(seconds).unwrap_or(i64::MAX));
        if seconds == 0 || time > FOREVER { FOREVER } else { time }
    }

    /// The item and the seconds it has left, removing it once expired.
    async fn payload(&self, key: &str) -> Option<(Value, i64)> {
        let contents = self.disk.get(&self.path(key)).await.ok()?;
        let expire = contents.get(..10).and_then(|expire| expire.parse::<i64>().ok());
        let now = Carbon::now().timestamp();
        match expire {
            Some(expire) if now < expire => match serde_json::from_str(&contents[10..]) {
                Ok(value) => Some((value, expire - now)),
                Err(_) => {
                    let _ = self.forget(key).await;
                    None
                }
            },
            _ => {
                let _ = self.forget(key).await;
                None
            }
        }
    }
}

#[async_trait]
impl Store for StorageStore {
    async fn get(&self, key: &str) -> Result<Option<Value>> {
        Ok(self.payload(key).await.map(|(value, _)| value))
    }

    async fn put(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        let contents = format!("{:010}{value}", Self::expiration(seconds));
        self.disk.put(&self.path(key), contents).await
    }

    async fn add(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        if self.get(key).await?.is_some() {
            return Ok(false);
        }
        self.put(key, value, seconds).await
    }

    async fn increment(&self, key: &str, value: i64) -> Result<i64> {
        let (current, seconds) = match self.payload(key).await {
            Some((current, seconds)) => (int_value(&current), u64::try_from(seconds).unwrap_or(0)),
            None => (0, 0),
        };
        let incremented = current + value;
        self.put(key, Value::from(incremented), seconds).await?;
        Ok(incremented)
    }

    async fn forever(&self, key: &str, value: Value) -> Result<bool> {
        self.put(key, value, 0).await
    }

    async fn touch(&self, key: &str, seconds: u64) -> Result<bool> {
        match self.get(key).await? {
            Some(value) => self.put(key, value, seconds).await,
            None => Ok(false),
        }
    }

    async fn forget(&self, key: &str) -> Result<bool> {
        let path = self.path(key);
        if !self.disk.exists(&path).await? {
            return Ok(false);
        }
        let forgotten = self.disk.delete(path.as_str()).await?;
        if forgotten {
            let flexible = self.path(&format!("{FLEXIBLE_CREATED_KEY_PREFIX}{key}"));
            if self.disk.exists(&flexible).await? {
                self.disk.delete(flexible.as_str()).await?;
            }
        }
        Ok(forgotten)
    }

    async fn flush(&self) -> Result<bool> {
        if self.directory.is_empty() {
            let files = self.disk.all_files("").await?;
            return Ok(files.is_empty() || self.disk.delete(files).await?);
        }
        if self.disk.exists(&self.directory).await? {
            self.disk.delete_directory(&self.directory).await?;
        }
        self.disk.make_directory(&self.directory).await
    }

    fn get_prefix(&self) -> String {
        self.prefix.clone()
    }
}
