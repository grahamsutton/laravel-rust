//! Attachments: files from disk, from storage disks, or raw data.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use illuminate_support::Result;
use illuminate_support::error::RuntimeException;
use serde::{Deserialize, Serialize};

/// The data behind a lazily resolved attachment.
type DataResolver = Arc<dyn Fn() -> Vec<u8> + Send + Sync>;

#[derive(Clone)]
enum Source {
    Path(PathBuf),
    Storage { disk: Option<String>, path: String },
    Data(DataResolver),
}

/// A file attached to a mail message.
///
/// Attachments are resolved (read from disk or storage) when the message is
/// sent, so building them is cheap:
///
/// ```
/// use illuminate_mail::Attachment;
///
/// let attachment = Attachment::from_path("/path/to/file.pdf")
///     .as_("Invoice.pdf")
///     .with_mime("application/pdf");
///
/// assert_eq!(attachment.name.as_deref(), Some("Invoice.pdf"));
///
/// let report = Attachment::from_data(|| b"id,total\n1,10".to_vec(), "report.csv");
/// assert_eq!(report.name.as_deref(), Some("report.csv"));
/// ```
#[derive(Clone)]
pub struct Attachment {
    source: Source,
    /// The attached file's name.
    pub name: Option<String>,
    /// The attached file's MIME type.
    pub mime: Option<String>,
    /// The content ID, when the attachment is embedded inline.
    pub content_id: Option<String>,
}

impl fmt::Debug for Attachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source = match &self.source {
            Source::Path(path) => format!("path: {}", path.display()),
            Source::Storage { disk, path } => {
                format!("storage: {}:{path}", disk.as_deref().unwrap_or("default"))
            }
            Source::Data(_) => "data".to_string(),
        };
        f.debug_struct("Attachment")
            .field("source", &source)
            .field("name", &self.name)
            .field("mime", &self.mime)
            .field("content_id", &self.content_id)
            .finish()
    }
}

impl Attachment {
    fn new(source: Source) -> Self {
        Self {
            source,
            name: None,
            mime: None,
            content_id: None,
        }
    }

