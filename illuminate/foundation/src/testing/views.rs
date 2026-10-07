//! Render views, Blade templates and components directly in tests —
//! Laravel's `InteractsWithViews`.

use illuminate_support::{MessageBag, Value, ValueExt};
use illuminate_view::{Component, Factory, IntoViewData, ViewErrorBag, ViewValue};

use super::{TestApp, TestComponent, TestView};

impl TestApp {
    /// Render a view without making a request:
    ///
    /// ```ignore
    /// app.view("welcome", json!({"name": "Taylor"})).assert_see("Taylor");
    /// ```
    pub fn view(&self, name: &str, data: impl IntoViewData) -> TestView {
        TestView::new(Factory::resolve().make(name, data))
    }

    /// Render a raw Blade template:
    ///
    /// ```ignore
    /// app.blade("<x-greeting :name=\"$name\" />", json!({"name": "Taylor"})).assert_see("Taylor");
    /// ```
    pub fn blade(&self, template: &str, data: impl IntoViewData) -> TestView {
        TestView::new(Factory::resolve().inline(template, data))
    }

    /// Render a class-based component on its own:
    ///
    /// ```ignore
    /// app.component(Profile { name: "Taylor".into() }).assert_see("Taylor");
    /// ```
    pub fn component<C: Component>(&self, component: C) -> TestComponent<C> {
        TestComponent::new(component)
    }

    /// Share validation errors with every view rendered afterwards, as the
    /// `$errors` bag:
    ///
    /// ```ignore
    /// app.with_view_errors(json!({"name": ["Please provide a valid name."]}))
    ///     .view("form", ())
    ///     .assert_see("Please provide a valid name.");
    /// ```
    pub fn with_view_errors(&mut self, errors: Value) -> &mut Self {
        self.with_view_errors_in(errors, "default")
    }

    /// Share validation errors in a named error bag with every view.
    pub fn with_view_errors_in(&mut self, errors: Value, bag: &str) -> &mut Self {
        let mut messages = MessageBag::new();
        if let Value::Object(errors) = errors {
            for (key, value) in errors {
                match value {
                    Value::Array(items) => {
                        for message in items {
                            messages.add(key.clone(), message.to_string_lossy());
                        }
                    }
                    Value::Null => {}
                    message => {
                        messages.add(key.clone(), message.to_string_lossy());
                    }
                }
            }
        }
        Factory::resolve().share(
            "errors",
            ViewValue::object(ViewErrorBag::new().put(bag, messages)),
        );
        self
    }
}
