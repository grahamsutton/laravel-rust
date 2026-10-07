//! Vite: load your application's compiled assets, or the dev server's
//! hot-reloading ones, with a single `@vite` directive.
//!
//! ```blade
//! <head>
//!     @vite(['resources/css/app.css', 'resources/js/app.js'])
//! </head>
//! ```
//!
//! When `public/hot` exists (written by `npm run dev`), assets are served by
//! the Vite dev server. Otherwise they're read from the build manifest in
//! `public/build/manifest.json`.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock, RwLock};

use indexmap::IndexMap;

use illuminate_container::Container;
use illuminate_support::{HtmlString, Map, Result, Str, Value, ValueExt};

use crate::helpers::public_path;

/// The Vite manifest could not be found (run `npm run build`).
#[derive(Debug, Clone)]
pub struct ViteManifestNotFoundException(pub String);

impl fmt::Display for ViteManifestNotFoundException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Vite manifest not found at: {}", self.0)
    }
}

impl std::error::Error for ViteManifestNotFoundException {}

/// An asset could not be found in the Vite manifest.
#[derive(Debug, Clone)]
pub struct ViteException(pub String);

impl fmt::Display for ViteException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ViteException {}

/// Manifests are read once per path.
type Manifest = Arc<Map<String, Value>>;

static MANIFESTS: LazyLock<RwLock<HashMap<String, Manifest>>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// Per-application Vite configuration, stored in the container.
#[derive(Debug)]
struct ViteState {
    nonce: Option<String>,
    integrity_key: Option<String>,
    entry_points: Vec<String>,
    hot_file: Option<String>,
    build_directory: String,
    manifest_filename: String,
    script_tag_attributes: Map<String, Value>,
    style_tag_attributes: Map<String, Value>,
    preload_tag_attributes: Map<String, Value>,
    preloaded_assets: IndexMap<String, Vec<String>>,
    faked: bool,
}

impl Default for ViteState {
    fn default() -> Self {
        Self {
            nonce: None,
            integrity_key: Some("integrity".to_string()),
            entry_points: Vec::new(),
            hot_file: None,
            build_directory: "build".to_string(),
            manifest_filename: "manifest.json".to_string(),
            script_tag_attributes: Map::new(),
            style_tag_attributes: Map::new(),
            preload_tag_attributes: Map::new(),
            preloaded_assets: IndexMap::new(),
            faked: false,
        }
    }
}

#[derive(Default)]
struct ViteStore(RwLock<ViteState>);

fn store() -> Arc<ViteStore> {
    let container = Container::get_instance();
    container.singleton_if::<ViteStore>(|_| Arc::new(ViteStore::default()));
    container.make::<ViteStore>()
}

fn read<T>(callback: impl FnOnce(&ViteState) -> T) -> T {
    callback(&store().0.read().unwrap())
}

fn write<T>(callback: impl FnOnce(&mut ViteState) -> T) -> T {
    callback(&mut store().0.write().unwrap())
}

/// The Vite facade.
pub struct Vite;

impl Vite {
    /// Generate the tags for the given entry points (`@vite([...])`).
    pub fn render(entry_points: &[&str]) -> Result<HtmlString> {
        Self::render_from(entry_points, None)
    }

