//! Files uploaded with a request.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use illuminate_support::Str;

use crate::testing::FileFactory;

/// A file uploaded through a `multipart/form-data` request.
///
/// Uploaded files are cheap to clone, and clones share their
/// [`hash_name`](UploadedFile::hash_name): store a file in a controller and
/// your test can still ask the original for the name it was stored under.
#[derive(Clone, Debug)]
pub struct UploadedFile {
    original_name: String,
    mime_type: String,
    contents: Bytes,
    size_to_report: Option<usize>,
    hash_name: Arc<OnceLock<String>>,
}

impl PartialEq for UploadedFile {
    fn eq(&self, other: &Self) -> bool {
        self.original_name == other.original_name
            && self.mime_type == other.mime_type
            && self.contents == other.contents
            && self.size_to_report == other.size_to_report
    }
}

impl UploadedFile {
    /// Create an uploaded file from its client name, MIME type and contents.
    pub fn new(
        original_name: impl Into<String>,
        mime_type: impl Into<String>,
        contents: impl Into<Bytes>,
    ) -> Self {
        Self {
            original_name: original_name.into(),
            mime_type: mime_type.into(),
            contents: contents.into(),
            size_to_report: None,
            hash_name: Arc::new(OnceLock::new()),
        }
    }

    /// Begin creating a fake file for testing.
    ///
    /// ```
    /// use illuminate_http::UploadedFile;
    ///
    /// let document = UploadedFile::fake().create("document.pdf", 1024);
    /// assert_eq!(document.size(), 1024 * 1024);
    /// assert_eq!(document.mime_type(), "application/pdf");
    ///
    /// let avatar = UploadedFile::fake().image("avatar.jpg", 200, 200);
    /// assert_eq!(avatar.dimensions(), Some((200, 200)));
    /// ```
    pub fn fake() -> FileFactory {
        FileFactory
    }

    // ------------------------------------------------------------------
    // Testing helpers
    // ------------------------------------------------------------------

    /// Report the given size (in kilobytes) instead of the real one — the
    /// fake file's `size()` method.
    ///
    /// ```
    /// use illuminate_http::UploadedFile;
    ///
    /// let avatar = UploadedFile::fake().image("avatar.jpg", 10, 10).with_size(100);
    /// assert_eq!(avatar.size(), 100 * 1024);
    /// ```
    pub fn with_size(mut self, kilobytes: usize) -> Self {
        self.size_to_report = Some(kilobytes * 1024);
        self
    }

