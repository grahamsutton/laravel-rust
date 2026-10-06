//! URL generation: `url()`, `route()`, `asset()`, and signed URLs.
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_container::Container;
//! use illuminate_routing::{route, url, Route};
//! use illuminate_support::json;
//!
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container);
//!
//! Route::get("/user/{id}/profile", || async { "Profile" }).name("profile");
//!
//! assert_eq!(url("/posts/1"), "http://localhost/posts/1");
//! assert_eq!(
//!     route("profile", json!({"id": 1, "photos": "yes"})).unwrap(),
//!     "http://localhost/user/1/profile?photos=yes"
//! );
//! ```

use std::sync::{Arc, RwLock};
use std::time::Duration;

use hmac::{Hmac, Mac};
use indexmap::IndexMap;
use regex::Regex;
use sha2::Sha256;
use subtle::ConstantTimeEq;

use illuminate_config::Repository;
use illuminate_container::{Container, try_app};
use illuminate_http::{Request, current_request};
use illuminate_support::error::{InvalidArgumentException, RuntimeException};
use illuminate_support::{Arr, Carbon, Result, Str, Value, ValueExt};

use crate::exceptions::{RouteNotFoundException, UrlGenerationException};
use crate::params::{IntoRouteParameters, RouteParameter, RouteParameters, parameter_string};
use crate::route::RouteDefinition;
use crate::router::{Router, router};

/// Resolves the keys used to sign URLs: the current key first, followed by
/// any previous keys that should still validate.
pub type KeyResolver = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

/// When a temporary signed URL expires.
pub trait Expiration {
    /// The UNIX timestamp at which the URL expires.
    fn expires_at(&self) -> i64;
}

impl Expiration for Carbon {
    fn expires_at(&self) -> i64 {
        self.timestamp()
    }
}

impl Expiration for &Carbon {
    fn expires_at(&self) -> i64 {
        self.timestamp()
    }
}

/// A duration from now.
impl Expiration for Duration {
    fn expires_at(&self) -> i64 {
        Carbon::now().timestamp() + self.as_secs() as i64
    }
}

/// Request-scoped default URL parameters, set with `URL::defaults` while a
/// request is being handled.
#[derive(Debug, Default)]
pub struct UrlDefaults(RwLock<IndexMap<String, String>>);

impl UrlDefaults {
    /// The default parameters.
    pub fn all(&self) -> IndexMap<String, String> {
        self.0.read().unwrap().clone()
    }

    fn extend(&self, defaults: IndexMap<String, String>) {
        self.0.write().unwrap().extend(defaults);
    }
}

#[derive(Default)]
struct UrlState {
    forced_root: Option<String>,
    force_scheme: Option<String>,
    asset_root: Option<String>,
    defaults: IndexMap<String, String>,
    key_resolver: Option<KeyResolver>,
    request: Option<Request>,
}

/// Laravel's URL generator.
///
/// URLs are made absolute using the current request's scheme and host, or
/// the `app.url` configuration value outside of a request.
pub struct UrlGenerator {
    router: Router,
    state: RwLock<UrlState>,
}

impl std::fmt::Debug for UrlGenerator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UrlGenerator").finish_non_exhaustive()
    }
}

/// The characters Laravel leaves unencoded in generated route URLs.
const DONT_ENCODE: &[u8] = b"/@:;,=+!*|?&#%";

impl UrlGenerator {
    /// Create a URL generator for the given router's routes.
    pub fn new(router: Router) -> Self {
        Self {
            router,
            state: RwLock::new(UrlState::default()),
        }
    }

    /// The router whose named routes are used.
    pub fn router(&self) -> &Router {
        &self.router
    }

    // ------------------------------------------------------------------
    // The request
    // ------------------------------------------------------------------

    /// The request URLs are generated for: the current request, or the one
    /// given to [`UrlGenerator::set_request`].
    pub fn get_request(&self) -> Option<Request> {
        current_request().or_else(|| self.state.read().unwrap().request.clone())
    }

    /// Set the request used outside of HTTP handling (e.g. in tests or jobs).
    pub fn set_request(&self, request: Option<Request>) {
        self.state.write().unwrap().request = request;
    }

    fn config_url() -> Option<String> {
        try_app::<Repository>()
            .map(|config| config.string("app.url"))
            .filter(|url| !url.is_empty())
            .map(|url| url.trim_end_matches('/').to_string())
    }

    fn request_root(&self) -> String {
        match self.get_request() {
            Some(request) => request.root(),
            None => Self::config_url().unwrap_or_else(|| "http://localhost".to_string()),
        }
    }

    // ------------------------------------------------------------------
    // The current URL
    // ------------------------------------------------------------------

