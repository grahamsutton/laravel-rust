//! Assertions about rendered views and components — Laravel's `TestView`
//! and `TestComponent`.

use std::fmt;

use illuminate_http::IntoResponse;
use illuminate_support::{Arr, Value};
use illuminate_view::{
    Component, ComponentAttributeBag, ComponentView, Factory, View, ViewInfo, ViewValue,
};

use super::content;
use super::test_response::{gather_view_data, view_has, view_has_all, view_has_with, view_missing};

/// Render a view, returning the HTML and the data it was rendered with
/// (after its composers ran). Rendering errors fail the test.
fn render(view: &View) -> (String, Value) {
    let response = view.clone().into_response();
    match response.extension::<ViewInfo>() {
        Some(info) => (response.content_string(), info.data.clone()),
        // Rendering failed and the response is an error page: render again
        // to surface the error itself.
        None => match view.render() {
            Ok(html) => (html, view.data_json()),
            Err(error) => panic!("{error:#}"),
        },
    }
}

fn fail(message: String) -> ! {
    panic!("{message}");
}

/// The content assertions shared by [`TestView`] and [`TestComponent`].
macro_rules! content_assertions {
    () => {
        /// The rendered HTML.
        pub fn rendered(&self) -> &str {
            &self.rendered
        }

        /// Assert the (escaped) text appears in the rendered output.
        pub fn assert_see(&self, value: &str) -> &Self {
            if let Err(message) = content::contains(&self.rendered, &illuminate_support::e(value)) {
                fail(message);
            }
            self
        }

        /// Assert the given HTML appears, unescaped, in the rendered output.
        pub fn assert_see_html(&self, value: &str) -> &Self {
            if let Err(message) = content::contains(&self.rendered, value) {
                fail(message);
            }
            self
        }

        /// Assert the given (escaped) strings appear in order.
        pub fn assert_see_in_order(&self, values: &[&str]) -> &Self {
            if let Err(message) =
                content::see_in_order(&self.rendered, &content::prepare(values, true))
            {
                fail(message);
            }
            self
        }

        /// Assert the given HTML strings appear, unescaped, in order.
        pub fn assert_see_html_in_order(&self, values: &[&str]) -> &Self {
            if let Err(message) =
                content::see_in_order(&self.rendered, &content::prepare(values, false))
            {
                fail(message);
            }
            self
        }

        /// Assert the text appears in the rendered output with tags stripped.
        pub fn assert_see_text(&self, value: &str) -> &Self {
            if let Err(message) = content::see_in_html(
                &self.rendered,
                &content::prepare(&[value], true),
                false,
                false,
            ) {
                fail(message);
            }
            self
        }

        /// Assert the given strings appear in order in the rendered text.
        pub fn assert_see_text_in_order(&self, values: &[&str]) -> &Self {
            if let Err(message) =
                content::see_in_html(&self.rendered, &content::prepare(values, true), true, false)
            {
                fail(message);
            }
            self
        }

        /// Assert the (escaped) text does not appear in the rendered output.
        pub fn assert_dont_see(&self, value: &str) -> &Self {
            if let Err(message) =
                content::not_contains(&self.rendered, &illuminate_support::e(value))
            {
                fail(message);
            }
            self
        }

        /// Assert the given HTML does not appear in the rendered output.
        pub fn assert_dont_see_html(&self, value: &str) -> &Self {
            if let Err(message) = content::not_contains(&self.rendered, value) {
                fail(message);
            }
            self
        }
    };
}

/// A rendered view, with assertions — what [`TestApp::view`] and
/// [`TestApp::blade`] return.
///
/// ```ignore
/// app.view("welcome", json!({"name": "Taylor"}))
///     .assert_see("Taylor")
///     .assert_view_has("name", json!("Taylor"));
///
/// let html = app.blade("<x-alert :type=\"$type\" />", json!({"type": "error"})).to_string();
/// ```
///
/// [`TestApp::view`]: super::TestApp::view
/// [`TestApp::blade`]: super::TestApp::blade
pub struct TestView {
    view: View,
    rendered: String,
    data: Value,
}