    /// Generate the tags for the given entry points from a specific build
    /// directory.
    pub fn render_from(entry_points: &[&str], build_directory: Option<&str>) -> Result<HtmlString> {
        if read(|s| s.faked) {
            return Ok(HtmlString::new(""));
        }
        let mut entry_points: Vec<String> = entry_points.iter().map(|e| e.to_string()).collect();
        if entry_points.is_empty() {
            entry_points = read(|s| s.entry_points.clone());
        }
        let build_directory = build_directory
            .map(str::to_string)
            .unwrap_or_else(|| read(|s| s.build_directory.clone()));

        if Self::is_running_hot() {
            let tags: String = std::iter::once("@vite/client".to_string())
                .chain(entry_points)
                .map(|entry| make_tag(&hot_asset(&entry), None, &[]))
                .collect();
            return Ok(HtmlString::new(tags));
        }

        let manifest = manifest(&build_directory)?;
        let integrity = read(|s| s.integrity_key.clone());

        let mut tags: Vec<String> = Vec::new();
        let mut preloads: Vec<(String, Option<Map<String, Value>>)> = Vec::new();

        let css_chunk = |css: &str| -> Option<Map<String, Value>> {
            manifest.values().find_map(|chunk| match chunk {
                Value::Object(map) if map.get("file").and_then(Value::as_str) == Some(css) => Some(map.clone()),
                _ => None,
            })
        };

        for entry in &entry_points {
            let chunk = chunk(&manifest, entry)?;
            let file = chunk_file(&chunk);
            preloads.push((asset_path(&format!("{build_directory}/{file}")), Some(chunk.clone())));

            for import in resolve_imports(&manifest, &chunk) {
                let Some(Value::Object(imported)) = manifest.get(&import) else {
                    continue;
                };
                preloads.push((
                    asset_path(&format!("{build_directory}/{}", chunk_file(imported))),
                    Some(imported.clone()),
                ));
                for css in strings(imported.get("css")) {
                    let url = asset_path(&format!("{build_directory}/{css}"));
                    let css_chunk = css_chunk(&css);
                    tags.push(make_tag(&url, css_chunk.as_ref(), &integrity_attribute(&integrity, css_chunk.as_ref())));
                    preloads.push((url, css_chunk));
                }
            }

            let url = asset_path(&format!("{build_directory}/{file}"));
            tags.push(make_tag(&url, Some(&chunk), &integrity_attribute(&integrity, Some(&chunk))));

            for css in strings(chunk.get("css")) {
                let url = asset_path(&format!("{build_directory}/{css}"));
                let css_chunk = css_chunk(&css);
                tags.push(make_tag(&url, css_chunk.as_ref(), &integrity_attribute(&integrity, css_chunk.as_ref())));
                preloads.push((url, css_chunk));
            }
        }

        let mut seen = std::collections::HashSet::new();
        tags.retain(|tag| seen.insert(tag.clone()));
        let (stylesheets, scripts): (Vec<String>, Vec<String>) =
            tags.into_iter().partition(|tag| tag.starts_with("<link"));

        let mut seen = std::collections::HashSet::new();
        preloads.retain(|(url, _)| seen.insert(url.clone()));
        // Stylesheets are preloaded first (a stable sort keeps the order).
        preloads.sort_by_key(|(url, _)| !is_css_path(url));
        let preloads: String = preloads
            .iter()
            .map(|(url, chunk)| make_preload_tag(url, chunk.as_ref(), &integrity))
            .collect();

        Ok(HtmlString::new(format!("{preloads}{}{}", stylesheets.concat(), scripts.concat())))
    }

    /// The React Refresh runtime script, when the dev server is running
    /// (`@viteReactRefresh`).
    pub fn react_refresh() -> Option<HtmlString> {
        if !Self::is_running_hot() || read(|s| s.faked) {
            return None;
        }
        let nonce = parse_attributes(&nonce_attribute());
        Some(HtmlString::new(format!(
            "<script type=\"module\" {}>\n    import RefreshRuntime from '{}'\n    RefreshRuntime.injectIntoGlobalHook(window)\n    window.$RefreshReg$ = () => {{}}\n    window.$RefreshSig$ = () => (type) => type\n    window.__vite_plugin_react_preamble_installed__ = true\n</script>",
            nonce.join(" "),
            hot_asset("@react-refresh")
        )))
    }

    /// The URL of an asset processed by Vite (`Vite::asset('resources/images/logo.png')`).
    pub fn asset(asset: &str) -> Result<String> {
        Self::asset_from(asset, None)
    }

    /// The URL of an asset in a specific build directory.
    pub fn asset_from(asset: &str, build_directory: Option<&str>) -> Result<String> {
        if Self::is_running_hot() {
            return Ok(hot_asset(asset));
        }
        let build_directory = build_directory
            .map(str::to_string)
            .unwrap_or_else(|| read(|s| s.build_directory.clone()));
        let chunk = chunk(&*manifest(&build_directory)?, asset)?;
        Ok(asset_path(&format!("{build_directory}/{}", chunk_file(&chunk))))
    }

    /// The contents of a built asset (for inlining).
    pub fn content(asset: &str) -> Result<String> {
        let build_directory = read(|s| s.build_directory.clone());
        let chunk = chunk(&*manifest(&build_directory)?, asset)?;
        let path = public_path(&format!("{build_directory}/{}", chunk_file(&chunk)));
        std::fs::read_to_string(&path)
            .map_err(|_| ViteException(format!("Unable to locate file from Vite manifest: {path}.")).into())
    }

    /// Determine if the Vite dev server is running (`public/hot` exists).
    pub fn is_running_hot() -> bool {
        std::path::Path::new(&Self::hot_file()).is_file()
    }

    /// The URL of the Vite dev server.
    pub fn dev_server_url() -> Option<String> {
        if !Self::is_running_hot() {
            return None;
        }
        std::fs::read_to_string(Self::hot_file()).ok().map(|url| url.trim_end().to_string())
    }

    /// The path of the "hot" file.
    pub fn hot_file() -> String {
        read(|s| s.hot_file.clone()).unwrap_or_else(|| public_path("hot"))
    }

