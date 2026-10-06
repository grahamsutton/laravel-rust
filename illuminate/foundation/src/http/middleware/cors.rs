//! Cross-Origin Resource Sharing.

use illuminate_config::Repository;
use illuminate_container::try_app;
use illuminate_http::{Middleware, Next, Request, Response, async_trait};
use illuminate_support::{Result, Str, Value, ValueExt};

/// CORS options, as found in `config/cors.rs`.
#[derive(Clone, Debug)]
pub struct CorsOptions {
    pub paths: Vec<String>,
    pub allowed_methods: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub allowed_origins_patterns: Vec<String>,
    pub allowed_headers: Vec<String>,
    pub exposed_headers: Vec<String>,
    pub max_age: i64,
    pub supports_credentials: bool,
}

impl Default for CorsOptions {
    fn default() -> Self {
        Self {
            paths: vec!["api/*".into(), "sanctum/csrf-cookie".into()],
            allowed_methods: vec!["*".into()],
            allowed_origins: vec!["*".into()],
            allowed_origins_patterns: Vec::new(),
            allowed_headers: vec!["*".into()],
            exposed_headers: Vec::new(),
            max_age: 0,
            supports_credentials: false,
        }
    }
}

fn strings(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items.iter().map(|v| v.to_string_lossy()).collect(),
        Value::Null => Vec::new(),
        other => vec![other.to_string_lossy()],
    }
}

impl CorsOptions {
    /// Read the options from the `cors` configuration.
    pub fn from_config() -> Self {
        let Some(config) = try_app::<Repository>() else {
            return Self::default();
        };
        let defaults = Self::default();
        let get = |key: &str| config.get(&format!("cors.{key}"));
        Self {
            paths: if get("paths").is_null() { defaults.paths } else { strings(&get("paths")) },
            allowed_methods: if get("allowed_methods").is_null() { defaults.allowed_methods } else { strings(&get("allowed_methods")) },
            allowed_origins: if get("allowed_origins").is_null() { defaults.allowed_origins } else { strings(&get("allowed_origins")) },
            allowed_origins_patterns: strings(&get("allowed_origins_patterns")),
            allowed_headers: if get("allowed_headers").is_null() { defaults.allowed_headers } else { strings(&get("allowed_headers")) },
            exposed_headers: strings(&get("exposed_headers")),
            max_age: get("max_age").to_i64_lossy().unwrap_or(0),
            supports_credentials: get("supports_credentials").truthy(),
        }
    }

    fn origin_allowed(&self, origin: &str) -> bool {
        if self.allowed_origins.iter().any(|o| o == "*" || o == origin) {
            return true;
        }
        self.allowed_origins.iter().any(|o| o.contains('*') && Str::is(o, origin))
            || self
                .allowed_origins_patterns
                .iter()
                .any(|pattern| regex::Regex::new(pattern.trim_matches('/')).is_ok_and(|r| r.is_match(origin)))
    }
}

/// Handle CORS preflight requests and add CORS headers to responses.
#[derive(Clone, Debug, Default)]
pub struct HandleCors {
    options: Option<CorsOptions>,
}

impl HandleCors {
    /// Use explicit options instead of the `cors` configuration.
    pub fn with_options(options: CorsOptions) -> Self {
        Self {
            options: Some(options),
        }
    }

    fn options(&self) -> CorsOptions {
        self.options.clone().unwrap_or_else(CorsOptions::from_config)
    }
}

fn has_matching_path(request: &Request, options: &CorsOptions) -> bool {
    options.paths.iter().any(|path| {
        let path = if path == "/" { path.as_str() } else { path.trim_matches('/') };
        request.full_url_is(path) || request.is(path)
    })
}

fn vary(response: &mut Response, header: &str) {
    match response.header("vary") {
        Some(existing) if !existing.split(',').any(|h| h.trim().eq_ignore_ascii_case(header)) => {
            response.set_header("Vary", &format!("{existing}, {header}"));
        }
        Some(_) => {}
        None => response.set_header("Vary", header),
    }
}

fn add_origin(response: &mut Response, request: &Request, options: &CorsOptions) {
    let origin = request.header("origin").unwrap_or_default();
    if options.allowed_origins.iter().any(|o| o == "*") && !options.supports_credentials {
        response.set_header("Access-Control-Allow-Origin", "*");
    } else if options.origin_allowed(&origin) {
        response.set_header("Access-Control-Allow-Origin", &origin);
        vary(response, "Origin");
    } else {
        vary(response, "Origin");
    }
    if options.supports_credentials {
        response.set_header("Access-Control-Allow-Credentials", "true");
    }
}

#[async_trait]
impl Middleware for HandleCors {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let options = self.options();
        if !has_matching_path(&request, &options) {
            return Ok(next.run(request).await);
        }

        let is_preflight = request.is_method("OPTIONS")
            && request.has_header("origin")
            && request.has_header("access-control-request-method");

        if is_preflight {
            let mut response = Response::no_content();
            add_origin(&mut response, &request, &options);
            let methods = if options.allowed_methods.iter().any(|m| m == "*") {
                request
                    .header("access-control-request-method")
                    .unwrap_or_default()
                    .to_uppercase()
            } else {
                options.allowed_methods.join(", ").to_uppercase()
            };
            response.set_header("Access-Control-Allow-Methods", &methods);
            let headers = if options.allowed_headers.iter().any(|h| h == "*") {
                request.header("access-control-request-headers").unwrap_or_default()
            } else {
                options.allowed_headers.join(", ")
            };
            if !headers.is_empty() {
                response.set_header("Access-Control-Allow-Headers", &headers);
            }
            if options.max_age > 0 {
                response.set_header("Access-Control-Max-Age", &options.max_age.to_string());
            }
            vary(&mut response, "Access-Control-Request-Method");
            return Ok(response);
        }

        let mut response = next.run(request.clone()).await;
        if request.is_method("OPTIONS") {
            vary(&mut response, "Access-Control-Request-Method");
        }
        add_origin(&mut response, &request, &options);
        if !options.exposed_headers.is_empty() {
            response.set_header("Access-Control-Expose-Headers", &options.exposed_headers.join(", "));
        }
        Ok(response)
    }
}