impl TestView {
    /// Render the view. Rendering errors fail the test.
    pub fn new(view: View) -> Self {
        let (rendered, data) = render(&view);
        Self {
            data: Value::Object(gather_view_data(&data)),
            view,
            rendered,
        }
    }

    /// The view that was rendered.
    pub fn view(&self) -> &View {
        &self.view
    }

    /// Get a piece of the view's data (`None` for all of it).
    pub fn view_data<'a>(&self, key: impl Into<Option<&'a str>>) -> Value {
        match key.into() {
            Some(key) => Arr::get(&self.data, key),
            None => self.data.clone(),
        }
    }

    content_assertions!();

    /// Assert the text does not appear in the rendered output with tags stripped.
    pub fn assert_dont_see_text(&self, value: &str) -> &Self {
        if let Err(message) = content::see_in_html(
            &self.rendered,
            &content::prepare(&[value], true),
            false,
            true,
        ) {
            fail(message);
        }
        self
    }

    /// Assert the view has a piece of data — and, when given, that it equals
    /// the value: `assert_view_has("name", json!("Taylor"))`.
    pub fn assert_view_has(&self, key: &str, value: impl Into<Option<Value>>) -> &Self {
        if let Err(message) = view_has(&self.data, key, value.into()) {
            fail(message);
        }
        self
    }

    /// Assert the view's data at `key` passes the truth test.
    pub fn assert_view_has_with(&self, key: &str, callback: impl FnOnce(&Value) -> bool) -> &Self {
        if let Err(message) = view_has_with(&self.data, key, callback) {
            fail(message);
        }
        self
    }

    /// Assert the view has all of the given data: an object of key/value
    /// pairs, or a list of keys.
    pub fn assert_view_has_all(&self, bindings: Value) -> &Self {
        if let Err(message) = view_has_all(&self.data, bindings) {
            fail(message);
        }
        self
    }

    /// Assert the view is missing a piece of data.
    pub fn assert_view_missing(&self, key: &str) -> &Self {
        if let Err(message) = view_missing(&self.data, key) {
            fail(message);
        }
        self
    }

    /// Assert the view rendered nothing.
    pub fn assert_view_empty(&self) -> &Self {
        if !self.rendered.is_empty() {
            fail(format!(
                "Failed asserting that '{}' is empty.",
                self.rendered
            ));
        }
        self
    }
}

impl fmt::Display for TestView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.rendered)
    }
}

impl fmt::Debug for TestView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestView")
            .field("view", &self.view.name())
            .field("rendered", &self.rendered)
            .finish()
    }
}

/// A rendered component, with assertions — what [`TestApp::component`]
/// returns.
///
/// ```ignore
/// app.component(Alert { kind: "error".into() })
///     .assert_see("alert-error");
/// ```
///
/// [`TestApp::component`]: super::TestApp::component
pub struct TestComponent<C> {
    component: C,
    rendered: String,
}

impl<C: Component> TestComponent<C> {
    /// Render the component on its own: its view (or inline template) with
    /// the component's data and an empty `$attributes` bag.
    pub fn new(component: C) -> Self {
        let factory = Factory::resolve();
        let mut data = component.data();
        data.entry("attributes".to_string())
            .or_insert_with(|| ViewValue::object(ComponentAttributeBag::new()));

        let view = match component.render() {
            ComponentView::View(name) => factory.make(&name, data),
            ComponentView::Inline(template) if factory.exists(&template) => {
                factory.make(&template, data)
            }
            ComponentView::Inline(template) => factory.inline(&template, data),
        };
        let (rendered, _) = render(&view);

        Self {
            component,
            rendered,
        }
    }

    /// The component that was rendered.
    pub fn component(&self) -> &C {
        &self.component
    }

    content_assertions!();

    /// Assert the text does not appear in the rendered output with tags stripped.
    pub fn assert_dont_see_text(&self, value: &str) -> &Self {
        let text = content::strip_tags(&self.rendered);
        if let Err(message) = content::not_contains(&text, &illuminate_support::e(value)) {
            fail(message);
        }
        self
    }
}

impl<C> fmt::Display for TestComponent<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.rendered)
    }
}

impl<C> fmt::Debug for TestComponent<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestComponent")
            .field("rendered", &self.rendered)
            .finish_non_exhaustive()
    }
}