    /// An MD5 hash of the manifest, for cache busting.
    pub fn manifest_hash() -> Option<String> {
        if Self::is_running_hot() {
            return None;
        }
        let build_directory = read(|s| s.build_directory.clone());
        let contents = std::fs::read(manifest_path(&build_directory)).ok()?;
        use md5::Digest as _;
        Some(hex::encode(md5::Md5::digest(&contents)))
    }

    /// Generate (or use) a Content Security Policy nonce for the tags.
    pub fn use_csp_nonce(nonce: Option<&str>) -> String {
        let nonce = nonce.map(str::to_string).unwrap_or_else(|| Str::random(40));
        write(|s| s.nonce = Some(nonce.clone()));
        nonce
    }

    /// The Content Security Policy nonce, if any.
    pub fn csp_nonce() -> Option<String> {
        read(|s| s.nonce.clone())
    }

    /// The manifest key holding Subresource Integrity hashes (`None` disables SRI).
    pub fn use_integrity_key(key: Option<&str>) {
        write(|s| s.integrity_key = key.map(str::to_string));
    }

    /// Set the default entry points for `@vite` without arguments.
    pub fn with_entry_points(entry_points: &[&str]) {
        write(|s| s.entry_points = entry_points.iter().map(|e| e.to_string()).collect());
    }

    /// Use a different build directory (relative to `public`).
    pub fn use_build_directory(path: &str) {
        write(|s| s.build_directory = path.trim_matches('/').to_string());
    }

    /// Use a different "hot" file.
    pub fn use_hot_file(path: &str) {
        write(|s| s.hot_file = Some(path.to_string()));
    }

    /// Use a different manifest file name.
    pub fn use_manifest_filename(filename: &str) {
        write(|s| s.manifest_filename = filename.to_string());
    }

    /// Add attributes to every generated `<script>` tag.
    pub fn use_script_tag_attributes(attributes: Value) {
        write(|s| s.script_tag_attributes.extend(object(attributes)));
    }

    /// Add attributes to every generated stylesheet `<link>` tag.
    pub fn use_style_tag_attributes(attributes: Value) {
        write(|s| s.style_tag_attributes.extend(object(attributes)));
    }

    /// Add attributes to every generated preload `<link>` tag.
    pub fn use_preload_tag_attributes(attributes: Value) {
        write(|s| s.preload_tag_attributes.extend(object(attributes)));
    }

    /// The assets preloaded by the tags generated so far (for `Link` headers).
    pub fn preloaded_assets() -> IndexMap<String, Vec<String>> {
        read(|s| s.preloaded_assets.clone())
    }

    /// Render nothing from `@vite`, for tests (`$this->withoutVite()`).
    pub fn fake() {
        write(|s| s.faked = true);
    }

    /// Reset the Vite state.
    pub fn flush() {
        write(|s| {
            s.preloaded_assets.clear();
            s.nonce = None;
        });
    }
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn hot_asset(asset: &str) -> String {
    format!("{}/{asset}", Vite::dev_server_url().unwrap_or_default())
}

fn asset_path(path: &str) -> String {
    illuminate_routing::asset(path)
}

fn manifest_path(build_directory: &str) -> String {
    public_path(&format!("{build_directory}/{}", read(|s| s.manifest_filename.clone())))
}

fn manifest(build_directory: &str) -> Result<Arc<Map<String, Value>>> {
    let path = manifest_path(build_directory);
    if let Some(manifest) = MANIFESTS.read().unwrap().get(&path) {
        return Ok(manifest.clone());
    }
    let contents = std::fs::read_to_string(&path).map_err(|_| ViteManifestNotFoundException(path.clone()))?;
    let manifest = Arc::new(object(serde_json::from_str(&contents)?));
    MANIFESTS.write().unwrap().insert(path, manifest.clone());
    Ok(manifest)
}

fn chunk(manifest: &Map<String, Value>, file: &str) -> Result<Map<String, Value>> {
    match manifest.get(file) {
        Some(Value::Object(chunk)) => Ok(chunk.clone()),
        _ => Err(ViteException(format!("Unable to locate file in Vite manifest: {file}.")).into()),
    }
}

fn chunk_file(chunk: &Map<String, Value>) -> String {
    chunk.get("file").map(|f| f.to_string_lossy()).unwrap_or_default()
}

fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items.iter().map(|item| item.to_string_lossy()).collect(),
        _ => Vec::new(),
    }
}

