//! Files uploaded with a request.

use std::path::{Path, PathBuf};

use bytes::Bytes;
use illuminate_support::Str;

/// A file uploaded through a `multipart/form-data` request.
#[derive(Clone, Debug, PartialEq)]
pub struct UploadedFile {
    original_name: String,
    mime_type: String,
    contents: Bytes,
}

impl UploadedFile {
    pub fn new(original_name: impl Into<String>, mime_type: impl Into<String>, contents: impl Into<Bytes>) -> Self {
        Self {
            original_name: original_name.into(),
            mime_type: mime_type.into(),
            contents: contents.into(),
        }
    }

    /// Create a fake file for testing.
    ///
    /// ```
    /// use illuminate_http::UploadedFile;
    ///
    /// let file = UploadedFile::fake("avatar.jpg", 1024);
    /// assert_eq!(file.extension(), "jpg");
    /// assert_eq!(file.size(), 1024);
    /// ```
    pub fn fake(name: &str, kilobytes: usize) -> Self {
        let mime = mime_guess::from_path(name)
            .first_or_octet_stream()
            .essence_str()
            .to_string();
        Self::new(name, mime, vec![0u8; kilobytes])
    }

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
        mime_guess::get_mime_extensions_str(&self.mime_type)
            .and_then(|exts| exts.first())
            .map(|e| e.to_string())
            .unwrap_or(from_name)
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
        self.contents.len()
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

    /// A random, unique name for the file, keeping its extension.
    pub fn hash_name(&self) -> String {
        let ext = self.extension();
        if ext.is_empty() {
            Str::random(40)
        } else {
            format!("{}.{}", Str::random(40), ext)
        }
    }

    /// Move the file into the given directory with the given name.
    pub async fn move_to(&self, directory: impl AsRef<Path>, name: Option<&str>) -> std::io::Result<PathBuf> {
        let directory = directory.as_ref();
        tokio::fs::create_dir_all(directory).await?;
        let path = directory.join(name.map(String::from).unwrap_or_else(|| self.hash_name()));
        tokio::fs::write(&path, &self.contents).await?;
        Ok(path)
    }
}
