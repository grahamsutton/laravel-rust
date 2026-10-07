//! Bootstrap and custom pagination views, and pretty JSON.

use illuminate_pagination::{
    LengthAwarePaginator, Paginator, PaginatorOptions, render_views_using, use_bootstrap,
    use_bootstrap_five, use_bootstrap_three, use_tailwind,
};
use illuminate_support::json;

fn paginator(page: u64) -> LengthAwarePaginator<u64> {
    LengthAwarePaginator::new(vec![1, 2], 6, 2, page, PaginatorOptions::path("/users"))
}

fn simple(page: u64, more: bool) -> Paginator<u64> {
    let items = if more { vec![1, 2, 3] } else { vec![1] };
    Paginator::new(items, 2, page, PaginatorOptions::path("/users"))
}

#[test]
fn bootstrap_four_matches_laravels_view() {
    let html = paginator(1)
        .links_with("pagination::bootstrap-4")
        .to_string();
    assert_eq!(
        html,
        concat!(
            "<nav><ul class=\"pagination\">",
            "<li class=\"page-item disabled\" aria-disabled=\"true\" aria-label=\"&laquo; Previous\"><span class=\"page-link\" aria-hidden=\"true\">&lsaquo;</span></li>",
            "<li class=\"page-item active\" aria-current=\"page\"><span class=\"page-link\">1</span></li>",
            "<li class=\"page-item\"><a class=\"page-link\" href=\"/users?page=2\">2</a></li>",
            "<li class=\"page-item\"><a class=\"page-link\" href=\"/users?page=3\">3</a></li>",
            "<li class=\"page-item\"><a class=\"page-link\" href=\"/users?page=2\" rel=\"next\" aria-label=\"Next &raquo;\">&rsaquo;</a></li>",
            "</ul></nav>",
        )
    );

    let html = paginator(3)
        .links_with("pagination::bootstrap-4")
        .to_string();
    assert!(html.contains("<li class=\"page-item\"><a class=\"page-link\" href=\"/users?page=2\" rel=\"prev\" aria-label=\"&laquo; Previous\">&lsaquo;</a></li>"));
    assert!(html.contains("<li class=\"page-item disabled\" aria-disabled=\"true\" aria-label=\"Next &raquo;\"><span class=\"page-link\" aria-hidden=\"true\">&rsaquo;</span></li>"));
}

#[test]
fn bootstrap_three_has_no_page_item_classes() {
    let html = paginator(2)
        .links_with("pagination::bootstrap-3")
        .to_string();
    assert!(
        html.starts_with(
            "<nav><ul class=\"pagination\"><li><a href=\"/users?page=1\" rel=\"prev\""
        )
    );
    assert!(html.contains("<li class=\"active\" aria-current=\"page\"><span>2</span></li>"));
    assert!(html.contains("<li><a href=\"/users?page=3\">3</a></li>"));
    assert!(!html.contains("page-item"));
}

#[test]
fn bootstrap_five_shows_a_summary_and_mobile_links() {
    let html = paginator(2)
        .links_with("pagination::bootstrap-5")
        .to_string();
    assert!(
        html.starts_with("<nav class=\"d-flex justify-items-center justify-content-between\">")
    );
    assert!(html.contains("<div class=\"d-flex justify-content-between flex-fill d-sm-none\"><ul class=\"pagination\"><li class=\"page-item\"><a class=\"page-link\" href=\"/users?page=1\" rel=\"prev\">&laquo; Previous</a></li>"));
    assert!(html.contains("<div class=\"small text-muted\">Showing <span class=\"fw-semibold\">3</span> to <span class=\"fw-semibold\">4</span> of <span class=\"fw-semibold\">6</span> results</div>"));
    assert!(html.contains("<li class=\"page-item active\" aria-current=\"page\"><span class=\"page-link\">2</span></li>"));
}

