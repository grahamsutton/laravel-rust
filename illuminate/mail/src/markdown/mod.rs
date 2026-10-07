//! Markdown mailables: Laravel's mail components, Markdown to HTML
//! conversion, and CSS inlining.
//!
//! Markdown mail templates are Blade views that use the framework's mail
//! components:
//!
//! ```blade
//! <x-mail::message>
//! # Order Shipped
//!
//! Your order has been shipped!
//!
//! <x-mail::button :url="$url">
//! View Order
//! </x-mail::button>
//!
//! Thanks,<br>
//! {{ config('app.name') }}
//! </x-mail::message>
//! ```
//!
//! The same template renders twice: once with the HTML components (the
//! result is converted from Markdown and styled with the theme's CSS,
//! inlined into the markup) and once with the plain-text components.
//!
//! The components (`message`, `layout`, `header`, `footer`, `button`,
//! `panel`, `table`, `subcopy`) and the default theme are built in. To
//! customize them, copy them into a directory listed in
//! `mail.markdown.paths` (Laravel's `resources/views/vendor/mail`): a
//! component found at `{path}/html/button.blade.html` (or `.blade.php`)
//! overrides the built-in one, as does a theme at
//! `{path}/html/themes/{theme}.css`. Custom themes may also live at
//! `resources/views/mail/{theme}.css`; choose one with
//! `mail.markdown.theme`.

mod commonmark;
mod css_inliner;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use illuminate_config::Repository;
use illuminate_container::try_app;
use illuminate_support::{HtmlString, Result, Str};
use illuminate_view::{
    BladeCompiler, Component, ComponentArgs, ComponentView, Factory, IntoViewData, ViewData,
    ViewValue,
};
use regex::Regex;

pub use commonmark::to_html;
pub use css_inliner::inline_css;

/// The default mail theme's stylesheet.
pub const DEFAULT_THEME: &str = include_str!("../../resources/views/html/themes/default.css");

/// The components, their HTML and text templates, and the attributes they
/// take as props.
const COMPONENTS: &[(&str, &str, &str, &[&str])] = &[
    (
        "message",
        include_str!("../../resources/views/html/message.blade.html"),
        include_str!("../../resources/views/text/message.blade.html"),
        &[],
    ),
    (
        "layout",
        include_str!("../../resources/views/html/layout.blade.html"),
        include_str!("../../resources/views/text/layout.blade.html"),
        &[],
    ),
    (
        "header",
        include_str!("../../resources/views/html/header.blade.html"),
        include_str!("../../resources/views/text/header.blade.html"),
        &["url"],
    ),
    (
        "footer",
        include_str!("../../resources/views/html/footer.blade.html"),
        include_str!("../../resources/views/text/footer.blade.html"),
        &[],
    ),
    (
        "button",
        include_str!("../../resources/views/html/button.blade.html"),
        include_str!("../../resources/views/text/button.blade.html"),
        &["url", "color", "align"],
    ),
    (
        "panel",
        include_str!("../../resources/views/html/panel.blade.html"),
        include_str!("../../resources/views/text/panel.blade.html"),
        &[],
    ),
    (
        "subcopy",
        include_str!("../../resources/views/html/subcopy.blade.html"),
        include_str!("../../resources/views/text/subcopy.blade.html"),
        &[],
    ),
    (
        "table",
        include_str!("../../resources/views/html/table.blade.html"),
        include_str!("../../resources/views/text/table.blade.html"),
        &[],
    ),
];

/// Which set of components is being rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Html,
    Text,
}

#[derive(Clone)]
struct RenderState {
    mode: Mode,
    paths: Vec<PathBuf>,
}

thread_local! {
    /// Blade renders synchronously, so the components being rendered (HTML
    /// or text) are tracked per thread for the duration of a render.
    static STATE: RefCell<Vec<RenderState>> = const { RefCell::new(Vec::new()) };
}

struct StateGuard;

impl Drop for StateGuard {
    fn drop(&mut self) {
        STATE.with(|state| {
            state.borrow_mut().pop();
        });
    }
}

fn current_state() -> RenderState {
    STATE.with(|state| {
        state.borrow().last().cloned().unwrap_or(RenderState {
            mode: Mode::Html,
            paths: Vec::new(),
        })
    })
}

/// A mail component (`<x-mail::button>`, ...).
struct MailComponent {
    view: ComponentView,
    data: ViewData,
}

impl Component for MailComponent {
    fn render(&self) -> ComponentView {
        self.view.clone()
    }

    fn data(&self) -> ViewData {
        self.data.clone()
    }
}

