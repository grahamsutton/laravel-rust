//! Which skeleton files belong in a new application.
//!
//! The rules mirror the skeleton's `.gitignore` files, so the installer
//! embeds exactly what `git ls-files skeleton` lists: build output,
//! dependencies, local environment files, logs, and databases stay behind.
//! Under `storage/` and `bootstrap/cache/` only the `.gitignore` files are
//! kept, which recreates the directory structure without runtime leftovers.
//!
//! This file is shared with `build.rs` (through `#[path]`), so it must not
//! depend on anything outside `std`.

/// Directories (relative to the skeleton, `/`-separated) that are never copied.
const IGNORED_DIRECTORIES: &[&str] = &[
    "target",
    "node_modules",
    "public/build",
    "public/storage",
    ".git",
    ".idea",
    ".vscode",
    ".zed",
];

/// Files (relative to the skeleton) that are never copied.
const IGNORED_FILES: &[&str] = &[
    ".env",
    ".env.backup",
    ".env.production",
    "public/hot",
    "public/storage",
    "Cargo.lock",
];

/// Directories whose contents are runtime state: only `.gitignore` is kept.
const STATE_DIRECTORIES: &[&str] = &["storage", "bootstrap/cache"];

/// Determine whether a skeleton directory should be skipped entirely.
pub fn is_ignored_directory(path: &str) -> bool {
    IGNORED_DIRECTORIES.contains(&path)
}

/// Determine whether a skeleton file belongs in a new application.
pub fn should_embed(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);

    if IGNORED_FILES.contains(&path) || name == ".DS_Store" || name == "Thumbs.db" {
        return false;
    }

    if IGNORED_DIRECTORIES
        .iter()
        .any(|directory| path.starts_with(&format!("{directory}/")))
    {
        return false;
    }

    if name.ends_with(".log") || name.contains(".sqlite") {
        return false;
    }

    if let Some(rest) = path.strip_prefix("storage/")
        && !rest.contains('/')
        && name.ends_with(".key")
    {
        return false;
    }

    if STATE_DIRECTORIES
        .iter()
        .any(|directory| path.starts_with(&format!("{directory}/")))
    {
        return name == ".gitignore";
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_files_are_embedded() {
        for path in [
            "Cargo.toml",
            ".env.example",
            ".gitignore",
            ".cargo/config.toml",
            "app/models/user.rs",
            "database/.gitignore",
            "database/migrations/0001_01_01_000000_create_users_table.rs",
            "public/favicon.ico",
            "resources/views/welcome.blade.html",
            "storage/app/.gitignore",
            "storage/framework/cache/data/.gitignore",
            "bootstrap/cache/.gitignore",
            "bootstrap/app.rs",
        ] {
            assert!(should_embed(path), "{path} should be embedded");
        }
    }

    #[test]
    fn ignored_files_stay_behind() {
        for path in [
            ".env",
            ".env.backup",
            "storage/logs/laravel.log",
            "database/database.sqlite",
            "database/database.sqlite-journal",
            "storage/oauth-private.key",
            "storage/framework/sessions/abc123",
            "storage/framework/views/compiled.html",
            "bootstrap/cache/routes.json",
            "public/hot",
            "public/build/manifest.json",
            "node_modules/vite/package.json",
            "target/debug/artisan",
            "app/.DS_Store",
            "npm-debug.log",
            "Cargo.lock",
        ] {
            assert!(!should_embed(path), "{path} should not be embedded");
        }
    }

    #[test]
    fn build_and_dependency_directories_are_skipped() {
        for path in ["target", "node_modules", "public/build", ".git"] {
            assert!(is_ignored_directory(path), "{path} should be skipped");
        }
        for path in ["app", "public", "storage", "resources/js"] {
            assert!(!is_ignored_directory(path), "{path} should be walked");
        }
    }
}
