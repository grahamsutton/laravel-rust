//! Route parameters for URL generation.
//!
//! The `route()` helper accepts its parameters in many shapes, just like
//! Laravel's arrays: nothing at all, a single value, a tuple of positional
//! values, `(name, value)` pairs, a JSON object, or models that implement
//! [`UrlRoutable`]:
//!
//! ```
//! use illuminate_routing::{IntoRouteParameters, UrlRoutable};
//! use illuminate_support::json;
//!
//! struct Post { id: u64 }
//!
//! impl UrlRoutable for Post {
//!     fn route_key(&self) -> String {
//!         self.id.to_string()
//!     }
//! }
//!
//! let post = Post { id: 1 };
//!
//! assert_eq!(().into_route_parameters().len(), 0);
//! assert_eq!(5.into_route_parameters().len(), 1);
//! assert_eq!((1, "taylor").into_route_parameters().len(), 2);
//! assert_eq!([("post", 1), ("page", 2)].into_route_parameters().len(), 2);
//! assert_eq!(json!({"post": 1, "search": "rocket"}).into_route_parameters().len(), 2);
//! assert_eq!((&post,).into_route_parameters().len(), 1);
//! ```

use illuminate_support::{Value, ValueExt};

/// Models (or anything else) that can be used as route parameters.
///
/// The Eloquent `#[derive(Model)]` macro implements this for models using
/// their primary key; implement it yourself to customize the key:
///
/// ```
/// use illuminate_routing::UrlRoutable;
///
/// struct Post { id: u64, slug: String }
///
/// impl UrlRoutable for Post {
///     fn route_key(&self) -> String {
///         self.slug.clone()
///     }
///
///     fn route_key_name() -> &'static str {
///         "slug"
///     }
/// }
/// ```
pub trait UrlRoutable {
    /// The value of the model's route key.
    fn route_key(&self) -> String;

    /// The name of the model's route key.
    fn route_key_name() -> &'static str
    where
        Self: Sized,
    {
        "id"
    }

    /// The value of another field, used for routes with custom binding keys
    /// (`/posts/{post:slug}`). Returning `None` falls back to the route key.
    fn route_field(&self, field: &str) -> Option<String> {
        let _ = field;
        None
    }
}

/// A single route parameter value.
pub enum RouteParameter<'a> {
    /// A plain value (string, number, list, ...).
    Value(Value),
    /// A model, resolved to its route key (or a custom binding field).
    Routable(&'a dyn UrlRoutable),
}

impl RouteParameter<'_> {
    /// Determine if the value is null (and should be treated as missing).
    pub fn is_null(&self) -> bool {
        matches!(self, RouteParameter::Value(Value::Null))
    }

    /// Resolve the parameter into a value, using the binding field for models.
    pub fn resolve(&self, binding_field: Option<&str>) -> Value {
        match self {
            RouteParameter::Value(value) => value.clone(),
            RouteParameter::Routable(model) => Value::String(
                binding_field
                    .and_then(|field| model.route_field(field))
                    .unwrap_or_else(|| model.route_key()),
            ),
        }
    }
}

impl std::fmt::Debug for RouteParameter<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteParameter::Value(value) => f.debug_tuple("Value").field(value).finish(),
            RouteParameter::Routable(model) => {
                f.debug_tuple("Routable").field(&model.route_key()).finish()
            }
        }
    }
}

/// An ordered list of named and positional route parameters.
#[derive(Debug, Default)]
pub struct RouteParameters<'a> {
    items: Vec<(Option<String>, RouteParameter<'a>)>,
}

impl<'a> RouteParameters<'a> {
    /// Create an empty parameter list.
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Add a positional parameter.
    pub fn push(mut self, value: impl IntoRouteParameter<'a>) -> Self {
        self.items.push((None, value.into_route_parameter()));
        self
    }

    /// Add (or replace) a named parameter.
    pub fn with(mut self, key: impl Into<String>, value: impl IntoRouteParameter<'a>) -> Self {
        self.insert(key.into(), value.into_route_parameter());
        self
    }

    pub(crate) fn insert(&mut self, key: String, value: RouteParameter<'a>) {
        match self.items.iter_mut().find(|(k, _)| k.as_deref() == Some(key.as_str())) {
            Some(slot) => slot.1 = value,
            None => self.items.push((Some(key), value)),
        }
    }