    /// The full URL of the current request, including the query string.
    pub fn full(&self) -> String {
        match self.get_request() {
            Some(request) => request.full_url(),
            None => self.to("/"),
        }
    }

    /// The URL of the current request, without the query string.
    pub fn current(&self) -> String {
        match self.get_request() {
            Some(request) => self.to(request.uri().path()),
            None => self.to("/"),
        }
    }

    /// The URL of the previous request: the `Referer` header, or the
    /// previous URL remembered by the session, or the root.
    pub fn previous(&self) -> String {
        self.previous_url().unwrap_or_else(|| self.to("/"))
    }

    /// The URL of the previous request, or the given fallback.
    pub fn previous_or(&self, fallback: &str) -> String {
        self.previous_url().unwrap_or_else(|| self.to(fallback))
    }

    fn previous_url(&self) -> Option<String> {
        let request = self.get_request()?;
        if let Some(referer) = request.header("referer").filter(|r| !r.is_empty()) {
            return Some(self.to(&referer));
        }
        match request.attribute("_previous_url") {
            Value::String(url) if !url.is_empty() => Some(self.to(&url)),
            _ => None,
        }
    }

    /// The path of the previous URL (`/dashboard`).
    pub fn previous_path(&self) -> String {
        let previous = self.previous();
        let path = url_path(&previous);
        let base = url_path(&self.to("/"));
        let path = if base != "/" {
            path.strip_prefix(&base).unwrap_or(&path).to_string()
        } else {
            path
        };
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() {
            "/".to_string()
        } else {
            trimmed.to_string()
        }
    }

    // ------------------------------------------------------------------
    // Plain URLs
    // ------------------------------------------------------------------

    /// Generate an absolute URL to the given path. Valid URLs are returned
    /// untouched.
    ///
    /// ```
    /// use illuminate_routing::{Router, UrlGenerator};
    ///
    /// let url = UrlGenerator::new(Router::new());
    /// assert_eq!(url.to("/posts/1"), "http://localhost/posts/1");
    /// assert_eq!(url.to("posts?page=2"), "http://localhost/posts?page=2");
    /// assert_eq!(url.to("https://laravel.com"), "https://laravel.com");
    /// ```
    pub fn to(&self, path: &str) -> String {
        self.to_with(path, &[], None)
    }

    /// Generate a URL with extra path segments, optionally forcing the scheme.
    pub fn to_with(&self, path: &str, extra: &[&str], secure: Option<bool>) -> String {
        if Self::is_valid_url(path) {
            return path.to_string();
        }
        let tail = extra
            .iter()
            .map(|segment| raw_url_encode(segment))
            .collect::<Vec<_>>()
            .join("/");
        let root = self.format_root(&self.format_scheme(secure), None);
        let (path, query) = extract_query_string(path);
        let path = format!("/{}", format!("{path}/{tail}").trim_matches('/'));
        format!("{}{query}", self.format(&root, &path))
    }

    /// Generate a URL with query string parameters merged into any existing ones.
    ///
    /// ```
    /// use illuminate_routing::{Router, UrlGenerator};
    /// use illuminate_support::json;
    ///
    /// let url = UrlGenerator::new(Router::new());
    /// assert_eq!(
    ///     url.query("/posts?sort=latest", json!({"search": "Laravel"})),
    ///     "http://localhost/posts?sort=latest&search=Laravel"
    /// );
    /// assert_eq!(
    ///     url.query("/posts", json!({"columns": ["title", "body"]})),
    ///     "http://localhost/posts?columns%5B0%5D=title&columns%5B1%5D=body"
    /// );
    /// ```
    pub fn query(&self, path: &str, query: Value) -> String {
        let (path, existing) = extract_query_string(path);
        let mut merged = illuminate_http::input::parse_query(existing.trim_start_matches('?'));
        if let (Value::Object(base), Value::Object(extra)) = (&mut merged, query) {
            for (key, value) in extra {
                base.insert(key, value);
            }
        }
        let query = build_query(&merged);
        let url = self.to(&format!("{path}?{query}"));
        url.trim_end_matches('?').to_string()
    }

    /// Generate a secure (HTTPS) URL to the given path.
    pub fn secure(&self, path: &str) -> String {
        self.to_with(path, &[], Some(true))
    }

    /// Generate a URL to an application asset, using `app.asset_url` when set.
    ///
    /// ```
    /// use illuminate_routing::{Router, UrlGenerator};
    ///
    /// let url = UrlGenerator::new(Router::new());
    /// assert_eq!(url.asset("css/app.css"), "http://localhost/css/app.css");
    /// ```
    pub fn asset(&self, path: &str) -> String {
        self.asset_with(path, None)
    }

