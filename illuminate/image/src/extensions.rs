//! Reading images from requests, uploaded files, and filesystem disks.

use std::sync::Arc;

use illuminate_filesystem::{FilesystemAdapter, Storage};
use illuminate_http::{Request, UploadedFile};

use crate::instance::Image;

/// Retrieve uploaded images from the request: `request.image("avatar")`.
///
/// ```
/// use illuminate_http::{Request, UploadedFile};
/// use illuminate_image::RequestImageExt;
///
/// let request = Request::create("/avatar", "POST");
/// request.attach_file("avatar", UploadedFile::new("me.png", "image/png", b"\x89PNG\r\n\x1a\n".to_vec()));
///
/// let image = request.image("avatar").unwrap();
///
/// assert_eq!(image.file().unwrap().client_original_name(), "me.png");
/// assert!(request.image("missing").is_none());
/// ```
pub trait RequestImageExt {
    /// Retrieve a file from the request as an image, or `None` if no file
    /// was uploaded under the given key.
    fn image(&self, key: &str) -> Option<Image>;
}

impl RequestImageExt for Request {
    fn image(&self, key: &str) -> Option<Image> {
        self.file(key).map(|file| Image::from_upload(&file))
    }
}

/// Turn an uploaded file into an image: `file.image()`.
///
/// ```
/// use illuminate_http::UploadedFile;
/// use illuminate_image::UploadedFileImageExt;
///
/// let file = UploadedFile::new("me.png", "image/png", b"\x89PNG\r\n\x1a\n".to_vec());
///
/// let image = file.image().cover(400, 400);
///
/// assert_eq!(image.file(), Some(&file));
/// ```
pub trait UploadedFileImageExt {
    /// Create an image from the uploaded file.
    fn image(&self) -> Image;
}

impl UploadedFileImageExt for UploadedFile {
    fn image(&self) -> Image {
        Image::from_upload(self)
    }
}

/// Read images from the default disk: `Storage::image("avatars/photo.jpg")`.
///
/// ```
/// use illuminate_filesystem::Storage;
/// use illuminate_image::StorageImageExt;
///
/// let image = Storage::image("avatars/photo.jpg");
///
/// assert!(!image.is_loaded());
/// ```
pub trait StorageImageExt {
    /// Create an image from a file on the default disk. The file is read lazily.
    fn image(path: &str) -> Image;
}

impl StorageImageExt for Storage {
    fn image(path: &str) -> Image {
        Image::from_default_disk(path)
    }
}

/// Read images from a filesystem disk: `Storage::disk("public")?.image("avatars/photo.jpg")`.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_filesystem::FilesystemAdapter;
/// use illuminate_image::FilesystemImageExt;
///
/// let disk = Arc::new(FilesystemAdapter::local(std::env::temp_dir()));
///
/// let image = disk.image("avatars/photo.jpg");
///
/// assert!(!image.is_loaded());
/// ```
pub trait FilesystemImageExt {
    /// Create an image from a file on the disk. The file is read lazily.
    fn image(&self, path: &str) -> Image;
}

impl FilesystemImageExt for Arc<FilesystemAdapter> {
    fn image(&self, path: &str) -> Image {
        let disk = self.clone();
        let path = path.to_string();

        Image::lazy(move || {
            let (disk, path) = (disk.clone(), path.clone());
            async move { disk.bytes(&path).await }
        })
    }
}
