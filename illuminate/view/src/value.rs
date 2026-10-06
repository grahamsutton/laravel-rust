//! The values Blade templates work with.
//!
//! PHP templates operate on PHP values: scalars, ordered arrays, objects and
//! closures. [`ViewValue`] is the Rust spelling of that idea. View data
//! handed to a view as JSON is converted into view values once (arrays are
//! reference counted, so passing them around a template is cheap), and rich
//! objects — component attribute bags, slots, error bags, dates, or your own
//! types — participate through the [`ViewObject`] trait.
//!
//! ```
//! use illuminate_view::ViewValue;
//! use illuminate_support::json;
//!
//! let user = ViewValue::from(json!({"name": "Taylor", "roles": ["admin"]}));
//!
//! assert_eq!(user.get("name"), Some(ViewValue::from("Taylor")));
//! assert!(user.truthy());
//! assert_eq!(user.to_json(), json!({"name": "Taylor", "roles": ["admin"]}));
//! ```

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use illuminate_support::{Carbon, HtmlString, Map, Result, Value};
use indexmap::IndexMap;

use crate::php;

/// A callable registered with Blade: receives the evaluated arguments and
/// returns a value.
pub type ViewFunction = Arc<dyn Fn(&[ViewValue]) -> Result<ViewValue> + Send + Sync>;

/// A closure value, such as an arrow function written in a template
/// (`fn ($user) => $user->name`) or a callable handed to the view.
#[derive(Clone)]
pub struct ViewClosure(ViewFunction);

impl ViewClosure {
    /// Wrap a Rust closure so it may be called from a template.
    pub fn new(
        callback: impl Fn(&[ViewValue]) -> Result<ViewValue> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(callback))
    }

    /// Wrap an already shared callable.
    pub fn from_arc(callback: ViewFunction) -> Self {
        Self(callback)
    }

    /// Invoke the closure.
    pub fn call(&self, args: &[ViewValue]) -> Result<ViewValue> {
        (self.0)(args)
    }

    /// Determine if two closures are the same closure.
    pub fn ptr_eq(&self, other: &ViewClosure) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for ViewClosure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Closure")
    }
}

// ----------------------------------------------------------------------
// Array keys
// ----------------------------------------------------------------------

/// The key of a PHP array: an integer or a string.
///
/// Like PHP, strings holding canonical integers (`"7"`, but not `"07"`) are
/// stored as integer keys.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArrayKey {
    /// An integer key.
    Int(i64),
    /// A string key.
    Str(Arc<str>),
}

impl ArrayKey {
    /// Build a key from a string, normalizing integer-like strings.
    pub fn new(key: &str) -> Self {
        match canonical_int(key) {
            Some(i) => ArrayKey::Int(i),
            None => ArrayKey::Str(key.into()),
        }
    }

    /// Build a key from an arbitrary value using PHP's key casting rules.
    pub fn from_value(value: &ViewValue) -> Result<Self> {
        Ok(match value {
            ViewValue::Int(i) => ArrayKey::Int(*i),
            ViewValue::Str(s) | ViewValue::Html(s) => ArrayKey::new(s),
            ViewValue::Bool(b) => ArrayKey::Int(*b as i64),
            ViewValue::Float(f) => ArrayKey::Int(*f as i64),
            ViewValue::Null => ArrayKey::Str("".into()),
            other => {
                return Err(crate::exception::TypeError::new(format!(
                    "Illegal offset type: {}",
                    other.type_name()
                ))
                .into());
            }
        })
    }

    /// The key as a view value.
    pub fn to_value(&self) -> ViewValue {
        match self {
            ArrayKey::Int(i) => ViewValue::Int(*i),
            ArrayKey::Str(s) => ViewValue::Str(s.clone()),
        }
    }

    /// Determine if the key is a string key.
    pub fn is_str(&self) -> bool {
        matches!(self, ArrayKey::Str(_))
    }

    fn as_ref(&self) -> KeyRef<'_> {
        match self {
            ArrayKey::Int(i) => KeyRef::Int(*i),
            ArrayKey::Str(s) => KeyRef::Str(s),
        }
    }
}

