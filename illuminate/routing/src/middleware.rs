//! Route middleware: names, instances, and the router's built-in middleware.
//!
//! Middleware may be attached to routes by *name* — an alias such as
//! `"auth"`, an alias with parameters such as `"throttle:60,1"` or
//! `"can:update,post"`, or the name of a middleware group such as `"web"` —
//! or as ready-made instances:
//!
//! ```
//! use illuminate_http::middleware_fn;
//! use illuminate_routing::{IntoMiddleware, RouteMiddleware};
//!
//! let middleware = ("auth", "throttle:60,1").into_middleware();
//! assert_eq!(middleware[1].base_name(), "throttle");
//! assert_eq!(middleware[1].parameters(), vec!["60", "1"]);
//!
//! let inline = middleware_fn(|request, next| async move { Ok(next.run(request).await) });
//! assert_eq!(inline.into_middleware()[0].name(), "Closure");
//! ```

use std::sync::Arc;

use illuminate_http::{Middleware, Next, Request, Response, async_trait};
use illuminate_support::Result;

use crate::exceptions::InvalidSignatureException;
use crate::url::url_generator;

/// Builds a middleware instance from the parameters given after the `:` in
/// a middleware name (`"throttle:60,1"` → `["60", "1"]`).
pub type MiddlewareFactory = Arc<dyn Fn(&[String]) -> Arc<dyn Middleware> + Send + Sync>;

/// A middleware attached to a route: either a name to be resolved by the
/// router (an alias or group, optionally with parameters), or an instance.
#[derive(Clone)]
pub enum RouteMiddleware {
    /// A middleware alias or group name, such as `"auth"` or `"throttle:60,1"`.
    Name(String),
    /// A ready-made middleware instance, with a display name.
    Instance {
        name: String,
        middleware: Arc<dyn Middleware>,
    },
}

impl RouteMiddleware {
    /// Reference a middleware alias or group by name.
    pub fn named(name: impl Into<String>) -> Self {
        Self::Name(name.into())
    }

    /// Use a middleware instance, displayed by its type name.
    pub fn of<M: Middleware>(middleware: M) -> Self {
        Self::Instance {
            name: std::any::type_name::<M>().to_string(),
            middleware: Arc::new(middleware),
        }
    }

    /// Use a shared middleware instance with an explicit display name.
    pub fn instance(name: impl Into<String>, middleware: Arc<dyn Middleware>) -> Self {
        Self::Instance {
            name: name.into(),
            middleware,
        }
    }

    /// The full name of the middleware (`"throttle:60,1"`), or the display
    /// name of an instance.
    pub fn name(&self) -> &str {
        match self {
            Self::Name(name) => name,
            Self::Instance { name, .. } => name,
        }
    }

    /// The name without any parameters (`"throttle:60,1"` → `"throttle"`).
    pub fn base_name(&self) -> &str {
        match self {
            Self::Name(name) => name.split_once(':').map_or(name.as_str(), |(base, _)| base),
            Self::Instance { name, .. } => name,
        }
    }

    /// The parameters given after the `:` (`"can:update,post"` → `["update", "post"]`).
    pub fn parameters(&self) -> Vec<String> {
        match self {
            Self::Name(name) => parse_parameters(name),
            Self::Instance { .. } => Vec::new(),
        }
    }

    /// Determine if this is a middleware instance rather than a name.
    pub fn is_instance(&self) -> bool {
        matches!(self, Self::Instance { .. })
    }

    /// Determine if `self` (an excluded middleware) excludes `candidate`.
    ///
    /// Names match exactly, or by base name when the exclusion has no
    /// parameters (`without_middleware("throttle")` removes
    /// `"throttle:60,1"`). Instances match by identity, or by type name.
    pub(crate) fn excludes(&self, candidate: &RouteMiddleware) -> bool {
        if let (
            Self::Instance { middleware: a, .. },
            Self::Instance { middleware: b, .. },
        ) = (self, candidate)
            && same_instance(a, b) {
                return true;
            }
        if self.is_instance() && self.name() == "Closure" {
            return false;
        }
        let excluded = self.name();
        let name = candidate.name();
        name == excluded || (!excluded.contains(':') && candidate.base_name() == excluded)
    }

    /// Determine if two entries are duplicates of each other.
    pub(crate) fn same_as(&self, other: &RouteMiddleware) -> bool {
        match (self, other) {
            (Self::Name(a), Self::Name(b)) => a == b,
            (Self::Instance { middleware: a, .. }, Self::Instance { middleware: b, .. }) => {
                same_instance(a, b)
            }
            _ => false,
        }
    }
}

fn same_instance(a: &Arc<dyn Middleware>, b: &Arc<dyn Middleware>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
}

impl std::fmt::Debug for RouteMiddleware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Name(name) => f.debug_tuple("Name").field(name).finish(),
            Self::Instance { name, .. } => f.debug_tuple("Instance").field(name).finish(),
        }
    }
}

