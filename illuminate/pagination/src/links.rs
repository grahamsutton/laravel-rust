//! Pagination link windows and the default Tailwind CSS markup.

use indexmap::IndexMap;

use illuminate_support::e;

use crate::length_aware::LengthAwarePaginator;
use crate::resolvers::translate;

/// A group of page links, or a "..." separator.
#[derive(Clone, Debug, PartialEq)]
pub enum Element {
    Dots,
    Pages(IndexMap<u64, String>),
}

/// Build the link "window" around the current page, exactly like Laravel's
/// `UrlWindow`.
pub fn elements(current: u64, last: u64, on_each_side: u64, url: impl Fn(u64) -> String) -> Vec<Element> {
    let range = |start: u64, end: u64| -> IndexMap<u64, String> {
        (start.max(1)..=end.min(last)).map(|page| (page, url(page))).collect()
    };

    let (first, slider, final_pages) = if last < on_each_side * 2 + 8 {
        (Some(range(1, last)), None, None)
    } else {
        let window = on_each_side + 4;
        let has_pages = last > 1;
        if !has_pages {
            (None, None, None)
        } else if current <= window {
            (Some(range(1, window + on_each_side)), None, Some(range(last - 1, last)))
        } else if current > last.saturating_sub(window) {
            (
                Some(range(1, 2)),
                None,
                Some(range(last.saturating_sub(window + on_each_side.saturating_sub(1)), last)),
            )
        } else {
            (
                Some(range(1, 2)),
                Some(range(current - on_each_side, current + on_each_side)),
                Some(range(last - 1, last)),
            )
        }
    };

    let mut out = Vec::new();
    if let Some(first) = first {
        out.push(Element::Pages(first));
    }
    if let Some(slider) = slider {
        out.push(Element::Dots);
        out.push(Element::Pages(slider));
    }
    if let Some(final_pages) = final_pages {
        out.push(Element::Dots);
        out.push(Element::Pages(final_pages));
    }
    out
}

const PREV_ICON: &str = r#"<svg class="w-5 h-5" fill="currentColor" viewBox="0 0 20 20"><path fill-rule="evenodd" d="M12.707 5.293a1 1 0 010 1.414L9.414 10l3.293 3.293a1 1 0 01-1.414 1.414l-4-4a1 1 0 010-1.414l4-4a1 1 0 011.414 0z" clip-rule="evenodd" /></svg>"#;
const NEXT_ICON: &str = r#"<svg class="w-5 h-5" fill="currentColor" viewBox="0 0 20 20"><path fill-rule="evenodd" d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z" clip-rule="evenodd" /></svg>"#;

const DISABLED_BUTTON: &str = "inline-flex items-center px-4 py-2 text-sm font-medium text-gray-600 bg-white border border-gray-300 cursor-not-allowed leading-5 rounded-md dark:text-gray-300 dark:bg-gray-700 dark:border-gray-600";
const BUTTON: &str = "inline-flex items-center px-4 py-2 text-sm font-medium text-gray-800 bg-white border border-gray-300 leading-5 rounded-md hover:text-gray-700 focus:outline-none focus:ring ring-gray-300 focus:border-blue-300 active:bg-gray-100 active:text-gray-800 transition ease-in-out duration-150 dark:bg-gray-800 dark:border-gray-600 dark:text-gray-200 dark:focus:border-blue-700 dark:active:bg-gray-700 dark:active:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-900 dark:hover:text-gray-200";
const PAGE_LINK: &str = "inline-flex items-center px-4 py-2 -ml-px text-sm font-medium text-gray-700 bg-white border border-gray-300 leading-5 hover:text-gray-700 focus:outline-none focus:ring ring-gray-300 focus:border-blue-300 active:bg-gray-100 active:text-gray-700 transition ease-in-out duration-150 dark:bg-gray-800 dark:border-gray-600 dark:text-gray-300 dark:hover:text-gray-300 dark:active:bg-gray-700 dark:focus:border-blue-800 hover:bg-gray-100 dark:hover:bg-gray-900";
const CURRENT_PAGE: &str = "inline-flex items-center px-4 py-2 -ml-px text-sm font-medium text-gray-700 bg-gray-200 border border-gray-300 cursor-default leading-5 dark:bg-gray-700 dark:border-gray-600 dark:text-gray-300";
const DOTS: &str = "inline-flex items-center px-4 py-2 -ml-px text-sm font-medium text-gray-700 bg-white border border-gray-300 cursor-default leading-5 dark:bg-gray-800 dark:border-gray-600 dark:text-gray-300";
const ARROW_LINK: &str = "inline-flex items-center px-2 py-2 text-sm font-medium text-gray-500 bg-white border border-gray-300 leading-5 hover:text-gray-400 focus:outline-none focus:ring ring-gray-300 focus:border-blue-300 active:bg-gray-100 active:text-gray-500 transition ease-in-out duration-150 dark:bg-gray-800 dark:border-gray-600 dark:active:bg-gray-700 dark:focus:border-blue-800 dark:text-gray-300 dark:hover:bg-gray-900 dark:hover:text-gray-300";
const ARROW_DISABLED: &str = "inline-flex items-center px-2 py-2 text-sm font-medium text-gray-500 bg-white border border-gray-300 cursor-not-allowed leading-5 dark:bg-gray-700 dark:border-gray-600 dark:text-gray-400";

