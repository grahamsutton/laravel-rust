//! Where an image's contents come from, loaded lazily and at most once.

use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use bytes::Bytes;
use tokio::sync::OnceCell;

use illuminate_filesystem::Filesystem;
use illuminate_support::Result;

use crate::exception::ImageException;

/// A boxed, sendable future resolving to an image's contents.
pub(crate) type ContentsFuture = Pin<Box<dyn Future<Output = Result<Bytes>> + Send + 'static>>;

/// A lazy loader for an image's contents.
pub(crate) type Loader = Arc<dyn Fn() -> ContentsFuture + Send + Sync>;

enum Origin {
    Bytes(Bytes),
    Base64(String),
    Path(PathBuf),
    Lazy { description: String, loader: Loader },
}

/// An image's contents: given up front, or loaded on first use and then
/// shared by every image derived from it.
pub(crate) struct Source {
    origin: Origin,
    contents: OnceCell<Bytes>,
}

impl Source {
    pub(crate) fn bytes(contents: Bytes) -> Self {
        Self {
            contents: OnceCell::new_with(Some(contents.clone())),
            origin: Origin::Bytes(contents),
        }
    }

    pub(crate) fn base64(encoded: String) -> Self {
        Self::from_origin(Origin::Base64(encoded))
    }

    pub(crate) fn path(path: PathBuf) -> Self {
        Self::from_origin(Origin::Path(path))
    }

    pub(crate) fn lazy(description: impl Into<String>, loader: Loader) -> Self {
        Self::from_origin(Origin::Lazy {
            description: description.into(),
            loader,
        })
    }

    fn from_origin(origin: Origin) -> Self {
        Self {
            origin,
            contents: OnceCell::new(),
        }
    }

    /// Whether the contents have been loaded.
    pub(crate) fn is_loaded(&self) -> bool {
        self.contents.initialized()
    }

    /// Load the contents (once), returning them.
    pub(crate) async fn contents(&self) -> Result<Bytes> {
        self.contents.get_or_try_init(|| self.load()).await.cloned()
    }

    /// The contents, if they can be had without waiting on I/O: they were
    /// given up front, were already loaded, or come from base64 or a local
    /// path (which is read synchronously).
    pub(crate) fn contents_now(&self) -> Option<Result<Bytes>> {
        if let Some(contents) = self.contents.get() {
            return Some(Ok(contents.clone()));
        }

        let loaded = match &self.origin {
            Origin::Bytes(contents) => Ok(contents.clone()),
            Origin::Base64(encoded) => decode_base64(encoded),
            Origin::Path(path) => Filesystem::new().get_bytes_sync(path).map(Bytes::from),
            Origin::Lazy { .. } => return None,
        };

        Some(loaded.inspect(|contents| {
            let _ = self.contents.set(contents.clone());
        }))
    }

    async fn load(&self) -> Result<Bytes> {
        match &self.origin {
            Origin::Bytes(contents) => Ok(contents.clone()),
            Origin::Base64(encoded) => decode_base64(encoded),
            Origin::Path(path) => Ok(Filesystem::new().get_bytes(path).await?.into()),
            Origin::Lazy { loader, .. } => loader().await,
        }
    }
}

impl fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let origin = match &self.origin {
            Origin::Bytes(contents) => format!("bytes ({} bytes)", contents.len()),
            Origin::Base64(_) => "base64".to_string(),
            Origin::Path(path) => format!("path ({})", path.display()),
            Origin::Lazy { description, .. } => description.clone(),
        };
        f.debug_struct("Source")
            .field("origin", &origin)
            .field("loaded", &self.is_loaded())
            .finish()
    }
}

/// Decode base64 image data strictly, failing with Laravel's message.
fn decode_base64(encoded: &str) -> Result<Bytes> {
    let cleaned: String = encoded
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();

    STANDARD
        .decode(&cleaned)
        .or_else(|_| STANDARD_NO_PAD.decode(&cleaned))
        .ok()
        .filter(|decoded| !decoded.is_empty())
        .map(Bytes::from)
        .ok_or_else(|| ImageException::new("Invalid base64 image data.").into())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn lazy_sources_are_loaded_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let source = Source::lazy(
            "test",
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(Bytes::from_static(b"contents")) })
            }),
        );

        assert!(!source.is_loaded());
        assert!(source.contents_now().is_none());
        assert_eq!(source.contents().await.unwrap(), "contents");
        assert_eq!(source.contents().await.unwrap(), "contents");
        assert!(source.is_loaded());
        assert_eq!(source.contents_now().unwrap().unwrap(), "contents");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_loads_are_retried() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let source = Source::lazy(
            "flaky",
            Arc::new(move || {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if attempt == 0 {
                        Err(ImageException::new("Not yet.").into())
                    } else {
                        Ok(Bytes::from_static(b"ok"))
                    }
                })
            }),
        );

        assert!(source.contents().await.is_err());
        assert_eq!(source.contents().await.unwrap(), "ok");
    }

    #[test]
    fn base64_is_decoded_strictly() {
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), "hello");
        assert_eq!(decode_base64("aGVsbG8").unwrap(), "hello");
        assert_eq!(decode_base64("aGVs\nbG8=").unwrap(), "hello");
        assert_eq!(
            decode_base64("not base64!").unwrap_err().to_string(),
            "Invalid base64 image data."
        );
        assert_eq!(
            decode_base64("").unwrap_err().to_string(),
            "Invalid base64 image data."
        );
    }

    #[test]
    fn sources_describe_themselves() {
        let source = Source::bytes(Bytes::from_static(b"abc"));
        assert!(source.is_loaded());
        assert!(format!("{source:?}").contains("bytes (3 bytes)"));
        assert!(format!("{:?}", Source::path("/tmp/a.png".into())).contains("path (/tmp/a.png)"));
        assert!(format!("{:?}", Source::base64("abc".into())).contains("base64"));
    }
}