impl PartialEq for RouteMiddleware {
    fn eq(&self, other: &Self) -> bool {
        self.same_as(other)
    }
}

/// Split the parameters off a middleware name (`"role:editor,admin"`).
pub fn parse_parameters(name: &str) -> Vec<String> {
    match name.split_once(':') {
        Some((_, parameters)) if !parameters.is_empty() => {
            parameters.split(',').map(str::to_string).collect()
        }
        _ => Vec::new(),
    }
}

/// Anything that may be given to `middleware(...)` and `without_middleware(...)`.
pub trait IntoMiddleware {
    /// Convert into a list of route middleware.
    fn into_middleware(self) -> Vec<RouteMiddleware>;
}

impl IntoMiddleware for RouteMiddleware {
    fn into_middleware(self) -> Vec<RouteMiddleware> {
        vec![self]
    }
}

impl IntoMiddleware for &str {
    fn into_middleware(self) -> Vec<RouteMiddleware> {
        vec![RouteMiddleware::Name(self.to_string())]
    }
}

impl IntoMiddleware for String {
    fn into_middleware(self) -> Vec<RouteMiddleware> {
        vec![RouteMiddleware::Name(self)]
    }
}

impl IntoMiddleware for &String {
    fn into_middleware(self) -> Vec<RouteMiddleware> {
        vec![RouteMiddleware::Name(self.clone())]
    }
}

/// Shared instances (such as those made with `middleware_fn`) are displayed
/// as `Closure`, just like inline middleware in Laravel.
impl IntoMiddleware for Arc<dyn Middleware> {
    fn into_middleware(self) -> Vec<RouteMiddleware> {
        vec![RouteMiddleware::instance("Closure", self)]
    }
}

impl IntoMiddleware for () {
    fn into_middleware(self) -> Vec<RouteMiddleware> {
        Vec::new()
    }
}

impl<T: IntoMiddleware> IntoMiddleware for Vec<T> {
    fn into_middleware(self) -> Vec<RouteMiddleware> {
        self.into_iter().flat_map(IntoMiddleware::into_middleware).collect()
    }
}

impl<T: IntoMiddleware, const N: usize> IntoMiddleware for [T; N] {
    fn into_middleware(self) -> Vec<RouteMiddleware> {
        self.into_iter().flat_map(IntoMiddleware::into_middleware).collect()
    }
}

impl<T: IntoMiddleware + Clone> IntoMiddleware for &[T] {
    fn into_middleware(self) -> Vec<RouteMiddleware> {
        self.iter()
            .cloned()
            .flat_map(IntoMiddleware::into_middleware)
            .collect()
    }
}

macro_rules! tuple_middleware {
    ($($name:ident),+) => {
        impl<$($name: IntoMiddleware),+> IntoMiddleware for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_middleware(self) -> Vec<RouteMiddleware> {
                let ($($name,)+) = self;
                let mut middleware = Vec::new();
                $(middleware.extend($name.into_middleware());)+
                middleware
            }
        }
    };
}

tuple_middleware!(A);
tuple_middleware!(A, B);
tuple_middleware!(A, B, C);
tuple_middleware!(A, B, C, D);
tuple_middleware!(A, B, C, D, E);
tuple_middleware!(A, B, C, D, E, F);
tuple_middleware!(A, B, C, D, E, F, G);
tuple_middleware!(A, B, C, D, E, F, G, H);

/// Sort middleware by the router's priority list, keeping the relative order
/// of everything else (a port of Laravel's `SortedMiddleware`), then remove
/// duplicates.
pub(crate) fn sort_middleware(priority: &[String], middleware: Vec<RouteMiddleware>) -> Vec<RouteMiddleware> {
    let mut middleware = middleware;

    if !priority.is_empty() {
        'restart: loop {
            let mut last: Option<(usize, usize)> = None;
            for index in 0..middleware.len() {
                let Some(priority_index) = priority_index(priority, &middleware[index]) else {
                    continue;
                };
                if let Some((last_index, last_priority)) = last
                    && priority_index < last_priority {
                        let item = middleware.remove(index);
                        middleware.insert(last_index, item);
                        continue 'restart;
                    }
                last = Some((index, priority_index));
            }
            break;
        }
    }

    unique_middleware(middleware)
}

fn priority_index(priority: &[String], middleware: &RouteMiddleware) -> Option<usize> {
    let base = middleware.base_name();
    priority
        .iter()
        .position(|p| p == base || p == middleware.name())
}

/// Remove duplicate middleware, keeping the first occurrence.
pub(crate) fn unique_middleware(middleware: Vec<RouteMiddleware>) -> Vec<RouteMiddleware> {
    let mut unique: Vec<RouteMiddleware> = Vec::with_capacity(middleware.len());
    for entry in middleware {
        if !unique.iter().any(|existing| existing.same_as(&entry)) {
            unique.push(entry);
        }
    }
    unique
}