/// Find a template file in a directory, by name, with any Blade extension.
fn find_template(directory: &Path, name: &str) -> Option<PathBuf> {
    ["blade.html", "blade.php", "html"]
        .iter()
        .map(|extension| directory.join(format!("{name}.{extension}")))
        .find(|path| path.is_file())
}

fn register_components(blade: &BladeCompiler) {
    blade.function("Markdown::parse", |args| {
        let text = args
            .first()
            .map(ViewValue::to_string_lossy)
            .unwrap_or_default();
        Ok(ViewValue::html(to_html(&text)))
    });
    for (name, html, text, props) in COMPONENTS {
        let (name, html, text, props) = (*name, *html, *text, *props);
        blade.component(&format!("mail::{name}"), move |args: &mut ComponentArgs| {
            let state = current_state();
            let mut data = ViewData::new();
            for prop in props {
                if let Some(value) = args.take(prop) {
                    data.insert((*prop).to_string(), value);
                }
            }
            let directory = match state.mode {
                Mode::Html => "html",
                Mode::Text => "text",
            };
            let custom = state
                .paths
                .iter()
                .find_map(|path| find_template(&path.join(directory), name));
            let view = match custom {
                Some(path) => ComponentView::view(format!("__path::{}", path.display())),
                None => ComponentView::inline(match state.mode {
                    Mode::Html => html,
                    Mode::Text => text,
                }),
            };
            Ok(MailComponent { view, data })
        });
    }
}

static NEWLINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\r\n]{2,}").unwrap());
static ENTITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"&(?:#[xX][a-fA-F0-9]{1,6}|#[0-9]{1,7}|[a-zA-Z][a-zA-Z0-9]{1,31});").unwrap()
});

/// Decode HTML entities, like PHP's `html_entity_decode`.
pub fn html_entity_decode(text: &str) -> String {
    ENTITY
        .replace_all(text, |captures: &regex::Captures<'_>| {
            commonmark::decode_entity(&captures[0]).unwrap_or_else(|| captures[0].to_string())
        })
        .into_owned()
}

/// The Markdown mail renderer.
///
/// ```
/// use illuminate_mail::Markdown;
/// use illuminate_support::json;
/// use illuminate_view::Factory;
///
/// let markdown = Markdown::new(Factory::new(Vec::<String>::new()));
///
/// let template = "<x-mail::message>\n# Hello {{ $name }}\n\n<x-mail::button :url=\"$url\">\nGo\n</x-mail::button>\n</x-mail::message>";
/// let data = json!({"name": "Taylor", "url": "https://laravel.com"});
///
/// let html = markdown.render_template(template, data.clone()).unwrap();
/// assert!(html.contains("<h1 style=\""));
/// assert!(html.contains(">Hello Taylor</h1>"));
/// assert!(html.contains("href=\"https://laravel.com\""));
///
/// let text = markdown.render_text_template(template, data).unwrap();
/// assert!(text.contains("# Hello Taylor"));
/// assert!(text.contains("Go: https://laravel.com"));
/// ```
#[derive(Clone, Debug)]
pub struct Markdown {
    factory: Factory,
    theme: String,
    paths: Vec<PathBuf>,
}

impl Markdown {
    /// Create a renderer using the given view factory and the default theme.
    pub fn new(factory: Factory) -> Self {
        Self::register(&factory);
        Self {
            factory,
            theme: "default".to_string(),
            paths: Vec::new(),
        }
    }

    /// Create a renderer configured from `mail.markdown.theme` and
    /// `mail.markdown.paths`.
    pub fn from_config(factory: Factory, config: &Repository) -> Self {
        let mut markdown = Self::new(factory);
        markdown.theme = config.string_or("mail.markdown.theme", "default");
        markdown.paths = config
            .strings("mail.markdown.paths")
            .into_iter()
            .map(PathBuf::from)
            .collect();
        markdown
    }

    /// Resolve a renderer for the application's view factory and configuration.
    pub fn resolve() -> Self {
        let factory = Factory::resolve();
        match try_app::<Repository>() {
            Some(config) => Self::from_config(factory, &config),
            None => Self::new(factory),
        }
    }

    /// Register the mail components and the `Markdown::parse` template
    /// function with a view factory's Blade compiler. This is idempotent.
    pub fn register(factory: &Factory) {
        if !factory.blade().has_function("Markdown::parse") {
            register_components(factory.blade());
        }
    }

    /// Set the theme to use (`default`, or the name of a custom theme).
    pub fn theme(mut self, theme: impl Into<String>) -> Self {
        let theme = theme.into();
        if !theme.is_empty() {
            self.theme = theme;
        }
        self
    }

    /// Get the theme currently being used by the renderer.
    pub fn get_theme(&self) -> &str {
        &self.theme
    }