    /// Report the given MIME type instead of the one guessed from the name —
    /// the fake file's `mimeType()` method.
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = mime_type.into();
        self
    }

    // ------------------------------------------------------------------
    // Inspection
    // ------------------------------------------------------------------

    /// The original name of the file on the client's machine.
    pub fn client_original_name(&self) -> &str {
        &self.original_name
    }

    /// Alias of `client_original_name`.
    pub fn original_name(&self) -> &str {
        &self.original_name
    }

    /// The extension from the client's original file name.
    pub fn client_original_extension(&self) -> String {
        Path::new(&self.original_name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    }

    /// The extension guessed from the client's MIME type.
    pub fn client_extension(&self) -> String {
        crate::testing::MimeType::search(&self.mime_type).unwrap_or_default()
    }

    /// The file's extension, guessed from its MIME type (falling back to the
    /// client provided name).
    pub fn extension(&self) -> String {
        let from_name = self.client_original_extension();
        if !from_name.is_empty() {
            let guessed = mime_guess::from_ext(&from_name).first_or_octet_stream();
            if guessed.essence_str() == self.mime_type {
                return from_name;
            }
        }
        crate::testing::MimeType::search(&self.mime_type).unwrap_or(from_name)
    }

    /// The MIME type reported by the client.
    pub fn client_mime_type(&self) -> &str {
        &self.mime_type
    }

    /// The MIME type of the file.
    pub fn mime_type(&self) -> &str {
        &self.mime_type
    }

    /// The size of the file in bytes.
    pub fn size(&self) -> usize {
        match self.size_to_report {
            Some(size) if size > 0 => size,
            _ => self.contents.len(),
        }
    }

    /// The raw contents of the file.
    pub fn bytes(&self) -> &Bytes {
        &self.contents
    }

    /// The contents of the file as (lossy) UTF-8 text.
    pub fn get(&self) -> String {
        String::from_utf8_lossy(&self.contents).into_owned()
    }

    /// Whether the upload was successful.
    pub fn is_valid(&self) -> bool {
        !self.original_name.is_empty()
    }

    /// The width and height of the image, if the file is one.
    ///
    /// ```
    /// use illuminate_http::UploadedFile;
    ///
    /// let photo = UploadedFile::fake().image("photo.png", 64, 32);
    /// assert_eq!(photo.dimensions(), Some((64, 32)));
    /// assert_eq!(UploadedFile::fake().create("notes.txt", 1).dimensions(), None);
    /// ```
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        if self.contents.is_empty() {
            return None;
        }
        image::ImageReader::new(Cursor::new(self.contents.as_ref()))
            .with_guessed_format()
            .ok()?
            .into_dimensions()
            .ok()
    }

    /// A random, unique name for the file, keeping its extension.
    ///
    /// The random part is generated once and remembered (by every clone of
    /// the file), so it matches the name the file was stored under:
    ///
    /// ```
    /// use illuminate_http::UploadedFile;
    ///
    /// let file = UploadedFile::fake().image("avatar.jpg", 10, 10);
    /// let copy = file.clone();
    ///
    /// assert_eq!(file.hash_name(), copy.hash_name());
    /// assert!(file.hash_name().ends_with(".jpg"));
    /// assert_eq!(file.hash_name().len(), 44);
    /// ```
    pub fn hash_name(&self) -> String {
        let hash = self.hash_name.get_or_init(|| Str::random(40));
        let ext = self.extension();
        if ext.is_empty() {
            hash.clone()
        } else {
            format!("{hash}.{ext}")
        }
    }

    /// The hash name, inside the given directory (`avatars/<hash>.jpg`).
    pub fn hash_name_in(&self, path: &str) -> String {
        let path = path.trim_end_matches('/');
        if path.is_empty() {
            self.hash_name()
        } else {
            format!("{path}/{}", self.hash_name())
        }
    }

    /// Move the file into the given directory with the given name.
    pub async fn move_to(
        &self,
        directory: impl AsRef<Path>,
        name: Option<&str>,
    ) -> std::io::Result<PathBuf> {
        let directory = directory.as_ref();
        tokio::fs::create_dir_all(directory).await?;
        let path = directory.join(name.map(String::from).unwrap_or_else(|| self.hash_name()));
        tokio::fs::write(&path, &self.contents).await?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_files_report_their_size_in_kilobytes() {
        let file = UploadedFile::fake().create("document.pdf", 2);
        assert_eq!(file.size(), 2048);
        assert!(file.bytes().is_empty());
        assert_eq!(file.mime_type(), "application/pdf");
        assert_eq!(file.client_extension(), "pdf");

        // A zero size reports the real size, like Laravel.
        let file = UploadedFile::fake().create_with_content("notes.txt", "Hello");
        assert_eq!(file.size(), 5);
        assert_eq!(file.get(), "Hello");
    }

    #[test]
    fn hash_names_are_shared_between_clones_but_not_files() {
        let file = UploadedFile::fake().create("a.txt", 1);
        let other = UploadedFile::fake().create("a.txt", 1);
        assert_eq!(file.hash_name(), file.clone().hash_name());
        assert_ne!(file.hash_name(), other.hash_name());
        assert_eq!(file, other);
        assert!(file.hash_name_in("docs/").starts_with("docs/"));
        assert!(file.hash_name().ends_with(".txt"));
    }
}