    /// Generate a URL to an asset, optionally forcing the scheme.
    pub fn asset_with(&self, path: &str, secure: Option<bool>) -> String {
        if Self::is_valid_url(path) {
            return path.to_string();
        }
        let root = self
            .asset_root()
            .unwrap_or_else(|| self.format_root(&self.format_scheme(secure), None));
        format!("{}{}", Str::finish(&remove_index(&root), "/"), path.trim_matches('/'))
    }

    /// Generate a URL to a secure asset.
    pub fn secure_asset(&self, path: &str) -> String {
        self.asset_with(path, Some(true))
    }

    /// Generate a URL to an asset from a custom root domain such as a CDN.
    pub fn asset_from(&self, root: &str, path: &str) -> String {
        let root = self.format_root(&self.format_scheme(None), Some(root));
        format!("{}/{}", remove_index(&root), path.trim_matches('/'))
    }

    fn asset_root(&self) -> Option<String> {
        if let Some(root) = self.state.read().unwrap().asset_root.clone() {
            return Some(root);
        }
        try_app::<Repository>()
            .map(|config| config.string("app.asset_url"))
            .filter(|url| !url.is_empty())
            .map(|url| url.trim_end_matches('/').to_string())
    }

    /// The scheme for generated URLs (`"https://"`).
    pub fn format_scheme(&self, secure: Option<bool>) -> String {
        match secure {
            Some(true) => return "https://".to_string(),
            Some(false) => return "http://".to_string(),
            None => {}
        }
        if let Some(scheme) = self.state.read().unwrap().force_scheme.clone() {
            return scheme;
        }
        match self.get_request() {
            Some(request) => format!("{}://", request.scheme()),
            None => {
                let root = self.request_root();
                if root.starts_with("https://") {
                    "https://".to_string()
                } else {
                    "http://".to_string()
                }
            }
        }
    }

    /// The root URL with the given scheme.
    pub fn format_root(&self, scheme: &str, root: Option<&str>) -> String {
        let root = match root {
            Some(root) => root.to_string(),
            None => self
                .state
                .read()
                .unwrap()
                .forced_root
                .clone()
                .unwrap_or_else(|| self.request_root()),
        };
        if let Some(rest) = root.strip_prefix("http://") {
            format!("{scheme}{rest}")
        } else if let Some(rest) = root.strip_prefix("https://") {
            format!("{scheme}{rest}")
        } else {
            format!("{scheme}{root}")
        }
    }

    /// Join a root and a path into a URL.
    pub fn format(&self, root: &str, path: &str) -> String {
        let path = format!("/{}", path.trim_matches('/'));
        format!("{root}{path}").trim_matches('/').to_string()
    }

    /// Determine if the given path is already a valid URL.
    pub fn is_valid_url(path: &str) -> bool {
        let prefix = Regex::new(r"^(#|//|https?://|(mailto|tel|sms):)").expect("valid regex");
        let url = Regex::new(r"^[a-zA-Z][a-zA-Z0-9+.\-]*://[^\s]+$").expect("valid regex");
        prefix.is_match(path) || url.is_match(path)
    }

    // ------------------------------------------------------------------
    // Configuration
    // ------------------------------------------------------------------

    /// Force the scheme for generated URLs (`Some("https")`).
    pub fn force_scheme(&self, scheme: Option<&str>) {
        self.state.write().unwrap().force_scheme = scheme.map(|s| format!("{}://", s.trim_end_matches("://")));
    }

    /// Force all generated URLs to use HTTPS.
    pub fn force_https(&self, force: bool) {
        if force {
            self.force_scheme(Some("https"));
        }
    }

    /// Force the root URL for generated URLs.
    pub fn force_root_url(&self, root: Option<&str>) {
        self.state.write().unwrap().forced_root = root.map(|r| r.trim_end_matches('/').to_string());
    }

    /// Alias of [`UrlGenerator::force_root_url`].
    pub fn use_origin(&self, root: Option<&str>) {
        self.force_root_url(root);
    }

    /// Set the root URL used for assets (overriding `app.asset_url`).
    pub fn use_asset_origin(&self, root: Option<&str>) {
        self.state.write().unwrap().asset_root = root.map(|r| r.trim_end_matches('/').to_string());
    }

