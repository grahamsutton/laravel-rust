//! Middleware that normalize request input.

use illuminate_http::{Middleware, Next, Request, Response, async_trait};
use illuminate_support::{Result, Str, Value};

/// Trim whitespace from every string in the request input.
#[derive(Clone, Debug)]
pub struct TrimStrings {
    except: Vec<String>,
}

impl Default for TrimStrings {
    fn default() -> Self {
        Self {
            except: vec![
                "current_password".into(),
                "password".into(),
                "password_confirmation".into(),
            ],
        }
    }
}

impl TrimStrings {
    /// Also skip trimming for the given keys (wildcards allowed).
    pub fn except(mut self, keys: &[&str]) -> Self {
        self.except.extend(keys.iter().map(|k| k.to_string()));
        self
    }
}

#[async_trait]
impl Middleware for TrimStrings {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let except = self.except.clone();
        request.transform_input(&move |key, value| match value {
            Value::String(s) if !except.iter().any(|pattern| Str::is(pattern, key)) => {
                Value::String(s.trim().to_string())
            }
            other => other,
        });
        Ok(next.run(request).await)
    }
}

/// Convert empty strings in the request input to `null`.
#[derive(Clone, Debug, Default)]
pub struct ConvertEmptyStringsToNull {
    except: Vec<String>,
}

impl ConvertEmptyStringsToNull {
    /// Skip the given keys (wildcards allowed).
    pub fn except(mut self, keys: &[&str]) -> Self {
        self.except.extend(keys.iter().map(|k| k.to_string()));
        self
    }
}

#[async_trait]
impl Middleware for ConvertEmptyStringsToNull {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let except = self.except.clone();
        request.transform_input(&move |key, value| match value {
            Value::String(s) if s.is_empty() && !except.iter().any(|pattern| Str::is(pattern, key)) => {
                Value::Null
            }
            other => other,
        });
        Ok(next.run(request).await)
    }
}

/// Trust the forwarding headers sent by your load balancers or proxies.
#[derive(Clone, Debug, Default)]
pub struct TrustProxies {
    proxies: Vec<String>,
}

impl TrustProxies {
    /// Trust the given proxy addresses or CIDR ranges (`"*"` trusts every
    /// proxy).
    pub fn at(proxies: &[&str]) -> Self {
        Self {
            proxies: proxies.iter().map(|p| p.to_string()).collect(),
        }
    }
}

#[async_trait]
impl Middleware for TrustProxies {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let remote = request.remote_addr().map(|addr| addr.ip().to_string());
        let trusted = self.proxies.iter().any(|proxy| {
            proxy == "*" || proxy == "**" || remote.as_deref().is_some_and(|ip| super::ip_matches(ip, proxy))
        });
        if trusted {
            request.set_trust_proxies(true);
        }
        Ok(next.run(request).await)
    }
}

/// Add `X-Frame-Options: SAMEORIGIN` to every response.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameGuard;

#[async_trait]
impl Middleware for FrameGuard {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let mut response = next.run(request).await;
        if response.header("x-frame-options").is_none() {
            response.set_header("X-Frame-Options", "SAMEORIGIN");
        }
        Ok(response)
    }
}

/// Set HTTP cache headers: `cache.headers:public;max_age=2628000;etag`.
#[derive(Clone, Debug, Default)]
pub struct SetCacheHeaders {
    options: Vec<(String, Option<String>)>,
}

impl SetCacheHeaders {
    /// Build from the middleware parameter string(s).
    pub fn from_parameters(parameters: &[String]) -> Self {
        let options = parameters
            .join(";")
            .split(';')
            .filter(|o| !o.trim().is_empty())
            .map(|option| match option.split_once('=') {
                Some((key, value)) => (key.trim().to_string(), Some(value.trim().to_string())),
                None => (option.trim().to_string(), None),
            })
            .collect();
        Self { options }
    }
}

#[async_trait]
impl Middleware for SetCacheHeaders {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let is_read = request.is_method("GET") || request.is_method("HEAD");
        let mut response = next.run(request.clone()).await;
        if !is_read || !response.is_successful() {
            return Ok(response);
        }

        let mut directives = Vec::new();
        for (key, value) in &self.options {
            match key.as_str() {
                "etag" => {
                    use sha2::{Digest, Sha256};
                    let hash = hex::encode(Sha256::digest(response.content()));
                    let etag = format!("\"{}\"", &hash[..32]);
                    if request.header("if-none-match").as_deref() == Some(etag.as_str()) {
                        response = Response::no_content().with_status(304);
                    }
                    response.set_header("ETag", &etag);
                }
                "last_modified" => {
                    if let Some(value) = value {
                        response.set_header("Last-Modified", value);
                    }
                }
                other => {
                    let name = other.replace('_', "-");
                    directives.push(match value {
                        Some(value) => format!("{name}={value}"),
                        None => name,
                    });
                }
            }
        }
        if !directives.is_empty() {
            response.set_header("Cache-Control", directives.join(", "));
        }
        Ok(response)
    }
}
