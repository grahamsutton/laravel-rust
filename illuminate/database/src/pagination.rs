//! Cursor pagination: Laravel's `Cursor` and `CursorPaginator`.
//!
//! Cursor pagination places a "where" clause on the ordered columns instead
//! of an offset, so it stays fast on huge tables and never skips or repeats
//! rows while new ones are inserted. The cursor is an encoded string in the
//! `cursor` query string parameter.
//!
//! ```
//! use illuminate_database::pagination::Cursor;
//! use illuminate_support::json;
//!
//! let cursor = Cursor::new(json!({"id": 15}), true);
//! let encoded = cursor.encode();
//! assert_eq!(Cursor::from_encoded(&encoded), Some(cursor));
//! ```

use std::sync::{Arc, LazyLock, RwLock};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use illuminate_pagination::{current_path, query_string, translate};
use illuminate_support::{Arr, Collection, HtmlString, Map, Result, Value, e, json};
use serde::{Serialize, Serializer};

use crate::eloquent::Model;

type CursorResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

static CURSOR: LazyLock<RwLock<Option<CursorResolver>>> = LazyLock::new(|| RwLock::new(None));

/// Set the resolver for the current request's encoded cursor (Laravel's
/// `CursorPaginator::currentCursorResolver`). The framework reads it from
/// the request's query string.
///
/// ```
/// use illuminate_database::pagination::resolve_current_cursor_using;
///
/// resolve_current_cursor_using(|_name| None);
/// ```
pub fn resolve_current_cursor_using(
    resolver: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
) {
    *CURSOR.write().unwrap() = Some(Arc::new(resolver));
}

/// Resolve the current cursor for the given parameter name, if any.
pub fn resolve_current_cursor(cursor_name: &str) -> Option<Cursor> {
    let resolver = CURSOR.read().unwrap().clone();
    resolver
        .and_then(|resolve| resolve(cursor_name))
        .and_then(|encoded| Cursor::from_encoded(&encoded))
}

/// Thrown when a cursor lacks one of the ordered columns.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Unable to find parameter [{0}] in pagination item.")]
pub struct UnexpectedValueException(pub String);

/// A position in a cursor paginated result: the values of the ordered
/// columns of an item, and the direction to paginate in.
#[derive(Clone, Debug, PartialEq)]
pub struct Cursor {
    parameters: Map<String, Value>,
    points_to_next_items: bool,
}

