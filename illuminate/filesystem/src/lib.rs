//! # Illuminate Filesystem
//!
//! Laravel's filesystem: the local [`Filesystem`] utilities behind the
//! [`File`] facade, and the disks behind the [`Storage`] facade.
//!
//! Disks are configured in `config/filesystems.php`'s Rust equivalent —
//! the `filesystems.disks` configuration — and resolved by name:
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_config::Repository;
//! use illuminate_container::{Container, ServiceProvider};
//! use illuminate_filesystem::{FilesystemServiceProvider, Storage};
//! use illuminate_support::json;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! let root = tempfile::tempdir().unwrap();
//!
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container.clone());
//! container.instance(Repository::new(json!({
//!     "filesystems": {
//!         "default": "local",
//!         "disks": {
//!             "public": {
//!                 "driver": "local",
//!                 "root": root.path().to_string_lossy(),
//!                 "url": "http://localhost/storage",
//!                 "visibility": "public",
//!             },
//!         },
//!     },
//! })));
//! FilesystemServiceProvider.register(&container);
//!
//! let disk = Storage::disk("public").unwrap();
//! disk.put("avatars/1.jpg", b"...").await.unwrap();
//!
//! assert_eq!(disk.url("avatars/1.jpg").unwrap(), "http://localhost/storage/avatars/1.jpg");
//! assert_eq!(disk.files("avatars").await.unwrap(), vec!["avatars/1.jpg"]);
//! # });
//! ```
//!
//! Uploaded files store themselves with the [`UploadedFileExt`] extension
//! trait (`file.store("avatars").await?`), and `Storage::fake("disk")`
//! swaps a disk for a temporary one in your tests.

pub mod adapter;
pub mod driver;
pub mod exceptions;
pub mod filesystem;
pub mod manager;
pub mod path;
pub mod provider;
pub mod serve;
pub mod uploaded;

pub use adapter::{FilesystemAdapter, TemporaryUrlCallback};
pub use driver::{Driver, LocalDriver, Permissions, StorageAttributes, Visibility, WriteOptions};
pub use exceptions::{CorruptedPathDetected, FileNotFoundException, FilesystemException, PathTraversalDetected};
pub use filesystem::{File, Filesystem};
pub use manager::{DiskCreator, FilesystemManager, Storage};
pub use path::{IntoPaths, normalize_path};
pub use provider::FilesystemServiceProvider;
pub use serve::{ServeFile, ServedDisk, UrlSigner};
pub use uploaded::UploadedFileExt;

/// The facades provided by this component.
pub mod facades {
    pub use crate::filesystem::File;
    pub use crate::manager::Storage;
}

/// Everything you need in one import.
pub mod prelude {
    pub use crate::uploaded::UploadedFileExt;
    pub use crate::{File, Storage, Visibility};
}