impl Hash for ArrayKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state)
    }
}

impl fmt::Display for ArrayKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArrayKey::Int(i) => write!(f, "{i}"),
            ArrayKey::Str(s) => f.write_str(s),
        }
    }
}

impl fmt::Debug for ArrayKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArrayKey::Int(i) => write!(f, "{i}"),
            ArrayKey::Str(s) => write!(f, "{s:?}"),
        }
    }
}

impl From<&str> for ArrayKey {
    fn from(value: &str) -> Self {
        ArrayKey::new(value)
    }
}

impl From<String> for ArrayKey {
    fn from(value: String) -> Self {
        ArrayKey::new(&value)
    }
}

impl From<i64> for ArrayKey {
    fn from(value: i64) -> Self {
        ArrayKey::Int(value)
    }
}

impl From<usize> for ArrayKey {
    fn from(value: usize) -> Self {
        ArrayKey::Int(value as i64)
    }
}

/// A borrowed array key, used for allocation-free lookups.
#[derive(Clone, Copy)]
enum KeyRef<'a> {
    Int(i64),
    Str(&'a str),
}

impl Hash for KeyRef<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            KeyRef::Int(i) => {
                state.write_u8(0);
                i.hash(state);
            }
            KeyRef::Str(s) => {
                state.write_u8(1);
                s.hash(state);
            }
        }
    }
}

impl indexmap::Equivalent<ArrayKey> for KeyRef<'_> {
    fn equivalent(&self, key: &ArrayKey) -> bool {
        match (self, key) {
            (KeyRef::Int(a), ArrayKey::Int(b)) => a == b,
            (KeyRef::Str(a), ArrayKey::Str(b)) => *a == &**b,
            _ => false,
        }
    }
}

/// Parse a canonical decimal integer string ("12", "-3", but not "012").
pub(crate) fn canonical_int(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > 20 {
        return None;
    }
    let digits = if bytes[0] == b'-' { &bytes[1..] } else { bytes };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if digits.len() > 1 && digits[0] == b'0' {
        return None;
    }
    if bytes[0] == b'-' && digits == b"0" {
        return None;
    }
    s.parse::<i64>().ok()
}

// ----------------------------------------------------------------------
// Arrays
// ----------------------------------------------------------------------

/// An ordered PHP array: a list, a map, or a little of both.
#[derive(Clone, Default)]
pub struct ViewArray {
    entries: IndexMap<ArrayKey, ViewValue>,
    next_index: i64,
}