impl Cursor {
    /// Create a cursor from the ordered columns' values (a JSON object).
    pub fn new(parameters: impl Into<Value>, points_to_next_items: bool) -> Self {
        let parameters = match parameters.into() {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        Self {
            parameters,
            points_to_next_items,
        }
    }

    /// Get a parameter's value, failing when the cursor doesn't have it.
    pub fn parameter(&self, name: &str) -> Result<Value> {
        self.parameters
            .get(name)
            .cloned()
            .ok_or_else(|| UnexpectedValueException(name.to_string()).into())
    }

    /// Get the values of the given parameters.
    pub fn parameters(&self, names: &[&str]) -> Result<Vec<Value>> {
        names.iter().map(|name| self.parameter(name)).collect()
    }

    /// Whether the cursor points to the next set of items.
    pub fn points_to_next_items(&self) -> bool {
        self.points_to_next_items
    }

    /// Whether the cursor points to the previous set of items.
    pub fn points_to_previous_items(&self) -> bool {
        !self.points_to_next_items
    }

    /// The cursor as an array (its parameters plus `_pointsToNextItems`).
    pub fn to_array(&self) -> Map<String, Value> {
        let mut array = self.parameters.clone();
        array.insert(
            "_pointsToNextItems".to_string(),
            Value::Bool(self.points_to_next_items),
        );
        array
    }

    /// Encode the cursor as a URL-safe string.
    pub fn encode(&self) -> String {
        STANDARD
            .encode(Value::Object(self.to_array()).to_string())
            .replace('+', "-")
            .replace('/', "_")
            .replace('=', "")
    }

    /// Decode a cursor encoded with [`Cursor::encode`] (`None` when invalid).
    pub fn from_encoded(encoded: &str) -> Option<Self> {
        let mut base64 = encoded.replace('-', "+").replace('_', "/");
        while !base64.len().is_multiple_of(4) {
            base64.push('=');
        }
        let bytes = STANDARD.decode(base64).ok()?;
        let Value::Object(mut parameters) = serde_json::from_slice::<Value>(&bytes).ok()? else {
            return None;
        };
        let points_to_next_items = parameters.shift_remove("_pointsToNextItems")?;
        Some(Self {
            parameters,
            points_to_next_items: points_to_next_items.as_bool().unwrap_or(true),
        })
    }
}

/// Items that can be cursor paginated: the cursor reads the values of the
/// ordered columns from them.
pub trait CursorItem {
    /// The value of a cursor parameter (`id`, or `users.id`) for the item.
    fn cursor_parameter(&self, name: &str) -> Value;
}

fn after_last_dot(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

impl CursorItem for Value {
    fn cursor_parameter(&self, name: &str) -> Value {
        self.get(name)
            .filter(|value| !value.is_null())
            .or_else(|| self.get(after_last_dot(name)))
            .cloned()
            .unwrap_or(Value::Null)
    }
}

impl<M: Model> CursorItem for M {
    fn cursor_parameter(&self, name: &str) -> Value {
        // Ordering by a pivot column (`role_user.created_at`) reads the pivot.
        if name.contains('.')
            && let Value::Object(pivot) = self.get_attribute("pivot")
            && let Some(value) = pivot.get(after_last_dot(name))
        {
            return value.clone();
        }
        let value = self.get_attribute(name);
        if value.is_null() {
            self.get_attribute(after_last_dot(name))
        } else {
            value
        }
    }
}

/// The options of a cursor paginator.
#[derive(Clone, Debug)]
pub struct CursorPaginatorOptions {
    /// The base path for generated URLs.
    pub path: String,
    /// The query string parameter holding the cursor.
    pub cursor_name: String,
    /// The ordered columns stored in the cursors.
    pub parameters: Vec<String>,
    /// Extra query string parameters appended to every URL.
    pub query: Map<String, Value>,
    /// The URL fragment appended to every URL.
    pub fragment: Option<String>,
}

impl Default for CursorPaginatorOptions {
    fn default() -> Self {
        Self {
            path: current_path(),
            cursor_name: "cursor".to_string(),
            parameters: Vec::new(),
            query: Map::new(),
            fragment: None,
        }
    }
}

/// A cursor paginator: what `cursor_paginate()` returns. It only knows the
/// cursors of the previous and next pages, never the total.
#[derive(Clone, Debug)]
pub struct CursorPaginator<T> {
    items: Collection<T>,
    per_page: u64,
    cursor: Option<Cursor>,
    has_more: bool,
    options: CursorPaginatorOptions,
    first_parameters: Option<Map<String, Value>>,
    last_parameters: Option<Map<String, Value>>,
}

impl<T: CursorItem> CursorPaginator<T> {
    /// Create a cursor paginator. Pass up to `per_page + 1` items; the extra
    /// item signals that there are more.
    pub fn new(
        items: impl Into<Collection<T>>,
        per_page: u64,
        cursor: Option<Cursor>,
        mut options: CursorPaginatorOptions,
    ) -> Self {
        let per_page = per_page.max(1);
        let mut items = items.into().into_vec();
        let has_more = items.len() as u64 > per_page;
        items.truncate(per_page as usize);
        if cursor
            .as_ref()
            .is_some_and(Cursor::points_to_previous_items)
        {
            items.reverse();
        }
        if options.path != "/" {
            options.path = options.path.trim_end_matches('/').to_string();
        }
        let parameters_for = |item: &T| -> Map<String, Value> {
            options
                .parameters
                .iter()
                .filter(|name| !name.is_empty())
                .map(|name| (name.clone(), item.cursor_parameter(name)))
                .collect()
        };
        let first_parameters = items.first().map(parameters_for);
        let last_parameters = items.last().map(parameters_for);
        Self {
            items: items.into(),
            per_page,
            cursor,
            has_more,
            options,
            first_parameters,
            last_parameters,
        }
    }
}

impl<T> CursorPaginator<T> {
    /// The items on the current page.
    pub fn items(&self) -> &Collection<T> {
        &self.items
    }

    /// The items on the current page, by value.
    pub fn into_items(self) -> Collection<T> {
        self.items
    }

    /// Transform each item, keeping the pagination information.
    pub fn through<U>(self, callback: impl FnMut(T) -> U) -> CursorPaginator<U> {
        CursorPaginator {
            items: self.items.map(callback),
            per_page: self.per_page,
            cursor: self.cursor,
            has_more: self.has_more,
            options: self.options,
            first_parameters: self.first_parameters,
            last_parameters: self.last_parameters,
        }
    }

    /// The number of items shown per page.
    pub fn per_page(&self) -> u64 {
        self.per_page
    }

    /// The current cursor, if any.
    pub fn cursor(&self) -> Option<&Cursor> {
        self.cursor.as_ref()
    }

    /// The query string parameter holding the cursor.
    pub fn get_cursor_name(&self) -> &str {
        &self.options.cursor_name
    }

    /// Use a different query string parameter for the cursor.
    pub fn set_cursor_name(mut self, name: impl Into<String>) -> Self {
        self.options.cursor_name = name.into();
        self
    }

    /// The number of items on the current page.
    pub fn count(&self) -> usize {
        self.items.count()
    }

    /// Whether the current page is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether the current page has items.
    pub fn is_not_empty(&self) -> bool {
        !self.items.is_empty()
    }

    /// Whether there are more items after the current page.
    pub fn has_more_pages(&self) -> bool {
        match &self.cursor {
            None => self.has_more,
            Some(cursor) => cursor.points_to_previous_items() || self.has_more,
        }
    }

    /// Whether there are enough items to split into multiple pages.
    pub fn has_pages(&self) -> bool {
        !self.on_first_page() || self.has_more_pages()
    }

    /// Whether the paginator is on the first page.
    pub fn on_first_page(&self) -> bool {
        match &self.cursor {
            None => true,
            Some(cursor) => cursor.points_to_previous_items() && !self.has_more,
        }
    }

    /// Whether the paginator is on the last page.
    pub fn on_last_page(&self) -> bool {
        !self.has_more_pages()
    }

    /// The cursor of the previous page.
    pub fn previous_cursor(&self) -> Option<Cursor> {
        let cursor = self.cursor.as_ref()?;
        if cursor.points_to_previous_items() && !self.has_more {
            return None;
        }
        self.first_parameters
            .clone()
            .map(|parameters| Cursor::new(Value::Object(parameters), false))
    }

    /// The cursor of the next page.
    pub fn next_cursor(&self) -> Option<Cursor> {
        let exhausted = match &self.cursor {
            None => !self.has_more,
            Some(cursor) => cursor.points_to_next_items() && !self.has_more,
        };
        if exhausted {
            return None;
        }
        self.last_parameters
            .clone()
            .map(|parameters| Cursor::new(Value::Object(parameters), true))
    }

    /// The URL for a cursor.
    pub fn url(&self, cursor: Option<&Cursor>) -> String {
        let mut parameters = self.options.query.clone();
        if let Some(cursor) = cursor {
            parameters.insert(
                self.options.cursor_name.clone(),
                Value::String(cursor.encode()),
            );
        }
        let separator = if self.options.path.contains('?') {
            '&'
        } else {
            '?'
        };
        let fragment = self
            .options
            .fragment
            .as_ref()
            .map(|f| format!("#{f}"))
            .unwrap_or_default();
        format!(
            "{}{separator}{}{fragment}",
            self.options.path,
            Arr::query(&Value::Object(parameters))
        )
    }

    /// The URL of the previous page.
    pub fn previous_page_url(&self) -> Option<String> {
        self.previous_cursor().map(|cursor| self.url(Some(&cursor)))
    }

    /// The URL of the next page.
    pub fn next_page_url(&self) -> Option<String> {
        self.next_cursor().map(|cursor| self.url(Some(&cursor)))
    }

    /// The base path of the paginator's URLs.
    pub fn path(&self) -> &str {
        &self.options.path
    }

    /// Use a different base path for the URLs.
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.options.path = path.into();
        self
    }

    /// Add a query string value to the URLs (the cursor name is reserved).
    pub fn append(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        let key = key.into();
        if key != self.options.cursor_name {
            self.options.query.insert(key, value.into());
        }
        self
    }

    /// Add several query string values to the URLs.
    pub fn appends(mut self, values: Map<String, Value>) -> Self {
        for (key, value) in values {
            self = self.append(key, value);
        }
        self
    }

    /// Add the current request's query string to the URLs.
    pub fn with_query_string(self) -> Self {
        self.appends(query_string())
    }

    /// Set the URL fragment.
    pub fn fragment(mut self, fragment: impl Into<String>) -> Self {
        self.options.fragment = Some(fragment.into());
        self
    }

    /// The paginator's options.
    pub fn get_options(&self) -> &CursorPaginatorOptions {
        &self.options
    }

    /// Render "Previous" / "Next" links (Tailwind CSS markup).
    pub fn links(&self) -> HtmlString {
        if !self.has_pages() {
            return HtmlString::new(String::new());
        }
        let button = |url: Option<String>, label: &str, rel: &str| match url {
            Some(url) => format!(
                "<a href=\"{}\" rel=\"{rel}\" class=\"{BUTTON}\">{label}</a>",
                e(&url)
            ),
            None => format!("<span class=\"{DISABLED_BUTTON}\">{label}</span>"),
        };
        HtmlString::new(format!(
            "<nav role=\"navigation\" aria-label=\"{}\" class=\"flex gap-2 items-center justify-between\">{}{}</nav>",
            e(translate("Pagination Navigation")),
            button(
                self.previous_page_url(),
                &translate("pagination.previous"),
                "prev"
            ),
            button(self.next_page_url(), &translate("pagination.next"), "next"),
        ))
    }
}

const DISABLED_BUTTON: &str = "inline-flex items-center px-4 py-2 text-sm font-medium text-gray-600 bg-white border border-gray-300 cursor-not-allowed leading-5 rounded-md dark:text-gray-300 dark:bg-gray-700 dark:border-gray-600";
const BUTTON: &str = "inline-flex items-center px-4 py-2 text-sm font-medium text-gray-800 bg-white border border-gray-300 leading-5 rounded-md hover:text-gray-700 focus:outline-none focus:ring ring-gray-300 focus:border-blue-300 active:bg-gray-100 active:text-gray-800 transition ease-in-out duration-150 dark:bg-gray-800 dark:border-gray-600 dark:text-gray-200 dark:focus:border-blue-700 dark:active:bg-gray-700 dark:active:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-900 dark:hover:text-gray-200";

impl<T: Serialize> CursorPaginator<T> {
    /// The paginator as a value, in Laravel's JSON shape.
    pub fn to_array(&self) -> Value {
        json!({
            "data": illuminate_support::to_value(&self.items),
            "path": self.path(),
            "per_page": self.per_page,
            "next_cursor": self.next_cursor().map(|c| c.encode()),
            "next_page_url": self.next_page_url(),
            "prev_cursor": self.previous_cursor().map(|c| c.encode()),
            "prev_page_url": self.previous_page_url(),
        })
    }
}

impl<T: Serialize> Serialize for CursorPaginator<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.to_array().serialize(serializer)
    }
}

