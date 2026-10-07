//! A tiny local HTTP(S) server for the transport tests.
//!
//! The TLS fixtures in `tests/fixtures` are test-only: a throwaway CA
//! (`ca.pem`) and a `localhost` / `127.0.0.1` certificate signed by it
//! (`localhost.pem`, `localhost.key`), valid from 2000 until 2125.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use illuminate_container::{Container, LocalInstanceGuard};
use illuminate_support::{Value, json};
use md5::{Digest, Md5};
use tokio::net::TcpListener;

/// Install a fresh container for the current test.
pub fn fresh_app() -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    (container, guard)
}

/// The path of a TLS fixture.
pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[derive(Default)]
struct State {
    connections: AtomicUsize,
    flaky: AtomicUsize,
}

/// A running test server.
pub struct TestServer {
    pub addr: SocketAddr,
    scheme: &'static str,
    state: Arc<State>,
}

impl TestServer {
    /// Start a plain HTTP server on a random port.
    pub async fn start() -> Self {
        Self::spawn(None).await
    }

    /// Start an HTTPS server on a random port, using the test certificate.
    pub async fn start_tls() -> Self {
        use rustls::pki_types::pem::PemObject;
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};

        let certificate = CertificateDer::from_pem_file(fixture("localhost.pem")).unwrap();
        let key = PrivateKeyDer::from_pem_file(fixture("localhost.key")).unwrap();

        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate], key)
        .unwrap();

        Self::spawn(Some(tokio_rustls::TlsAcceptor::from(Arc::new(config)))).await
    }

    async fn spawn(tls: Option<tokio_rustls::TlsAcceptor>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(State::default());
        let scheme = if tls.is_some() { "https" } else { "http" };

        let shared = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                shared.connections.fetch_add(1, Ordering::SeqCst);

                let state = shared.clone();
                let tls = tls.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request| {
                        let state = state.clone();
                        async move { Ok::<_, Infallible>(route(request, addr, &state).await) }
                    });

                    let builder = hyper::server::conn::http1::Builder::new();
                    match tls {
                        Some(acceptor) => {
                            if let Ok(stream) = acceptor.accept(stream).await {
                                let _ = builder
                                    .serve_connection(TokioIo::new(stream), service)
                                    .await;
                            }
                        }
                        None => {
                            let _ = builder
                                .serve_connection(TokioIo::new(stream), service)
                                .await;
                        }
                    }
                });
            }
        });

        Self {
            addr,
            scheme,
            state,
        }
    }

    /// The URL of the given path on this server.
    pub fn url(&self, path: &str) -> String {
        format!("{}://{}{}", self.scheme, self.addr, path)
    }

    /// The URL of the given path, using the `localhost` host name.
    pub fn localhost_url(&self, path: &str) -> String {
        format!("{}://localhost:{}{}", self.scheme, self.addr.port(), path)
    }

    /// The number of TCP connections the server has accepted.
    pub fn connections(&self) -> usize {
        self.state.connections.load(Ordering::SeqCst)
    }
}

fn respond(status: u16, headers: &[(&str, &str)], body: impl Into<Bytes>) -> Response<Full<Bytes>> {
    let mut builder = Response::builder().status(status);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Full::new(body.into())).unwrap()
}

fn json_response(status: u16, value: Value) -> Response<Full<Bytes>> {
    respond(
        status,
        &[("Content-Type", "application/json")],
        value.to_string(),
    )
}