fn previous_next_buttons(out: &mut String, previous: Option<&String>, next: Option<&String>) {
    let prev_label = translate("pagination.previous");
    let next_label = translate("pagination.next");
    match previous {
        None => out.push_str(&format!("<span class=\"{DISABLED_BUTTON}\">{prev_label}</span>")),
        Some(url) => out.push_str(&format!(
            "<a href=\"{}\" rel=\"prev\" class=\"{BUTTON}\">{prev_label}</a>",
            e(url)
        )),
    }
    match next {
        Some(url) => out.push_str(&format!(
            "<a href=\"{}\" rel=\"next\" class=\"{BUTTON}\">{next_label}</a>",
            e(url)
        )),
        None => out.push_str(&format!("<span class=\"{DISABLED_BUTTON}\">{next_label}</span>")),
    }
}

/// Render the full Tailwind pagination links for a length-aware paginator.
pub fn render_tailwind<T>(paginator: &LengthAwarePaginator<T>) -> String {
    if !paginator.has_pages() {
        return String::new();
    }
    let previous = paginator.previous_page_url();
    let next = paginator.next_page_url();
    let prev_label = translate("pagination.previous");
    let next_label = translate("pagination.next");

    let mut out = format!(
        "<nav role=\"navigation\" aria-label=\"{}\">",
        e(translate("Pagination Navigation"))
    );

    // Small screens: just "Previous" and "Next".
    out.push_str("<div class=\"flex gap-2 items-center justify-between sm:hidden\">");
    previous_next_buttons(&mut out, previous.as_ref(), next.as_ref());
    out.push_str("</div>");

    out.push_str("<div class=\"hidden sm:flex-1 sm:flex sm:gap-2 sm:items-center sm:justify-between\">");
    out.push_str("<div><p class=\"text-sm text-gray-700 leading-5 dark:text-gray-600\">");
    out.push_str(&translate("Showing"));
    out.push(' ');
    match (paginator.first_item(), paginator.last_item()) {
        (Some(first), Some(last)) => out.push_str(&format!(
            "<span class=\"font-medium\">{first}</span> {} <span class=\"font-medium\">{last}</span>",
            translate("to")
        )),
        _ => out.push_str(&paginator.count().to_string()),
    }
    out.push_str(&format!(
        " {} <span class=\"font-medium\">{}</span> {}",
        translate("of"),
        paginator.total(),
        translate("results")
    ));
    out.push_str("</p></div>");

    out.push_str("<div><span class=\"inline-flex rtl:flex-row-reverse shadow-sm rounded-md\">");
    match &previous {
        None => out.push_str(&format!(
            "<span aria-disabled=\"true\" aria-label=\"{}\"><span class=\"{ARROW_DISABLED} rounded-l-md\" aria-hidden=\"true\">{PREV_ICON}</span></span>",
            e(&prev_label)
        )),
        Some(url) => out.push_str(&format!(
            "<a href=\"{}\" rel=\"prev\" class=\"{ARROW_LINK} rounded-l-md\" aria-label=\"{}\">{PREV_ICON}</a>",
            e(url),
            e(&prev_label)
        )),
    }

    for element in paginator.elements() {
        match element {
            Element::Dots => out.push_str(&format!(
                "<span aria-disabled=\"true\"><span class=\"{DOTS}\">...</span></span>"
            )),
            Element::Pages(pages) => {
                for (page, url) in pages {
                    if page == paginator.current_page() {
                        out.push_str(&format!(
                            "<span aria-current=\"page\"><span class=\"{CURRENT_PAGE}\">{page}</span></span>"
                        ));
                    } else {
                        let label = translate("Go to page :page").replace(":page", &page.to_string());
                        out.push_str(&format!(
                            "<a href=\"{}\" class=\"{PAGE_LINK}\" aria-label=\"{}\">{page}</a>",
                            e(&url),
                            e(&label)
                        ));
                    }
                }
            }
        }
    }

    match &next {
        Some(url) => out.push_str(&format!(
            "<a href=\"{}\" rel=\"next\" class=\"{ARROW_LINK} -ml-px rounded-r-md\" aria-label=\"{}\">{NEXT_ICON}</a>",
            e(url),
            e(&next_label)
        )),
        None => out.push_str(&format!(
            "<span aria-disabled=\"true\" aria-label=\"{}\"><span class=\"{ARROW_DISABLED} -ml-px rounded-r-md\" aria-hidden=\"true\">{NEXT_ICON}</span></span>",
            e(&next_label)
        )),
    }
    out.push_str("</span></div></div></nav>");
    out
}

