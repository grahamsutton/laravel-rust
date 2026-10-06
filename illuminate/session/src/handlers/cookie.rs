use std::sync::{Arc, RwLock};

use illuminate_cookie::CookieJar;
use illuminate_http::{Request, async_trait};
use illuminate_support::{Result, Value, json};

use super::{SessionHandler, current_time};

/// Stores the whole session in a cookie on the client.
///
/// The payload (`{"data": ..., "expires": ...}`) is queued on the request's
/// cookie queue under the session ID; the `EncryptCookies` middleware then
/// encrypts it like every other cookie. Browsers cap cookies at roughly 4KB,
/// so keep cookie-backed sessions small.
#[derive(Debug)]
pub struct CookieSessionHandler {
    jar: Arc<CookieJar>,
    minutes: i64,
    expire_on_close: bool,
    request: RwLock<Option<Request>>,
}

impl CookieSessionHandler {
    /// Create a handler that queues cookies with the given jar.
    pub fn new(jar: Arc<CookieJar>, minutes: i64, expire_on_close: bool) -> Self {
        Self {
            jar,
            minutes,
            expire_on_close,
            request: RwLock::new(None),
        }
    }

    fn request(&self) -> Option<Request> {
        self.request.read().unwrap().clone()
    }

    fn queue(&self, cookie: illuminate_http::Cookie) {
        match self.request() {
            Some(request) => self.jar.queue_on(&request, cookie),
            None => self.jar.queue(cookie),
        }
    }
}

#[async_trait]
impl SessionHandler for CookieSessionHandler {
    async fn read(&self, session_id: &str) -> Result<String> {
        let Some(value) = self
            .request()
            .and_then(|request| request.cookie(session_id))
        else {
            return Ok(String::new());
        };
        let Ok(decoded) = serde_json::from_str::<Value>(&value) else {
            return Ok(String::new());
        };
        let expires = decoded.get("expires").and_then(Value::as_i64);
        match (expires, decoded.get("data").and_then(Value::as_str)) {
            (Some(expires), Some(data)) if current_time() <= expires => Ok(data.to_string()),
            _ => Ok(String::new()),
        }
    }

    async fn write(&self, session_id: &str, data: &str) -> Result<()> {
        let payload = json!({
            "data": data,
            "expires": current_time() + self.minutes * 60,
        });
        let minutes = if self.expire_on_close {
            0
        } else {
            self.minutes
        };
        self.queue(self.jar.make(session_id, payload.to_string(), minutes));
        Ok(())
    }

    async fn destroy(&self, session_id: &str) -> Result<()> {
        self.queue(self.jar.forget(session_id));
        Ok(())
    }

    async fn gc(&self, _lifetime: u64) -> Result<usize> {
        Ok(0)
    }

    fn needs_request(&self) -> bool {
        true
    }

    fn set_request(&self, request: &Request) {
        *self.request.write().unwrap() = Some(request.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_cookie::CookieQueue;
    use illuminate_http::{HeaderMap, HeaderValue};

    #[tokio::test]
    async fn it_round_trips_sessions_through_cookies() {
        let jar = Arc::new(CookieJar::new());
        let handler = CookieSessionHandler::new(jar.clone(), 120, false);
        let request = Request::create("/", "GET");
        handler.set_request(&request);
        assert!(handler.needs_request());

        handler.write("abc", r#"{"name":"Taylor"}"#).await.unwrap();
        let cookie = CookieQueue::for_request(&request)
            .queued("abc", None)
            .unwrap();
        assert_eq!(cookie.minutes, Some(120));

        let mut headers = HeaderMap::new();
        let header = format!(
            "abc={}",
            cookie
                .to_header_value()
                .split(';')
                .next()
                .unwrap()
                .trim_start_matches("abc=")
        );
        headers.insert("cookie", HeaderValue::from_str(&header).unwrap());
        let next_request = Request::create_with("/", "GET", json!({}), headers);
        handler.set_request(&next_request);
        assert_eq!(handler.read("abc").await.unwrap(), r#"{"name":"Taylor"}"#);
        assert_eq!(handler.read("other").await.unwrap(), "");

        handler.destroy("abc").await.unwrap();
        assert!(
            CookieQueue::for_request(&next_request)
                .queued("abc", None)
                .unwrap()
                .is_cleared()
        );
        assert_eq!(handler.gc(10).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn expired_cookie_sessions_are_ignored() {
        let handler = CookieSessionHandler::new(Arc::new(CookieJar::new()), 120, true);
        let expired = json!({"data": "x", "expires": current_time() - 10}).to_string();
        let request = Request::create("/", "GET");
        request.set_cookie("abc", expired);
        handler.set_request(&request);
        assert_eq!(handler.read("abc").await.unwrap(), "");

        handler.write("abc", "y").await.unwrap();
        let cookie = CookieQueue::for_request(&request)
            .queued("abc", None)
            .unwrap();
        assert_eq!(cookie.minutes, None);
    }
}
