use illuminate_http::async_trait;
use illuminate_support::Result;

use super::SessionHandler;

/// A handler that stores nothing at all.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullSessionHandler;

impl NullSessionHandler {
    /// Create the handler.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SessionHandler for NullSessionHandler {
    async fn read(&self, _session_id: &str) -> Result<String> {
        Ok(String::new())
    }

    async fn write(&self, _session_id: &str, _data: &str) -> Result<()> {
        Ok(())
    }

    async fn destroy(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }

    async fn gc(&self, _lifetime: u64) -> Result<usize> {
        Ok(0)
    }
}