async fn route(
    request: Request<Incoming>,
    addr: SocketAddr,
    state: &State,
) -> Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    let body = body
        .collect()
        .await
        .map(|body| body.to_bytes())
        .unwrap_or_default();
    let path = parts.uri.path().to_string();
    let header = |name: &str| {
        parts
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };

    match path.as_str() {
        "/json" => json_response(
            200,
            json!({"name": "Taylor", "framework": "Laravel", "versions": [11, 12, 13]}),
        ),
        "/text" => respond(200, &[("X-Custom", "yes")], "Hello World"),
        "/headers" => respond(
            200,
            &[
                ("X-Many", "a"),
                ("X-Many", "b"),
                ("Set-Cookie", "flavor=chocolate; Path=/"),
            ],
            "",
        ),
        "/slow" => {
            tokio::time::sleep(Duration::from_secs(2)).await;
            respond(200, &[], "finally")
        }
        "/flaky" => {
            let attempt = state.flaky.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt < 3 {
                respond(500, &[], format!("attempt {attempt} failed"))
            } else {
                respond(200, &[], format!("recovered on attempt {attempt}"))
            }
        }
        "/basic" => {
            if header("authorization") == "Basic dGF5bG9yOnNlY3JldA==" {
                respond(200, &[], "basic ok")
            } else {
                respond(
                    401,
                    &[("WWW-Authenticate", "Basic realm=\"test\"")],
                    "Unauthorized",
                )
            }
        }
        "/digest" => digest(parts.method.as_str(), &header("authorization")),
        "/redirect-loop" => respond(302, &[("Location", "/redirect-loop")], ""),
        "/see-other" => respond(303, &[("Location", "/echo?from=see-other")], ""),
        "/temporary" => respond(307, &[("Location", "/echo")], ""),
        "/login" => respond(
            302,
            &[
                ("Location", "/echo"),
                ("Set-Cookie", "session=abc123; Path=/; HttpOnly"),
            ],
            "",
        ),
        "/elsewhere" => {
            let location = format!("http://localhost:{}/echo", addr.port());
            respond(302, &[("Location", location.as_str())], "")
        }
        "/close" => respond(200, &[("Connection", "close")], "bye"),
        "/multipart" => multipart(&header("content-type"), body).await,
        "/download" => respond(200, &[], "downloaded contents"),
        _ if path.starts_with("/status/") => {
            let status: u16 = path["/status/".len()..].parse().unwrap_or(500);
            respond(status, &[], format!("status {status}"))
        }
        _ if path.starts_with("/redirect/") => {
            let remaining: u32 = path["/redirect/".len()..].parse().unwrap_or(0);
            let location = if remaining <= 1 {
                "/echo".to_string()
            } else {
                format!("/redirect/{}", remaining - 1)
            };
            respond(302, &[("Location", location.as_str())], "")
        }
        _ => {
            let headers: BTreeMap<String, String> = parts
                .headers
                .iter()
                .map(|(name, value)| {
                    (
                        name.to_string(),
                        value.to_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();

            json_response(
                200,
                json!({
                    "method": parts.method.as_str(),
                    "path": path,
                    "query": parts.uri.query().unwrap_or_default(),
                    "headers": headers,
                    "body": String::from_utf8_lossy(&body),
                }),
            )
        }
    }
}

fn digest(method: &str, authorization: &str) -> Response<Full<Bytes>> {
    let challenge = respond(
        401,
        &[(
            "WWW-Authenticate",
            r#"Digest realm="testrealm", qop="auth", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        )],
        "Unauthorized",
    );

    let Some(params) = authorization.strip_prefix("Digest ") else {
        return challenge;
    };

    let field = |name: &str| -> String {
        params
            .split(", ")
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
            .unwrap_or_default()
            .trim_matches('"')
            .to_string()
    };

    let md5 = |value: String| hex::encode(Md5::digest(value.as_bytes()));
    let ha1 = md5(format!("{}:testrealm:secret", field("username")));
    let ha2 = md5(format!("{method}:{}", field("uri")));
    let expected = md5(format!(
        "{ha1}:{}:{}:{}:{}:{ha2}",
        field("nonce"),
        field("nc"),
        field("cnonce"),
        field("qop")
    ));

    if field("username") == "taylor"
        && field("response") == expected
        && field("opaque") == "5ccc069c403ebaf9f0171e9517f40e41"
    {
        respond(200, &[], "digest ok")
    } else {
        challenge
    }
}

async fn multipart(content_type: &str, body: Bytes) -> Response<Full<Bytes>> {
    let Ok(boundary) = multer::parse_boundary(content_type) else {
        return respond(400, &[], "missing boundary");
    };

    let stream = futures::stream::once(async move { Ok::<_, Infallible>(body) });
    let mut multipart = multer::Multipart::new(stream, boundary);
    let mut fields = Vec::new();

    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().map(str::to_string);
                let filename = field.file_name().map(str::to_string);
                let content_type = field.content_type().map(|mime| mime.to_string());
                let text = field.text().await.unwrap_or_default();
                fields.push(json!({"name": name, "filename": filename, "content_type": content_type, "text": text}));
            }
            Ok(None) => break,
            Err(error) => return respond(400, &[], error.to_string()),
        }
    }

    json_response(200, Value::Array(fields))
}
