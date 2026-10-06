//! Compiling route URIs into regular expressions.
//!
//! This follows the rules of Symfony's route compiler, which Laravel uses
//! under the hood: parameters match `[^/]+` by default, constraints added
//! with `where` replace that default, and trailing optional parameters
//! (`{name?}`) may be left off entirely.

use indexmap::IndexMap;
use regex::Regex;

use crate::exceptions::InvalidRouteException;

/// The characters Symfony treats as separators between route tokens.
const SEPARATORS: &str = "/,;.:-_~+*=@|";

/// A route URI (and optional domain) compiled into regular expressions.
#[derive(Debug, Clone)]
pub struct CompiledRoute {
    regex: Regex,
    host_regex: Option<Regex>,
    path_variables: Vec<String>,
    host_variables: Vec<String>,
}

#[derive(Debug)]
enum Token {
    Text(String),
    Variable {
        separator: String,
        requirement: String,
        name: String,
    },
}

impl CompiledRoute {
    /// Compile the given URI, domain, constraints and optional parameters.
    pub fn compile(
        uri: &str,
        domain: Option<&str>,
        wheres: &IndexMap<String, String>,
        optional: &[String],
    ) -> Result<Self, InvalidRouteException> {
        let optional_marker = Regex::new(r"\{(\w+)\?\}").expect("valid optional regex");
        let uri = optional_marker.replace_all(uri, "{$1}");
        let path = format!("/{}", uri.trim_start_matches('/'));
        let (regex, path_variables) = compile_pattern(&path, wheres, optional, false)
            .map_err(|reason| InvalidRouteException {
                uri: uri.to_string(),
                reason,
            })?;

        let (host_regex, host_variables) = match domain.filter(|d| !d.is_empty()) {
            Some(domain) => {
                let (regex, variables) = compile_pattern(domain, wheres, &[], true).map_err(|reason| {
                    InvalidRouteException {
                        uri: domain.to_string(),
                        reason,
                    }
                })?;
                (Some(regex), variables)
            }
            None => (None, Vec::new()),
        };

        Ok(Self {
            regex,
            host_regex,
            path_variables,
            host_variables,
        })
    }

    /// The compiled path regular expression.
    pub fn regex(&self) -> &Regex {
        &self.regex
    }

    /// The compiled host regular expression, for routes with a domain.
    pub fn host_regex(&self) -> Option<&Regex> {
        self.host_regex.as_ref()
    }

    /// The names of the variables in the path.
    pub fn path_variables(&self) -> &[String] {
        &self.path_variables
    }

    /// The names of the variables in the host.
    pub fn host_variables(&self) -> &[String] {
        &self.host_variables
    }

    /// Determine if the given (decoded) path matches.
    pub fn matches_path(&self, path: &str) -> bool {
        self.regex.is_match(path)
    }

    /// Determine if the given host matches (always true without a domain).
    pub fn matches_host(&self, host: &str) -> bool {
        self.host_regex.as_ref().is_none_or(|regex| regex.is_match(host))
    }

    /// Match the path and host, returning the bound parameters: host
    /// parameters first, then path parameters, skipping empty values.
    pub fn bind(&self, path: &str, host: &str) -> Option<IndexMap<String, String>> {
        let mut parameters = IndexMap::new();

        if let Some(regex) = &self.host_regex {
            let captures = regex.captures(host)?;
            for name in &self.host_variables {
                if let Some(value) = captures.name(name).filter(|m| !m.as_str().is_empty()) {
                    parameters.insert(name.clone(), value.as_str().to_string());
                }
            }
        }

        let captures = self.regex.captures(path)?;
        for name in &self.path_variables {
            if let Some(value) = captures.name(name).filter(|m| !m.as_str().is_empty()) {
                parameters.insert(name.clone(), value.as_str().to_string());
            }
        }

        Some(parameters)
    }
}

