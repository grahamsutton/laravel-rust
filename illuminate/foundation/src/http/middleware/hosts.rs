//! Trusted hosts and proxies, and `Link` headers for preloaded assets.

use std::net::IpAddr;

use illuminate_http::{HttpException, Middleware, Next, Request, Response, async_trait};
use illuminate_support::Result;
use regex::Regex;

use crate::application::Application;
use crate::vite::Vite;

/// Determine if the IP address is the given address or falls inside the
/// given CIDR range (`10.0.0.0/8`, `2001:db8::/32`). `*` matches anything.
///
/// ```
/// use illuminate_foundation::http::middleware::ip_matches;
///
/// assert!(ip_matches("192.168.1.10", "192.168.1.0/24"));
/// assert!(ip_matches("10.1.2.3", "10.1.2.3"));
/// assert!(!ip_matches("192.168.2.10", "192.168.1.0/24"));
/// assert!(ip_matches("2001:db8::1", "2001:db8::/32"));
/// ```
pub fn ip_matches(ip: &str, pattern: &str) -> bool {
    if pattern == "*" || pattern == "**" {
        return true;
    }
    let Ok(ip) = ip.parse::<IpAddr>() else {
        return false;
    };
    let (network, bits) = match pattern.split_once('/') {
        Some((network, bits)) => (network, bits.parse::<u32>().ok()),
        None => (pattern, None),
    };
    let Ok(network) = network.parse::<IpAddr>() else {
        return false;
    };
    match (ip, network) {
        (IpAddr::V4(ip), IpAddr::V4(network)) => {
            let bits = bits.unwrap_or(32);
            if bits > 32 {
                return false;
            }
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            u32::from(ip) & mask == u32::from(network) & mask
        }
        (IpAddr::V6(ip), IpAddr::V6(network)) => {
            let bits = bits.unwrap_or(128);
            if bits > 128 {
                return false;
            }
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
            u128::from(ip) & mask == u128::from(network) & mask
        }
        _ => false,
    }
}

/// Only answer requests for the hosts you trust, protecting against host
/// header injection (`$middleware->trustHosts(...)`).
///
/// Patterns are regular expressions matched against the request's host. By
/// default, the application URL's host and its subdomains are trusted. The
/// check is skipped in the `local` environment and while running tests,
/// like Laravel's.
#[derive(Clone, Debug)]
pub struct TrustHosts {
    hosts: Vec<String>,
    subdomains: bool,
    always: bool,
}

impl TrustHosts {
    /// Trust the given host patterns, plus every subdomain of the
    /// application URL's host when `subdomains` is true.
    pub fn at(hosts: &[&str], subdomains: bool) -> Self {
        Self {
            hosts: hosts.iter().map(|host| host.to_string()).collect(),
            subdomains,
            always: false,
        }
    }

    /// Check the host in every environment, including `local` and tests.
    pub fn always(mut self) -> Self {
        self.always = true;
        self
    }

    /// The trusted host patterns.
    pub fn hosts(&self, app: &Application) -> Vec<String> {
        let mut hosts = self.hosts.clone();
        if (hosts.is_empty() || self.subdomains)
            && let Some(pattern) = all_subdomains_of_application_url(app)
        {
            hosts.push(pattern);
        }
        hosts
    }
}

impl Default for TrustHosts {
    fn default() -> Self {
        Self::at(&[], true)
    }
}

/// `^(.+\.)?laravel\.com$` for an application URL of `https://laravel.com`.
fn all_subdomains_of_application_url(app: &Application) -> Option<String> {
    let url = app.config_repository().string("app.url");
    let host = url::Url::parse(&url).ok()?.host_str()?.to_string();
    Some(format!("^(.+\\.)?{}$", regex::escape(&host)))
}

#[async_trait]
impl Middleware for TrustHosts {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let app = Application::current();
        if self.always || !(app.is_local() || app.running_unit_tests()) {
            let host = request.host().to_lowercase();
            let trusted = self.hosts(&app).iter().any(|pattern| {
                Regex::new(&format!("(?i){pattern}")).is_ok_and(|regex| regex.is_match(&host))
            });
            if !trusted {
                return Err(HttpException::with_message(400, "Bad request.").into());
            }
        }
        Ok(next.run(request).await)
    }
}

/// Add a `Link` header preloading the assets `@vite` rendered, so browsers
/// can start downloading them before parsing the page.
#[derive(Clone, Copy, Debug, Default)]
pub struct AddLinkHeadersForPreloadedAssets {
    limit: Option<usize>,
}

impl AddLinkHeadersForPreloadedAssets {
    /// Only preload the first `limit` assets.
    pub fn limit(limit: usize) -> Self {
        Self { limit: Some(limit) }
    }
}

#[async_trait]
impl Middleware for AddLinkHeadersForPreloadedAssets {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let mut response = next.run(request.clone()).await;
        let assets = Vite::preloaded_assets_for(&request);
        if !assets.is_empty() {
            let link = assets
                .iter()
                .take(self.limit.unwrap_or(usize::MAX))
                .map(|(url, attributes)| format!("<{url}>; {}", attributes.join("; ")))
                .collect::<Vec<_>>()
                .join(", ");
            response.set_header("Link", &link);
        }
        Ok(response)
    }
}