    /// Set default values for route parameters. During a request, the
    /// defaults apply to that request only.
    pub fn defaults<K, V>(&self, defaults: impl IntoIterator<Item = (K, V)>)
    where
        K: Into<String>,
        V: ToString,
    {
        let defaults: IndexMap<String, String> = defaults
            .into_iter()
            .map(|(k, v)| (k.into(), v.to_string()))
            .collect();
        match current_request() {
            Some(request) => {
                let scoped = match request.extension::<UrlDefaults>() {
                    Some(scoped) => scoped,
                    None => {
                        let scoped = Arc::new(UrlDefaults::default());
                        request.set_extension(scoped.clone());
                        scoped
                    }
                };
                scoped.extend(defaults);
            }
            None => self.state.write().unwrap().defaults.extend(defaults),
        }
    }

    /// The default route parameters in effect.
    pub fn get_default_parameters(&self) -> IndexMap<String, String> {
        let mut defaults = self.state.read().unwrap().defaults.clone();
        if let Some(scoped) = current_request().and_then(|r| r.extension::<UrlDefaults>()) {
            defaults.extend(scoped.all());
        }
        defaults
    }

    /// Set the callback resolving the keys used to sign URLs.
    pub fn set_key_resolver(&self, resolver: impl Fn() -> Vec<String> + Send + Sync + 'static) {
        self.state.write().unwrap().key_resolver = Some(Arc::new(resolver));
    }

    /// The signing keys: `app.key` followed by `app.previous_keys`.
    fn keys(&self) -> Vec<String> {
        if let Some(resolver) = self.state.read().unwrap().key_resolver.clone() {
            return resolver();
        }
        let Some(config) = try_app::<Repository>() else {
            return Vec::new();
        };
        let mut keys = vec![config.string("app.key")];
        keys.extend(config.strings("app.previous_keys"));
        keys.into_iter().filter(|key| !key.is_empty()).collect()
    }

    // ------------------------------------------------------------------
    // Named routes
    // ------------------------------------------------------------------

