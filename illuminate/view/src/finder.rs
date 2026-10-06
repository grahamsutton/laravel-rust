//! Finding view files: dot notation, namespaces, and extensions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use illuminate_support::Result;

use crate::exception::InvalidArgumentException;

/// The delimiter between a namespace and a view name (`mail::layout`).
pub const HINT_PATH_DELIMITER: &str = "::";

/// Locates view files on disk.
///
/// `"admin.profile"` is looked up as `admin/profile.blade.html` (then
/// `.blade.php`, then plain `.html`) in each registered path, in order;
/// `"mail::layout"` is looked up in the paths registered for the `mail`
/// namespace.
#[derive(Debug)]
pub struct FileViewFinder {
    paths: RwLock<Vec<PathBuf>>,
    hints: RwLock<HashMap<String, Vec<PathBuf>>>,
    extensions: RwLock<Vec<String>>,
    views: RwLock<HashMap<String, PathBuf>>,
}

impl FileViewFinder {
    /// Create a finder for the given paths.
    pub fn new(paths: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
        Self {
            paths: RwLock::new(paths.into_iter().map(|p| resolve(p.into())).collect()),
            hints: RwLock::new(HashMap::new()),
            extensions: RwLock::new(vec!["blade.html".into(), "blade.php".into(), "html".into()]),
            views: RwLock::new(HashMap::new()),
        }
    }

    /// Find the file for a view.
    pub fn find(&self, name: &str) -> Result<PathBuf> {
        let name = name.trim();
        if let Some(path) = name.strip_prefix("__path::") {
            return Ok(PathBuf::from(path));
        }
        if let Some(path) = self.views.read().unwrap().get(name)
            && path.is_file()
        {
            return Ok(path.clone());
        }
        let found = if name.contains(HINT_PATH_DELIMITER) {
            self.find_namespaced(name)?
        } else {
            let paths = self.paths.read().unwrap().clone();
            self.find_in_paths(name, &paths)?
        };
        self.views
            .write()
            .unwrap()
            .insert(name.to_string(), found.clone());
        Ok(found)
    }

    /// Determine if a view exists.
    pub fn exists(&self, name: &str) -> bool {
        self.find(name).is_ok()
    }

    fn find_namespaced(&self, name: &str) -> Result<PathBuf> {
        let segments: Vec<&str> = name.split(HINT_PATH_DELIMITER).collect();
        if segments.len() != 2 {
            return Err(InvalidArgumentException::new(format!(
                "View [{name}] has an invalid name."
            ))
            .into());
        }
        let hints = self
            .hints
            .read()
            .unwrap()
            .get(segments[0])
            .cloned()
            .ok_or_else(|| {
                InvalidArgumentException::new(format!(
                    "No hint path defined for [{}].",
                    segments[0]
                ))
            })?;
        self.find_in_paths(segments[1], &hints)
    }

    fn find_in_paths(&self, name: &str, paths: &[PathBuf]) -> Result<PathBuf> {
        for path in paths {
            if let Some(found) = self.find_in_directory(path, name) {
                return Ok(found);
            }
        }
        Err(InvalidArgumentException::new(format!("View [{name}] not found.")).into())
    }

