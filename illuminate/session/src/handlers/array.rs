use std::collections::HashMap;
use std::sync::Mutex;

use illuminate_http::async_trait;
use illuminate_support::Result;

use super::{SessionHandler, current_time};

/// Keeps sessions in memory — perfect for tests. Data survives between
/// requests handled by the same application, but never touches the disk.
///
/// ```
/// use illuminate_session::{ArraySessionHandler, SessionHandler};
///
/// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// # runtime.block_on(async {
/// let handler = ArraySessionHandler::new(120);
/// handler.write("abc", "payload").await.unwrap();
///
/// assert_eq!(handler.read("abc").await.unwrap(), "payload");
/// assert_eq!(handler.read("missing").await.unwrap(), "");
/// # });
/// ```
#[derive(Debug)]
pub struct ArraySessionHandler {
    storage: Mutex<HashMap<String, (String, i64)>>,
    minutes: i64,
}

impl ArraySessionHandler {
    /// Create a handler whose sessions live for the given number of minutes.
    pub fn new(minutes: i64) -> Self {
        Self {
            storage: Mutex::new(HashMap::new()),
            minutes,
        }
    }

    /// Determine if the handler holds data for the given session.
    pub fn has(&self, session_id: &str) -> bool {
        self.storage.lock().unwrap().contains_key(session_id)
    }

    /// The raw data stored for a session, regardless of expiration.
    pub fn data(&self, session_id: &str) -> Option<String> {
        self.storage
            .lock()
            .unwrap()
            .get(session_id)
            .map(|(data, _)| data.clone())
    }

    /// The number of stored sessions.
    pub fn count(&self) -> usize {
        self.storage.lock().unwrap().len()
    }

    /// Move a session's last activity into the past (for testing expiry).
    pub fn travel(&self, session_id: &str, seconds: i64) {
        if let Some((_, time)) = self.storage.lock().unwrap().get_mut(session_id) {
            *time -= seconds;
        }
    }
}

#[async_trait]
impl SessionHandler for ArraySessionHandler {
    async fn read(&self, session_id: &str) -> Result<String> {
        let expiration = current_time() - self.minutes * 60;
        Ok(match self.storage.lock().unwrap().get(session_id) {
            Some((data, time)) if *time >= expiration => data.clone(),
            _ => String::new(),
        })
    }

    async fn write(&self, session_id: &str, data: &str) -> Result<()> {
        self.storage
            .lock()
            .unwrap()
            .insert(session_id.to_string(), (data.to_string(), current_time()));
        Ok(())
    }

    async fn destroy(&self, session_id: &str) -> Result<()> {
        self.storage.lock().unwrap().remove(session_id);
        Ok(())
    }

    async fn gc(&self, lifetime: u64) -> Result<usize> {
        let expiration = current_time() - lifetime as i64;
        let mut storage = self.storage.lock().unwrap();
        let before = storage.len();
        storage.retain(|_, (_, time)| *time >= expiration);
        Ok(before - storage.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sessions_expire_and_are_collected() {
        let handler = ArraySessionHandler::new(1);
        handler.write("old", "a").await.unwrap();
        handler.write("new", "b").await.unwrap();
        handler.travel("old", 120);

        assert_eq!(handler.read("old").await.unwrap(), "");
        assert_eq!(handler.read("new").await.unwrap(), "b");
        assert_eq!(handler.gc(60).await.unwrap(), 1);
        assert!(!handler.has("old"));
        assert_eq!(handler.count(), 1);

        handler.destroy("new").await.unwrap();
        assert_eq!(handler.data("new"), None);
    }
}