    /// Determine if a named parameter is present.
    pub fn contains_key(&self, key: &str) -> bool {
        self.items.iter().any(|(k, _)| k.as_deref() == Some(key))
    }

    /// The number of parameters.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Determine if there are no parameters.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Iterate over the parameters.
    pub fn iter(&self) -> impl Iterator<Item = (Option<&str>, &RouteParameter<'a>)> {
        self.items.iter().map(|(k, v)| (k.as_deref(), v))
    }

    /// Sort the named parameters by key, keeping positional parameters
    /// first (PHP's `ksort`, used when signing URLs).
    pub(crate) fn sort_by_key(&mut self) {
        self.items.sort_by(|(a, _), (b, _)| match (a, b) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => a.cmp(b),
        });
    }

    pub(crate) fn into_items(self) -> Vec<(Option<String>, RouteParameter<'a>)> {
        self.items
    }
}

/// A value usable as a single route parameter.
pub trait IntoRouteParameter<'a> {
    fn into_route_parameter(self) -> RouteParameter<'a>;
}

impl<'a> IntoRouteParameter<'a> for RouteParameter<'a> {
    fn into_route_parameter(self) -> RouteParameter<'a> {
        self
    }
}

impl<'a> IntoRouteParameter<'a> for Value {
    fn into_route_parameter(self) -> RouteParameter<'a> {
        RouteParameter::Value(self)
    }
}

impl<'a> IntoRouteParameter<'a> for &Value {
    fn into_route_parameter(self) -> RouteParameter<'a> {
        RouteParameter::Value(self.clone())
    }
}

impl<'a> IntoRouteParameter<'a> for &str {
    fn into_route_parameter(self) -> RouteParameter<'a> {
        RouteParameter::Value(Value::String(self.to_string()))
    }
}

impl<'a> IntoRouteParameter<'a> for String {
    fn into_route_parameter(self) -> RouteParameter<'a> {
        RouteParameter::Value(Value::String(self))
    }
}

impl<'a> IntoRouteParameter<'a> for &String {
    fn into_route_parameter(self) -> RouteParameter<'a> {
        RouteParameter::Value(Value::String(self.clone()))
    }
}

impl<'a, T: UrlRoutable> IntoRouteParameter<'a> for &'a T {
    fn into_route_parameter(self) -> RouteParameter<'a> {
        RouteParameter::Routable(self)
    }
}

macro_rules! scalar_parameter {
    ($($ty:ty),*) => {
        $(
            impl<'a> IntoRouteParameter<'a> for $ty {
                fn into_route_parameter(self) -> RouteParameter<'a> {
                    RouteParameter::Value(Value::from(self))
                }
            }

            impl<'a> IntoRouteParameters<'a> for $ty {
                fn into_route_parameters(self) -> RouteParameters<'a> {
                    RouteParameters::new().push(self)
                }
            }
        )*
    };
}

scalar_parameter!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64, bool);

/// Anything usable as the parameters of a route.
pub trait IntoRouteParameters<'a> {
    fn into_route_parameters(self) -> RouteParameters<'a>;
}

impl<'a> IntoRouteParameters<'a> for RouteParameters<'a> {
    fn into_route_parameters(self) -> RouteParameters<'a> {
        self
    }
}

impl<'a> IntoRouteParameters<'a> for () {
    fn into_route_parameters(self) -> RouteParameters<'a> {
        RouteParameters::new()
    }
}

/// Objects provide named parameters, arrays positional ones.
impl<'a> IntoRouteParameters<'a> for Value {
    fn into_route_parameters(self) -> RouteParameters<'a> {
        let mut parameters = RouteParameters::new();
        match self {
            Value::Null => {}
            Value::Object(map) => {
                for (key, value) in map {
                    parameters.insert(key, RouteParameter::Value(value));
                }
            }
            Value::Array(items) => {
                for item in items {
                    parameters = parameters.push(item);
                }
            }
            scalar => parameters = parameters.push(scalar),
        }
        parameters
    }
}

impl<'a> IntoRouteParameters<'a> for &Value {
    fn into_route_parameters(self) -> RouteParameters<'a> {
        self.clone().into_route_parameters()
    }
}