    /// Generate the absolute URL to a named route.
    ///
    /// Parameters that don't belong to the route are added to the query
    /// string. Missing required parameters produce an
    /// [`UrlGenerationException`], and unknown names a
    /// [`RouteNotFoundException`].
    pub fn route<'a>(&self, name: &str, parameters: impl IntoRouteParameters<'a>) -> Result<String> {
        self.route_with(name, parameters, true)
    }

    /// Generate the URL to a named route, absolute or relative (`/user/1`).
    pub fn route_with<'a>(
        &self,
        name: &str,
        parameters: impl IntoRouteParameters<'a>,
        absolute: bool,
    ) -> Result<String> {
        match self.router.get_by_name(name) {
            Some(route) => self.to_route(&route, parameters, absolute),
            None => Err(RouteNotFoundException::new(name).into()),
        }
    }

    /// Generate the URL to the given route.
    pub fn to_route<'a>(
        &self,
        route: &RouteDefinition,
        parameters: impl IntoRouteParameters<'a>,
        absolute: bool,
    ) -> Result<String> {
        let defaults = self.get_default_parameters();
        let mut parameters = format_parameters(route, parameters.into_route_parameters(), &defaults);

        let domain = route.get_domain().map(|domain| self.format_domain(&domain));
        let scheme = self.format_scheme(None);
        let root = self.format_root(&scheme, domain.as_deref());
        let root = replace_route_parameters(&root, &mut parameters, &defaults);
        // Optional parameters left out of the middle of a URI would leave
        // empty segments behind ("archive//2024"), so collapse them.
        let path = replace_route_parameters(&route.uri(), &mut parameters, &defaults);
        let path = Regex::new("/{2,}").expect("valid regex").replace_all(&path, "/").into_owned();

        let uri = add_query_string(&self.format(&root, &path), &parameters);

        let missing: Vec<String> = Regex::new(r"\{(.*?)\}")
            .expect("valid regex")
            .captures_iter(&uri)
            .map(|c| c[1].to_string())
            .collect();
        if !missing.is_empty() {
            return Err(UrlGenerationException::for_missing_parameters(
                route.get_name().as_deref(),
                &route.uri(),
                &missing,
            )
            .into());
        }

        let uri = encode_uri(&uri);

        if absolute {
            return Ok(uri);
        }

        let stripped = Regex::new(r"^(//|[^/?])+")
            .expect("valid regex")
            .replace(&uri, "")
            .into_owned();
        Ok(format!("/{}", stripped.trim_start_matches('/')))
    }

    fn format_domain(&self, domain: &str) -> String {
        let domain = format!("{}{domain}", self.format_scheme(None));
        match self.get_request() {
            Some(request) => {
                let secure = request.secure();
                let port = request.port();
                if (secure && port == 443) || (!secure && port == 80) {
                    domain
                } else {
                    format!("{domain}:{port}")
                }
            }
            None => domain,
        }
    }

    // ------------------------------------------------------------------
    // Signed URLs
    // ------------------------------------------------------------------

    /// Create a signed URL to a named route.
    pub fn signed_route<'a>(&self, name: &str, parameters: impl IntoRouteParameters<'a>) -> Result<String> {
        self.signed_route_with(name, parameters, None::<Duration>, true)
    }

    /// Create a temporary signed URL that expires at the given time.
    pub fn temporary_signed_route<'a>(
        &self,
        name: &str,
        expiration: impl Expiration,
        parameters: impl IntoRouteParameters<'a>,
    ) -> Result<String> {
        self.signed_route_with(name, parameters, Some(expiration), true)
    }

    /// Create a signed URL, with an optional expiration, absolute or relative.
    ///
    /// The URL is signed with an HMAC-SHA256 of the URL (keyed by
    /// `app.key`), carried in the `signature` query parameter alongside the
    /// optional `expires` timestamp — exactly like Laravel.
    pub fn signed_route_with<'a, E: Expiration>(
        &self,
        name: &str,
        parameters: impl IntoRouteParameters<'a>,
        expiration: Option<E>,
        absolute: bool,
    ) -> Result<String> {
        let mut parameters = parameters.into_route_parameters();
        if parameters.contains_key("signature") {
            return Err(InvalidArgumentException::new(
                "\"Signature\" is a reserved parameter when generating signed routes. Please rename your route parameter.",
            )
            .into());
        }
        if parameters.contains_key("expires") {
            return Err(InvalidArgumentException::new(
                "\"Expires\" is a reserved parameter when generating signed routes. Please rename your route parameter.",
            )
            .into());
        }

        if let Some(expiration) = expiration {
            parameters.insert(
                "expires".to_string(),
                RouteParameter::Value(Value::from(expiration.expires_at())),
            );
        }
        parameters.sort_by_key();

        let key = self.keys().into_iter().next().ok_or_else(|| {
            RuntimeException::new("No application encryption key has been specified.")
        })?;

        let route = self
            .router
            .get_by_name(name)
            .ok_or_else(|| RouteNotFoundException::new(name))?;

        let items = parameters.into_items();
        let unsigned: RouteParameters<'_> = clone_parameters(&items);
        let url = self.to_route(&route, unsigned, absolute)?;
        let signature = sign(&url, &key);

        let mut signed: RouteParameters<'_> = clone_parameters(&items);
        signed.insert("signature".to_string(), RouteParameter::Value(Value::String(signature)));
        self.to_route(&route, signed, absolute)
    }

    /// Determine if the request has a valid (absolute) signature.
    pub fn has_valid_signature(&self, request: &Request) -> bool {
        self.has_valid_signature_while_ignoring(request, &[], true)
    }

    /// Determine if the request has a valid relative signature.
    pub fn has_valid_relative_signature(&self, request: &Request) -> bool {
        self.has_valid_signature_while_ignoring(request, &[], false)
    }

    /// Determine if the request has a valid signature, ignoring the given
    /// query parameters.
    pub fn has_valid_signature_while_ignoring(&self, request: &Request, ignore: &[&str], absolute: bool) -> bool {
        self.has_correct_signature(request, absolute, ignore) && self.signature_has_not_expired(request)
    }

    /// Determine if the signature in the request is correct.
    pub fn has_correct_signature(&self, request: &Request, absolute: bool, ignore: &[&str]) -> bool {
        let Value::String(signature) = request.query("signature") else {
            return false;
        };

        let url = if absolute {
            request.url().trim_end_matches('/').to_string()
        } else {
            let path = request.uri().path().trim_matches('/');
            format!("/{path}")
        };

        let query = request
            .query_string()
            .unwrap_or_default()
            .split('&')
            .filter(|parameter| {
                let name = parameter.split('=').next().unwrap_or_default();
                name != "signature" && !ignore.contains(&name)
            })
            .collect::<Vec<_>>()
            .join("&");

        let original = format!("{url}?{query}");
        let original = original.trim_end_matches('?');

        self.keys()
            .iter()
            .any(|key| bool::from(sign(original, key).as_bytes().ct_eq(signature.as_bytes())))
    }

    /// Determine if the `expires` timestamp in the request has not passed.
    pub fn signature_has_not_expired(&self, request: &Request) -> bool {
        match request.query("expires") {
            Value::Null => true,
            Value::String(expires) if expires.is_empty() => true,
            Value::String(expires) => match expires.parse::<i64>() {
                Ok(timestamp) => Carbon::now().timestamp() <= timestamp,
                Err(_) => false,
            },
            _ => false,
        }
    }
}