impl ViewArray {
    /// Create an empty array.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty array with room for `capacity` items.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: IndexMap::with_capacity(capacity),
            next_index: 0,
        }
    }

    /// Create a list from the given values.
    pub fn from_list(items: impl IntoIterator<Item = ViewValue>) -> Self {
        let mut array = Self::new();
        for item in items {
            array.push(item);
        }
        array
    }

    /// Create an array from key / value pairs.
    pub fn from_pairs<K: Into<ArrayKey>>(pairs: impl IntoIterator<Item = (K, ViewValue)>) -> Self {
        let mut array = Self::new();
        for (key, value) in pairs {
            array.insert(key.into(), value);
        }
        array
    }

    /// The number of items in the array.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Determine if the array is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Append a value with the next integer key (`$array[] = $value`).
    pub fn push(&mut self, value: ViewValue) {
        let key = self.next_index;
        self.entries.insert(ArrayKey::Int(key), value);
        self.next_index = key.saturating_add(1);
    }

    /// Set the value for a key.
    pub fn insert(&mut self, key: ArrayKey, value: ViewValue) {
        if let ArrayKey::Int(i) = key
            && i >= self.next_index
        {
            self.next_index = i.saturating_add(1);
        }
        self.entries.insert(key, value);
    }

    /// Set the value for a string key (integer-like strings become integers).
    pub fn set(&mut self, key: &str, value: impl Into<ViewValue>) {
        self.insert(ArrayKey::new(key), value.into());
    }

    /// Get the value for a key.
    pub fn get(&self, key: &ArrayKey) -> Option<&ViewValue> {
        self.entries.get(&key.as_ref())
    }

    /// Get the value for a string key (integer-like strings are normalized).
    pub fn get_str(&self, key: &str) -> Option<&ViewValue> {
        match canonical_int(key) {
            Some(i) => self.entries.get(&KeyRef::Int(i)),
            None => self.entries.get(&KeyRef::Str(key)),
        }
    }

    /// Get the value at an integer key.
    pub fn get_int(&self, key: i64) -> Option<&ViewValue> {
        self.entries.get(&KeyRef::Int(key))
    }

    /// Get the value for a key given as a view value.
    pub fn get_value(&self, key: &ViewValue) -> Option<&ViewValue> {
        match key {
            ViewValue::Int(i) => self.get_int(*i),
            ViewValue::Str(s) | ViewValue::Html(s) => self.get_str(s),
            other => ArrayKey::from_value(other).ok().and_then(|k| self.get(&k)),
        }
    }

    /// Get a mutable reference to the value for a key.
    pub fn get_mut(&mut self, key: &ArrayKey) -> Option<&mut ViewValue> {
        self.entries.get_mut(&key.as_ref())
    }

    /// Determine if the key exists.
    pub fn contains_key(&self, key: &str) -> bool {
        self.get_str(key).is_some()
    }

    /// Remove a key, preserving the order of the remaining items.
    pub fn remove(&mut self, key: &ArrayKey) -> Option<ViewValue> {
        self.entries.shift_remove(&key.as_ref())
    }

    /// Iterate over the key / value pairs.
    pub fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&ArrayKey, &ViewValue)> + ExactSizeIterator {
        self.entries.iter()
    }

    /// Iterate over the keys.
    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &ArrayKey> + ExactSizeIterator {
        self.entries.keys()
    }

    /// Iterate over the values.
    pub fn values(&self) -> impl DoubleEndedIterator<Item = &ViewValue> + ExactSizeIterator {
        self.entries.values()
    }

    /// The first value.
    pub fn first(&self) -> Option<&ViewValue> {
        self.entries.first().map(|(_, v)| v)
    }

    /// The last value.
    pub fn last(&self) -> Option<&ViewValue> {
        self.entries.last().map(|(_, v)| v)
    }

    /// Determine if the keys are `0..n` in order (a JSON array).
    pub fn is_list(&self) -> bool {
        self.entries
            .keys()
            .enumerate()
            .all(|(i, key)| matches!(key, ArrayKey::Int(k) if *k == i as i64))
    }

    /// Convert the array into JSON.
    pub fn to_json(&self) -> Value {
        if self.is_list() {
            Value::Array(self.values().map(ViewValue::to_json).collect())
        } else {
            let mut map = Map::with_capacity(self.len());
            for (key, value) in self.iter() {
                map.insert(key.to_string(), value.to_json());
            }
            Value::Object(map)
        }
    }
}

impl fmt::Debug for ViewArray {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.entries.iter()).finish()
    }
}

impl FromIterator<ViewValue> for ViewArray {
    fn from_iter<T: IntoIterator<Item = ViewValue>>(iter: T) -> Self {
        Self::from_list(iter)
    }
}

impl FromIterator<(ArrayKey, ViewValue)> for ViewArray {
    fn from_iter<T: IntoIterator<Item = (ArrayKey, ViewValue)>>(iter: T) -> Self {
        Self::from_pairs(iter)
    }
}

// ----------------------------------------------------------------------
// Objects
// ----------------------------------------------------------------------

