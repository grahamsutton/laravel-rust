//! The file cache store.
//!
//! The layout matches Laravel's: each key is hashed with SHA-1 and stored at
//! `{directory}/ab/cd/abcd...`, and every file starts with a ten digit
//! expiration timestamp followed by the (JSON encoded) value.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use sha1::{Digest, Sha1};

use illuminate_filesystem::Filesystem;
use illuminate_support::{Carbon, Result, Value, json};

use crate::lock::{CacheLock, Lock, LockDriver, LockInfo};
use crate::repository::FLEXIBLE_CREATED_KEY_PREFIX;
use crate::store::{LockProvider, Store, int_value, separate_lock_store_required};
use crate::util::blocking;

/// The furthest expiration a ten digit timestamp can express ("forever").
const FOREVER: i64 = 9_999_999_999;

/// A cache store backed by files on the local disk.
///
/// ```
/// use illuminate_cache::{FileStore, Store};
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let directory = tempfile::tempdir().unwrap();
/// let store = FileStore::new(directory.path());
///
/// store.put("framework", json!("Laravel"), 60).await.unwrap();
///
/// assert_eq!(store.get("framework").await.unwrap(), Some(json!("Laravel")));
/// assert!(store.path("framework").starts_with(directory.path()));
/// # });
/// ```
#[derive(Debug, Clone)]
pub struct FileStore {
    files: Filesystem,
    directory: PathBuf,
    lock_directory: Option<PathBuf>,
    file_permission: Option<u32>,
}

