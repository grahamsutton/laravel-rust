//! A fast, production-ready HTTP server built on `hyper`.
//!
//! This is what `cargo artisan serve` boots. The server converts each
//! incoming `hyper` request into a [`Request`], hands it to your kernel, and
//! writes the resulting [`Response`] back to the client.

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use indexmap::IndexMap;
use tokio::net::TcpListener;

use illuminate_support::{Map, Value};

use crate::BoxFuture;
use crate::input::{insert_bracketed, normalize_lists, parse_query};
use crate::request::Request;
use crate::response::{HyperBody, Response};
use crate::uploaded_file::UploadedFile;

/// A request handler: the HTTP kernel, from the server's point of view.
pub type Handler = Arc<dyn Fn(Request) -> BoxFuture<'static, Response> + Send + Sync>;

/// The largest request body the server will buffer (64 MB).
pub const MAX_BODY_SIZE: usize = 64 * 1024 * 1024;

/// Serve requests on the given address until a shutdown signal (Ctrl+C).
pub async fn serve(addr: SocketAddr, handler: Handler) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    serve_listener(listener, handler, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}

/// Serve requests from an existing listener until `shutdown` resolves.
pub async fn serve_listener(
    listener: TcpListener,
    handler: Handler,
    shutdown: impl Future<Output = ()> + Send,
) -> std::io::Result<()> {
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, remote) = match accepted {
                    Ok(pair) => pair,
                    Err(_) => continue,
                };
                let handler = handler.clone();
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |req: hyper::Request<Incoming>| {
                        let handler = handler.clone();
                        async move {
                            let request = into_request(req, Some(remote)).await;
                            let response = match request {
                                Ok(request) => handler(request).await,
                                Err(response) => response,
                            };
                            Ok::<hyper::Response<HyperBody>, Infallible>(response.into_hyper())
                        }
                    });
                    let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                        .serve_connection_with_upgrades(TokioIo::new(stream), service)
                        .await;
                });
            }
            _ = &mut shutdown => break,
        }
    }
    Ok(())
}

/// Convert a `hyper` request into a framework [`Request`], buffering and
/// parsing the body (JSON, URL-encoded forms, and multipart uploads).
pub async fn into_request(
    req: hyper::Request<Incoming>,
    remote: Option<SocketAddr>,
) -> Result<Request, Response> {
    let (parts, body) = req.into_parts();

    let limited = http_body_util::Limited::new(body, MAX_BODY_SIZE);
    let bytes = match limited.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return Err(Response::make("413 | Content Too Large", 413)),
    };

    let content_type = parts
        .headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();

    if content_type.to_ascii_lowercase().starts_with("multipart/form-data") {
        let (input, files) = parse_multipart(&content_type, bytes.clone()).await;
        return Ok(Request::from_parts_with_files(
            parts.method,
            parts.uri,
            parts.version,
            parts.headers,
            bytes,
            input,
            files,
            remote,
        ));
    }

    Ok(Request::from_parts(
        parts.method,
        parts.uri,
        parts.version,
        parts.headers,
        bytes,
        remote,
    ))
}

/// Parse a multipart body into input fields and uploaded files.
pub async fn parse_multipart(
    content_type: &str,
    body: Bytes,
) -> (Value, IndexMap<String, Vec<UploadedFile>>) {
    let mut input = Value::Object(Map::new());
    let mut files: IndexMap<String, Vec<UploadedFile>> = IndexMap::new();

    let Ok(boundary) = multer::parse_boundary(content_type) else {
        return (input, files);
    };
    let stream = futures::stream::once(async move { Ok::<Bytes, Infallible>(body) });
    let mut multipart = multer::Multipart::new(stream, boundary);

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        let file_name = field.file_name().map(str::to_string);
        let mime = field
            .content_type()
            .map(|m| m.essence_str().to_string())
            .unwrap_or_else(|| "application/octet-stream".into());
        let Ok(data) = field.bytes().await else { continue };
        match file_name {
            Some(file_name) if !file_name.is_empty() => {
                let key = name.trim_end_matches("[]").to_string();
                files
                    .entry(key)
                    .or_default()
                    .push(UploadedFile::new(file_name, mime, data));
            }
            Some(_) => {}
            None => insert_bracketed(
                &mut input,
                &name,
                Value::String(String::from_utf8_lossy(&data).into_owned()),
            ),
        }
    }

    (normalize_lists(input), files)
}

/// Parse a raw query string (exposed for convenience).
pub fn query(query: &str) -> Value {
    parse_query(query)
}