/// The middleware resolved for a matched route, attached to the request as
/// an extension so the HTTP kernel can call `terminate` on them once the
/// response has been sent.
#[derive(Clone, Default)]
pub struct RouteMiddlewareStack(pub Vec<Arc<dyn Middleware>>);

impl std::fmt::Debug for RouteMiddlewareStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RouteMiddlewareStack").field(&self.0.len()).finish()
    }
}

/// Validate that the incoming request has a valid URL signature (the
/// `signed` middleware).
///
/// Use `"signed"` for absolute signatures, `"signed:relative"` for URLs
/// signed with `absolute: false`, and add query parameters that should be
/// ignored after it: `"signed:relative,page,order"`.
#[derive(Debug, Clone, Default)]
pub struct ValidateSignature {
    relative: bool,
    ignore: Vec<String>,
}

impl ValidateSignature {
    /// Validate absolute signatures.
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate relative signatures (URLs signed without their domain).
    pub fn relative() -> Self {
        Self {
            relative: true,
            ignore: Vec::new(),
        }
    }

    /// Ignore the given query parameters while validating.
    pub fn ignoring(mut self, parameters: &[&str]) -> Self {
        self.ignore.extend(parameters.iter().map(|p| p.to_string()));
        self
    }

    /// Build the middleware from route middleware parameters.
    pub fn from_parameters(parameters: &[String]) -> Self {
        let relative = parameters.first().is_some_and(|p| p == "relative");
        let ignore = parameters.iter().skip(usize::from(relative)).cloned().collect();
        Self { relative, ignore }
    }

    /// The factory registered for the `signed` alias.
    pub fn factory() -> MiddlewareFactory {
        Arc::new(|parameters| Arc::new(Self::from_parameters(parameters)) as Arc<dyn Middleware>)
    }
}

#[async_trait]
impl Middleware for ValidateSignature {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let ignore: Vec<&str> = self.ignore.iter().map(String::as_str).collect();
        if url_generator().has_valid_signature_while_ignoring(&request, &ignore, !self.relative) {
            return Ok(next.run(request).await);
        }
        Err(InvalidSignatureException.into_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_http::middleware_fn;

    fn names(list: &[RouteMiddleware]) -> Vec<&str> {
        list.iter().map(RouteMiddleware::name).collect()
    }

    #[test]
    fn many_shapes_convert_into_middleware() {
        assert_eq!(names(&"auth".into_middleware()), vec!["auth"]);
        assert_eq!(names(&["auth", "verified"].into_middleware()), vec!["auth", "verified"]);
        assert_eq!(names(&vec!["a".to_string()].into_middleware()), vec!["a"]);
        let inline = middleware_fn(|request, next| async move { Ok(next.run(request).await) });
        assert_eq!(names(&("web", inline).into_middleware()), vec!["web", "Closure"]);
        assert!(().into_middleware().is_empty());
    }

    #[test]
    fn parameters_are_parsed_from_names() {
        let middleware = RouteMiddleware::named("can:update,post");
        assert_eq!(middleware.base_name(), "can");
        assert_eq!(middleware.parameters(), vec!["update", "post"]);
        assert!(RouteMiddleware::named("auth").parameters().is_empty());
    }

    #[test]
    fn exclusions_match_by_name_or_base_name() {
        let throttle = RouteMiddleware::named("throttle:60,1");
        assert!(RouteMiddleware::named("throttle").excludes(&throttle));
        assert!(RouteMiddleware::named("throttle:60,1").excludes(&throttle));
        assert!(!RouteMiddleware::named("throttle:10,1").excludes(&throttle));
        assert!(!RouteMiddleware::named("auth").excludes(&throttle));

        let inline = middleware_fn(|request, next| async move { Ok(next.run(request).await) });
        let entry = RouteMiddleware::instance("Closure", inline.clone());
        assert!(RouteMiddleware::instance("Closure", inline).excludes(&entry));
        let other = middleware_fn(|request, next| async move { Ok(next.run(request).await) });
        assert!(!RouteMiddleware::instance("Closure", other).excludes(&entry));
    }

    #[test]
    fn middleware_is_sorted_by_priority_and_deduplicated() {
        let priority = vec!["session".to_string(), "auth".to_string(), "can".to_string()];
        let sorted = sort_middleware(
            &priority,
            vec![
                RouteMiddleware::named("log"),
                RouteMiddleware::named("can:update,post"),
                RouteMiddleware::named("auth"),
                RouteMiddleware::named("session"),
                RouteMiddleware::named("log"),
            ],
        );
        assert_eq!(names(&sorted), vec!["log", "session", "auth", "can:update,post"]);
    }

    #[test]
    fn signature_middleware_reads_its_parameters() {
        let middleware = ValidateSignature::from_parameters(&["relative".into(), "page".into()]);
        assert!(middleware.relative);
        assert_eq!(middleware.ignore, vec!["page"]);
        assert!(!ValidateSignature::from_parameters(&[]).relative);
    }
}