    /// Create a mail attachment from a path on disk.
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self::new(Source::Path(path.into()))
    }

    /// Create a mail attachment from a file on the default storage disk.
    pub fn from_storage(path: impl Into<String>) -> Self {
        Self::new(Source::Storage {
            disk: None,
            path: path.into(),
        })
    }

    /// Create a mail attachment from a file on the given storage disk.
    pub fn from_storage_disk(disk: impl Into<String>, path: impl Into<String>) -> Self {
        Self::new(Source::Storage {
            disk: Some(disk.into()),
            path: path.into(),
        })
    }

    /// Create a mail attachment from data produced by a closure.
    pub fn from_data<D: Into<Vec<u8>>>(
        data: impl Fn() -> D + Send + Sync + 'static,
        name: impl Into<String>,
    ) -> Self {
        Self::new(Source::Data(Arc::new(move || data().into()))).as_(name)
    }

    /// Create a mail attachment from in-memory bytes.
    pub fn from_bytes(data: impl Into<Vec<u8>>, name: impl Into<String>) -> Self {
        let data = Arc::new(data.into());
        Self::new(Source::Data(Arc::new(move || data.as_ref().clone()))).as_(name)
    }

    /// Create a mail attachment from an uploaded file.
    pub fn from_uploaded_file(file: &illuminate_http::UploadedFile) -> Self {
        let mime = file.mime_type().to_string();
        Self::from_bytes(file.bytes().to_vec(), file.client_original_name()).with_mime(mime)
    }

    /// Set the attached file's name (Laravel's `as`).
    pub fn as_(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the attached file's MIME type.
    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = Some(mime.into());
        self
    }

    /// Embed the attachment inline under the given content ID.
    pub fn inline(mut self, content_id: impl Into<String>) -> Self {
        self.content_id = Some(content_id.into());
        self
    }

    /// The path, when the attachment comes from a file on disk.
    pub fn path(&self) -> Option<&Path> {
        match &self.source {
            Source::Path(path) => Some(path),
            _ => None,
        }
    }

    /// The disk and path, when the attachment comes from a storage disk.
    pub fn storage(&self) -> Option<(Option<&str>, &str)> {
        match &self.source {
            Source::Storage { disk, path } => Some((disk.as_deref(), path.as_str())),
            _ => None,
        }
    }

    /// The attachment's data, when it was created from data.
    pub fn data(&self) -> Option<Vec<u8>> {
        match &self.source {
            Source::Data(resolver) => Some(resolver()),
            _ => None,
        }
    }

    /// Determine if this attachment is equivalent to another one.
    pub fn is_equivalent(&self, other: &Attachment) -> bool {
        let same_source = match (&self.source, &other.source) {
            (Source::Path(a), Source::Path(b)) => a == b,
            (Source::Storage { disk: a, path: p }, Source::Storage { disk: b, path: q }) => {
                a == b && p == q
            }
            (Source::Data(a), Source::Data(b)) => a() == b(),
            _ => false,
        };
        same_source && self.name == other.name && self.mime == other.mime
    }

    /// Read the attachment's contents, ready to be added to a message.
    pub async fn resolve(&self) -> Result<MessageAttachment> {
        let (data, name, mime) = match &self.source {
            Source::Path(path) => {
                let data = tokio::fs::read(path).await.map_err(|e| {
                    RuntimeException::new(format!(
                        "Unable to open path \"{}\" for attachment: {e}",
                        path.display()
                    ))
                })?;
                let name = self.name.clone().unwrap_or_else(|| file_name(path));
                (data, name, self.mime.clone())
            }
            Source::Storage { disk, path } => {
                let storage = match disk {
                    Some(disk) => illuminate_filesystem::Storage::disk(disk)?,
                    None => illuminate_filesystem::Storage::default_disk()?,
                };
                let data = storage.bytes(path).await?.to_vec();
                let mime = match &self.mime {
                    Some(mime) => Some(mime.clone()),
                    None => storage.mime_type(path).await.ok(),
                };
                let name = self
                    .name
                    .clone()
                    .unwrap_or_else(|| file_name(Path::new(path)));
                (data, name, mime)
            }
            Source::Data(resolver) => {
                let name = self.name.clone().ok_or_else(|| {
                    RuntimeException::new("Attachment requires a filename to be specified.")
                })?;
                (resolver(), name, self.mime.clone())
            }
        };
        let mime = mime.unwrap_or_else(|| guess_mime(&name));
        Ok(MessageAttachment {
            filename: name,
            content_type: mime,
            data,
            content_id: self.content_id.clone(),
        })
    }
}

impl From<&str> for Attachment {
    fn from(path: &str) -> Self {
        Attachment::from_path(path)
    }
}

impl From<String> for Attachment {
    fn from(path: String) -> Self {
        Attachment::from_path(path)
    }
}

impl From<PathBuf> for Attachment {
    fn from(path: PathBuf) -> Self {
        Attachment::from_path(path)
    }
}

impl From<&Path> for Attachment {
    fn from(path: &Path) -> Self {
        Attachment::from_path(path)
    }
}

impl<T: Attachable + ?Sized> From<&T> for Attachment {
    fn from(attachable: &T) -> Self {
        attachable.to_mail_attachment()
    }
}

/// Objects that can be attached to mail, like a stored `Photo` model.
///
/// ```
/// use illuminate_mail::{Attachable, Attachment};
///
/// struct Photo { path: String }
///
/// impl Attachable for Photo {
///     fn to_mail_attachment(&self) -> Attachment {
///         Attachment::from_storage_disk("s3", &self.path)
///     }
/// }
///
/// let attachment: Attachment = (&Photo { path: "photos/1.jpg".into() }).into();
/// assert_eq!(attachment.storage(), Some((Some("s3"), "photos/1.jpg")));
/// ```
pub trait Attachable {
    /// Get an attachment representation of the object.
    fn to_mail_attachment(&self) -> Attachment;
}