impl FileStore {
    /// Create a new file cache store in the given directory.
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            files: Filesystem::new(),
            directory: directory.into(),
            lock_directory: None,
            file_permission: None,
        }
    }

    /// Store locks in a separate directory (the `lock_path` option).
    pub fn with_lock_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.lock_directory = Some(directory.into());
        self
    }

    /// Give cache files (and their directories) the given permissions.
    pub fn with_file_permission(mut self, permission: u32) -> Self {
        self.file_permission = Some(permission);
        self
    }

    /// The working directory of the cache.
    pub fn get_directory(&self) -> &Path {
        &self.directory
    }

    /// The directory locks are stored in.
    pub fn get_lock_directory(&self) -> &Path {
        self.lock_directory.as_deref().unwrap_or(&self.directory)
    }

    /// The underlying filesystem.
    pub fn get_filesystem(&self) -> Filesystem {
        self.files
    }

    /// Determine if the lock store is separate from the cache store.
    pub fn has_separate_lock_store(&self) -> bool {
        self.lock_directory
            .as_ref()
            .is_some_and(|locks| *locks != self.directory)
    }

    /// The full path for the given cache key.
    pub fn path(&self, key: &str) -> PathBuf {
        let hash = hex::encode(Sha1::digest(key.as_bytes()));
        self.directory
            .join(&hash[0..2])
            .join(&hash[2..4])
            .join(&hash)
    }

    /// Refresh a lock's expiration if it is still owned by the given owner.
    pub async fn refresh_if_owned(&self, key: &str, owner: &str, seconds: u64) -> Result<bool> {
        let (store, key, owner) = (self.clone(), key.to_string(), owner.to_string());
        blocking(move || store.refresh_if_owned_sync(&key, &owner, seconds)).await
    }

    // ------------------------------------------------------------------
    // Blocking implementation
    // ------------------------------------------------------------------

    fn current_time() -> i64 {
        Carbon::now().timestamp()
    }

    fn expiration(seconds: u64) -> i64 {
        let time = Self::current_time().saturating_add(i64::try_from(seconds).unwrap_or(i64::MAX));
        if seconds == 0 || time > FOREVER {
            FOREVER
        } else {
            time
        }
    }

    fn payload(value: &Value, seconds: u64) -> String {
        format!("{:010}{}", Self::expiration(seconds), value)
    }

    fn ensure_cache_directory_exists(&self, path: &Path) -> Result<()> {
        let Some(directory) = path.parent() else {
            return Ok(());
        };
        if !directory.exists() {
            self.files.make_directory_sync(directory, 0o777, true)?;
            // Two levels of directories were created (e.g. 7e/24), so fix both.
            self.ensure_permissions_are_correct(directory);
            if let Some(parent) = directory.parent() {
                self.ensure_permissions_are_correct(parent);
            }
        }
        Ok(())
    }

    fn ensure_permissions_are_correct(&self, path: &Path) {
        if let Some(permission) = self.file_permission {
            let _ = self.files.chmod(path, permission);
        }
    }

    fn open_for_update(&self, path: &Path) -> Result<fs::File> {
        self.ensure_cache_directory_exists(path)?;
        Ok(fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?)
    }

    fn rewrite(file: &mut fs::File, contents: &str) -> std::io::Result<()> {
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(contents.as_bytes())
    }

    /// Parse a cache file: its expiration and its (still encoded) data.
    fn parse(contents: &str) -> Option<(i64, &str)> {
        let expire = contents.get(..10)?.parse::<i64>().ok()?;
        Some((expire, &contents[10..]))
    }

    fn get_payload_sync(&self, key: &str) -> Option<(Value, i64)> {
        let contents = self.files.shared_get_sync(self.path(key)).ok()?;
        let now = Self::current_time();
        match Self::parse(&contents) {
            Some((expire, data)) if now < expire => match serde_json::from_str(data) {
                Ok(value) => Some((value, expire - now)),
                Err(_) => {
                    self.forget_sync(key);
                    None
                }
            },
            _ => {
                self.forget_sync(key);
                None
            }
        }
    }

    fn put_sync(&self, key: &str, value: &Value, seconds: u64) -> Result<bool> {
        let path = self.path(key);
        self.ensure_cache_directory_exists(&path)?;
        self.files
            .put_locked_sync(&path, Self::payload(value, seconds))?;
        self.ensure_permissions_are_correct(&path);
        Ok(true)
    }

    fn add_sync(&self, key: &str, value: &Value, seconds: u64) -> Result<bool> {
        let path = self.path(key);
        let mut file = self.open_for_update(&path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Ok(false),
            Err(fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        let mut expire = String::new();
        let read = (&mut file).take(10).read_to_string(&mut expire);
        let expired = read.is_err()
            || expire.is_empty()
            || expire
                .parse::<i64>()
                .map(|expire| Self::current_time() >= expire)
                .unwrap_or(true);
        let result = if expired {
            Self::rewrite(&mut file, &Self::payload(value, seconds)).map(|_| true)
        } else {
            Ok(false)
        };
        file.unlock()?;
        let added = result?;
        if added {
            self.ensure_permissions_are_correct(&path);
        }
        Ok(added)
    }

    fn increment_sync(&self, key: &str, value: i64) -> Result<i64> {
        let path = self.path(key);
        let mut file = self.open_for_update(&path)?;
        file.lock()?;
        let result = (|| -> Result<i64> {
            let mut contents = String::new();
            let _ = file.read_to_string(&mut contents);
            let now = Self::current_time();
            let (current, remaining) = match Self::parse(&contents) {
                Some((expire, data)) if now < expire => {
                    let current = serde_json::from_str::<Value>(data)
                        .map(|v| int_value(&v))
                        .unwrap_or(0);
                    (current, (expire - now) as u64)
                }
                _ => (0, 0),
            };
            let incremented = current + value;
            Self::rewrite(&mut file, &Self::payload(&json!(incremented), remaining))?;
            Ok(incremented)
        })();
        file.unlock()?;
        self.ensure_permissions_are_correct(&path);
        result
    }

    fn forget_sync(&self, key: &str) -> bool {
        let path = self.path(key);
        if !path.exists() {
            return false;
        }
        let forgotten = fs::remove_file(&path).is_ok();
        if forgotten {
            let flexible = self.path(&format!("{FLEXIBLE_CREATED_KEY_PREFIX}{key}"));
            if flexible.exists() {
                let _ = fs::remove_file(flexible);
            }
        }
        forgotten
    }

    fn flush_directory(&self, directory: &Path) -> bool {
        if !directory.is_dir() {
            return false;
        }
        for directory in self.files.directories(directory) {
            if !self.files.delete_directory_sync(&directory) || directory.exists() {
                return false;
            }
        }
        true
    }

    fn refresh_if_owned_sync(&self, key: &str, owner: &str, seconds: u64) -> Result<bool> {
        let path = self.path(key);
        let mut file = self.open_for_update(&path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Ok(false),
            Err(fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        let result = (|| -> Result<bool> {
            let mut contents = String::new();
            let _ = file.read_to_string(&mut contents);
            let Some((expire, data)) = Self::parse(&contents) else {
                return Ok(false);
            };
            let current_owner = serde_json::from_str::<Value>(data).ok();
            if current_owner.as_ref().and_then(Value::as_str) != Some(owner)
                || Self::current_time() >= expire
            {
                return Ok(false);
            }
            Self::rewrite(&mut file, &Self::payload(&json!(owner), seconds))?;
            Ok(true)
        })();
        file.unlock()?;
        result
    }
}

#[async_trait]
impl Store for FileStore {
    async fn get(&self, key: &str) -> Result<Option<Value>> {
        let (store, key) = (self.clone(), key.to_string());
        blocking(move || Ok(store.get_payload_sync(&key).map(|(value, _)| value))).await
    }

    async fn put(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        let (store, key) = (self.clone(), key.to_string());
        blocking(move || store.put_sync(&key, &value, seconds)).await
    }

    async fn add(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        let (store, key) = (self.clone(), key.to_string());
        blocking(move || store.add_sync(&key, &value, seconds)).await
    }

    async fn increment(&self, key: &str, value: i64) -> Result<i64> {
        let (store, key) = (self.clone(), key.to_string());
        blocking(move || store.increment_sync(&key, value)).await
    }

    async fn forever(&self, key: &str, value: Value) -> Result<bool> {
        self.put(key, value, 0).await
    }

    async fn touch(&self, key: &str, seconds: u64) -> Result<bool> {
        let (store, key) = (self.clone(), key.to_string());
        blocking(move || match store.get_payload_sync(&key) {
            Some((value, _)) => store.put_sync(&key, &value, seconds),
            None => Ok(false),
        })
        .await
    }

    async fn forget(&self, key: &str) -> Result<bool> {
        let (store, key) = (self.clone(), key.to_string());
        blocking(move || Ok(store.forget_sync(&key))).await
    }

    async fn flush(&self) -> Result<bool> {
        let store = self.clone();
        blocking(move || Ok(store.flush_directory(&store.directory))).await
    }

    fn lock_provider(&self) -> Option<&dyn LockProvider> {
        Some(self)
    }

    async fn flush_locks(&self) -> Result<bool> {
        if !self.has_separate_lock_store() {
            return Err(separate_lock_store_required());
        }
        let store = self.clone();
        blocking(move || Ok(store.flush_directory(store.get_lock_directory()))).await
    }
}

impl LockProvider for FileStore {
    fn lock(&self, name: &str, seconds: u64, owner: Option<String>) -> Lock {
        let mut store = FileStore::new(self.get_lock_directory());
        store.file_permission = self.file_permission;
        Lock::new(
            Arc::new(FileLock::new(store)),
            format!("file-store-lock:{name}"),
            seconds,
            owner,
        )
    }
}

/// Locks for the file store: an atomic `add` on a lock file.
pub struct FileLock {
    store: Arc<FileStore>,
    cache_lock: CacheLock,
}

impl FileLock {
    /// Create a lock driver keeping its locks in the given store.
    pub fn new(store: FileStore) -> Self {
        let store = Arc::new(store);
        Self {
            cache_lock: CacheLock::new(store.clone()),
            store,
        }
    }
}

#[async_trait]
impl LockDriver for FileLock {
    async fn acquire(&self, lock: &LockInfo) -> Result<bool> {
        self.store
            .add(&lock.name, json!(lock.owner), lock.seconds)
            .await
    }

    async fn release(&self, lock: &LockInfo) -> Result<bool> {
        self.cache_lock.release(lock).await
    }

    async fn force_release(&self, lock: &LockInfo) -> Result<()> {
        self.cache_lock.force_release(lock).await
    }

    async fn current_owner(&self, lock: &LockInfo) -> Result<Option<String>> {
        self.cache_lock.current_owner(lock).await
    }

    async fn refresh(&self, lock: &LockInfo, seconds: u64) -> Result<bool> {
        self.store
            .refresh_if_owned(&lock.name, &lock.owner, seconds)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::freeze_time;

    fn store() -> (tempfile::TempDir, FileStore) {
        let directory = tempfile::tempdir().unwrap();
        let store = FileStore::new(directory.path().join("data"));
        (directory, store)
    }

    #[tokio::test]
    async fn it_uses_laravels_file_layout() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_dir, store) = store();
        store.put("foo", json!({"bar": "baz"}), 60).await.unwrap();

        // sha1("foo") = 0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33
        let path = store.path("foo");
        assert!(path.ends_with("0b/ee/0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33"));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"1700000060{"bar":"baz"}"#
        );

        store.forever("forever", json!(1)).await.unwrap();
        assert_eq!(
            fs::read_to_string(store.path("forever")).unwrap(),
            "99999999991"
        );
    }

    #[tokio::test]
    async fn items_expire_and_are_cleaned_up() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_dir, store) = store();
        store.put("foo", json!("bar"), 10).await.unwrap();
        time.travel_seconds(9);
        assert_eq!(store.get("foo").await.unwrap(), Some(json!("bar")));
        time.travel_seconds(1);
        assert_eq!(store.get("foo").await.unwrap(), None);
        assert!(!store.path("foo").exists());
        assert_eq!(store.get("never-stored").await.unwrap(), None);
    }

    #[tokio::test]
    async fn corrupted_files_are_forgotten() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_dir, store) = store();
        store.put("foo", json!("bar"), 10).await.unwrap();
        fs::write(store.path("foo"), "9999999999{not json").unwrap();
        assert_eq!(store.get("foo").await.unwrap(), None);
        assert!(!store.path("foo").exists());
    }

    #[tokio::test]
    async fn add_increment_and_touch() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_dir, store) = store();
        assert!(store.add("key", json!("first"), 10).await.unwrap());
        assert!(!store.add("key", json!("second"), 10).await.unwrap());
        assert_eq!(store.get("key").await.unwrap(), Some(json!("first")));
        time.travel_seconds(10);
        assert!(store.add("key", json!("third"), 10).await.unwrap());

        assert_eq!(store.increment("count", 1).await.unwrap(), 1);
        assert_eq!(store.increment("count", 5).await.unwrap(), 6);
        assert_eq!(store.decrement("count", 2).await.unwrap(), 4);
        assert!(
            fs::read_to_string(store.path("count"))
                .unwrap()
                .starts_with("9999999999")
        );

        // Incrementing keeps the remaining lifetime.
        store.put("limited", json!(1), 30).await.unwrap();
        time.travel_seconds(10);
        assert_eq!(store.increment("limited", 1).await.unwrap(), 2);
        time.travel_seconds(20);
        assert_eq!(store.get("limited").await.unwrap(), None);

        store.put("touched", json!(1), 10).await.unwrap();
        assert!(store.touch("touched", 100).await.unwrap());
        time.travel_seconds(50);
        assert!(store.get("touched").await.unwrap().is_some());
        assert!(!store.touch("missing", 100).await.unwrap());
    }

    #[tokio::test]
    async fn concurrent_adds_have_a_single_winner() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_dir, store) = store();
        let mut handles = Vec::new();
        for i in 0..10 {
            let store = store.clone();
            handles.push(std::thread::spawn(move || {
                store.add_sync("race", &json!(i), 60).unwrap()
            }));
        }
        let winners = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(winners, 1);
    }

    #[tokio::test]
    async fn forget_and_flush() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_dir, store) = store();
        assert!(!store.flush().await.unwrap());

        store.put("a", json!(1), 10).await.unwrap();
        store
            .put(&format!("{FLEXIBLE_CREATED_KEY_PREFIX}a"), json!(1), 10)
            .await
            .unwrap();
        assert!(store.forget("a").await.unwrap());
        assert!(
            !store
                .path(&format!("{FLEXIBLE_CREATED_KEY_PREFIX}a"))
                .exists()
        );
        assert!(!store.forget("a").await.unwrap());

        store.put("b", json!(2), 10).await.unwrap();
        assert!(store.flush().await.unwrap());
        assert_eq!(store.get("b").await.unwrap(), None);
        assert!(store.get_directory().is_dir());
    }

    #[tokio::test]
    async fn locks_live_in_the_lock_directory() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let directory = tempfile::tempdir().unwrap();
        let store = FileStore::new(directory.path().join("data"))
            .with_lock_directory(directory.path().join("locks"));
        assert!(store.has_separate_lock_store());

        let lock = store.lock("report", 10, None);
        assert!(lock.get().await.unwrap());
        assert!(!store.lock("report", 10, None).get().await.unwrap());
        assert!(lock.is_owned_by_current_process().await.unwrap());
        assert!(directory.path().join("locks").is_dir());

        time.travel_seconds(5);
        assert!(lock.refresh(None).await.unwrap());
        time.travel_seconds(6);
        assert!(!store.lock("report", 10, None).get().await.unwrap());

        let restored = store.restore_lock("report", lock.owner());
        assert!(restored.release().await.unwrap());
        assert!(!lock.is_locked().await.unwrap());

        assert!(store.lock("other", 0, None).get().await.unwrap());
        assert!(store.flush_locks().await.unwrap());
        assert!(store.lock("other", 0, None).get().await.unwrap());

        let shared = FileStore::new(directory.path().join("shared"));
        assert!(shared.flush_locks().await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn file_permissions_can_be_configured() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let directory = tempfile::tempdir().unwrap();
        let store = FileStore::new(directory.path()).with_file_permission(0o775);
        store.put("secret", json!(1), 10).await.unwrap();
        assert_eq!(
            Filesystem::new().permissions(store.path("secret")).unwrap(),
            "0775"
        );
    }
}