    /// Find a view (in dot notation) inside a single directory.
    pub fn find_in_directory(&self, directory: &Path, name: &str) -> Option<PathBuf> {
        let relative = name.replace('.', "/");
        for extension in self.extensions.read().unwrap().iter() {
            let candidate = directory.join(format!("{relative}.{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    /// Add a location to the end of the search paths.
    pub fn add_location(&self, location: impl Into<PathBuf>) {
        self.paths.write().unwrap().push(resolve(location.into()));
    }

    /// Add a location to the start of the search paths.
    pub fn prepend_location(&self, location: impl Into<PathBuf>) {
        self.paths
            .write()
            .unwrap()
            .insert(0, resolve(location.into()));
        self.flush();
    }

    /// Add a namespace hint (`mail` → `resources/views/vendor/mail`).
    pub fn add_namespace(
        &self,
        namespace: &str,
        hints: impl IntoIterator<Item = impl Into<PathBuf>>,
    ) {
        let mut all = self.hints.write().unwrap();
        let entry = all.entry(namespace.to_string()).or_default();
        entry.extend(hints.into_iter().map(|p| resolve(p.into())));
    }

    /// Prepend namespace hints.
    pub fn prepend_namespace(
        &self,
        namespace: &str,
        hints: impl IntoIterator<Item = impl Into<PathBuf>>,
    ) {
        let mut all = self.hints.write().unwrap();
        let entry = all.entry(namespace.to_string()).or_default();
        let mut hints: Vec<PathBuf> = hints.into_iter().map(|p| resolve(p.into())).collect();
        hints.append(entry);
        *entry = hints;
        drop(all);
        self.flush();
    }

    /// Replace a namespace's hints.
    pub fn replace_namespace(
        &self,
        namespace: &str,
        hints: impl IntoIterator<Item = impl Into<PathBuf>>,
    ) {
        self.hints.write().unwrap().insert(
            namespace.to_string(),
            hints.into_iter().map(|p| resolve(p.into())).collect(),
        );
        self.flush();
    }

    /// Register an extension to search for (searched first).
    pub fn add_extension(&self, extension: &str) {
        let mut extensions = self.extensions.write().unwrap();
        extensions.retain(|e| e != extension);
        extensions.insert(0, extension.to_string());
        drop(extensions);
        self.flush();
    }

    /// The search paths.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.paths.read().unwrap().clone()
    }

    /// Replace the search paths.
    pub fn set_paths(&self, paths: impl IntoIterator<Item = impl Into<PathBuf>>) {
        *self.paths.write().unwrap() = paths.into_iter().map(|p| resolve(p.into())).collect();
        self.flush();
    }

    /// The namespace hints.
    pub fn hints(&self) -> HashMap<String, Vec<PathBuf>> {
        self.hints.read().unwrap().clone()
    }

    /// The extensions searched for, in order.
    pub fn extensions(&self) -> Vec<String> {
        self.extensions.read().unwrap().clone()
    }

    /// Forget the views that have been found.
    pub fn flush(&self) {
        self.views.write().unwrap().clear();
    }
}

fn resolve(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

/// Normalize a view name: `admin/profile` → `admin.profile`.
pub fn normalize_name(name: &str) -> String {
    match name.split_once(HINT_PATH_DELIMITER) {
        Some((namespace, view)) => {
            format!("{namespace}{HINT_PATH_DELIMITER}{}", view.replace('/', "."))
        }
        None => name.replace('/', "."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_finds_views_by_dot_notation_and_namespace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("admin")).unwrap();
        std::fs::write(dir.path().join("admin/profile.blade.html"), "x").unwrap();
        std::fs::write(dir.path().join("legacy.blade.php"), "x").unwrap();
        std::fs::write(dir.path().join("static.html"), "x").unwrap();
        let mail = tempfile::tempdir().unwrap();
        std::fs::write(mail.path().join("layout.blade.html"), "x").unwrap();

        let finder = FileViewFinder::new([dir.path()]);
        finder.add_namespace("mail", [mail.path()]);

        assert!(
            finder
                .find("admin.profile")
                .unwrap()
                .ends_with("admin/profile.blade.html")
        );
        assert!(finder.find("legacy").unwrap().ends_with("legacy.blade.php"));
        assert!(finder.find("static").unwrap().ends_with("static.html"));
        assert!(
            finder
                .find("mail::layout")
                .unwrap()
                .ends_with("layout.blade.html")
        );
        assert_eq!(
            finder.find("missing").unwrap_err().to_string(),
            "View [missing] not found."
        );
        assert_eq!(
            finder.find("nope::x").unwrap_err().to_string(),
            "No hint path defined for [nope]."
        );
        assert_eq!(normalize_name("admin/profile"), "admin.profile");
    }
}