/// Render "Previous" / "Next" links for a simple paginator.
pub fn render_simple_tailwind(has_pages: bool, previous: Option<String>, next: Option<String>) -> String {
    if !has_pages {
        return String::new();
    }
    let mut out = format!(
        "<nav role=\"navigation\" aria-label=\"{}\" class=\"flex gap-2 items-center justify-between\">",
        e(translate("Pagination Navigation"))
    );
    previous_next_buttons(&mut out, previous.as_ref(), next.as_ref());
    out.push_str("</nav>");
    out
}

/// What the built-in views need to know about a paginator.
pub(crate) struct LinkData {
    pub has_pages: bool,
    pub current_page: u64,
    pub previous: Option<String>,
    pub next: Option<String>,
    /// The page elements (length-aware paginators only).
    pub elements: Vec<Element>,
    /// `(first item, last item, total)` (length-aware paginators only).
    pub summary: Option<(Option<u64>, Option<u64>, u64)>,
}

impl LinkData {
    /// The data handed to custom pagination views.
    pub(crate) fn to_view_data(&self) -> illuminate_support::Value {
        use illuminate_support::{Map, Value, json};
        let elements: Vec<Value> = self
            .elements
            .iter()
            .map(|element| match element {
                Element::Dots => Value::String("...".into()),
                Element::Pages(pages) => Value::Object(
                    pages
                        .iter()
                        .map(|(page, url)| (page.to_string(), Value::String(url.clone())))
                        .collect::<Map<String, Value>>(),
                ),
            })
            .collect();
        let (first_item, last_item, total) = match self.summary {
            Some((first, last, total)) => (json!(first), json!(last), json!(total)),
            None => (Value::Null, Value::Null, Value::Null),
        };
        json!({
            "paginator": {
                "has_pages": self.has_pages,
                "current_page": self.current_page,
                "on_first_page": self.previous.is_none(),
                "has_more_pages": self.next.is_some(),
                "previous_page_url": self.previous,
                "next_page_url": self.next,
                "first_item": first_item,
                "last_item": last_item,
                "total": total,
            },
            "elements": elements,
        })
    }
}

/// The Bootstrap flavours Laravel ships views for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bootstrap {
    Three,
    Four,
    Five,
}

impl Bootstrap {
    fn item(self, state: &str) -> String {
        match (self, state) {
            (Bootstrap::Three, "") => "<li>".to_string(),
            (Bootstrap::Three, state) => format!("<li class=\"{state}\""),
            (_, "") => "<li class=\"page-item\">".to_string(),
            (_, state) => format!("<li class=\"page-item {state}\""),
        }
    }

    fn link_class(self) -> &'static str {
        match self {
            Bootstrap::Three => "",
            _ => " class=\"page-link\"",
        }
    }
}

/// Render the page links of a Bootstrap 3 or 4 view, or of the desktop
/// part of the Bootstrap 5 view.
fn bootstrap_page_links(out: &mut String, flavour: Bootstrap, data: &LinkData) {
    let prev_label = translate("pagination.previous");
    let next_label = translate("pagination.next");
    let class = flavour.link_class();
    out.push_str("<ul class=\"pagination\">");
    match &data.previous {
        None => out.push_str(&format!(
            "{} aria-disabled=\"true\" aria-label=\"{prev_label}\"><span{class} aria-hidden=\"true\">&lsaquo;</span></li>",
            flavour.item("disabled")
        )),
        Some(url) => out.push_str(&format!(
            "{}<a{class} href=\"{}\" rel=\"prev\" aria-label=\"{prev_label}\">&lsaquo;</a></li>",
            flavour.item(""),
            e(url)
        )),
    }
    for element in &data.elements {
        match element {
            Element::Dots => out.push_str(&format!(
                "{} aria-disabled=\"true\"><span{class}>...</span></li>",
                flavour.item("disabled")
            )),
            Element::Pages(pages) => {
                for (page, url) in pages {
                    if *page == data.current_page {
                        out.push_str(&format!(
                            "{} aria-current=\"page\"><span{class}>{page}</span></li>",
                            flavour.item("active")
                        ));
                    } else {
                        out.push_str(&format!(
                            "{}<a{class} href=\"{}\">{page}</a></li>",
                            flavour.item(""),
                            e(url)
                        ));
                    }
                }
            }
        }
    }
    match &data.next {
        Some(url) => out.push_str(&format!(
            "{}<a{class} href=\"{}\" rel=\"next\" aria-label=\"{next_label}\">&rsaquo;</a></li>",
            flavour.item(""),
            e(url)
        )),
        None => out.push_str(&format!(
            "{} aria-disabled=\"true\" aria-label=\"{next_label}\"><span{class} aria-hidden=\"true\">&rsaquo;</span></li>",
            flavour.item("disabled")
        )),
    }
    out.push_str("</ul>");
}