#[test]
fn many_pages_render_dots() {
    let paginator = LengthAwarePaginator::new(vec![1], 100, 1, 50, PaginatorOptions::path("/p"));
    let html = paginator.links_with("pagination::bootstrap-4").to_string();
    assert_eq!(
        html.matches("<li class=\"page-item disabled\" aria-disabled=\"true\"><span class=\"page-link\">...</span></li>").count(),
        2
    );
}

#[test]
fn simple_bootstrap_views() {
    let html = simple(1, true)
        .links_with("pagination::simple-bootstrap-4")
        .to_string();
    assert_eq!(
        html,
        concat!(
            "<nav><ul class=\"pagination\">",
            "<li class=\"page-item disabled\" aria-disabled=\"true\"><span class=\"page-link\">&laquo; Previous</span></li>",
            "<li class=\"page-item\"><a class=\"page-link\" href=\"/users?page=2\" rel=\"next\">Next &raquo;</a></li>",
            "</ul></nav>",
        )
    );

    let html = simple(2, false)
        .links_with("pagination::simple-bootstrap-3")
        .to_string();
    assert_eq!(
        html,
        concat!(
            "<nav><ul class=\"pagination\">",
            "<li><a href=\"/users?page=1\" rel=\"prev\">&laquo; Previous</a></li>",
            "<li class=\"disabled\" aria-disabled=\"true\"><span>Next &raquo;</span></li>",
            "</ul></nav>",
        )
    );

    let html = simple(2, true)
        .links_with("pagination::bootstrap-5")
        .to_string();
    assert!(html.starts_with(
        "<nav role=\"navigation\" aria-label=\"Pagination Navigation\"><ul class=\"pagination\">"
    ));

    // A single page renders nothing.
    assert_eq!(
        simple(1, false)
            .links_with("pagination::simple-bootstrap-5")
            .to_string(),
        ""
    );
    let single = LengthAwarePaginator::new(vec![1], 1, 2, 1, PaginatorOptions::path("/"));
    assert_eq!(single.links_with("pagination::bootstrap-5").to_string(), "");
}

#[test]
fn presets_and_custom_views_change_the_default() {
    // Everything that touches the global defaults lives in this one test.
    assert!(
        paginator(1)
            .links()
            .to_string()
            .contains("aria-label=\"Pagination Navigation\"")
    );

    use_bootstrap();
    assert_eq!(Paginator::get_default_view(), "pagination::bootstrap-4");
    assert!(
        paginator(1)
            .links()
            .to_string()
            .starts_with("<nav><ul class=\"pagination\"><li class=\"page-item")
    );
    assert!(simple(1, true).links().to_string().contains("page-link"));

    use_bootstrap_three();
    assert_eq!(
        Paginator::get_default_simple_view(),
        "pagination::simple-bootstrap-3"
    );
    assert!(!paginator(1).render().to_string().contains("page-item"));

    use_bootstrap_five();
    assert!(paginator(1).links().to_string().contains("fw-semibold"));

    render_views_using(|view, data| {
        (view == "vendor.pagination").then(|| {
            format!(
                "{} of {} ({} groups)",
                data["paginator"]["current_page"],
                data["paginator"]["total"],
                data["elements"].as_array().map_or(0, Vec::len)
            )
        })
    });
    Paginator::default_view("vendor.pagination");
    assert_eq!(paginator(2).links().to_string(), "2 of 6 (1 groups)");
    // Unknown views without a renderer fall back to Tailwind.
    assert!(
        paginator(2)
            .links_with("missing.view")
            .to_string()
            .contains("Pagination Navigation")
    );

    use_tailwind();
    assert_eq!(Paginator::get_default_view(), "pagination::tailwind");
}

#[test]
fn paginators_render_pretty_json() {
    let json = paginator(1).to_pretty_json();
    assert!(json.starts_with(
        "{\n    \"current_page\": 1,\n    \"data\": [\n        1,\n        2\n    ],"
    ));
    let decoded: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded["total"], json!(6));

    let json = simple(1, true).to_pretty_json();
    assert!(json.contains("\n    \"next_page_url\": \"/users?page=2\","));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&simple(1, true).to_json()).unwrap()["per_page"],
        json!(2)
    );
}