/// Normalize an incoming request path the way Laravel's URI validator does:
/// trailing slashes are ignored and the root is always `/`.
pub fn normalize_path(decoded_path: &str) -> String {
    let trimmed = decoded_path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn variable_regex() -> Regex {
    Regex::new(r"\{(\w+)\}").expect("valid variable regex")
}

fn compile_pattern(
    pattern: &str,
    wheres: &IndexMap<String, String>,
    optional: &[String],
    is_host: bool,
) -> Result<(Regex, Vec<String>), String> {
    let default_separator = if is_host { "." } else { "/" };
    let variables = variable_regex();

    let mut tokens = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut position = 0;

    for captures in variables.captures_iter(pattern) {
        let whole = captures.get(0).expect("whole match");
        let name = captures[1].to_string();

        if names.contains(&name) {
            return Err(format!(
                "Route pattern \"{pattern}\" cannot reference variable name \"{name}\" more than once."
            ));
        }

        let preceding = &pattern[position..whole.start()];
        position = whole.end();

        let (text, separator) = match preceding.chars().last() {
            Some(last) if SEPARATORS.contains(last) => {
                (&preceding[..preceding.len() - last.len_utf8()], last.to_string())
            }
            _ => (preceding, String::new()),
        };

        if !text.is_empty() {
            tokens.push(Token::Text(text.to_string()));
        }

        let requirement = match wheres.get(&name) {
            Some(requirement) => strip_anchors(requirement),
            None => {
                let following = &pattern[position..];
                let next_separator = next_separator(following, &variables);
                let mut class = regex::escape(default_separator);
                if !next_separator.is_empty() && next_separator != default_separator {
                    class.push_str(&regex::escape(&next_separator));
                }
                format!("[^{class}]+")
            }
        };

        tokens.push(Token::Variable {
            separator,
            requirement,
            name: name.clone(),
        });
        names.push(name);
    }

    if position < pattern.len() {
        tokens.push(Token::Text(pattern[position..].to_string()));
    }

    // Trailing variables with defaults (optional parameters) may be omitted.
    let mut first_optional = usize::MAX;
    if !is_host {
        for (index, token) in tokens.iter().enumerate().rev() {
            match token {
                Token::Variable { name, .. } if optional.contains(name) => first_optional = index,
                _ => break,
            }
        }
    }

    let count = tokens.len();
    let mut regex = String::from("^");
    for (index, token) in tokens.iter().enumerate() {
        match token {
            Token::Text(text) => regex.push_str(&regex::escape(text)),
            Token::Variable {
                separator,
                requirement,
                name,
            } => {
                let separator = regex::escape(separator);
                if index == 0 && first_optional == 0 {
                    // When the only token is an optional variable, the separator is required.
                    regex.push_str(&format!("{separator}(?P<{name}>{requirement})?"));
                } else {
                    let mut part = format!("{separator}(?P<{name}>{requirement})");
                    if index >= first_optional {
                        part = format!("(?:{part}");
                        if index == count - 1 {
                            let closing = count - first_optional - usize::from(first_optional == 0);
                            part.push_str(&")?".repeat(closing));
                        }
                    }
                    regex.push_str(&part);
                }
            }
        }
    }
    regex.push('$');

    let regex = if is_host { format!("(?i){regex}") } else { regex };

    Regex::new(&regex)
        .map(|compiled| (compiled, names))
        .map_err(|error| error.to_string())
}

/// The separator immediately following a variable, if any.
fn next_separator(following: &str, variables: &Regex) -> String {
    let without_variables = variables.replace_all(following, "");
    match without_variables.chars().next() {
        Some(first) if SEPARATORS.contains(first) => first.to_string(),
        _ => String::new(),
    }
}

/// Constraints are matched against a single segment of the pattern, so any
/// anchors a developer adds are redundant (Symfony strips them too).
fn strip_anchors(requirement: &str) -> String {
    let requirement = requirement.strip_prefix('^').unwrap_or(requirement);
    let requirement = match requirement.strip_suffix('$') {
        Some(stripped) if !stripped.ends_with('\\') => stripped,
        _ => requirement,
    };
    if requirement.is_empty() {
        ".*".to_string()
    } else {
        requirement.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(uri: &str, wheres: &[(&str, &str)], optional: &[&str]) -> CompiledRoute {
        let wheres = wheres
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let optional: Vec<String> = optional.iter().map(|s| s.to_string()).collect();
        CompiledRoute::compile(uri, None, &wheres, &optional).unwrap()
    }

    #[test]
    fn static_routes_match_exactly() {
        let route = compile("users/profile", &[], &[]);
        assert!(route.matches_path("/users/profile"));
        assert!(!route.matches_path("/users"));
        assert!(!route.matches_path("/users/profile/edit"));
        assert!(compile("/", &[], &[]).matches_path("/"));
    }

    #[test]
    fn parameters_match_a_single_segment_by_default() {
        let route = compile("users/{id}", &[], &[]);
        assert_eq!(route.regex().as_str(), "^/users/(?P<id>[^/]+)$");
        let params = route.bind("/users/42", "localhost").unwrap();
        assert_eq!(params.get("id").map(String::as_str), Some("42"));
        assert!(!route.matches_path("/users/42/posts"));
        assert!(!route.matches_path("/users"));
    }

    #[test]
    fn trailing_optional_parameters_may_be_omitted() {
        let route = compile("users/{name}", &[], &["name"]);
        assert!(route.matches_path("/users"));
        assert!(route.matches_path("/users/taylor"));
        assert!(route.bind("/users", "").unwrap().is_empty());

        let only = compile("{page}", &[], &["page"]);
        assert!(only.matches_path("/"));
        assert!(only.matches_path("/about"));

        let two = compile("archive/{year}/{month}", &[], &["year", "month"]);
        assert!(two.matches_path("/archive"));
        assert!(two.matches_path("/archive/2024"));
        assert!(two.matches_path("/archive/2024/05"));
    }

    #[test]
    fn constraints_replace_the_default_requirement() {
        let route = compile("users/{id}", &[("id", "[0-9]+")], &[]);
        assert!(route.matches_path("/users/1"));
        assert!(!route.matches_path("/users/taylor"));

        let search = compile("search/{search}", &[("search", ".*")], &[]);
        let params = search.bind("/search/a/b/c", "").unwrap();
        assert_eq!(params["search"], "a/b/c");
    }

    #[test]
    fn separators_limit_the_default_requirement() {
        let route = compile("files/{name}.{ext}", &[], &[]);
        let params = route.bind("/files/report.pdf", "").unwrap();
        assert_eq!(params["name"], "report");
        assert_eq!(params["ext"], "pdf");
    }

    #[test]
    fn domains_capture_parameters() {
        let route = CompiledRoute::compile(
            "users/{id}",
            Some("{account}.example.com"),
            &IndexMap::new(),
            &[],
        )
        .unwrap();
        let params = route.bind("/users/1", "Acme.Example.com").unwrap();
        assert_eq!(params.keys().collect::<Vec<_>>(), vec!["account", "id"]);
        assert_eq!(params["account"], "Acme");
        assert!(route.bind("/users/1", "example.org").is_none());
    }

    #[test]
    fn anchors_in_constraints_are_ignored() {
        let route = compile("users/{id}", &[("id", "^[0-9]+$")], &[]);
        assert!(route.matches_path("/users/12"));
    }

    #[test]
    fn invalid_constraints_are_reported() {
        let error = CompiledRoute::compile(
            "users/{id}",
            None,
            &[("id".to_string(), "[0-9".to_string())].into_iter().collect(),
            &[],
        )
        .unwrap_err();
        assert!(error.to_string().contains("Unable to compile the route [users/{id}]"));
    }

    #[test]
    fn paths_are_normalized() {
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path("/users/"), "/users");
        assert_eq!(normalize_path(""), "/");
    }
}