fn resolve_imports(manifest: &Map<String, Value>, chunk: &Map<String, Value>) -> Vec<String> {
    fn walk(
        manifest: &Map<String, Value>,
        chunk: &Map<String, Value>,
        seen: &mut std::collections::HashSet<String>,
        out: &mut Vec<String>,
    ) {
        for import in strings(chunk.get("imports")) {
            if !seen.insert(import.clone()) {
                continue;
            }
            out.push(import.clone());
            if let Some(Value::Object(imported)) = manifest.get(&import) {
                walk(manifest, imported, seen, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(manifest, chunk, &mut std::collections::HashSet::new(), &mut out);
    out
}

fn is_css_path(path: &str) -> bool {
    static PATTERN: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\.(css|less|sass|scss|styl|stylus|pcss|postcss)(\?[^\.]*)?$").unwrap());
    PATTERN.is_match(path)
}

fn nonce_attribute() -> Vec<(String, Value)> {
    vec![("nonce".to_string(), Vite::csp_nonce().map(Value::String).unwrap_or(Value::Bool(false)))]
}

fn integrity_attribute(key: &Option<String>, chunk: Option<&Map<String, Value>>) -> Vec<(String, Value)> {
    match key {
        Some(key) => vec![(
            "integrity".to_string(),
            chunk.and_then(|c| c.get(key)).cloned().unwrap_or(Value::Bool(false)),
        )],
        None => Vec::new(),
    }
}

/// Build a `<script>` or stylesheet `<link>` tag.
fn make_tag(url: &str, _chunk: Option<&Map<String, Value>>, extra: &[(String, Value)]) -> String {
    let css = is_css_path(url);
    let mut attributes: Vec<(String, Value)> = if css {
        vec![("rel".into(), "stylesheet".into()), ("href".into(), url.into())]
    } else {
        vec![("type".into(), "module".into()), ("src".into(), url.into())]
    };
    attributes.extend(nonce_attribute());
    attributes.extend(extra.iter().cloned());
    let configured = read(|s| if css { s.style_tag_attributes.clone() } else { s.script_tag_attributes.clone() });
    merge(&mut attributes, configured);
    let attributes = parse_attributes(&attributes).join(" ");
    if css {
        format!("<link {attributes} />")
    } else {
        format!("<script {attributes}></script>")
    }
}

/// Build a `<link rel="preload">` / `<link rel="modulepreload">` tag.
fn make_preload_tag(url: &str, chunk: Option<&Map<String, Value>>, integrity: &Option<String>) -> String {
    let mut attributes: Vec<(String, Value)> = if is_css_path(url) {
        vec![("rel".into(), "preload".into()), ("as".into(), "style".into())]
    } else {
        vec![("rel".into(), "modulepreload".into()), ("as".into(), "script".into())]
    };
    attributes.push(("href".into(), url.into()));
    attributes.extend(nonce_attribute());
    let crossorigin = read(|s| {
        let source = if is_css_path(url) { &s.style_tag_attributes } else { &s.script_tag_attributes };
        source.get("crossorigin").cloned().unwrap_or(Value::Bool(false))
    });
    attributes.push(("crossorigin".into(), crossorigin));
    attributes.extend(integrity_attribute(integrity, chunk));
    merge(&mut attributes, read(|s| s.preload_tag_attributes.clone()));

    let without_href: Vec<(String, Value)> = attributes.iter().filter(|(k, _)| k != "href").cloned().collect();
    write(|s| s.preloaded_assets.insert(url.to_string(), parse_attributes(&without_href)));

    format!("<link {} />", parse_attributes(&attributes).join(" "))
}

fn merge(attributes: &mut Vec<(String, Value)>, extra: Map<String, Value>) {
    for (key, value) in extra {
        match attributes.iter_mut().find(|(k, _)| *k == key) {
            Some(existing) => existing.1 = value,
            None => attributes.push((key, value)),
        }
    }
}

/// `[("defer", true), ("nonce", false), ("id", "x")]` => `["defer", "id=\"x\""]`.
fn parse_attributes(attributes: &[(String, Value)]) -> Vec<String> {
    attributes
        .iter()
        .filter(|(_, value)| !matches!(value, Value::Null | Value::Bool(false)))
        .map(|(key, value)| match value {
            Value::Bool(true) => key.clone(),
            other => format!("{key}=\"{}\"", other.to_string_lossy()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_detects_css_paths() {
        assert!(is_css_path("resources/css/app.css"));
        assert!(is_css_path("app.scss?inline"));
        assert!(!is_css_path("resources/js/app.js"));
    }

    #[test]
    fn it_parses_attributes() {
        let attributes = vec![
            ("defer".to_string(), Value::Bool(true)),
            ("nonce".to_string(), Value::Bool(false)),
            ("id".to_string(), Value::String("x".into())),
        ];
        assert_eq!(parse_attributes(&attributes), vec!["defer".to_string(), "id=\"x\"".to_string()]);
    }
}