/// Rich objects that templates may interact with.
///
/// Implement this trait to hand your own types to a view. Every method has a
/// sensible default, so implement only what your object supports:
///
/// ```
/// use illuminate_view::{ViewObject, ViewValue};
/// use illuminate_support::Result;
///
/// struct Podcast { title: String }
///
/// impl ViewObject for Podcast {
///     fn class_name(&self) -> &str { "Podcast" }
///
///     fn get(&self, property: &str) -> Option<ViewValue> {
///         (property == "title").then(|| self.title.as_str().into())
///     }
///
///     fn call(&self, method: &str, _args: &[ViewValue]) -> Option<Result<ViewValue>> {
///         match method {
///             "shout" => Some(Ok(self.title.to_uppercase().into())),
///             _ => None,
///         }
///     }
/// }
///
/// let html = illuminate_view::Factory::new(Vec::<String>::new())
///     .render_inline(
///         "{{ $podcast->title }} / {{ $podcast->shout() }}",
///         illuminate_view::data([("podcast", ViewValue::object(Podcast { title: "Laravel".into() }))]),
///     )
///     .unwrap();
///
/// assert_eq!(html, "Laravel / LARAVEL");
/// ```
pub trait ViewObject: Any + Send + Sync {
    /// The object's class name, used in error messages.
    fn class_name(&self) -> &str {
        "object"
    }

    /// Read a property: `$object->name`. Return `None` when the property
    /// does not exist.
    fn get(&self, _property: &str) -> Option<ViewValue> {
        None
    }

    /// Call a method: `$object->name(...)`. Return `None` when the object
    /// has no such method.
    fn call(&self, _method: &str, _args: &[ViewValue]) -> Option<Result<ViewValue>> {
        None
    }

    /// Invoke the object itself: `$object(...)`.
    fn invoke(&self, _args: &[ViewValue]) -> Option<Result<ViewValue>> {
        None
    }

    /// Read an offset: `$object['key']`.
    fn offset_get(&self, _key: &ViewValue) -> Option<ViewValue> {
        None
    }

    /// The object's HTML. Objects that return HTML (like Laravel's
    /// `Htmlable`) are echoed by `{{ }}` without being escaped.
    fn to_html(&self) -> Option<String> {
        None
    }

    /// The object's string form (`__toString`). Return `None` when the
    /// object can't be converted to a string.
    fn to_string_value(&self) -> Option<String> {
        self.to_html()
    }

    /// The object's truthiness. Objects are truthy by default.
    fn truthy(&self) -> bool {
        true
    }

    /// The object's count, when it is countable.
    fn count(&self) -> Option<usize> {
        None
    }

    /// The key / value pairs to iterate with `@foreach`, when iterable.
    fn iterate(&self) -> Option<Vec<(ViewValue, ViewValue)>> {
        None
    }

    /// The object's JSON representation.
    fn to_json(&self) -> Value {
        Value::Object(Map::new())
    }
}

impl dyn ViewObject {
    /// Downcast the object to a concrete type.
    pub fn downcast_ref<T: ViewObject>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref::<T>()
    }

    /// Determine if the object is of the given concrete type.
    pub fn is<T: ViewObject>(&self) -> bool {
        (self as &dyn Any).is::<T>()
    }
}

// ----------------------------------------------------------------------
// Values
// ----------------------------------------------------------------------

/// A value inside a Blade template.
#[derive(Clone, Default)]
pub enum ViewValue {
    /// `null`
    #[default]
    Null,
    /// `true` / `false`
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A float.
    Float(f64),
    /// A string.
    Str(Arc<str>),
    /// An ordered array (lists, maps, collections, serialized models...).
    Array(Arc<ViewArray>),
    /// A string of HTML that is echoed without escaping (`HtmlString`).
    Html(Arc<str>),
    /// A rich object.
    Object(Arc<dyn ViewObject>),
    /// A closure.
    Closure(ViewClosure),
}

impl ViewValue {
    /// Create an HTML string, which `{{ }}` echoes without escaping.
    pub fn html(html: impl Into<String>) -> Self {
        ViewValue::Html(html.into().into())
    }

    /// Wrap an object.
    pub fn object(object: impl ViewObject) -> Self {
        ViewValue::Object(Arc::new(object))
    }

    /// Wrap a closure.
    pub fn closure(
        callback: impl Fn(&[ViewValue]) -> Result<ViewValue> + Send + Sync + 'static,
    ) -> Self {
        ViewValue::Closure(ViewClosure::new(callback))
    }

    /// Create a list.
    pub fn list(items: impl IntoIterator<Item = ViewValue>) -> Self {
        ViewValue::Array(Arc::new(ViewArray::from_list(items)))
    }

