#![allow(dead_code)]

use std::path::Path;

use illuminate_view::{Factory, IntoViewData};
use tempfile::TempDir;

/// A temporary views directory with a factory pointed at it.
pub struct Views {
    pub dir: TempDir,
    pub factory: Factory,
}

impl Views {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let factory = Factory::new([dir.path()]);
        Self { dir, factory }
    }

    /// Write a view file (`name` in dot notation, `.blade.html` added).
    pub fn add(&self, name: &str, contents: &str) -> &Self {
        self.add_file(&format!("{}.blade.html", name.replace('.', "/")), contents)
    }

    /// Write a file relative to the views directory.
    pub fn add_file(&self, relative: &str, contents: &str) -> &Self {
        let path = self.dir.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
        self
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Render a view by name.
    pub fn render(&self, name: &str, data: impl IntoViewData) -> String {
        match self.factory.make(name, data).render() {
            Ok(html) => html,
            Err(e) => panic!("rendering [{name}] failed: {e:#}"),
        }
    }

    /// Render an inline template.
    pub fn inline(&self, template: &str, data: impl IntoViewData) -> String {
        match self.factory.render_inline(template, data) {
            Ok(html) => html,
            Err(e) => panic!("rendering failed: {e:#}\n--- template ---\n{template}"),
        }
    }

    /// Render an inline template, expecting an error.
    pub fn inline_err(&self, template: &str, data: impl IntoViewData) -> illuminate_support::Error {
        match self.factory.render_inline(template, data) {
            Ok(html) => panic!("expected an error, got: {html}"),
            Err(e) => e,
        }
    }
}

/// Render an inline template with a fresh factory.
pub fn blade(template: &str, data: impl IntoViewData) -> String {
    Views::new().inline(template, data)
}
