//! The transport: sends requests over the network with hyper, speaking
//! HTTP/1.1 over plain TCP or TLS (rustls), and keeps idle connections
//! around for reuse.

mod digest;
pub(crate) mod tls;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::header::{AUTHORIZATION, CONNECTION, HOST, WWW_AUTHENTICATE};
use http::{HeaderValue, Method};
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1::{self, SendRequest};
use hyper_util::rt::TokioIo;
use illuminate_support::Result;
use tokio::net::TcpStream;
use url::{Host, Url};

use crate::exceptions::ConnectionException;
use crate::request::Request;
use crate::response::Response;

pub(crate) use tls::Verify;

/// How long an idle connection may wait in the pool.
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// The most idle connections kept per host.
const MAX_IDLE_PER_HOST: usize = 8;

type Sender = SendRequest<Full<Bytes>>;

/// The per-request transport options.
#[derive(Clone, Debug, Default)]
pub(crate) struct TransportOptions {
    /// The total time allowed for the exchange.
    pub timeout: Option<Duration>,
    /// The time allowed to establish a connection.
    pub connect_timeout: Option<Duration>,
    /// How the server's certificate is verified.
    pub verify: Verify,
    /// Digest authentication credentials.
    pub digest: Option<(String, String)>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct PoolKey {
    https: bool,
    host: String,
    port: u16,
    verify: Verify,
}

struct Idle {
    sender: Sender,
    since: Instant,
}

type IdlePool = Arc<Mutex<HashMap<PoolKey, Vec<Idle>>>>;

/// Sends requests, pooling keep-alive connections.
#[derive(Default)]
pub(crate) struct Transport {
    idle: IdlePool,
}

impl Transport {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Send the request and buffer the response.
    pub(crate) async fn send(
        &self,
        request: &Request,
        options: &TransportOptions,
    ) -> Result<Response> {
        let exchange = self.send_with_auth(request, options);

        match options.timeout {
            Some(timeout) => tokio::time::timeout(timeout, exchange).await.map_err(|_| {
                ConnectionException::new(format!(
                    "Operation timed out after {} milliseconds with 0 bytes received",
                    timeout.as_millis()
                ))
            })?,
            None => exchange.await,
        }
    }