    /// Create an associative array.
    pub fn map<K: Into<ArrayKey>>(pairs: impl IntoIterator<Item = (K, ViewValue)>) -> Self {
        ViewValue::Array(Arc::new(ViewArray::from_pairs(pairs)))
    }

    /// An empty array.
    pub fn empty_array() -> Self {
        ViewValue::Array(Arc::new(ViewArray::new()))
    }

    /// Convert any serializable value.
    pub fn from_serialize<T: serde::Serialize + ?Sized>(value: &T) -> Self {
        ViewValue::from(illuminate_support::to_value(value))
    }

    /// Determine if the value is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, ViewValue::Null)
    }

    /// PHP truthiness: `null`, `false`, `0`, `0.0`, `""`, `"0"` and empty
    /// arrays are falsy.
    pub fn truthy(&self) -> bool {
        match self {
            ViewValue::Null => false,
            ViewValue::Bool(b) => *b,
            ViewValue::Int(i) => *i != 0,
            ViewValue::Float(f) => *f != 0.0,
            ViewValue::Str(s) | ViewValue::Html(s) => !(s.is_empty() || &**s == "0"),
            ViewValue::Array(a) => !a.is_empty(),
            ViewValue::Object(o) => o.truthy(),
            ViewValue::Closure(_) => true,
        }
    }

    /// The PHP type name of the value (`string`, `int`, `array`, ...).
    pub fn type_name(&self) -> String {
        match self {
            ViewValue::Null => "null".into(),
            ViewValue::Bool(_) => "bool".into(),
            ViewValue::Int(_) => "int".into(),
            ViewValue::Float(_) => "float".into(),
            ViewValue::Str(_) => "string".into(),
            ViewValue::Array(_) => "array".into(),
            ViewValue::Html(_) => "Illuminate\\Support\\HtmlString".into(),
            ViewValue::Object(o) => o.class_name().to_string(),
            ViewValue::Closure(_) => "Closure".into(),
        }
    }

    /// Borrow the string, when the value is a string (or HTML string).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ViewValue::Str(s) | ViewValue::Html(s) => Some(s),
            _ => None,
        }
    }

    /// Borrow the array, when the value is an array.
    pub fn as_array(&self) -> Option<&ViewArray> {
        match self {
            ViewValue::Array(a) => Some(a),
            _ => None,
        }
    }

    /// Borrow the object, when the value is an object.
    pub fn as_object(&self) -> Option<&Arc<dyn ViewObject>> {
        match self {
            ViewValue::Object(o) => Some(o),
            _ => None,
        }
    }

    /// Downcast an object value to a concrete type.
    pub fn downcast_ref<T: ViewObject>(&self) -> Option<&T> {
        self.as_object().and_then(|o| o.downcast_ref::<T>())
    }

    /// The value as an integer, if it is numeric.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            ViewValue::Int(i) => Some(*i),
            ViewValue::Float(f) => Some(*f as i64),
            ViewValue::Bool(b) => Some(*b as i64),
            ViewValue::Str(s) => php::parse_numeric(s).map(|n| n.as_i64()),
            _ => None,
        }
    }

    /// The value as a float, if it is numeric.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ViewValue::Int(i) => Some(*i as f64),
            ViewValue::Float(f) => Some(*f),
            ViewValue::Bool(b) => Some(*b as i64 as f64),
            ViewValue::Str(s) => php::parse_numeric(s).map(|n| n.as_f64()),
            _ => None,
        }
    }

    /// Read a key (arrays) or property (objects).
    pub fn get(&self, key: &str) -> Option<ViewValue> {
        match self {
            ViewValue::Array(a) => a.get_str(key).cloned(),
            ViewValue::Object(o) => o.get(key).or_else(|| o.offset_get(&ViewValue::from(key))),
            _ => None,
        }
    }

    /// Convert the value to a string the way PHP's string cast would
    /// (`null` → `""`, `true` → `"1"`, floats with PHP's precision).
    /// Arrays are rendered as JSON.
    pub fn to_string_lossy(&self) -> String {
        match self {
            ViewValue::Null => String::new(),
            ViewValue::Bool(true) => "1".into(),
            ViewValue::Bool(false) => String::new(),
            ViewValue::Int(i) => i.to_string(),
            ViewValue::Float(f) => php::float_to_string(*f),
            ViewValue::Str(s) | ViewValue::Html(s) => s.to_string(),
            ViewValue::Array(_) => php::json_encode(self, 0).unwrap_or_default(),
            ViewValue::Object(o) => o
                .to_string_value()
                .unwrap_or_else(|| serde_json::to_string(&o.to_json()).unwrap_or_default()),
            ViewValue::Closure(_) => "Closure".into(),
        }
    }

    /// The count of the value: array length, or an object's count.
    pub fn count(&self) -> usize {
        match self {
            ViewValue::Null => 0,
            ViewValue::Array(a) => a.len(),
            ViewValue::Object(o) => o.count().unwrap_or(1),
            _ => 1,
        }
    }

    /// Convert the value into JSON.
    pub fn to_json(&self) -> Value {
        match self {
            ViewValue::Null => Value::Null,
            ViewValue::Bool(b) => Value::Bool(*b),
            ViewValue::Int(i) => Value::from(*i),
            ViewValue::Float(f) => serde_json::Number::from_f64(*f)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            ViewValue::Str(s) | ViewValue::Html(s) => Value::String(s.to_string()),
            ViewValue::Array(a) => a.to_json(),
            ViewValue::Object(o) => o.to_json(),
            ViewValue::Closure(_) => Value::Object(Map::new()),
        }
    }
}