    /// Register directories holding customized mail components (each with
    /// `html` and `text` subdirectories).
    pub fn load_components_from(
        mut self,
        paths: impl IntoIterator<Item = impl Into<PathBuf>>,
    ) -> Self {
        self.paths = paths.into_iter().map(Into::into).collect();
        self
    }

    /// The registered component paths.
    pub fn component_paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// The view factory.
    pub fn factory(&self) -> &Factory {
        &self.factory
    }

    fn with_state<R>(&self, mode: Mode, render: impl FnOnce() -> R) -> R {
        STATE.with(|state| {
            state.borrow_mut().push(RenderState {
                mode,
                paths: self.paths.clone(),
            });
        });
        let _guard = StateGuard;
        render()
    }

    /// Render a Markdown mail view into HTML (with the theme's CSS inlined).
    pub fn render(&self, view: &str, data: impl IntoViewData) -> Result<String> {
        let data = data.into_view_data();
        self.factory.flush_finder_cache();
        let contents = self.with_state(Mode::Html, || {
            self.factory.make(view, data.clone()).render()
        })?;
        self.finish_html(&contents, data)
    }

    /// Render a Markdown mail template string into HTML.
    pub fn render_template(&self, template: &str, data: impl IntoViewData) -> Result<String> {
        let data = data.into_view_data();
        let contents = self.with_state(Mode::Html, || {
            self.factory.render_inline(template, data.clone())
        })?;
        self.finish_html(&contents, data)
    }

    /// Render a Markdown mail view into plain text.
    pub fn render_text(&self, view: &str, data: impl IntoViewData) -> Result<String> {
        self.factory.flush_finder_cache();
        let contents = self.with_state(Mode::Text, || self.factory.make(view, data).render())?;
        Ok(finish_text(&contents))
    }

    /// Render a Markdown mail template string into plain text.
    pub fn render_text_template(&self, template: &str, data: impl IntoViewData) -> Result<String> {
        let contents =
            self.with_state(Mode::Text, || self.factory.render_inline(template, data))?;
        Ok(finish_text(&contents))
    }

    /// Parse the given Markdown text into HTML.
    ///
    /// ```
    /// use illuminate_mail::Markdown;
    ///
    /// assert_eq!(Markdown::parse("**Hi**").to_html(), "<p><strong>Hi</strong></p>\n");
    /// ```
    pub fn parse(text: &str) -> HtmlString {
        HtmlString::new(to_html(text))
    }

    fn finish_html(&self, contents: &str, data: ViewData) -> Result<String> {
        let css = self.theme_css(data)?;
        Ok(inline_css(&contents.replace("\\[", "["), &css))
    }

    /// The CSS of the current theme.
    fn theme_css(&self, data: ViewData) -> Result<String> {
        let custom = Str::start(&self.theme, "mail.");
        if self.factory.exists(&custom) {
            return self.factory.make(&custom, data).render();
        }
        if self.theme.contains("::") {
            return self.factory.make(&self.theme, data).render();
        }
        let file_name = format!("{}.css", self.theme);
        let candidates = self
            .factory
            .finder()
            .paths()
            .into_iter()
            .map(|path| path.join("mail").join(&file_name))
            .chain(
                self.paths
                    .iter()
                    .map(|path| path.join("html").join("themes").join(&file_name)),
            );
        for candidate in candidates {
            if candidate.is_file() {
                return Ok(std::fs::read_to_string(candidate)?);
            }
        }
        Ok(DEFAULT_THEME.to_string())
    }
}