impl<T> IntoIterator for CursorPaginator<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<T: Serialize> From<CursorPaginator<T>> for Value {
    fn from(paginator: CursorPaginator<T>) -> Self {
        paginator.to_array()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_round_trip_through_their_encoding() {
        let cursor = Cursor::new(json!({"id": 15, "name": "Taylor"}), false);
        let encoded = cursor.encode();
        assert!(!encoded.contains('='));
        assert_eq!(Cursor::from_encoded(&encoded), Some(cursor.clone()));
        assert_eq!(cursor.parameter("id").unwrap(), json!(15));
        assert!(cursor.points_to_previous_items());
        assert_eq!(
            cursor.parameter("email").unwrap_err().to_string(),
            "Unable to find parameter [email] in pagination item."
        );
        assert_eq!(Cursor::from_encoded("not a cursor"), None);
        // Laravel's encoding: base64 of the JSON, URL-safe, without padding.
        assert_eq!(
            Cursor::new(json!({"id": 1}), true).encode(),
            "eyJpZCI6MSwiX3BvaW50c1RvTmV4dEl0ZW1zIjp0cnVlfQ"
        );
    }

    #[test]
    fn paginators_know_their_neighbours() {
        let options = CursorPaginatorOptions {
            path: "/users".into(),
            parameters: vec!["id".into()],
            ..Default::default()
        };
        let items = vec![json!({"id": 1}), json!({"id": 2}), json!({"id": 3})];
        let paginator = CursorPaginator::new(items.clone(), 2, None, options.clone());
        assert_eq!(paginator.count(), 2);
        assert!(paginator.on_first_page());
        assert!(paginator.has_more_pages());
        assert_eq!(paginator.previous_cursor(), None);
        assert_eq!(
            paginator.next_cursor(),
            Some(Cursor::new(json!({"id": 2}), true))
        );
        let next = paginator.next_page_url().unwrap();
        assert!(next.starts_with("/users?cursor="));

        // Going back: the items arrive in reverse order.
        let back = Cursor::new(json!({"id": 3}), false);
        let paginator = CursorPaginator::new(
            vec![json!({"id": 2}), json!({"id": 1})],
            2,
            Some(back),
            options,
        );
        assert_eq!(paginator.items()[0], json!({"id": 1}));
        assert!(paginator.on_first_page());
        assert_eq!(paginator.previous_cursor(), None);
        assert_eq!(
            paginator.next_cursor(),
            Some(Cursor::new(json!({"id": 2}), true))
        );
        let array = paginator.to_array();
        assert_eq!(array["per_page"], json!(2));
        assert_eq!(array["prev_cursor"], Value::Null);
        assert!(paginator.links().to_string().contains("rel=\"next\""));
    }
}