/// Render "Previous" / "Next" Bootstrap list items.
fn bootstrap_previous_next(out: &mut String, flavour: Bootstrap, data: &LinkData) {
    let prev_label = translate("pagination.previous");
    let next_label = translate("pagination.next");
    let class = flavour.link_class();
    out.push_str("<ul class=\"pagination\">");
    match &data.previous {
        None => out.push_str(&format!(
            "{} aria-disabled=\"true\"><span{class}>{prev_label}</span></li>",
            flavour.item("disabled")
        )),
        Some(url) => out.push_str(&format!(
            "{}<a{class} href=\"{}\" rel=\"prev\">{prev_label}</a></li>",
            flavour.item(""),
            e(url)
        )),
    }
    match &data.next {
        Some(url) => out.push_str(&format!(
            "{}<a{class} href=\"{}\" rel=\"next\">{next_label}</a></li>",
            flavour.item(""),
            e(url)
        )),
        None => out.push_str(&format!(
            "{} aria-disabled=\"true\"><span{class}>{next_label}</span></li>",
            flavour.item("disabled")
        )),
    }
    out.push_str("</ul>");
}

/// Render Laravel's `pagination::bootstrap-{3,4,5}` views.
pub(crate) fn render_bootstrap(flavour: Bootstrap, data: &LinkData) -> String {
    if !data.has_pages {
        return String::new();
    }
    if flavour != Bootstrap::Five {
        let mut out = "<nav>".to_string();
        bootstrap_page_links(&mut out, flavour, data);
        out.push_str("</nav>");
        return out;
    }

    let mut out = "<nav class=\"d-flex justify-items-center justify-content-between\">".to_string();
    out.push_str("<div class=\"d-flex justify-content-between flex-fill d-sm-none\">");
    bootstrap_previous_next(&mut out, flavour, data);
    out.push_str("</div>");
    out.push_str("<div class=\"d-none flex-sm-fill d-sm-flex align-items-sm-center justify-content-sm-between\">");
    let (first, last, total) = data.summary.unwrap_or((None, None, 0));
    out.push_str(&format!(
        "<div class=\"small text-muted\">{} <span class=\"fw-semibold\">{}</span> {} <span class=\"fw-semibold\">{}</span> {} <span class=\"fw-semibold\">{total}</span> {}</div>",
        translate("Showing"),
        first.map(|n| n.to_string()).unwrap_or_default(),
        translate("to"),
        last.map(|n| n.to_string()).unwrap_or_default(),
        translate("of"),
        translate("results"),
    ));
    out.push_str("<div>");
    bootstrap_page_links(&mut out, flavour, data);
    out.push_str("</div></div></nav>");
    out
}

/// Render Laravel's `pagination::simple-bootstrap-{3,4,5}` views.
pub(crate) fn render_simple_bootstrap(flavour: Bootstrap, data: &LinkData) -> String {
    if !data.has_pages {
        return String::new();
    }
    let mut out = if flavour == Bootstrap::Five {
        format!(
            "<nav role=\"navigation\" aria-label=\"{}\">",
            translate("Pagination Navigation")
        )
    } else {
        "<nav>".to_string()
    };
    bootstrap_previous_next(&mut out, flavour, data);
    out.push_str("</nav>");
    out
}

/// Which built-in view a view name refers to: `(flavour, simple)`, with
/// `None` as the flavour for Tailwind.
pub(crate) fn builtin_view(view: &str) -> Option<(Option<Bootstrap>, bool)> {
    Some(match view {
        "pagination::tailwind" => (None, false),
        "pagination::simple-tailwind" => (None, true),
        "pagination::bootstrap-3" | "pagination::default" => (Some(Bootstrap::Three), false),
        "pagination::simple-bootstrap-3" | "pagination::simple-default" => (Some(Bootstrap::Three), true),
        "pagination::bootstrap-4" => (Some(Bootstrap::Four), false),
        "pagination::simple-bootstrap-4" => (Some(Bootstrap::Four), true),
        "pagination::bootstrap-5" => (Some(Bootstrap::Five), false),
        "pagination::simple-bootstrap-5" => (Some(Bootstrap::Five), true),
        _ => return None,
    })
}