fn clone_parameters<'a>(items: &[(Option<String>, RouteParameter<'a>)]) -> RouteParameters<'a> {
    let mut parameters = RouteParameters::new();
    for (key, value) in items {
        let value = match value {
            RouteParameter::Value(value) => RouteParameter::Value(value.clone()),
            RouteParameter::Routable(model) => RouteParameter::Routable(*model),
        };
        match key {
            Some(key) => parameters.insert(key.clone(), value),
            None => parameters = parameters.push(value),
        }
    }
    parameters
}

fn sign(url: &str, key: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(url.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn url_path(url: &str) -> String {
    let without_scheme = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    let path_start = without_scheme.find('/').map(|i| &without_scheme[i..]).unwrap_or("/");
    let path = path_start.split(['?', '#']).next().unwrap_or("/");
    if path.is_empty() { "/".to_string() } else { path.to_string() }
}

fn remove_index(root: &str) -> String {
    if root.contains("index.php") {
        root.replace("/index.php", "")
    } else {
        root.to_string()
    }
}

fn extract_query_string(path: &str) -> (&str, &str) {
    match path.find('?') {
        Some(position) => (&path[..position], &path[position..]),
        None => (path, ""),
    }
}

/// PHP's `rawurlencode`.
pub(crate) fn raw_url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Encode a generated URI, leaving URL syntax characters alone.
fn encode_uri(uri: &str) -> String {
    let mut out = String::with_capacity(uri.len());
    for byte in uri.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b if DONT_ENCODE.contains(&b) => out.push(b as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Build a query string the way Laravel's `Arr::query` does (RFC 3986).
pub(crate) fn build_query(value: &Value) -> String {
    let without_nulls = strip_nulls(value);
    Arr::query(&without_nulls).replace('+', "%20")
}

fn strip_nulls(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), strip_nulls(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().filter(|v| !v.is_null()).map(strip_nulls).collect()),
        other => other.clone(),
    }
}

/// Parameters after matching them up with the route: `(name, value)` for
/// named parameters (`None` values are placeholders for parameters that
/// weren't given) and `(None, value)` for leftover positional parameters.
type Formatted = Vec<(Option<String>, Option<Value>)>;

/// Match the given parameters up with the route's parameters, by name
/// first and then by position — a faithful port of Laravel's
/// `RouteUrlGenerator::formatParameters`.
fn format_parameters(
    route: &RouteDefinition,
    parameters: RouteParameters<'_>,
    defaults: &IndexMap<String, String>,
) -> Formatted {
    let mut named_input: Vec<(String, RouteParameter<'_>)> = Vec::new();
    let mut positional: Vec<RouteParameter<'_>> = Vec::new();
    for (key, value) in parameters.into_items() {
        match key {
            Some(key) => named_input.push((key, value)),
            None => positional.push(value),
        }
    }

    let route_parameters = route.parameter_names();
    let optional = route.optional_parameter_names();
    let default_key = |name: &str| match route.binding_field_for(name) {
        Some(field) => format!("{name}:{field}"),
        None => name.to_string(),
    };

    let mut named: IndexMap<String, Option<RouteParameter<'_>>> = IndexMap::new();
    let mut required_missing: Vec<String> = Vec::new();

    for name in &route_parameters {
        if let Some(position) = named_input.iter().position(|(key, value)| key == name && !value.is_null()) {
            let (_, value) = named_input.remove(position);
            named.insert(name.clone(), Some(value));
            continue;
        }
        if !defaults.contains_key(&default_key(name)) && !optional.contains(name) {
            required_missing.push(name.clone());
        }
        named.insert(name.clone(), None);
    }

    // Positional parameters fill the required parameters that are missing, in order.
    if positional.len() == required_missing.len() {
        for name in required_missing.iter().rev() {
            match positional.pop() {
                Some(value) => {
                    named.insert(name.clone(), Some(value));
                }
                None => break,
            }
        }
    }

    let empty = named.values().filter(|v| v.is_none()).count();
    let mut offset: isize = 0;
    if !required_missing.is_empty() && positional.len() != empty {
        offset = named.get_index_of(&required_missing[0]).unwrap_or(0) as isize;
        let remaining = empty as isize - offset - positional.len() as isize;
        if remaining < 0 {
            offset += remaining;
        }
        offset = offset.max(0);
    } else if required_missing.is_empty() && !positional.is_empty() {
        let mut remaining = positional.len();
        for index in (0..named.len()).rev() {
            if named[index].is_none() {
                offset = index as isize;
                remaining -= 1;
                if remaining == 0 {
                    break;
                }
            }
        }
    }

    for index in (offset as usize)..named.len() {
        if named[index].is_some() {
            continue;
        }
        if !positional.is_empty() {
            named[index] = Some(positional.remove(0));
        }
    }

    let mut formatted: Formatted = Vec::new();
    for (name, value) in named {
        let field = route.binding_field_for(&name);
        let value = match value {
            Some(value) => Some(value.resolve(field.as_deref())),
            None => defaults.get(&default_key(&name)).map(|d| Value::String(d.clone())),
        };
        formatted.push((Some(name), value));
    }
    for (key, value) in named_input {
        formatted.push((Some(key), Some(value.resolve(None))));
    }
    for value in positional {
        formatted.push((None, Some(value.resolve(None))));
    }
    formatted
}

fn encode_parameter(value: &Value) -> String {
    parameter_string(value)
        .replace('%', "%25")
        .replace('?', "%3F")
        .replace('#', "%23")
}

/// Replace the `{parameters}` in a path (or domain) with their values,
/// removing them from the list as they are used.
fn replace_route_parameters(path: &str, parameters: &mut Formatted, defaults: &IndexMap<String, String>) -> String {
    let named = Regex::new(r"\{(.*?)(\?)?\}").expect("valid regex");
    let path = named.replace_all(path, |captures: &regex::Captures<'_>| {
        let name = &captures[1];
        let position = parameters.iter().position(|(key, _)| key.as_deref() == Some(name));
        if let Some(position) = position {
            let filled = parameters[position]
                .1
                .as_ref()
                .is_some_and(|v| !v.is_null() && !parameter_string(v).is_empty());
            if filled {
                let (_, value) = parameters.remove(position);
                return encode_parameter(&value.unwrap_or(Value::Null));
            }
        }
        if let Some(default) = defaults.get(name) {
            return encode_parameter(&Value::String(default.clone()));
        }
        if let Some(position) = position {
            parameters.remove(position);
        }
        captures[0].to_string()
    });

    let any = Regex::new(r"\{.*?\}").expect("valid regex");
    let path = any.replace_all(&path, |captures: &regex::Captures<'_>| {
        let placeholder = &captures[0];
        let position = parameters.iter().position(|(key, _)| key.is_none());
        match position {
            None if !placeholder.ends_with("?}") => placeholder.to_string(),
            None => String::new(),
            Some(position) => {
                let (_, value) = parameters.remove(position);
                encode_parameter(&value.unwrap_or(Value::Null))
            }
        }
    });

    let optional = Regex::new(r"\{.*?\?\}").expect("valid regex");
    optional.replace_all(&path, "").trim_matches('/').to_string()
}

/// Append the remaining parameters as a query string, keeping any fragment
/// at the very end.
fn add_query_string(uri: &str, parameters: &Formatted) -> String {
    let (uri, fragment) = match uri.split_once('#') {
        Some((uri, fragment)) => (uri.to_string(), Some(fragment.to_string())),
        None => (uri.to_string(), None),
    };

    let mut keyed = illuminate_support::Map::new();
    let mut numeric = Vec::new();
    for (key, value) in parameters {
        let Some(value) = value else { continue };
        match key {
            Some(key) => {
                keyed.insert(key.clone(), value.clone());
            }
            None => numeric.push(value.to_string_lossy()),
        }
    }

    let mut query = build_query(&Value::Object(keyed));
    if !numeric.is_empty() {
        query.push('&');
        query.push_str(&numeric.join("&"));
    }
    let query = query.trim_matches('&');

    let mut result = uri;
    if !query.is_empty() {
        result.push('?');
        result.push_str(query);
    }
    if let Some(fragment) = fragment {
        result.push('#');
        result.push_str(&fragment);
    }
    result
}

/// Get the URL generator from the container, registering one for the
/// application's router if none has been bound yet.
pub fn url_generator() -> Arc<UrlGenerator> {
    if let Some(url) = try_app::<UrlGenerator>() {
        return url;
    }
    let container = Container::get_instance();
    let router = router();
    container.singleton_if::<UrlGenerator>(move |_| Arc::new(UrlGenerator::new((*router).clone())));
    container.make::<UrlGenerator>()
}

/// The `URL` facade.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_routing::{Route, URL};
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container);
///
/// Route::get("/unsubscribe/{user}", || async { "Unsubscribed" }).name("unsubscribe");
/// URL::set_key_resolver(|| vec!["secret".to_string()]);
///
/// let url = URL::signed_route("unsubscribe", 1).unwrap();
/// assert!(url.starts_with("http://localhost/unsubscribe/1?signature="));
/// ```
pub struct URL;