impl<'a> IntoRouteParameters<'a> for &str {
    fn into_route_parameters(self) -> RouteParameters<'a> {
        RouteParameters::new().push(self)
    }
}

impl<'a> IntoRouteParameters<'a> for String {
    fn into_route_parameters(self) -> RouteParameters<'a> {
        RouteParameters::new().push(self)
    }
}

impl<'a> IntoRouteParameters<'a> for &String {
    fn into_route_parameters(self) -> RouteParameters<'a> {
        RouteParameters::new().push(self)
    }
}

impl<'a, T: UrlRoutable> IntoRouteParameters<'a> for &'a T {
    fn into_route_parameters(self) -> RouteParameters<'a> {
        RouteParameters::new().push(self)
    }
}

impl<'a, K: Into<String>, P: IntoRouteParameter<'a>, const N: usize> IntoRouteParameters<'a> for [(K, P); N] {
    fn into_route_parameters(self) -> RouteParameters<'a> {
        self.into_iter()
            .fold(RouteParameters::new(), |parameters, (key, value)| parameters.with(key, value))
    }
}

impl<'a, K: Into<String>, P: IntoRouteParameter<'a>> IntoRouteParameters<'a> for Vec<(K, P)> {
    fn into_route_parameters(self) -> RouteParameters<'a> {
        self.into_iter()
            .fold(RouteParameters::new(), |parameters, (key, value)| parameters.with(key, value))
    }
}

macro_rules! tuple_parameters {
    ($($name:ident),+) => {
        impl<'a, $($name: IntoRouteParameter<'a>),+> IntoRouteParameters<'a> for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_route_parameters(self) -> RouteParameters<'a> {
                let ($($name,)+) = self;
                RouteParameters::new()$(.push($name))+
            }
        }
    };
}

tuple_parameters!(A);
tuple_parameters!(A, B);
tuple_parameters!(A, B, C);
tuple_parameters!(A, B, C, D);
tuple_parameters!(A, B, C, D, E);
tuple_parameters!(A, B, C, D, E, F);
tuple_parameters!(A, B, C, D, E, F, G);
tuple_parameters!(A, B, C, D, E, F, G, H);

/// Convert a parameter value into the string placed in a URL.
pub(crate) fn parameter_string(value: &Value) -> String {
    value.to_string_lossy()
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    struct Post {
        id: u64,
        slug: &'static str,
    }

    impl UrlRoutable for Post {
        fn route_key(&self) -> String {
            self.id.to_string()
        }

        fn route_field(&self, field: &str) -> Option<String> {
            (field == "slug").then(|| self.slug.to_string())
        }
    }

    fn describe(parameters: RouteParameters<'_>) -> Vec<(Option<String>, Value)> {
        parameters
            .into_items()
            .into_iter()
            .map(|(k, v)| (k, v.resolve(None)))
            .collect()
    }

    #[test]
    fn values_become_named_or_positional_parameters() {
        assert_eq!(
            describe(json!({"id": 1, "tab": "posts"}).into_route_parameters()),
            vec![(Some("id".into()), json!(1)), (Some("tab".into()), json!("posts"))]
        );
        assert_eq!(
            describe(json!([1, 2]).into_route_parameters()),
            vec![(None, json!(1)), (None, json!(2))]
        );
        assert!(json!(null).into_route_parameters().is_empty());
    }

    #[test]
    fn models_resolve_to_their_route_key_or_binding_field() {
        let post = Post { id: 7, slug: "hello-world" };
        let parameters = (&post, "draft").into_route_parameters().into_items();
        assert_eq!(parameters[0].1.resolve(None), json!("7"));
        assert_eq!(parameters[0].1.resolve(Some("slug")), json!("hello-world"));
        assert_eq!(parameters[1].1.resolve(None), json!("draft"));
        assert_eq!(Post::route_key_name(), "id");
    }

    #[test]
    fn pairs_become_named_parameters() {
        let parameters = vec![("user", 1), ("user", 2)].into_route_parameters();
        assert_eq!(describe(parameters), vec![(Some("user".into()), json!(2))]);
        let mut sorted = [("b", 1), ("a", 2)].into_route_parameters().push("x");
        sorted.sort_by_key();
        let keys: Vec<Option<&str>> = sorted.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec![None, Some("a"), Some("b")]);
    }
}
