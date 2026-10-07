//! Laravel Precognition: anticipate the outcome of a future HTTP request.
//!
//! A *precognitive* request runs a route's middleware and resolves its
//! handler's arguments — validating form requests along the way — but never
//! runs the handler itself. That lets your frontend offer "live" validation
//! without duplicating your backend validation rules.
//!
//! The `precognitive` route middleware marks requests carrying the
//! `Precognition: true` header as precognitive; everything else in the
//! framework (the router, the validator, the session) simply asks the
//! request:
//!
//! ```
//! use illuminate_http::Request;
//!
//! let request = Request::create("/users", "POST");
//! request.set_header("Precognition", "true");
//!
//! assert!(request.is_attempting_precognition());
//! assert!(!request.is_precognitive());
//!
//! // What the `precognitive` middleware does...
//! request.set_attribute("precognitive", true);
//!
//! assert!(request.is_precognitive());
//! ```

use indexmap::IndexMap;

use illuminate_support::{Result, ValueExt};

use crate::exceptions::HttpResponseException;
use crate::request::Request;
use crate::response::Response;

/// The request attribute marking a request as precognitive.
const PRECOGNITIVE_ATTRIBUTE: &str = "precognitive";

/// Laravel Precognition's headers and responses.
///
/// ```
/// use illuminate_http::Precognition;
///
/// let response = Precognition::success_response();
///
/// assert_eq!(response.status_code(), 204);
/// assert_eq!(response.header("Precognition-Success").as_deref(), Some("true"));
/// ```
pub struct Precognition;

impl Precognition {
    /// The request header asking for a precognitive request, and the
    /// response header confirming one was handled.
    pub const HEADER: &'static str = "Precognition";

    /// The response header sent when a precognitive request succeeded.
    pub const SUCCESS_HEADER: &'static str = "Precognition-Success";

    /// The request header listing the only attributes to validate
    /// (comma-separated, `*` matching a single segment: `users.*.email`).
    pub const VALIDATE_ONLY_HEADER: &'static str = "Precognition-Validate-Only";

    /// The response for a successful precognitive request: `204 No Content`
    /// with a `Precognition-Success: true` header.
    pub fn success_response() -> Response {
        Response::no_content().with_header(Self::SUCCESS_HEADER, "true")
    }

    /// End a precognitive request successfully — Laravel's
    /// `abort(204, headers: ['Precognition-Success' => 'true'])`.
    ///
    /// The error carries [`Precognition::success_response`], which the
    /// exception handler returns as-is.
    ///
    /// ```
    /// use illuminate_http::Precognition;
    /// use illuminate_http::exceptions::HttpResponseException;
    ///
    /// let error = Precognition::abort_successfully::<()>().unwrap_err();
    /// let response = error.downcast_ref::<HttpResponseException>().unwrap().take_response().unwrap();
    ///
    /// assert_eq!(response.status_code(), 204);
    /// ```
    pub fn abort_successfully<T>() -> Result<T> {
        Err(HttpResponseException::new(Self::success_response()).into())
    }
}

impl Request {
    /// Determine if the request is attempting to be precognitive (it
    /// carries the `Precognition: true` header).
    pub fn is_attempting_precognition(&self) -> bool {
        self.header(Precognition::HEADER).as_deref() == Some("true")
    }

    /// Determine if the request is precognitive: the `precognitive`
    /// middleware accepted its `Precognition` header, so the route's
    /// handler won't run.
    ///
    /// Use it to customize validation rules, or to skip side effects in
    /// your own middleware:
    ///
    /// ```
    /// use illuminate_http::Request;
    ///
    /// fn password_rules(request: &Request) -> &'static str {
    ///     if request.is_precognitive() {
    ///         "required|min:8"
    ///     } else {
    ///         "required|min:8|uncompromised"
    ///     }
    /// }
    ///
    /// assert_eq!(password_rules(&Request::default()), "required|min:8|uncompromised");
    /// ```
    pub fn is_precognitive(&self) -> bool {
        self.attribute(PRECOGNITIVE_ATTRIBUTE).truthy()
    }

    /// Filter the given rules (keyed by attribute) down to the attributes
    /// listed in the `Precognition-Validate-Only` header. Without the
    /// header, every rule is kept.
    ///
    /// ```
    /// use illuminate_http::Request;
    /// use indexmap::IndexMap;
    ///
    /// let request = Request::create("/users", "POST");
    /// request.set_header("Precognition-Validate-Only", "name,users.*.email");
    ///
    /// let rules: IndexMap<String, &str> = [
    ///     ("name".to_string(), "required"),
    ///     ("email".to_string(), "required|email"),
    ///     ("users.0.email".to_string(), "email"),
    /// ]
    /// .into_iter()
    /// .collect();
    ///
    /// let filtered = request.filter_precognitive_rules(rules);
    /// assert_eq!(filtered.keys().collect::<Vec<_>>(), ["name", "users.0.email"]);
    /// ```
    pub fn filter_precognitive_rules<T>(&self, rules: IndexMap<String, T>) -> IndexMap<String, T> {
        let Some(validate_only) = self.precognitive_validate_only() else {
            return rules;
        };
        rules
            .into_iter()
            .filter(|(attribute, _)| matches_any(&validate_only, attribute))
            .collect()
    }

