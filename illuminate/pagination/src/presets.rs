//! Choosing the markup of pagination links: Tailwind CSS (the default),
//! Bootstrap 3, 4 or 5, or a view of your own.
//!
//! ```
//! use illuminate_pagination::{LengthAwarePaginator, PaginatorOptions};
//!
//! let paginator = LengthAwarePaginator::new(vec![1, 2], 6, 2, 2, PaginatorOptions::path("/users"));
//!
//! let html = paginator.links_with("pagination::bootstrap-5").to_string();
//! assert!(html.contains(r#"<li class="page-item active" aria-current="page"><span class="page-link">2</span></li>"#));
//! ```

use std::sync::{Arc, LazyLock, RwLock};

use illuminate_support::Value;

/// The view used for length-aware paginators by default.
pub const TAILWIND: &str = "pagination::tailwind";
/// The view used for simple paginators by default.
pub const SIMPLE_TAILWIND: &str = "pagination::simple-tailwind";

type ViewRenderer = Arc<dyn Fn(&str, Value) -> Option<String> + Send + Sync>;

static DEFAULT_VIEW: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new(TAILWIND.to_string()));
static DEFAULT_SIMPLE_VIEW: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new(SIMPLE_TAILWIND.to_string()));
static VIEW_RENDERER: LazyLock<RwLock<Option<ViewRenderer>>> = LazyLock::new(|| RwLock::new(None));

/// Set the default pagination view (`pagination::bootstrap-5`, or one of
/// your own views).
pub fn default_view(view: &str) {
    *DEFAULT_VIEW.write().unwrap() = view.to_string();
}

/// Set the default "simple" pagination view.
pub fn default_simple_view(view: &str) {
    *DEFAULT_SIMPLE_VIEW.write().unwrap() = view.to_string();
}

/// The current default pagination view.
pub fn get_default_view() -> String {
    DEFAULT_VIEW.read().unwrap().clone()
}

/// The current default "simple" pagination view.
pub fn get_default_simple_view() -> String {
    DEFAULT_SIMPLE_VIEW.read().unwrap().clone()
}

/// Render pagination links with Tailwind CSS (the default).
pub fn use_tailwind() {
    default_view(TAILWIND);
    default_simple_view(SIMPLE_TAILWIND);
}

/// Render pagination links with Bootstrap (version 4).
pub fn use_bootstrap() {
    use_bootstrap_four();
}

/// Render pagination links with Bootstrap 3.
pub fn use_bootstrap_three() {
    default_view("pagination::bootstrap-3");
    default_simple_view("pagination::simple-bootstrap-3");
}

/// Render pagination links with Bootstrap 4.
pub fn use_bootstrap_four() {
    default_view("pagination::bootstrap-4");
    default_simple_view("pagination::simple-bootstrap-4");
}

/// Render pagination links with Bootstrap 5.
pub fn use_bootstrap_five() {
    default_view("pagination::bootstrap-5");
    default_simple_view("pagination::simple-bootstrap-5");
}

/// Render views that aren't built in (your own pagination views) with the
/// given renderer — the view component's hook, Laravel's
/// `viewFactoryResolver`. It receives the view name and the data
/// (`paginator` and `elements`), and returns the HTML.
pub fn render_views_using(
    renderer: impl Fn(&str, Value) -> Option<String> + Send + Sync + 'static,
) {
    *VIEW_RENDERER.write().unwrap() = Some(Arc::new(renderer));
}

/// Render a custom view with the registered renderer.
pub(crate) fn render_custom(view: &str, data: Value) -> Option<String> {
    let renderer = VIEW_RENDERER.read().unwrap().clone();
    renderer.and_then(|render| render(view, data))
}