impl URL {
    /// The underlying URL generator.
    pub fn generator() -> Arc<UrlGenerator> {
        url_generator()
    }

    /// The URL of the current request, without the query string.
    pub fn current() -> String {
        url_generator().current()
    }

    /// The full URL of the current request.
    pub fn full() -> String {
        url_generator().full()
    }

    /// The URL of the previous request.
    pub fn previous() -> String {
        url_generator().previous()
    }

    /// The URL of the previous request, or a fallback.
    pub fn previous_or(fallback: &str) -> String {
        url_generator().previous_or(fallback)
    }

    /// The path of the previous request.
    pub fn previous_path() -> String {
        url_generator().previous_path()
    }

    /// Generate an absolute URL to the given path.
    pub fn to(path: &str) -> String {
        url_generator().to(path)
    }

    /// Generate a URL with merged query string parameters.
    pub fn query(path: &str, query: Value) -> String {
        url_generator().query(path, query)
    }

    /// Generate a secure URL to the given path.
    pub fn secure(path: &str) -> String {
        url_generator().secure(path)
    }

    /// Generate a URL to an asset.
    pub fn asset(path: &str) -> String {
        url_generator().asset(path)
    }

    /// Generate a URL to a secure asset.
    pub fn secure_asset(path: &str) -> String {
        url_generator().secure_asset(path)
    }