fn finish_text(contents: &str) -> String {
    html_entity_decode(&NEWLINES.replace_all(contents, "\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn markdown() -> Markdown {
        Markdown::new(Factory::new(Vec::<String>::new()))
    }

    const TEMPLATE: &str = "<x-mail::message>\n# Order Shipped\n\nYour order #{{ $id }} has shipped & is on its way!\n\n<x-mail::button :url=\"$url\" color=\"success\">\nView Order\n</x-mail::button>\n\n<x-mail::panel>\nThis is the panel content.\n</x-mail::panel>\n\n<x-mail::table>\n| Laravel | Table |\n| ------- | ----: |\n| Col 2   | $10   |\n</x-mail::table>\n\nThanks,<br>\nThe Team\n</x-mail::message>\n";

    #[test]
    fn markdown_mail_renders_html_with_inlined_styles() {
        let html = markdown()
            .render_template(
                TEMPLATE,
                json!({"id": 42, "url": "https://example.com/orders/42?a=1&b=2"}),
            )
            .unwrap();
        assert!(html.starts_with("<!DOCTYPE html"));
        // Markdown was converted and styled.
        assert!(html.contains(">Order Shipped</h1>"), "{html}");
        let h1 = &html[html.find("<h1 style=\"").unwrap()..];
        let h1 = &h1[..h1.find('>').unwrap()];
        assert!(
            h1.contains("font-size: 18px;") && h1.contains("box-sizing: border-box;"),
            "{h1}"
        );
        assert!(html.contains("Your order #42 has shipped &amp; is on its way!"));
        // The button keeps its markup and gets the theme's styles.
        assert!(html.contains("href=\"https://example.com/orders/42?a=1&amp;b=2\""));
        assert!(html.contains("class=\"button button-success\""));
        assert!(html.contains("background-color: #16a34a;"));
        // Panels and tables are converted too.
        assert!(html.contains("<p style=\"") && html.contains(">This is the panel content.</p>"));
        assert!(html.contains("<th style=\""));
        assert!(html.contains(">Laravel</th>"));
        assert!(html.contains("<td align=\"right\" style=\""));
        // The layout's media queries stay in a style block.
        assert!(html.contains("@media only screen and (max-width: 600px)"));
        assert!(html.contains("Thanks,<br>"));
        assert!(html.contains("All rights reserved."));
    }

    #[test]
    fn markdown_mail_renders_plain_text() {
        let text = markdown()
            .render_text_template(
                TEMPLATE,
                json!({"id": 42, "url": "https://example.com/orders/42?a=1&b=2"}),
            )
            .unwrap();
        assert!(text.contains("# Order Shipped"));
        assert!(text.contains("Your order #42 has shipped & is on its way!"));
        assert!(text.contains("View Order: https://example.com/orders/42?a=1&b=2"));
        assert!(text.contains("This is the panel content."));
        assert!(text.contains("| Laravel | Table |"));
        assert!(!text.contains("<table"));
        assert!(!text.contains("<br>"), "tags are stripped: {text}");
        assert!(!text.contains("\n\n\n"));
    }

    #[test]
    fn custom_components_and_themes_override_the_defaults() {
        let views = tempfile::tempdir().unwrap();
        let vendor = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(vendor.path().join("html/themes")).unwrap();
        std::fs::create_dir_all(vendor.path().join("text")).unwrap();
        std::fs::write(
            vendor.path().join("html/button.blade.html"),
            "@props(['url'])\n<a class=\"custom\" href=\"{{ $url }}\">{{ $slot }}</a>",
        )
        .unwrap();
        std::fs::write(
            vendor.path().join("text/button.blade.html"),
            "[{{ $slot }}]({{ $url }})",
        )
        .unwrap();
        std::fs::write(
            vendor.path().join("html/themes/brand.css"),
            ".custom { color: hotpink; }",
        )
        .unwrap();

        let markdown = Markdown::new(Factory::new([views.path()]))
            .load_components_from([vendor.path()])
            .theme("brand");
        assert_eq!(markdown.get_theme(), "brand");
        let template = "<x-mail::button url=\"/go\">Go</x-mail::button>";
        let html = markdown.render_template(template, json!({})).unwrap();
        assert_eq!(
            html,
            "<a class=\"custom\" href=\"/go\" style=\"color: hotpink;\">Go</a>"
        );
        assert_eq!(
            markdown.render_text_template(template, json!({})).unwrap(),
            "[Go](/go)"
        );

        // A theme in resources/views/mail wins.
        std::fs::create_dir_all(views.path().join("mail")).unwrap();
        std::fs::write(
            views.path().join("mail/brand.css"),
            ".custom { color: red; }",
        )
        .unwrap();
        let html = markdown.render_template(template, json!({})).unwrap();
        assert!(html.contains("style=\"color: red;\""));
    }

    #[test]
    fn markdown_views_render_from_files() {
        let views = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(views.path().join("mail")).unwrap();
        std::fs::write(
            views.path().join("mail/welcome.blade.html"),
            "<x-mail::message>\nWelcome, **{{ $name }}**!\n</x-mail::message>",
        )
        .unwrap();
        let markdown = Markdown::new(Factory::new([views.path()]));
        let html = markdown
            .render("mail.welcome", json!({"name": "Abigail"}))
            .unwrap();
        assert!(html.contains(">Abigail</strong>!</p>"));
        let text = markdown
            .render_text("mail.welcome", json!({"name": "Abigail"}))
            .unwrap();
        assert!(text.contains("Welcome, **Abigail**!"));
    }

    #[test]
    fn entities_are_decoded_like_php() {
        assert_eq!(
            html_entity_decode("Tom &amp; Jerry &#039;s &lt;3 &unknown;"),
            "Tom & Jerry 's <3 &unknown;"
        );
    }

    #[test]
    fn parse_converts_markdown() {
        assert_eq!(Markdown::parse("# Hi").to_html(), "<h1>Hi</h1>\n");
    }
}
