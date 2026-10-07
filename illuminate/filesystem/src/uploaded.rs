//! Storing uploaded files: `$request->file('avatar')->store('avatars')`.

use async_trait::async_trait;

use illuminate_http::UploadedFile;
use illuminate_support::Result;

use crate::Storage;
use crate::driver::Visibility;

/// Store uploaded files on your disks.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_filesystem::{FilesystemServiceProvider, Storage, UploadedFileExt};
/// use illuminate_http::UploadedFile;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({"filesystems": {"default": "local"}})));
/// FilesystemServiceProvider.register(&container);
/// Storage::fake("local").unwrap();
///
/// let file = UploadedFile::fake().create("avatar.jpg", 10);
///
/// let path = file.store_as("avatars", "1.jpg").await.unwrap();
///
/// assert_eq!(path, "avatars/1.jpg");
/// Storage::disk("local").unwrap().assert_exists("avatars/1.jpg").await;
/// # });
/// ```
#[async_trait]
pub trait UploadedFileExt {
    /// Store the file in the given directory of the default disk, under a
    /// unique generated name. Returns the stored path.
    async fn store(&self, path: &str) -> Result<String>;

    /// Store the file on the given disk, under a unique generated name.
    async fn store_on(&self, path: &str, disk: &str) -> Result<String>;

    /// Store the file under the given name on the default disk.
    async fn store_as(&self, path: &str, name: &str) -> Result<String>;

    /// Store the file under the given name on the given disk.
    async fn store_as_on(&self, path: &str, name: &str, disk: &str) -> Result<String>;

    /// Store the file with public visibility on the default disk.
    async fn store_publicly(&self, path: &str) -> Result<String>;

    /// Store the file with public visibility on the given disk.
    async fn store_publicly_on(&self, path: &str, disk: &str) -> Result<String>;

    /// Store the file under the given name, with public visibility.
    async fn store_publicly_as(&self, path: &str, name: &str) -> Result<String>;

    /// Store the file under the given name, with public visibility, on the given disk.
    async fn store_publicly_as_on(&self, path: &str, name: &str, disk: &str) -> Result<String>;
}

async fn store(
    file: &UploadedFile,
    path: &str,
    name: Option<&str>,
    disk: Option<&str>,
    visibility: Option<Visibility>,
) -> Result<String> {
    let disk = match disk {
        Some(disk) => Storage::disk(disk)?,
        None => Storage::default_disk()?,
    };
    let name = name.map(str::to_string).unwrap_or_else(|| file.hash_name());
    disk.store_file(path, file, &name, visibility).await
}

#[async_trait]
impl UploadedFileExt for UploadedFile {
    async fn store(&self, path: &str) -> Result<String> {
        store(self, path, None, None, None).await
    }

    async fn store_on(&self, path: &str, disk: &str) -> Result<String> {
        store(self, path, None, Some(disk), None).await
    }

    async fn store_as(&self, path: &str, name: &str) -> Result<String> {
        store(self, path, Some(name), None, None).await
    }

    async fn store_as_on(&self, path: &str, name: &str, disk: &str) -> Result<String> {
        store(self, path, Some(name), Some(disk), None).await
    }

    async fn store_publicly(&self, path: &str) -> Result<String> {
        store(self, path, None, None, Some(Visibility::Public)).await
    }

    async fn store_publicly_on(&self, path: &str, disk: &str) -> Result<String> {
        store(self, path, None, Some(disk), Some(Visibility::Public)).await
    }

    async fn store_publicly_as(&self, path: &str, name: &str) -> Result<String> {
        store(self, path, Some(name), None, Some(Visibility::Public)).await
    }

    async fn store_publicly_as_on(&self, path: &str, name: &str, disk: &str) -> Result<String> {
        store(self, path, Some(name), Some(disk), Some(Visibility::Public)).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use illuminate_config::Repository;
    use illuminate_container::{Container, ServiceProvider};
    use illuminate_support::json;

    use super::*;
    use crate::FilesystemServiceProvider;

    fn setup() -> (Arc<Container>, illuminate_container::LocalInstanceGuard) {
        let container = Arc::new(Container::new());
        let guard = Container::set_local_instance(container.clone());
        container.instance(Repository::new(json!({
            "filesystems": {"default": "local", "disks": {
                "local": {"driver": "local", "root": "/nonexistent"},
                "s3": {"driver": "s3"},
            }},
        })));
        FilesystemServiceProvider.register(&container);
        (container, guard)
    }

    #[tokio::test]
    async fn uploaded_files_can_be_stored() {
        let (_container, _guard) = setup();
        let local = Storage::fake("local").unwrap();
        let s3 = Storage::fake("s3").unwrap();
        let file = UploadedFile::new("photo.png", "image/png", b"\x89PNG\r\n\x1a\n".to_vec());

        let path = file.store("photos").await.unwrap();
        assert!(path.starts_with("photos/") && path.ends_with(".png"));
        local.assert_exists(path.as_str()).await;

        let path = file.store_on("photos", "s3").await.unwrap();
        s3.assert_exists(path.as_str()).await;

        assert_eq!(
            file.store_as_on("photos", "a.png", "s3").await.unwrap(),
            "photos/a.png"
        );
        s3.assert_exists("photos/a.png").await;

        let path = file.store_publicly_as("public", "b.png").await.unwrap();
        local.assert_exists(path.as_str()).await;
        #[cfg(unix)]
        assert_eq!(
            local.get_visibility("public/b.png").await.unwrap(),
            Visibility::Public
        );

        let path = file.store_publicly("public").await.unwrap();
        local.assert_exists(path.as_str()).await;
        let path = file.store_publicly_on("public", "s3").await.unwrap();
        s3.assert_exists(path.as_str()).await;
        let path = file
            .store_publicly_as_on("public", "c.png", "s3")
            .await
            .unwrap();
        assert_eq!(path, "public/c.png");

        assert!(file.store_as("../escape", "x.png").await.is_err());
        local.assert_count("photos", 1).await;
    }
}