    async fn send_with_auth(
        &self,
        request: &Request,
        options: &TransportOptions,
    ) -> Result<Response> {
        let url = Url::parse(request.url()).map_err(|error| {
            ConnectionException::new(format!("Invalid URL [{}]: {error}.", request.url()))
        })?;

        if !matches!(url.scheme(), "http" | "https") {
            return Err(ConnectionException::new(format!(
                "Unsupported protocol [{}] for [{}].",
                url.scheme(),
                request.url()
            ))
            .into());
        }

        let response = self.exchange(&url, request, options, None).await?;

        let Some((username, password)) = &options.digest else {
            return Ok(response);
        };

        if response.status() != 401 {
            return Ok(response);
        }

        let challenge = response
            .headers()
            .get_all(WWW_AUTHENTICATE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(digest::Challenge::parse);

        match challenge {
            Some(challenge) => {
                let authorization =
                    challenge.authorize(username, password, request.method(), &origin_form(&url));
                self.exchange(&url, request, options, Some(authorization))
                    .await
            }
            None => Ok(response),
        }
    }

    /// Perform a single request / response exchange.
    async fn exchange(
        &self,
        url: &Url,
        request: &Request,
        options: &TransportOptions,
        authorization: Option<String>,
    ) -> Result<Response> {
        let key = PoolKey {
            https: url.scheme() == "https",
            host: url.host_str().unwrap_or_default().to_ascii_lowercase(),
            port: url.port_or_known_default().unwrap_or(80),
            verify: options.verify.clone(),
        };

        let build = || build_request(url, request, authorization.as_deref());

        let (mut sender, response) = match self.checkout(&key) {
            Some(mut sender) => match sender.try_send_request(build()?).await {
                Ok(response) => (sender, response),
                Err(error)
                    if error.message().is_some()
                        || retryable(request.http_method(), error.error()) =>
                {
                    // The pooled connection went stale; try once more on a fresh one.
                    let mut sender = connect(url, &key, options).await?;
                    let response = sender
                        .send_request(build()?)
                        .await
                        .map_err(transfer_error)?;
                    (sender, response)
                }
                Err(error) => return Err(transfer_error(error.into_error())),
            },
            None => {
                let mut sender = connect(url, &key, options).await?;
                let response = sender
                    .send_request(build()?)
                    .await
                    .map_err(transfer_error)?;
                (sender, response)
            }
        };

        let (parts, body) = response.into_parts();
        let body = body.collect().await.map_err(transfer_error)?.to_bytes();

        let closing = header_has_token(&parts.headers, "close")
            || header_has_token(request.headers(), "close");
        if !closing && sender.is_ready() {
            checkin(&self.idle, key, sender);
        } else if !closing {
            // The connection is still finishing up; pool it once it's ready,
            // without making the caller wait.
            let idle = self.idle.clone();
            tokio::spawn(async move {
                let ready = tokio::time::timeout(Duration::from_secs(1), sender.ready()).await;
                if matches!(ready, Ok(Ok(()))) {
                    checkin(&idle, key, sender);
                }
            });
        }

        Ok(Response::from_parts(
            parts.status,
            parts.version,
            parts.headers,
            body,
        ))
    }

    fn checkout(&self, key: &PoolKey) -> Option<Sender> {
        let mut idle = self.idle.lock().unwrap();
        let connections = idle.get_mut(key)?;

        while let Some(connection) = connections.pop() {
            if connection.since.elapsed() < IDLE_TIMEOUT
                && !connection.sender.is_closed()
                && connection.sender.is_ready()
            {
                return Some(connection.sender);
            }
        }

        None
    }

    /// The number of idle connections in the pool.
    #[cfg(test)]
    pub(crate) fn idle_connections(&self) -> usize {
        self.idle.lock().unwrap().values().map(Vec::len).sum()
    }
}

/// Return a connection to the pool.
fn checkin(idle: &IdlePool, key: PoolKey, sender: Sender) {
    let mut idle = idle.lock().unwrap();
    let connections = idle.entry(key).or_default();
    connections.retain(|connection| {
        connection.since.elapsed() < IDLE_TIMEOUT && !connection.sender.is_closed()
    });

    if connections.len() < MAX_IDLE_PER_HOST {
        connections.push(Idle {
            sender,
            since: Instant::now(),
        });
    }
}

/// Establish a new connection (and TLS session) to the URL's host.
async fn connect(url: &Url, key: &PoolKey, options: &TransportOptions) -> Result<Sender> {
    let host = url.host().ok_or_else(|| {
        ConnectionException::new(format!("URL rejected: No host part in the URL [{url}]."))
    })?;
    let port = key.port;
    let display = url.host_str().unwrap_or_default().to_string();

    let connecting = async {
        let stream = match &host {
            Host::Domain(domain) => TcpStream::connect((*domain, port)).await,
            Host::Ipv4(ip) => TcpStream::connect((*ip, port)).await,
            Host::Ipv6(ip) => TcpStream::connect((*ip, port)).await,
        }
        .map_err(|error| {
            ConnectionException::new(format!(
                "Failed to connect to {display} port {port}: {error}"
            ))
        })?;

        let _ = stream.set_nodelay(true);

        if key.https {
            let config = tls::client_config(&key.verify)?;
            let server_name = tls::server_name(&display)?;
            let stream = tokio_rustls::TlsConnector::from(config)
                .connect(server_name, stream)
                .await
                .map_err(|error| {
                    ConnectionException::new(format!("SSL connect error for {display}: {error}"))
                })?;
            handshake(stream).await
        } else {
            handshake(stream).await
        }
    };

    match options.connect_timeout {
        Some(timeout) => tokio::time::timeout(timeout, connecting).await.map_err(|_| {
            ConnectionException::new(format!(
                "Connection timed out after {} milliseconds connecting to {display} port {port}",
                timeout.as_millis()
            ))
        })?,
        None => connecting.await,
    }
}

async fn handshake<T>(io: T) -> Result<Sender>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let (sender, connection) = http1::Builder::new()
        .title_case_headers(true)
        .handshake(TokioIo::new(io))
        .await
        .map_err(transfer_error)?;