    /// Generate a URL to an asset on another root.
    pub fn asset_from(root: &str, path: &str) -> String {
        url_generator().asset_from(root, path)
    }

    /// Generate the URL to a named route.
    pub fn route<'a>(name: &str, parameters: impl IntoRouteParameters<'a>) -> Result<String> {
        url_generator().route(name, parameters)
    }

    /// Generate the URL to a named route, absolute or relative.
    pub fn route_with<'a>(name: &str, parameters: impl IntoRouteParameters<'a>, absolute: bool) -> Result<String> {
        url_generator().route_with(name, parameters, absolute)
    }

    /// Create a signed URL to a named route.
    pub fn signed_route<'a>(name: &str, parameters: impl IntoRouteParameters<'a>) -> Result<String> {
        url_generator().signed_route(name, parameters)
    }

    /// Create a temporary signed URL to a named route.
    pub fn temporary_signed_route<'a>(
        name: &str,
        expiration: impl Expiration,
        parameters: impl IntoRouteParameters<'a>,
    ) -> Result<String> {
        url_generator().temporary_signed_route(name, expiration, parameters)
    }

    /// Create a signed URL, absolute or relative, with an optional expiration.
    pub fn signed_route_with<'a, E: Expiration>(
        name: &str,
        parameters: impl IntoRouteParameters<'a>,
        expiration: Option<E>,
        absolute: bool,
    ) -> Result<String> {
        url_generator().signed_route_with(name, parameters, expiration, absolute)
    }

    /// Determine if the request has a valid signature.
    pub fn has_valid_signature(request: &Request) -> bool {
        url_generator().has_valid_signature(request)
    }

    /// Determine if the request has a valid relative signature.
    pub fn has_valid_relative_signature(request: &Request) -> bool {
        url_generator().has_valid_relative_signature(request)
    }

    /// Determine if the request has a valid signature, ignoring some query parameters.
    pub fn has_valid_signature_while_ignoring(request: &Request, ignore: &[&str], absolute: bool) -> bool {
        url_generator().has_valid_signature_while_ignoring(request, ignore, absolute)
    }

    /// Set default values for route parameters.
    pub fn defaults<K: Into<String>, V: ToString>(defaults: impl IntoIterator<Item = (K, V)>) {
        url_generator().defaults(defaults)
    }

    /// Force the scheme of generated URLs.
    pub fn force_scheme(scheme: Option<&str>) {
        url_generator().force_scheme(scheme)
    }

    /// Force generated URLs to use HTTPS.
    pub fn force_https(force: bool) {
        url_generator().force_https(force)
    }

    /// Force the root URL of generated URLs.
    pub fn force_root_url(root: Option<&str>) {
        url_generator().force_root_url(root)
    }

    /// Alias of [`URL::force_root_url`].
    pub fn use_origin(root: Option<&str>) {
        url_generator().use_origin(root)
    }

    /// Set the root URL of generated asset URLs.
    pub fn use_asset_origin(root: Option<&str>) {
        url_generator().use_asset_origin(root)
    }

    /// Set the callback resolving the URL signing keys.
    pub fn set_key_resolver(resolver: impl Fn() -> Vec<String> + Send + Sync + 'static) {
        url_generator().set_key_resolver(resolver)
    }

    /// Determine if the given path is a valid URL.
    pub fn is_valid_url(path: &str) -> bool {
        UrlGenerator::is_valid_url(path)
    }
}
