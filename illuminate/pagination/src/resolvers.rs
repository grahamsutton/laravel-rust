//! Hooks letting the application tell paginators about the current request.

use std::sync::{Arc, LazyLock, RwLock};

use illuminate_support::{Map, Value};

type PageResolver = Arc<dyn Fn(&str) -> Option<u64> + Send + Sync>;
type PathResolver = Arc<dyn Fn() -> String + Send + Sync>;
type QueryResolver = Arc<dyn Fn() -> Map<String, Value> + Send + Sync>;
type Translator = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

static PAGE: LazyLock<RwLock<Option<PageResolver>>> = LazyLock::new(|| RwLock::new(None));
static PATH: LazyLock<RwLock<Option<PathResolver>>> = LazyLock::new(|| RwLock::new(None));
static QUERY: LazyLock<RwLock<Option<QueryResolver>>> = LazyLock::new(|| RwLock::new(None));
static TRANSLATOR: LazyLock<RwLock<Option<Translator>>> = LazyLock::new(|| RwLock::new(None));

/// Set the resolver for the current page (usually read from the request).
pub fn resolve_current_page_using(resolver: impl Fn(&str) -> Option<u64> + Send + Sync + 'static) {
    *PAGE.write().unwrap() = Some(Arc::new(resolver));
}

/// Set the resolver for the current path (usually the request URL).
pub fn resolve_current_path_using(resolver: impl Fn() -> String + Send + Sync + 'static) {
    *PATH.write().unwrap() = Some(Arc::new(resolver));
}

/// Set the resolver for the current query string.
pub fn resolve_query_string_using(resolver: impl Fn() -> Map<String, Value> + Send + Sync + 'static) {
    *QUERY.write().unwrap() = Some(Arc::new(resolver));
}

/// Set the translator used for "Previous" / "Next" labels.
pub fn translate_using(translator: impl Fn(&str) -> Option<String> + Send + Sync + 'static) {
    *TRANSLATOR.write().unwrap() = Some(Arc::new(translator));
}

/// Resolve the current page for the given page name (1 by default).
pub fn current_page(page_name: &str) -> u64 {
    let resolver = PAGE.read().unwrap().clone();
    resolver
        .and_then(|resolve| resolve(page_name))
        .filter(|page| *page >= 1)
        .unwrap_or(1)
}

/// Resolve the current path ("/" by default).
pub fn current_path() -> String {
    let resolver = PATH.read().unwrap().clone();
    resolver.map(|resolve| resolve()).unwrap_or_else(|| "/".to_string())
}

/// Resolve the current query string.
pub fn query_string() -> Map<String, Value> {
    let resolver = QUERY.read().unwrap().clone();
    resolver.map(|resolve| resolve()).unwrap_or_default()
}

/// Translate a pagination line, falling back to Laravel's English defaults.
pub fn translate(key: &str) -> String {
    let translator = TRANSLATOR.read().unwrap().clone();
    if let Some(line) = translator.and_then(|t| t(key)).filter(|line| line != key) {
        return line;
    }
    match key {
        "pagination.previous" => "&laquo; Previous".to_string(),
        "pagination.next" => "Next &raquo;".to_string(),
        other => other.to_string(),
    }
}