    tokio::spawn(async move {
        let _ = connection.await;
    });

    Ok(sender)
}

/// The path and query of the URL (the HTTP/1.1 request target).
fn origin_form(url: &Url) -> String {
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    }
}

fn build_request(
    url: &Url,
    request: &Request,
    authorization: Option<&str>,
) -> Result<http::Request<Full<Bytes>>> {
    let mut builder = http::Request::builder()
        .method(request.http_method().clone())
        .uri(origin_form(url));

    let headers = builder.headers_mut().expect("the request builder is valid");
    headers.extend(request.headers().clone());

    if !headers.contains_key(HOST) {
        let host = match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
            None => url.host_str().unwrap_or_default().to_string(),
        };
        headers.insert(HOST, HeaderValue::from_str(&host)?);
    }

    if let Some(authorization) = authorization {
        headers.insert(AUTHORIZATION, HeaderValue::from_str(authorization)?);
    }

    Ok(builder.body(Full::new(request.bytes().clone()))?)
}

/// Whether a failed exchange on a reused connection may safely be retried.
fn retryable(method: &Method, error: &hyper::Error) -> bool {
    method.is_idempotent()
        && (error.is_canceled() || error.is_incomplete_message() || error.is_closed())
}

fn header_has_token(headers: &http::HeaderMap, token: &str) -> bool {
    headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|value| value.trim().eq_ignore_ascii_case(token))
}

/// Describe a transfer failure, including its causes.
fn transfer_error(error: impl std::error::Error) -> illuminate_support::Error {
    let mut message = error.to_string();
    let mut source = error.source();

    while let Some(cause) = source {
        let cause_message = cause.to_string();
        if !message.contains(&cause_message) {
            message.push_str(": ");
            message.push_str(&cause_message);
        }
        source = cause.source();
    }

    ConnectionException::new(message).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_builds_origin_form_requests() {
        let url = Url::parse("http://example.com:8080/users?page=2").unwrap();
        let request = Request::new("GET", url.as_str()).with_header("X-Test", "1");
        let built = build_request(&url, &request, Some("Digest x")).unwrap();

        assert_eq!(built.uri(), "/users?page=2");
        assert_eq!(built.headers()[HOST], "example.com:8080");
        assert_eq!(built.headers()[AUTHORIZATION], "Digest x");
        assert_eq!(built.headers()["x-test"], "1");

        let url = Url::parse("https://laravel.com").unwrap();
        let built = build_request(&url, &Request::new("GET", url.as_str()), None).unwrap();
        assert_eq!(built.uri(), "/");
        assert_eq!(built.headers()[HOST], "laravel.com");
    }

    #[test]
    fn it_reads_connection_tokens() {
        let mut headers = http::HeaderMap::new();
        headers.insert(CONNECTION, HeaderValue::from_static("keep-alive, Close"));
        assert!(header_has_token(&headers, "close"));
        assert!(!header_has_token(&http::HeaderMap::new(), "close"));
    }

    #[tokio::test]
    async fn invalid_urls_are_connection_errors() {
        let transport = Transport::new();
        let options = TransportOptions::default();

        let error = transport
            .send(&Request::new("GET", "users"), &options)
            .await
            .unwrap_err();
        assert!(error.to_string().starts_with("Invalid URL [users]"));

        let error = transport
            .send(&Request::new("GET", "ftp://example.com"), &options)
            .await
            .unwrap_err();
        assert!(error.is::<ConnectionException>());
        assert!(error.to_string().contains("Unsupported protocol [ftp]"));
        assert_eq!(transport.idle_connections(), 0);
    }
}