impl fmt::Debug for ViewValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViewValue::Null => f.write_str("null"),
            ViewValue::Bool(b) => write!(f, "{b}"),
            ViewValue::Int(i) => write!(f, "{i}"),
            ViewValue::Float(x) => write!(f, "{x:?}"),
            ViewValue::Str(s) => write!(f, "{s:?}"),
            ViewValue::Array(a) => write!(f, "{a:?}"),
            ViewValue::Html(s) => write!(f, "Html({s:?})"),
            ViewValue::Object(o) => write!(f, "{}", o.class_name()),
            ViewValue::Closure(_) => f.write_str("Closure"),
        }
    }
}

/// Values compare with PHP's strict (`===`) semantics.
impl PartialEq for ViewValue {
    fn eq(&self, other: &Self) -> bool {
        php::strict_eq(self, other)
    }
}

impl fmt::Display for ViewValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}

// ----------------------------------------------------------------------
// Conversions
// ----------------------------------------------------------------------

impl From<Value> for ViewValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Null => ViewValue::Null,
            Value::Bool(b) => ViewValue::Bool(b),
            Value::Number(n) => number_to_view(&n),
            Value::String(s) => ViewValue::Str(s.into()),
            Value::Array(items) => {
                let mut array = ViewArray::with_capacity(items.len());
                for item in items {
                    array.push(ViewValue::from(item));
                }
                ViewValue::Array(Arc::new(array))
            }
            Value::Object(map) => {
                let mut array = ViewArray::with_capacity(map.len());
                for (key, item) in map {
                    array.insert(ArrayKey::new(&key), ViewValue::from(item));
                }
                ViewValue::Array(Arc::new(array))
            }
        }
    }
}

impl From<&Value> for ViewValue {
    fn from(value: &Value) -> Self {
        match value {
            Value::Null => ViewValue::Null,
            Value::Bool(b) => ViewValue::Bool(*b),
            Value::Number(n) => number_to_view(n),
            Value::String(s) => ViewValue::Str(s.as_str().into()),
            Value::Array(items) => {
                ViewValue::Array(Arc::new(items.iter().map(ViewValue::from).collect()))
            }
            Value::Object(map) => {
                let mut array = ViewArray::with_capacity(map.len());
                for (key, item) in map {
                    array.insert(ArrayKey::new(key), ViewValue::from(item));
                }
                ViewValue::Array(Arc::new(array))
            }
        }
    }
}

