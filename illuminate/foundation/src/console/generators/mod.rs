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
pub mod resource;
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

/// Add lines to a file unless they're already present. Module declarations
/// and re-exports go into their own blocks, in the order rustfmt keeps
/// them; anything else is appended.
pub fn append_lines(path: &Path, lines: &[String]) -> Result<()> {
    let original = std::fs::read_to_string(path).unwrap_or_default();
    let mut contents = original.clone();
    for line in lines {
        if !contents.lines().any(|existing| existing.trim() == line.trim()) {
            contents = insert_line(&contents, line);
        }
    }
    if contents != original {
        std::fs::write(path, contents)?;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Module,
    Export,
    Other,
}

fn line_kind(line: &str) -> LineKind {
    let line = line.trim();
    if !line.ends_with(';') {
        LineKind::Other
    } else if line.starts_with("pub mod ") || line.starts_with("mod ") {
        LineKind::Module
    } else if line.starts_with("pub use ") {
        LineKind::Export
    } else {
        LineKind::Other
    }
}

/// What a declaration is sorted by: the module name or the re-exported path.
fn sort_key(line: &str) -> &str {
    let line = line.trim();
    line.strip_prefix("pub mod ")
        .or_else(|| line.strip_prefix("mod "))
        .or_else(|| line.strip_prefix("pub use "))
        .unwrap_or(line)
}

fn insert_line(contents: &str, line: &str) -> String {
    let mut lines: Vec<String> = contents.lines().map(str::to_string).collect();
    let kind = line_kind(line);
    let last_of = |lines: &[String], kind: LineKind| lines.iter().rposition(|l| line_kind(l) == kind);

    if kind == LineKind::Other {
        lines.push(line.to_string());
    } else if let Some(last) = last_of(&lines, kind) {
        // Keep the block sorted.
        let mut start = last;
        while start > 0 && line_kind(&lines[start - 1]) == kind {
            start -= 1;
        }
        let at = (start..=last)
            .find(|&i| sort_key(&lines[i]) > sort_key(line))
            .unwrap_or(last + 1);
        lines.insert(at, line.to_string());
    } else {
        // A new block: modules go after the file's `//!` docs and
        // attributes, re-exports after the modules.
        let header = lines
            .iter()
            .take_while(|l| l.starts_with("//!") || l.starts_with("#!["))
            .count();
        let at = match kind {
            LineKind::Export => last_of(&lines, LineKind::Module).map_or(header, |i| i + 1),
            _ => header,
        };
        let mut block = vec![line.to_string()];
        if at < lines.len() && !lines[at].trim().is_empty() {
            block.push(String::new());
        }
        if at > 0 && !lines[at - 1].trim().is_empty() {
            block.insert(0, String::new());
        }
        lines.splice(at..at, block);
    }

    let mut out = lines.join("\n");
    out.push('\n');
    out
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
    fn declarations_are_inserted_into_sorted_blocks() {
        let models = "pub mod user;\n\npub use user::User;\n";
        let models = insert_line(models, "pub mod podcast;");
        let models = insert_line(&models, "pub use podcast::Podcast;");
        assert_eq!(models, "pub mod podcast;\npub mod user;\n\npub use podcast::Podcast;\npub use user::User;\n");

        let controllers = "//! Your application's controllers.\n";
        let controllers = insert_line(controllers, "pub mod podcast_controller;");
        let controllers = insert_line(&controllers, "pub use podcast_controller::PodcastController;");
        assert_eq!(
            controllers,
            "//! Your application's controllers.\n\npub mod podcast_controller;\n\npub use podcast_controller::PodcastController;\n"
        );

        let fresh = insert_line("", "pub mod process_podcast;");
        let fresh = insert_line(&fresh, "pub use process_podcast::ProcessPodcast;");
        assert_eq!(fresh, "pub mod process_podcast;\n\npub use process_podcast::ProcessPodcast;\n");

        let tests = "mod example_test;\n\nuse laravel::testing::TestApp;\n\npub fn app() {}\n";
        assert_eq!(
            insert_line(tests, "mod podcast_test;"),
            "mod example_test;\nmod podcast_test;\n\nuse laravel::testing::TestApp;\n\npub fn app() {}\n"
        );

        let app = "pub mod http;\npub mod models;\npub mod providers;\n";
        assert_eq!(
            insert_line(app, "pub mod jobs;"),
            "pub mod http;\npub mod jobs;\npub mod models;\npub mod providers;\n"
        );
    }

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
