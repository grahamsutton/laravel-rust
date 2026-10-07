//! `make:*` — code generators.
//!
//! Generators create a new file from a stub and register it with Rust's
//! module system: `make:controller Admin/UserController` creates
//! `app/http/controllers/admin/user_controller.rs`, declares the `admin`
//! module, and re-exports `UserController`, so `crate::app::http::controllers::admin::UserController`
//! is ready to use.

pub mod commands;
pub mod component;
pub mod messaging;
pub mod migration;
pub mod model;
pub mod tables;
pub mod stubs;

use std::path::{Path, PathBuf};

use illuminate_support::{Result, Str, error::bail};

use crate::application::Application;

/// How a generated file is wired into the module tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Registration {
    /// Declare the module and re-export the type from the parent `mod.rs`.
    ModuleAndExport,
    /// Declare the module only.
    Module,
    /// Discovered automatically at build time (migrations, seeders, commands).
    Discovered,
    /// Not a Rust module (views, other files).
    None,
}

/// A parsed generator name: `Admin/UserController` => (`["admin"]`, `UserController`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualifiedName {
    pub namespace: Vec<String>,
    pub class: String,
}

impl QualifiedName {
    /// Parse a name given on the command line.
    pub fn parse(name: &str) -> Self {
        let normalized = name.trim().replace('\\', "/").replace("::", "/");
        let mut parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();
        let class = parts.pop().map(Str::studly).unwrap_or_default();
        Self {
            namespace: parts.iter().map(|p| Str::snake(&Str::studly(p))).collect(),
            class,
        }
    }

    /// The file stem for the class (`UserController` => `user_controller`).
    pub fn file_stem(&self) -> String {
        Str::snake(&self.class)
    }

    /// The Rust module path relative to the generator's base module.
    pub fn module_path(&self) -> String {
        let mut segments = self.namespace.clone();
        segments.push(self.file_stem());
        segments.join("::")
    }
}

/// Write a generated file and register it in the module tree.
pub fn generate(
    app: &Application,
    base_dir: &str,
    name: &QualifiedName,
    extension: &str,
    contents: &str,
    registration: Registration,
    force: bool,
) -> Result<PathBuf> {
    let mut dir = PathBuf::from(app.base_path(base_dir));
    for segment in &name.namespace {
        dir.push(segment);
    }
    let path = dir.join(format!("{}.{extension}", name.file_stem()));

    if path.exists() && !force {
        bail!("already exists");
    }

    std::fs::create_dir_all(&dir)?;
    std::fs::write(&path, contents)?;

    match registration {
        Registration::ModuleAndExport | Registration::Module => {
            let base = PathBuf::from(app.base_path(base_dir));
            register_module_chain(&base, &name.namespace)?;
            let module = name.file_stem();
            let mut lines = vec![format!("pub mod {module};")];
            if registration == Registration::ModuleAndExport {
                lines.push(format!("pub use {module}::{};", name.class));
            }
            append_lines(&dir.join("mod.rs"), &lines)?;
        }
        Registration::Discovered | Registration::None => {}
    }

    Ok(path)
}

/// Ensure every directory between `base` and the namespace has a `mod.rs`
/// and is declared by its parent.
fn register_module_chain(base: &Path, namespace: &[String]) -> Result<()> {
    ensure_module_declared(base)?;
    let mut current = base.to_path_buf();
    for segment in namespace {
        let parent_mod = current.join("mod.rs");
        current.push(segment);
        std::fs::create_dir_all(&current)?;
        if !current.join("mod.rs").exists() {
            std::fs::write(current.join("mod.rs"), "")?;
        }
        append_lines(&parent_mod, &[format!("pub mod {segment};")])?;
    }
    Ok(())
}

/// Make sure a base directory (e.g. `app/rules`) has a `mod.rs` and is
/// declared in its parent's `mod.rs` (e.g. `app/mod.rs`).
fn ensure_module_declared(dir: &Path) -> Result<()> {
    if dir.join("mod.rs").exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("mod.rs"), "")?;
    if let (Some(parent), Some(name)) = (dir.parent(), dir.file_name().and_then(|n| n.to_str())) {
        let parent_mod = parent.join("mod.rs");
        if parent_mod.exists() {
            append_lines(&parent_mod, &[format!("pub mod {name};")])?;
        } else if parent.join("lib.rs").exists() {
            append_lines(&parent.join("lib.rs"), &[format!("pub mod {name};")])?;
        }
    }
    Ok(())
}

/// Append lines to a file unless they're already present.
pub fn append_lines(path: &Path, lines: &[String]) -> Result<()> {
    let mut contents = std::fs::read_to_string(path).unwrap_or_default();
    let mut changed = false;
    for line in lines {
        if !contents.lines().any(|existing| existing.trim() == line.trim()) {
            if !contents.is_empty() && !contents.ends_with('\n') {
                contents.push('\n');
            }
            contents.push_str(line);
            contents.push('\n');
            changed = true;
        }
    }
    if changed {
        std::fs::write(path, contents)?;
    }
    Ok(())
}

/// Replace `{{ placeholder }}` markers in a stub.
pub fn populate(stub: &str, replacements: &[(&str, &str)]) -> String {
    let mut out = stub.to_string();
    for (key, value) in replacements {
        out = out.replace(&format!("{{{{ {key} }}}}"), value);
        out = out.replace(&format!("{{{{{key}}}}}"), value);
    }
    out
}

/// A path relative to the application's base path, for display.
pub fn relative(app: &Application, path: &Path) -> String {
    let base = app.base_path("");
    path.to_string_lossy()
        .strip_prefix(&base)
        .map(|p| p.trim_start_matches(['/', '\\']).to_string())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_parses_names() {
        let name = QualifiedName::parse("Admin/UserController");
        assert_eq!(name.namespace, vec!["admin"]);
        assert_eq!(name.class, "UserController");
        assert_eq!(name.file_stem(), "user_controller");
        assert_eq!(QualifiedName::parse("Api\\V1\\PostController").namespace, vec!["api", "v1"]);
        assert_eq!(QualifiedName::parse("send_emails").class, "SendEmails");
    }

    #[test]
    fn it_populates_stubs() {
        assert_eq!(populate("pub struct {{ class }};", &[("class", "Post")]), "pub struct Post;");
    }

    #[test]
    fn it_generates_and_registers_modules() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app")).unwrap();
        std::fs::write(dir.path().join("app/mod.rs"), "pub mod models;\n").unwrap();
        let app = Application::new_detached(dir.path());

        let name = QualifiedName::parse("Admin/Uppercase");
        let path = generate(&app, "app/rules", &name, "rs", "// rule", Registration::ModuleAndExport, false).unwrap();
        assert!(path.ends_with("app/rules/admin/uppercase.rs"));
        assert!(std::fs::read_to_string(dir.path().join("app/mod.rs")).unwrap().contains("pub mod rules;"));
        assert!(std::fs::read_to_string(dir.path().join("app/rules/mod.rs")).unwrap().contains("pub mod admin;"));
        let leaf = std::fs::read_to_string(dir.path().join("app/rules/admin/mod.rs")).unwrap();
        assert!(leaf.contains("pub mod uppercase;") && leaf.contains("pub use uppercase::Uppercase;"));

        assert!(generate(&app, "app/rules", &name, "rs", "", Registration::ModuleAndExport, false).is_err());
    }
}
