//! Maintenance mode and static files.

use std::path::{Component, Path, PathBuf};

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use illuminate_http::{Cookie, HttpException, Middleware, Next, Request, Response, async_trait};
use illuminate_support::{Carbon, Result, Str, Value, ValueExt, json};

use crate::application::Application;

/// The cookie that lets you browse the site during maintenance.
pub struct MaintenanceModeBypassCookie;

impl MaintenanceModeBypassCookie {
    /// The cookie name.
    pub const NAME: &'static str = "laravel_maintenance";

    /// Create a bypass cookie for the given secret, valid for 12 hours.
    pub fn create(secret: &str) -> Cookie {
        let expires_at = Carbon::now().add_hours(12).timestamp();
        let payload = json!({
            "expires_at": expires_at,
            "mac": Self::mac(&expires_at.to_string(), secret),
        });
        let encoded = base64::engine::general_purpose::STANDARD.encode(payload.to_string());
        Cookie::new(Self::NAME, encoded).minutes(720)
    }

    /// Determine if the cookie value is valid for the given secret.
    pub fn is_valid(cookie: &str, secret: &str) -> bool {
        let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(cookie) else {
            return false;
        };
        let Ok(payload) = serde_json::from_slice::<Value>(&decoded) else {
            return false;
        };
        let Some(expires_at) = payload.get("expires_at").and_then(|v| v.to_i64_lossy()) else {
            return false;
        };
        let Some(mac) = payload.get("mac").and_then(Value::as_str) else {
            return false;
        };
        let expected = Self::mac(&expires_at.to_string(), secret);
        use subtle::ConstantTimeEq;
        bool::from(expected.as_bytes().ct_eq(mac.as_bytes()))
            && Carbon::from_timestamp(expires_at).is_future()
    }

    fn mac(value: &str, secret: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
        mac.update(value.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }
}

/// Respond with "503 | Service Unavailable" while the application is down.
#[derive(Clone, Debug, Default)]
pub struct PreventRequestsDuringMaintenance {
    except: Vec<String>,
}

impl PreventRequestsDuringMaintenance {
    /// URIs that remain reachable during maintenance.
    pub fn except(mut self, uris: &[&str]) -> Self {
        self.except.extend(uris.iter().map(|u| u.to_string()));
        self
    }
}

#[async_trait]
impl Middleware for PreventRequestsDuringMaintenance {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let Some(app) = Application::try_current() else {
            return Ok(next.run(request).await);
        };
        if !app.is_down_for_maintenance()
            || self.except.iter().any(|uri| request.is(uri.trim_matches('/')) || Str::is(uri, &request.full_url()))
        {
            return Ok(next.run(request).await);
        }

        let data = app.maintenance_data().unwrap_or_else(|| json!({}));

        if let Some(secret) = data.get("secret").and_then(Value::as_str) {
            if request.path() == secret {
                return Ok(Response::redirect("/").with_cookie(MaintenanceModeBypassCookie::create(secret)));
            }
            if request
                .cookie(MaintenanceModeBypassCookie::NAME)
                .is_some_and(|cookie| MaintenanceModeBypassCookie::is_valid(&cookie, secret))
            {
                return Ok(next.run(request).await);
            }
        }

        if let Some(redirect) = data.get("redirect").and_then(Value::as_str) {
            let path = if redirect == "/" { "/" } else { redirect.trim_matches('/') };
            if !request.expects_json() && request.path() != path {
                return Ok(Response::redirect(if path == "/" { "/".to_string() } else { format!("/{path}") }));
            }
        }

        let status = data.get("status").and_then(|s| s.to_i64_lossy()).unwrap_or(503) as u16;
        let mut exception = HttpException::with_message(status, "Service Unavailable");
        if let Some(retry) = data.get("retry").filter(|r| !r.is_null()) {
            exception = exception.header("Retry-After", retry.to_string_lossy());
        }
        if let Some(refresh) = data.get("refresh").filter(|r| !r.is_null()) {
            exception = exception.header("Refresh", refresh.to_string_lossy());
        }

        if let Some(template) = data.get("template").and_then(Value::as_str) {
            if !request.expects_json() {
                let mut response = Response::make(template.to_string(), status);
                for (name, value) in &exception.headers {
                    response.set_header(name, value);
                }
                return Ok(response);
            }
        }

        Err(exception.into())
    }
}

/// Resolve a request path to a file inside the public directory, refusing
/// anything that would escape it.
pub fn public_file(public: &Path, request_path: &str) -> Option<PathBuf> {
    let relative = request_path.trim_start_matches('/');
    if relative.is_empty() {
        return None;
    }
    let candidate = Path::new(relative);
    if candidate
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let full = public.join(candidate);
    full.is_file().then_some(full)
}

/// Serve files that exist in the `public` directory, like `php artisan serve`.
#[derive(Clone, Debug)]
pub struct ServePublicFiles {
    root: PathBuf,
}

impl ServePublicFiles {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

#[async_trait]
impl Middleware for ServePublicFiles {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        if request.is_method("GET") || request.is_method("HEAD") {
            if let Some(file) = public_file(&self.root, &request.decoded_path()) {
                if let Ok(response) = Response::file(&file).await {
                    return Ok(response.with_header("Cache-Control", "public, max-age=3600"));
                }
            }
        }
        Ok(next.run(request).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_cookies_round_trip() {
        let cookie = MaintenanceModeBypassCookie::create("secret-token");
        assert!(MaintenanceModeBypassCookie::is_valid(&cookie.value, "secret-token"));
        assert!(!MaintenanceModeBypassCookie::is_valid(&cookie.value, "other"));
        assert!(!MaintenanceModeBypassCookie::is_valid("garbage", "secret-token"));
    }

    #[test]
    fn public_files_cannot_escape_the_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("robots.txt"), "User-agent: *").unwrap();
        assert!(public_file(dir.path(), "/robots.txt").is_some());
        assert!(public_file(dir.path(), "/../etc/passwd").is_none());
        assert!(public_file(dir.path(), "/").is_none());
        assert!(public_file(dir.path(), "/missing.txt").is_none());
    }
}