fn number_to_view(n: &serde_json::Number) -> ViewValue {
    if let Some(i) = n.as_i64() {
        ViewValue::Int(i)
    } else {
        ViewValue::Float(n.as_f64().unwrap_or(0.0))
    }
}

impl From<bool> for ViewValue {
    fn from(value: bool) -> Self {
        ViewValue::Bool(value)
    }
}

macro_rules! from_int {
    ($($t:ty),*) => {
        $(impl From<$t> for ViewValue {
            fn from(value: $t) -> Self {
                ViewValue::Int(value as i64)
            }
        })*
    };
}

from_int!(i8, i16, i32, i64, u8, u16, u32);

impl From<u64> for ViewValue {
    fn from(value: u64) -> Self {
        i64::try_from(value)
            .map(ViewValue::Int)
            .unwrap_or(ViewValue::Float(value as f64))
    }
}

impl From<usize> for ViewValue {
    fn from(value: usize) -> Self {
        ViewValue::from(value as u64)
    }
}

impl From<f64> for ViewValue {
    fn from(value: f64) -> Self {
        ViewValue::Float(value)
    }
}

impl From<f32> for ViewValue {
    fn from(value: f32) -> Self {
        ViewValue::Float(value as f64)
    }
}

impl From<&str> for ViewValue {
    fn from(value: &str) -> Self {
        ViewValue::Str(value.into())
    }
}

impl From<String> for ViewValue {
    fn from(value: String) -> Self {
        ViewValue::Str(value.into())
    }
}

impl From<&String> for ViewValue {
    fn from(value: &String) -> Self {
        ViewValue::Str(value.as_str().into())
    }
}

impl From<Arc<str>> for ViewValue {
    fn from(value: Arc<str>) -> Self {
        ViewValue::Str(value)
    }
}

impl From<HtmlString> for ViewValue {
    fn from(value: HtmlString) -> Self {
        ViewValue::Html(value.0.into())
    }
}

impl From<ViewArray> for ViewValue {
    fn from(value: ViewArray) -> Self {
        ViewValue::Array(Arc::new(value))
    }
}

impl From<ViewClosure> for ViewValue {
    fn from(value: ViewClosure) -> Self {
        ViewValue::Closure(value)
    }
}

impl From<Arc<dyn ViewObject>> for ViewValue {
    fn from(value: Arc<dyn ViewObject>) -> Self {
        ViewValue::Object(value)
    }
}

impl From<Carbon> for ViewValue {
    fn from(value: Carbon) -> Self {
        ViewValue::object(crate::objects::DateObject(value))
    }
}

impl From<()> for ViewValue {
    fn from(_: ()) -> Self {
        ViewValue::Null
    }
}

impl<T: Into<ViewValue>> From<Option<T>> for ViewValue {
    fn from(value: Option<T>) -> Self {
        value.map(Into::into).unwrap_or(ViewValue::Null)
    }
}

impl<T: Into<ViewValue>> From<Vec<T>> for ViewValue {
    fn from(value: Vec<T>) -> Self {
        ViewValue::list(value.into_iter().map(Into::into))
    }
}

impl<K: Into<String>, T: Into<ViewValue>> From<IndexMap<K, T>> for ViewValue {
    fn from(value: IndexMap<K, T>) -> Self {
        ViewValue::map(
            value
                .into_iter()
                .map(|(k, v)| (ArrayKey::new(&k.into()), v.into())),
        )
    }
}

impl<K: Into<String>, T: Into<ViewValue>> From<BTreeMap<K, T>> for ViewValue {
    fn from(value: BTreeMap<K, T>) -> Self {
        ViewValue::map(
            value
                .into_iter()
                .map(|(k, v)| (ArrayKey::new(&k.into()), v.into())),
        )
    }
}

impl<K: Into<String>, T: Into<ViewValue>> From<HashMap<K, T>> for ViewValue {
    fn from(value: HashMap<K, T>) -> Self {
        ViewValue::map(
            value
                .into_iter()
                .map(|(k, v)| (ArrayKey::new(&k.into()), v.into())),
        )
    }
}

impl From<&ViewValue> for ViewValue {
    fn from(value: &ViewValue) -> Self {
        value.clone()
    }
}

