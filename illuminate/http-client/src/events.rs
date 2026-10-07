//! The events fired while sending requests.
//!
//! `RequestSending` fires before each request is sent, `ResponseReceived`
//! after a response is received, and `ConnectionFailed` when no response
//! could be received at all. Listen for them like any other event:
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_events::Event;
//! use illuminate_http_client::Http;
//! use illuminate_http_client::events::{RequestSending, ResponseReceived};
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! Event::fake();
//! Http::fake();
//!
//! Http::get("https://laravel.com").await?;
//!
//! Event::assert_dispatched_with(|event: &RequestSending| event.request.url() == "https://laravel.com");
//! Event::assert_dispatched::<ResponseReceived>();
//! # Ok::<(), illuminate_support::Error>(())
//! # }).unwrap();
//! ```

use crate::exceptions::ConnectionException;
use crate::request::Request;
use crate::response::Response;

/// Fired before a request is sent.
#[derive(Clone, Debug)]
pub struct RequestSending {
    /// The request about to be sent.
    pub request: Request,
}

/// Fired after a response has been received.
#[derive(Clone, Debug)]
pub struct ResponseReceived {
    /// The request that was sent.
    pub request: Request,
    /// The response that was received.
    pub response: Response,
}

/// Fired when no response could be received for a request.
#[derive(Clone, Debug)]
pub struct ConnectionFailed {
    /// The request that failed.
    pub request: Request,
    /// The connection exception.
    pub exception: ConnectionException,
}
