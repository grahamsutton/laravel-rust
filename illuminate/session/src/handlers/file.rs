use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use illuminate_http::async_trait;
use illuminate_support::Result;
use illuminate_support::error::InvalidArgumentException;

use super::SessionHandler;

/// Stores each session in its own file inside the `session.files` directory.
///
/// A session expires once its file hasn't been written for `session.lifetime`
/// minutes; expired files are swept by the garbage collection lottery.
///
/// ```
/// use illuminate_session::{FileSessionHandler, SessionHandler};
///
/// # let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
/// # runtime.block_on(async {
/// let directory = std::env::temp_dir().join(format!("sessions-doc-{}", std::process::id()));
/// let handler = FileSessionHandler::new(&directory, 120);
///
/// handler.write("abc", "payload").await.unwrap();
/// assert_eq!(handler.read("abc").await.unwrap(), "payload");
///
/// handler.destroy("abc").await.unwrap();
/// assert_eq!(handler.read("abc").await.unwrap(), "");
/// # std::fs::remove_dir_all(&directory).ok();
/// # });
/// ```
#[derive(Clone, Debug)]
pub struct FileSessionHandler {
    path: PathBuf,
    minutes: i64,
}

impl FileSessionHandler {
    /// Create a handler storing sessions in `path`, valid for `minutes`.
    pub fn new(path: impl AsRef<Path>, minutes: i64) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            minutes,
        }
    }

    /// The directory sessions are stored in.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file holding the given session (session IDs are alphanumeric, so
    /// they can never escape the directory).
    fn file(&self, session_id: &str) -> Option<PathBuf> {
        let valid = !session_id.is_empty() && session_id.chars().all(|c| c.is_ascii_alphanumeric());
        valid.then(|| self.path.join(session_id))
    }
}

#[async_trait]
impl SessionHandler for FileSessionHandler {
    async fn read(&self, session_id: &str) -> Result<String> {
        let Some(file) = self.file(session_id) else {
            return Ok(String::new());
        };
        let metadata = match tokio::fs::metadata(&file).await {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => return Ok(String::new()),
        };
        let lifetime = Duration::from_secs(self.minutes.max(0) as u64 * 60);
        let fresh = metadata
            .modified()
            .ok()
            .and_then(|modified| {
                SystemTime::now()
                    .checked_sub(lifetime)
                    .map(|cutoff| modified >= cutoff)
            })
            .unwrap_or(true);
        if !fresh {
            return Ok(String::new());
        }
        match tokio::fs::read_to_string(&file).await {
            Ok(contents) => Ok(contents),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(String::new()),
            Err(error) => Err(error.into()),
        }
    }

    async fn write(&self, session_id: &str, data: &str) -> Result<()> {
        let file = self.file(session_id).ok_or_else(|| {
            InvalidArgumentException::new(format!("Invalid session ID [{session_id}]."))
        })?;
        tokio::fs::create_dir_all(&self.path).await?;
        // Write to a hidden temporary file first so readers never see a partial session.
        let temporary = self
            .path
            .join(format!(".{session_id}.{}.tmp", rand::random::<u32>()));
        tokio::fs::write(&temporary, data).await?;
        if let Err(error) = tokio::fs::rename(&temporary, &file).await {
            tokio::fs::remove_file(&temporary).await.ok();
            return Err(error.into());
        }
        Ok(())
    }

    async fn destroy(&self, session_id: &str) -> Result<()> {
        let Some(file) = self.file(session_id) else {
            return Ok(());
        };
        match tokio::fs::remove_file(file).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    async fn gc(&self, lifetime: u64) -> Result<usize> {
        let mut entries = match tokio::fs::read_dir(&self.path).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.into()),
        };
        let cutoff = SystemTime::now()
            .checked_sub(Duration::from_secs(lifetime))
            .unwrap_or(SystemTime::UNIX_EPOCH);

        let mut deleted = 0;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(metadata) = entry.metadata().await else {
                continue;
            };
            let expired = metadata.modified().is_ok_and(|modified| modified <= cutoff);
            if metadata.is_file() && expired && tokio::fs::remove_file(entry.path()).await.is_ok() {
                deleted += 1;
            }
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age(path: &Path, seconds: u64) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(seconds))
            .unwrap();
    }

    #[tokio::test]
    async fn it_reads_writes_and_destroys_sessions() {
        let directory = tempfile::tempdir().unwrap();
        let handler = FileSessionHandler::new(directory.path().join("sessions"), 120);

        assert_eq!(handler.read("abc").await.unwrap(), "");
        handler.write("abc", "first").await.unwrap();
        handler.write("abc", "second").await.unwrap();
        assert_eq!(handler.read("abc").await.unwrap(), "second");
        assert_eq!(std::fs::read_dir(handler.path()).unwrap().count(), 1);

        handler.destroy("abc").await.unwrap();
        handler.destroy("abc").await.unwrap();
        assert_eq!(handler.read("abc").await.unwrap(), "");
    }

    #[tokio::test]
    async fn expired_sessions_are_not_read() {
        let directory = tempfile::tempdir().unwrap();
        let handler = FileSessionHandler::new(directory.path(), 1);
        handler.write("abc", "data").await.unwrap();
        age(&directory.path().join("abc"), 120);
        assert_eq!(handler.read("abc").await.unwrap(), "");
    }

    #[tokio::test]
    async fn garbage_collection_removes_old_files() {
        let directory = tempfile::tempdir().unwrap();
        let handler = FileSessionHandler::new(directory.path(), 120);
        handler.write("old", "a").await.unwrap();
        handler.write("new", "b").await.unwrap();
        std::fs::write(directory.path().join(".gitignore"), "*").unwrap();
        age(&directory.path().join("old"), 3600);
        age(&directory.path().join(".gitignore"), 3600);

        assert_eq!(handler.gc(1800).await.unwrap(), 1);
        assert!(!directory.path().join("old").exists());
        assert!(directory.path().join("new").exists());
        assert!(directory.path().join(".gitignore").exists());

        let missing = FileSessionHandler::new(directory.path().join("missing"), 120);
        assert_eq!(missing.gc(10).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn session_ids_cannot_escape_the_directory() {
        let directory = tempfile::tempdir().unwrap();
        let handler = FileSessionHandler::new(directory.path().join("sessions"), 120);
        std::fs::write(directory.path().join("secret"), "top secret").unwrap();

        assert_eq!(handler.read("../secret").await.unwrap(), "");
        assert!(handler.write("../secret", "x").await.is_err());
        handler.destroy("../secret").await.unwrap();
        assert!(directory.path().join("secret").exists());
    }
}