impl From<ViewValue> for Value {
    fn from(value: ViewValue) -> Self {
        value.to_json()
    }
}

/// The data handed to a view: variable names mapped to values.
///
/// `ViewData` dereferences to an `IndexMap<String, ViewValue>`, so all of
/// the map methods (`insert`, `get`, `iter`, ...) are available.
#[derive(Clone, Default)]
pub struct ViewData(IndexMap<String, ViewValue>);

impl ViewData {
    /// Create empty view data.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create empty view data with room for `capacity` entries.
    pub fn with_capacity(capacity: usize) -> Self {
        Self(IndexMap::with_capacity(capacity))
    }

    /// Unwrap the underlying map.
    pub fn into_inner(self) -> IndexMap<String, ViewValue> {
        self.0
    }

    /// The data as JSON.
    pub fn to_json(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(k, v)| (k.clone(), v.to_json()))
                .collect(),
        )
    }
}

impl std::ops::Deref for ViewData {
    type Target = IndexMap<String, ViewValue>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ViewData {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl fmt::Debug for ViewData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromIterator<(String, ViewValue)> for ViewData {
    fn from_iter<T: IntoIterator<Item = (String, ViewValue)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl IntoIterator for ViewData {
    type Item = (String, ViewValue);
    type IntoIter = indexmap::map::IntoIter<String, ViewValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a ViewData {
    type Item = (&'a String, &'a ViewValue);
    type IntoIter = indexmap::map::Iter<'a, String, ViewValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl From<IndexMap<String, ViewValue>> for ViewData {
    fn from(map: IndexMap<String, ViewValue>) -> Self {
        Self(map)
    }
}

impl From<ViewData> for ViewValue {
    fn from(data: ViewData) -> Self {
        ViewValue::map(data.0.into_iter().map(|(k, v)| (ArrayKey::new(&k), v)))
    }
}

/// Build view data from name / value pairs.
///
/// ```
/// use illuminate_view::{data, ViewValue};
///
/// let data = data([("name", ViewValue::from("Taylor")), ("admin", true.into())]);
/// assert_eq!(data.len(), 2);
/// ```
pub fn data<K: Into<String>, V: Into<ViewValue>>(
    pairs: impl IntoIterator<Item = (K, V)>,
) -> ViewData {
    pairs
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect()
}

/// Convert serializable data (a JSON object) into view data.
pub(crate) fn view_data_from_serialize<T: serde::Serialize + ?Sized>(data: &T) -> ViewData {
    match illuminate_support::to_value(data) {
        Value::Object(map) => map
            .into_iter()
            .map(|(k, v)| (k, ViewValue::from(v)))
            .collect(),
        _ => ViewData::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn integer_like_keys_are_normalized() {
        let value = ViewValue::from(json!({"1": "a", "01": "b", "-2": "c"}));
        let array = value.as_array().unwrap();
        assert_eq!(array.get_int(1).unwrap().as_str(), Some("a"));
        assert_eq!(array.get_str("01").unwrap().as_str(), Some("b"));
        assert_eq!(array.get_int(-2).unwrap().as_str(), Some("c"));
        assert!(array.get_str("1").is_some());
    }

    #[test]
    fn lists_round_trip_through_json() {
        let original =
            json!({"users": [{"name": "Taylor"}, {"name": "Abigail"}], "total": 2, "ratio": 0.5});
        assert_eq!(ViewValue::from(original.clone()).to_json(), original);
    }

    #[test]
    fn push_uses_the_next_integer_key() {
        let mut array = ViewArray::new();
        array.insert(ArrayKey::Int(5), 1.into());
        array.push(2.into());
        assert!(array.get_int(6).is_some());
        assert!(!array.is_list());
    }

    #[test]
    fn values_have_php_truthiness() {
        assert!(!ViewValue::from("0").truthy());
        assert!(!ViewValue::from(0.0).truthy());
        assert!(!ViewValue::empty_array().truthy());
        assert!(ViewValue::from("false").truthy());
        assert!(ViewValue::html("<b>").truthy());
    }
}