    /// Determine if the given (concrete) attribute should be validated for
    /// this request: it is listed in the `Precognition-Validate-Only`
    /// header, or the header is absent.
    ///
    /// ```
    /// use illuminate_http::Request;
    ///
    /// let request = Request::create("/users", "POST");
    /// assert!(request.should_validate_precognitive_attribute("email"));
    ///
    /// request.set_header("Precognition-Validate-Only", "profile.*");
    /// assert!(request.should_validate_precognitive_attribute("profile.bio"));
    /// assert!(!request.should_validate_precognitive_attribute("profile.links.0"));
    /// assert!(!request.should_validate_precognitive_attribute("email"));
    /// ```
    pub fn should_validate_precognitive_attribute(&self, attribute: &str) -> bool {
        self.precognitive_validate_only()
            .is_none_or(|validate_only| matches_any(&validate_only, attribute))
    }

    /// The attribute patterns listed in the `Precognition-Validate-Only`
    /// header.
    fn precognitive_validate_only(&self) -> Option<Vec<String>> {
        if !self.has_header(Precognition::VALIDATE_ONLY_HEADER) {
            return None;
        }
        let header = self
            .header(Precognition::VALIDATE_ONLY_HEADER)
            .unwrap_or_default();
        Some(
            header
                .split(',')
                .map(|pattern| pattern.trim().to_string())
                .collect(),
        )
    }
}

fn matches_any(patterns: &[String], attribute: &str) -> bool {
    patterns
        .iter()
        .any(|pattern| matches_pattern(pattern.as_bytes(), attribute.as_bytes()))
}

/// Match an attribute against a validate-only pattern, where `*` matches
/// one or more characters within a single dot-separated segment (Laravel's
/// `[^.]+`).
fn matches_pattern(pattern: &[u8], attribute: &[u8]) -> bool {
    match pattern.split_first() {
        None => attribute.is_empty(),
        Some((b'*', rest)) => {
            let segment = attribute.iter().take_while(|byte| **byte != b'.').count();
            (1..=segment).any(|taken| matches_pattern(rest, &attribute[taken..]))
        }
        Some((byte, rest)) => {
            attribute.first() == Some(byte) && matches_pattern(rest, &attribute[1..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with(headers: &[(&str, &str)]) -> Request {
        let request = Request::create("/users", "POST");
        for (name, value) in headers {
            request.set_header(name, value);
        }
        request
    }

    #[test]
    fn only_the_exact_true_header_attempts_precognition() {
        assert!(!Request::default().is_attempting_precognition());
        assert!(request_with(&[("Precognition", "true")]).is_attempting_precognition());
        assert!(!request_with(&[("Precognition", "1")]).is_attempting_precognition());
        assert!(!request_with(&[("Precognition", "false")]).is_attempting_precognition());
    }

    #[test]
    fn requests_are_precognitive_once_marked() {
        let request = request_with(&[("Precognition", "true")]);
        assert!(!request.is_precognitive());

        request.set_attribute("precognitive", true);
        assert!(request.is_precognitive());

        request.set_attribute("precognitive", false);
        assert!(!request.is_precognitive());
    }

    #[test]
    fn rules_are_kept_without_the_validate_only_header() {
        let rules: IndexMap<String, u8> = [("name".to_string(), 1), ("email".to_string(), 2)]
            .into_iter()
            .collect();
        assert_eq!(
            Request::default().filter_precognitive_rules(rules.clone()),
            rules
        );
    }

    #[test]
    fn rules_are_filtered_by_the_validate_only_header() {
        let request = request_with(&[(
            "Precognition-Validate-Only",
            "name, users.*.email,profile.*",
        )]);
        let rules: IndexMap<String, u8> = [
            "name",
            "email",
            "users.0.email",
            "users.1.email",
            "users.0.name",
            "users.email",
            "profile.bio",
            "profile.links.0",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, key)| (key.to_string(), index as u8))
        .collect();

        let filtered = request.filter_precognitive_rules(rules);
        assert_eq!(
            filtered.keys().map(String::as_str).collect::<Vec<_>>(),
            ["name", "users.0.email", "users.1.email", "profile.bio"]
        );
    }

    #[test]
    fn an_empty_validate_only_header_validates_nothing() {
        let request = request_with(&[("Precognition-Validate-Only", "")]);
        assert!(!request.should_validate_precognitive_attribute("name"));
    }

    #[test]
    fn wildcards_match_a_single_non_empty_segment() {
        let matches = |pattern: &str, attribute: &str| {
            matches_pattern(pattern.as_bytes(), attribute.as_bytes())
        };
        assert!(matches("users.*.email", "users.0.email"));
        assert!(matches("users.*.*", "users.12.name"));
        assert!(matches("*", "name"));
        assert!(matches("user_*", "user_name"));
        assert!(!matches("user_*", "user_"));
        assert!(!matches("users.*.email", "users..email"));
        assert!(!matches("users.*", "users.0.email"));
        assert!(!matches("name", "name.first"));
        assert!(!matches("name", "nam"));
    }

    #[test]
    fn the_success_response_is_an_empty_204() {
        let response = Precognition::success_response();
        assert_eq!(response.status_code(), 204);
        assert!(response.content().is_empty());
        assert_eq!(
            response.header("precognition-success").as_deref(),
            Some("true")
        );

        let error = Precognition::abort_successfully::<()>().unwrap_err();
        let response = error
            .downcast_ref::<HttpResponseException>()
            .and_then(HttpResponseException::take_response)
            .unwrap();
        assert_eq!(response.status_code(), 204);
    }
}