/// An attachment whose contents have been read, ready to be sent.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageAttachment {
    /// The file name.
    pub filename: String,
    /// The MIME type.
    pub content_type: String,
    /// The file's contents.
    #[serde(with = "base64_bytes")]
    pub data: Vec<u8>,
    /// The content ID, for attachments embedded inline.
    pub content_id: Option<String>,
}

impl fmt::Debug for MessageAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessageAttachment")
            .field("filename", &self.filename)
            .field("content_type", &self.content_type)
            .field("size", &self.data.len())
            .field("content_id", &self.content_id)
            .finish()
    }
}

impl MessageAttachment {
    /// Determine if the attachment is embedded inline.
    pub fn is_inline(&self) -> bool {
        self.content_id.is_some()
    }

    /// The contents as (lossy) UTF-8 text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Guess a MIME type from a file name.
pub(crate) fn guess_mime(name: &str) -> String {
    mime_guess::from_path(name).first().map_or_else(
        || "application/octet-stream".to_string(),
        |m| m.essence_str().to_string(),
    )
}

mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(data))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        STANDARD.decode(encoded).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn path_attachments_read_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invoice.pdf");
        std::fs::write(&path, b"%PDF").unwrap();

        let resolved = Attachment::from_path(&path).resolve().await.unwrap();
        assert_eq!(resolved.filename, "invoice.pdf");
        assert_eq!(resolved.content_type, "application/pdf");
        assert_eq!(resolved.data, b"%PDF");

        let renamed = Attachment::from_path(&path)
            .as_("Invoice.pdf")
            .with_mime("application/x-pdf")
            .resolve()
            .await
            .unwrap();
        assert_eq!(renamed.filename, "Invoice.pdf");
        assert_eq!(renamed.content_type, "application/x-pdf");

        let missing = Attachment::from_path(dir.path().join("missing.pdf"))
            .resolve()
            .await;
        assert!(
            missing
                .unwrap_err()
                .to_string()
                .contains("Unable to open path")
        );
    }

    #[tokio::test]
    async fn data_attachments_require_a_name() {
        let resolved = Attachment::from_data(|| "a,b", "data.csv")
            .resolve()
            .await
            .unwrap();
        assert_eq!(resolved.text(), "a,b");
        assert_eq!(resolved.content_type, "text/csv");

        let mut unnamed = Attachment::from_bytes(vec![1, 2], "x");
        unnamed.name = None;
        assert!(unnamed.resolve().await.is_err());
    }

    #[test]
    fn attachments_can_be_compared() {
        let a = Attachment::from_path("/a.pdf").as_("A.pdf");
        assert!(a.is_equivalent(&Attachment::from_path("/a.pdf").as_("A.pdf")));
        assert!(!a.is_equivalent(&Attachment::from_path("/a.pdf")));
        assert!(
            Attachment::from_bytes("x", "x.txt")
                .is_equivalent(&Attachment::from_data(|| "x", "x.txt"))
        );
        assert!(
            !Attachment::from_storage("a").is_equivalent(&Attachment::from_storage_disk("s3", "a"))
        );
    }

    #[test]
    fn message_attachments_serialize_as_base64() {
        let attachment = MessageAttachment {
            filename: "a.txt".into(),
            content_type: "text/plain".into(),
            data: b"hello".to_vec(),
            content_id: None,
        };
        let json = serde_json::to_value(&attachment).unwrap();
        assert_eq!(json["data"], "aGVsbG8=");
        let back: MessageAttachment = serde_json::from_value(json).unwrap();
        assert_eq!(back, attachment);
    }
}
