//! Fluent rule builders that compile down to string rules, exactly like
//! Laravel's `Stringable` rule objects (`In`, `Dimensions`, `Date`, ...).

use std::marker::PhantomData;

use illuminate_support::{Carbon, Conditionable, Value, ValueExt};

use super::{IntoRuleItems, RuleItem, RuleSet, quote_values};

fn stringify<V: Into<Value>>(values: impl IntoIterator<Item = V>) -> Vec<String> {
    values.into_iter().map(|v| v.into().to_string_lossy()).collect()
}

fn str_items(rules: Vec<String>) -> Vec<RuleItem> {
    rules.into_iter().map(RuleItem::Str).collect()
}

fn unique(rules: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(rules.len());
    for rule in rules {
        if !out.contains(&rule) {
            out.push(rule);
        }
    }
    out
}

macro_rules! list_rule {
    ($(#[$doc:meta])* $name:ident, $rule:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq)]
        pub struct $name {
            values: Vec<String>,
        }

        impl $name {
            /// Create the rule from a list of values.
            pub fn new<V: Into<Value>>(values: impl IntoIterator<Item = V>) -> Self {
                Self { values: stringify(values) }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}:{}", $rule, quote_values(&self.values))
            }
        }

        impl IntoRuleItems for $name {
            fn into_rule_items(self) -> Vec<RuleItem> {
                vec![RuleItem::Str(self.to_string())]
            }
        }
    };
}

list_rule!(
    /// The field must be one of the given values (`Rule::in_`).
    In,
    "in"
);
list_rule!(
    /// The field must not be one of the given values (`Rule::not_in`).
    NotIn,
    "not_in"
);
list_rule!(
    /// The array must contain all of the given values (`Rule::contains`).
    Contains,
    "contains"
);
list_rule!(
    /// The array must contain none of the given values (`Rule::doesnt_contain`).
    DoesntContain,
    "doesnt_contain"
);

/// The field must be an array whose keys are limited to the given list.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrayRule {
    keys: Vec<String>,
}

impl ArrayRule {
    /// Create the rule (an empty list allows any keys).
    pub fn new<V: Into<Value>>(keys: impl IntoIterator<Item = V>) -> Self {
        Self { keys: stringify(keys) }
    }
}

impl std::fmt::Display for ArrayRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.keys.is_empty() {
            f.write_str("array")
        } else {
            write!(f, "array:{}", self.keys.join(","))
        }
    }
}

impl IntoRuleItems for ArrayRule {
    fn into_rule_items(self) -> Vec<RuleItem> {
        vec![RuleItem::Str(self.to_string())]
    }
}

/// The field must be an array whose keys are all in the given list.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrayKeys {
    keys: Vec<String>,
}

impl ArrayKeys {
    /// Create the rule.
    pub fn new<V: Into<Value>>(keys: impl IntoIterator<Item = V>) -> Self {
        Self { keys: stringify(keys) }
    }
}

impl std::fmt::Display for ArrayKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "array_keys:{}", self.keys.join(","))
    }
}

impl IntoRuleItems for ArrayKeys {
    fn into_rule_items(self) -> Vec<RuleItem> {
        vec![RuleItem::Str(self.to_string())]
    }
}

// ----------------------------------------------------------------------
// Dimensions
// ----------------------------------------------------------------------

/// Image dimension constraints (`Rule::dimensions()`).
///
/// ```
/// use illuminate_validation::Rule;
///
/// let rule = Rule::dimensions().max_width(1000).max_height(500).ratio(1.5);
/// assert_eq!(rule.to_string(), "dimensions:max_width=1000,max_height=500,ratio=1.5");
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dimensions {
    constraints: Vec<(String, String)>,
}

impl Dimensions {
    /// Create an empty set of constraints.
    pub fn new() -> Self {
        Self::default()
    }

    fn set(mut self, key: &str, value: impl ToString) -> Self {
        let value = value.to_string();
        match self.constraints.iter_mut().find(|(k, _)| k == key) {
            Some(existing) => existing.1 = value,
            None => self.constraints.push((key.to_string(), value)),
        }
        self
    }

    /// The image must be exactly this wide.
    pub fn width(self, value: u32) -> Self {
        self.set("width", value)
    }

    /// The image must be exactly this tall.
    pub fn height(self, value: u32) -> Self {
        self.set("height", value)
    }

    /// The image must be at least this wide.
    pub fn min_width(self, value: u32) -> Self {
        self.set("min_width", value)
    }

    /// The image must be at least this tall.
    pub fn min_height(self, value: u32) -> Self {
        self.set("min_height", value)
    }

    /// The image may be at most this wide.
    pub fn max_width(self, value: u32) -> Self {
        self.set("max_width", value)
    }

    /// The image may be at most this tall.
    pub fn max_height(self, value: u32) -> Self {
        self.set("max_height", value)
    }

    /// The width / height ratio (`1.5`, or use [`Dimensions::ratio_str`] for `"3/2"`).
    pub fn ratio(self, value: f64) -> Self {
        self.set("ratio", illuminate_support::value::format_float(value))
    }

    /// The ratio as a fraction string (`"3/2"`).
    pub fn ratio_str(self, value: &str) -> Self {
        self.set("ratio", value)
    }

    /// The minimum width / height ratio.
    pub fn min_ratio(self, value: f64) -> Self {
        self.set("min_ratio", illuminate_support::value::format_float(value))
    }

    /// The maximum width / height ratio.
    pub fn max_ratio(self, value: f64) -> Self {
        self.set("max_ratio", illuminate_support::value::format_float(value))
    }

    /// The ratio must be between the given bounds.
    pub fn ratio_between(self, min: f64, max: f64) -> Self {
        self.min_ratio(min).max_ratio(max)
    }
}

impl Conditionable for Dimensions {}

impl std::fmt::Display for Dimensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let constraints: Vec<String> = self.constraints.iter().map(|(k, v)| format!("{k}={v}")).collect();
        write!(f, "dimensions:{}", constraints.join(","))
    }
}

impl IntoRuleItems for Dimensions {
    fn into_rule_items(self) -> Vec<RuleItem> {
        vec![RuleItem::Str(self.to_string())]
    }
}

// ----------------------------------------------------------------------
// Dates
// ----------------------------------------------------------------------

/// A date given to the date rule builder: a string (`"tomorrow"`,
/// `"2024-01-01"`, another field's name) or a [`Carbon`] instance.
#[derive(Clone, Debug)]
pub enum DateArg {
    /// A date string or field name.
    Str(String),
    /// A concrete date.
    Date(Carbon),
}

impl From<&str> for DateArg {
    fn from(value: &str) -> Self {
        DateArg::Str(value.to_string())
    }
}

impl From<String> for DateArg {
    fn from(value: String) -> Self {
        DateArg::Str(value)
    }
}

impl From<Carbon> for DateArg {
    fn from(value: Carbon) -> Self {
        DateArg::Date(value)
    }
}

/// The fluent date rule builder (`Rule::date()`).
///
/// ```
/// use illuminate_validation::Rule;
///
/// let rule = Rule::date().format("Y-m-d").after_today();
/// assert_eq!(rule.to_rules(), vec!["date_format:Y-m-d", "after:today"]);
/// ```
#[derive(Clone, Debug, Default)]
pub struct DateRule {
    format: Option<String>,
    constraints: Vec<String>,
}

impl DateRule {
    /// Create the rule.
    pub fn new() -> Self {
        Self::default()
    }

    /// Require a specific format (`date_format`).
    pub fn format(mut self, format: &str) -> Self {
        self.format = Some(format.to_string());
        self
    }

    fn format_date(&self, date: DateArg) -> String {
        match date {
            DateArg::Str(s) => s,
            DateArg::Date(d) => crate::date::format_carbon(&d, self.format.as_deref().unwrap_or("Y-m-d")),
        }
    }

    fn add(mut self, rule: String) -> Self {
        self.constraints.push(rule);
        self
    }

    /// The date must be before today.
    pub fn before_today(self) -> Self {
        self.before("today")
    }

    /// The date must be after today.
    pub fn after_today(self) -> Self {
        self.after("today")
    }

    /// The date must be today or before.
    pub fn today_or_before(self) -> Self {
        self.before_or_equal("today")
    }

    /// The date must be today or after.
    pub fn today_or_after(self) -> Self {
        self.after_or_equal("today")
    }

    /// The date must be in the past.
    pub fn past(self) -> Self {
        self.before("now")
    }

    /// The date must be in the future.
    pub fn future(self) -> Self {
        self.after("now")
    }

    /// The date must be now or in the past.
    pub fn now_or_past(self) -> Self {
        self.before_or_equal("now")
    }

    /// The date must be now or in the future.
    pub fn now_or_future(self) -> Self {
        self.after_or_equal("now")
    }

    /// The date must be before the given date.
    pub fn before(self, date: impl Into<DateArg>) -> Self {
        let date = self.format_date(date.into());
        self.add(format!("before:{date}"))
    }

    /// The date must be after the given date.
    pub fn after(self, date: impl Into<DateArg>) -> Self {
        let date = self.format_date(date.into());
        self.add(format!("after:{date}"))
    }

    /// The date must be before or equal to the given date.
    pub fn before_or_equal(self, date: impl Into<DateArg>) -> Self {
        let date = self.format_date(date.into());
        self.add(format!("before_or_equal:{date}"))
    }

    /// The date must be after or equal to the given date.
    pub fn after_or_equal(self, date: impl Into<DateArg>) -> Self {
        let date = self.format_date(date.into());
        self.add(format!("after_or_equal:{date}"))
    }

    /// The date must be strictly between the given dates.
    pub fn between(self, from: impl Into<DateArg>, to: impl Into<DateArg>) -> Self {
        self.after(from).before(to)
    }

    /// The date must be between the given dates, inclusive.
    pub fn between_or_equal(self, from: impl Into<DateArg>, to: impl Into<DateArg>) -> Self {
        self.after_or_equal(from).before_or_equal(to)
    }

    /// The string rules this builder compiles to.
    pub fn to_rules(&self) -> Vec<String> {
        let mut rules = vec![match &self.format {
            Some(format) => format!("date_format:{format}"),
            None => "date".to_string(),
        }];
        rules.extend(self.constraints.iter().cloned());
        rules
    }
}

impl Conditionable for DateRule {}

impl std::fmt::Display for DateRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_rules().join("|"))
    }
}

impl IntoRuleItems for DateRule {
    fn into_rule_items(self) -> Vec<RuleItem> {
        str_items(self.to_rules())
    }
}

// ----------------------------------------------------------------------
// Numbers
// ----------------------------------------------------------------------

fn num(value: f64) -> String {
    illuminate_support::value::format_float(value)
}

/// The fluent numeric rule builder (`Rule::numeric()`).
///
/// ```
/// use illuminate_validation::Rule;
///
/// let rule = Rule::numeric().integer().min(1.0).max(10.0);
/// assert_eq!(rule.to_string(), "numeric|integer|min:1|max:10");
/// ```
#[derive(Clone, Debug)]
pub struct NumericRule {
    constraints: Vec<String>,
}

impl Default for NumericRule {
    fn default() -> Self {
        Self {
            constraints: vec!["numeric".to_string()],
        }
    }
}

impl NumericRule {
    /// Create the rule.
    pub fn new() -> Self {
        Self::default()
    }

    fn add(mut self, rule: String) -> Self {
        self.constraints.push(rule);
        self
    }

    /// The number must be between `min` and `max`.
    pub fn between(self, min: f64, max: f64) -> Self {
        self.add(format!("between:{},{}", num(min), num(max)))
    }

    /// The number must have exactly `places` decimal places.
    pub fn decimal(self, places: u32) -> Self {
        self.add(format!("decimal:{places}"))
    }

    /// The number must have between `min` and `max` decimal places.
    pub fn decimal_between(self, min: u32, max: u32) -> Self {
        self.add(format!("decimal:{min},{max}"))
    }

    /// The number must differ from another field.
    pub fn different(self, field: &str) -> Self {
        self.add(format!("different:{field}"))
    }

    /// The integer must have exactly `length` digits.
    pub fn digits(self, length: u32) -> Self {
        self.integer().add(format!("digits:{length}"))
    }

    /// The integer must have between `min` and `max` digits.
    pub fn digits_between(self, min: u32, max: u32) -> Self {
        self.integer().add(format!("digits_between:{min},{max}"))
    }

    /// The number must be greater than another field.
    pub fn greater_than(self, field: &str) -> Self {
        self.add(format!("gt:{field}"))
    }

    /// The number must be greater than or equal to another field.
    pub fn greater_than_or_equal_to(self, field: &str) -> Self {
        self.add(format!("gte:{field}"))
    }

    /// The number must be an integer.
    pub fn integer(self) -> Self {
        self.add("integer".to_string())
    }

    /// The number must be an integer *type* (`integer:strict`).
    pub fn strict_integer(self) -> Self {
        self.add("integer:strict".to_string())
    }

    /// The number must be less than another field.
    pub fn less_than(self, field: &str) -> Self {
        self.add(format!("lt:{field}"))
    }

    /// The number must be less than or equal to another field.
    pub fn less_than_or_equal_to(self, field: &str) -> Self {
        self.add(format!("lte:{field}"))
    }

    /// The number may be at most `value`.
    pub fn max(self, value: f64) -> Self {
        self.add(format!("max:{}", num(value)))
    }

    /// The integer may have at most `value` digits.
    pub fn max_digits(self, value: u32) -> Self {
        self.add(format!("max_digits:{value}"))
    }

    /// The number must be at least `value`.
    pub fn min(self, value: f64) -> Self {
        self.add(format!("min:{}", num(value)))
    }

    /// The integer must have at least `value` digits.
    pub fn min_digits(self, value: u32) -> Self {
        self.add(format!("min_digits:{value}"))
    }

    /// The number must be a multiple of `value`.
    pub fn multiple_of(self, value: f64) -> Self {
        self.add(format!("multiple_of:{}", num(value)))
    }

    /// The number must match another field.
    pub fn same(self, field: &str) -> Self {
        self.add(format!("same:{field}"))
    }

    /// The integer must be exactly `value`.
    pub fn exactly(self, value: i64) -> Self {
        self.integer().add(format!("size:{value}"))
    }

    /// The string rules this builder compiles to.
    pub fn to_rules(&self) -> Vec<String> {
        unique(self.constraints.clone())
    }
}

impl Conditionable for NumericRule {}

impl std::fmt::Display for NumericRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_rules().join("|"))
    }
}

impl IntoRuleItems for NumericRule {
    fn into_rule_items(self) -> Vec<RuleItem> {
        str_items(self.to_rules())
    }
}

// ----------------------------------------------------------------------
// Strings
// ----------------------------------------------------------------------

/// The fluent string rule builder (`Rule::string()`).
///
/// ```
/// use illuminate_validation::Rule;
///
/// let rule = Rule::string().min(3).max(255).alpha_dash(true);
/// assert_eq!(rule.to_string(), "string|min:3|max:255|alpha_dash:ascii");
/// ```
#[derive(Clone, Debug)]
pub struct StringRule {
    constraints: Vec<String>,
}

impl Default for StringRule {
    fn default() -> Self {
        Self {
            constraints: vec!["string".to_string()],
        }
    }
}

impl StringRule {
    /// Create the rule.
    pub fn new() -> Self {
        Self::default()
    }

    fn add(mut self, rule: String) -> Self {
        self.constraints.push(rule);
        self
    }

    /// Only letters (ASCII letters when `ascii` is true).
    pub fn alpha(self, ascii: bool) -> Self {
        self.add(if ascii { "alpha:ascii" } else { "alpha" }.to_string())
    }

    /// Only letters, numbers, dashes and underscores.
    pub fn alpha_dash(self, ascii: bool) -> Self {
        self.add(if ascii { "alpha_dash:ascii" } else { "alpha_dash" }.to_string())
    }

    /// Only letters and numbers.
    pub fn alpha_numeric(self, ascii: bool) -> Self {
        self.add(if ascii { "alpha_num:ascii" } else { "alpha_num" }.to_string())
    }

    /// Only 7-bit ASCII characters.
    pub fn ascii(self) -> Self {
        self.add("ascii".to_string())
    }

    /// Between `min` and `max` characters.
    pub fn between(self, min: usize, max: usize) -> Self {
        self.add(format!("between:{min},{max}"))
    }

    /// Must not end with any of the values.
    pub fn doesnt_end_with<S: AsRef<str>>(self, values: impl IntoIterator<Item = S>) -> Self {
        let values: Vec<String> = values.into_iter().map(|v| v.as_ref().to_string()).collect();
        self.add(format!("doesnt_end_with:{}", values.join(",")))
    }

    /// Must not start with any of the values.
    pub fn doesnt_start_with<S: AsRef<str>>(self, values: impl IntoIterator<Item = S>) -> Self {
        let values: Vec<String> = values.into_iter().map(|v| v.as_ref().to_string()).collect();
        self.add(format!("doesnt_start_with:{}", values.join(",")))
    }

    /// Must end with one of the values.
    pub fn ends_with<S: AsRef<str>>(self, values: impl IntoIterator<Item = S>) -> Self {
        let values: Vec<String> = values.into_iter().map(|v| v.as_ref().to_string()).collect();
        self.add(format!("ends_with:{}", values.join(",")))
    }

    /// Exactly `length` characters.
    pub fn exactly(self, length: usize) -> Self {
        self.add(format!("size:{length}"))
    }

    /// Must be lowercase.
    pub fn lowercase(self) -> Self {
        self.add("lowercase".to_string())
    }

    /// At most `length` characters.
    pub fn max(self, length: usize) -> Self {
        self.add(format!("max:{length}"))
    }

    /// At least `length` characters.
    pub fn min(self, length: usize) -> Self {
        self.add(format!("min:{length}"))
    }

    /// Must start with one of the values.
    pub fn starts_with<S: AsRef<str>>(self, values: impl IntoIterator<Item = S>) -> Self {
        let values: Vec<String> = values.into_iter().map(|v| v.as_ref().to_string()).collect();
        self.add(format!("starts_with:{}", values.join(",")))
    }

    /// Must be uppercase.
    pub fn uppercase(self) -> Self {
        self.add("uppercase".to_string())
    }

    /// The string rules this builder compiles to.
    pub fn to_rules(&self) -> Vec<String> {
        unique(self.constraints.clone())
    }
}

impl Conditionable for StringRule {}

impl std::fmt::Display for StringRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_rules().join("|"))
    }
}

impl IntoRuleItems for StringRule {
    fn into_rule_items(self) -> Vec<RuleItem> {
        str_items(self.to_rules())
    }
}

// ----------------------------------------------------------------------
// E-mail
// ----------------------------------------------------------------------

/// The fluent e-mail rule builder (`Rule::email()`).
///
/// ```
/// use illuminate_validation::Rule;
///
/// let rule = Rule::email().rfc_compliant(true).validate_mx_record();
/// assert_eq!(rule.to_string(), "email:strict,dns");
/// ```
#[derive(Clone, Debug, Default)]
pub struct EmailRule {
    rfc: bool,
    strict: bool,
    dns: bool,
    spoof: bool,
    filter: bool,
    filter_unicode: bool,
    custom: Vec<RuleItem>,
}

impl EmailRule {
    /// Create the rule.
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate against the RFCs (strictly — failing on warnings — when `strict`).
    pub fn rfc_compliant(mut self, strict: bool) -> Self {
        if strict {
            self.strict = true;
        } else {
            self.rfc = true;
        }
        self
    }

    /// Alias of `rfc_compliant(true)`.
    pub fn strict(self) -> Self {
        self.rfc_compliant(true)
    }

    /// Check the domain's MX record (`dns`). Without network access this is
    /// accepted but not enforced.
    pub fn validate_mx_record(mut self) -> Self {
        self.dns = true;
        self
    }

    /// Prevent spoofing with homograph characters (`spoof`); accepted but
    /// not enforced.
    pub fn prevent_spoofing(mut self) -> Self {
        self.spoof = true;
        self
    }

    /// Validate like PHP's `filter_var` (`filter`, or `filter_unicode`).
    pub fn with_native_validation(mut self, allow_unicode: bool) -> Self {
        if allow_unicode {
            self.filter_unicode = true;
        } else {
            self.filter = true;
        }
        self
    }

    /// Add additional rules.
    pub fn rules(mut self, rules: impl Into<RuleSet>) -> Self {
        self.custom.extend(rules.into().items);
        self
    }

    fn rule_string(&self) -> String {
        let mut modes = Vec::new();
        for (enabled, name) in [
            (self.rfc, "rfc"),
            (self.strict, "strict"),
            (self.dns, "dns"),
            (self.spoof, "spoof"),
            (self.filter, "filter"),
            (self.filter_unicode, "filter_unicode"),
        ] {
            if enabled {
                modes.push(name);
            }
        }
        if modes.is_empty() {
            "email".to_string()
        } else {
            format!("email:{}", modes.join(","))
        }
    }
}

impl Conditionable for EmailRule {}

impl std::fmt::Display for EmailRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.rule_string())
    }
}

impl IntoRuleItems for EmailRule {
    fn into_rule_items(self) -> Vec<RuleItem> {
        let mut items = vec![RuleItem::Str(self.rule_string())];
        items.extend(self.custom);
        items
    }
}

// ----------------------------------------------------------------------
// Files
// ----------------------------------------------------------------------

/// A file size: kilobytes as a number, or a string with a `kb`, `mb`,
/// `gb` or `tb` suffix (`"10mb"`).
#[derive(Clone, Debug, PartialEq)]
pub struct FileSize(pub f64);

impl From<u64> for FileSize {
    fn from(kilobytes: u64) -> Self {
        FileSize(kilobytes as f64)
    }
}

impl From<u32> for FileSize {
    fn from(kilobytes: u32) -> Self {
        FileSize(kilobytes as f64)
    }
}

impl From<i32> for FileSize {
    fn from(kilobytes: i32) -> Self {
        FileSize(kilobytes as f64)
    }
}

impl From<f64> for FileSize {
    fn from(kilobytes: f64) -> Self {
        FileSize(kilobytes)
    }
}

impl From<&str> for FileSize {
    /// Parse a size with a unit suffix.
    ///
    /// # Panics
    ///
    /// Panics on an unknown suffix, like Laravel's "Invalid file size suffix." exception.
    fn from(size: &str) -> Self {
        let size = size.trim().to_ascii_lowercase();
        let number: String = size.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let value: f64 = number.parse().unwrap_or(0.0);
        let factor = if size.ends_with("kb") {
            1.0
        } else if size.ends_with("mb") {
            1_000.0
        } else if size.ends_with("gb") {
            1_000_000.0
        } else if size.ends_with("tb") {
            1_000_000_000.0
        } else {
            panic!("Invalid file size suffix.");
        };
        FileSize((value * factor).round())
    }
}

/// The fluent file rule builder (`Rule::file()`, `File::types([...])`).
///
/// ```
/// use illuminate_validation::{FileRule, Rule};
///
/// let rule = FileRule::types(["mp3", "wav"]).min("1kb").max("10mb");
/// assert_eq!(rule.to_rules(), vec!["file", "mimes:mp3,wav", "between:1,10000"]);
///
/// let image = Rule::image_file().max(1024).dimensions(Rule::dimensions().max_width(1000));
/// assert_eq!(image.to_rules(), vec!["file", "max:1024", "image", "dimensions:max_width=1000"]);
/// ```
#[derive(Clone, Debug, Default)]
pub struct FileRule {
    mimetypes: Vec<String>,
    extensions: Vec<String>,
    min: Option<f64>,
    max: Option<f64>,
    encoding: Option<String>,
    custom: Vec<RuleItem>,
}

impl FileRule {
    /// Create the rule.
    pub fn new() -> Self {
        Self::default()
    }

    /// Only allow the given MIME types or extensions (`["mp3", "audio/*"]`).
    pub fn types<S: AsRef<str>>(types: impl IntoIterator<Item = S>) -> Self {
        Self {
            mimetypes: types.into_iter().map(|t| t.as_ref().to_string()).collect(),
            ..Self::default()
        }
    }

    /// An image file (`image` rule), optionally allowing SVGs.
    pub fn image(allow_svg: bool) -> ImageFile {
        ImageFile::new(allow_svg)
    }

    /// Only allow the given client-provided extensions.
    pub fn extensions<S: AsRef<str>>(mut self, extensions: impl IntoIterator<Item = S>) -> Self {
        self.extensions = extensions.into_iter().map(|e| e.as_ref().to_ascii_lowercase()).collect();
        self
    }

    /// The file must be exactly this size.
    pub fn size(mut self, size: impl Into<FileSize>) -> Self {
        let size = size.into().0;
        self.min = Some(size);
        self.max = Some(size);
        self
    }

    /// The file size must be between the given sizes.
    pub fn between(mut self, min: impl Into<FileSize>, max: impl Into<FileSize>) -> Self {
        self.min = Some(min.into().0);
        self.max = Some(max.into().0);
        self
    }

    /// The minimum file size.
    pub fn min(mut self, size: impl Into<FileSize>) -> Self {
        self.min = Some(size.into().0);
        self
    }

    /// The maximum file size.
    pub fn max(mut self, size: impl Into<FileSize>) -> Self {
        self.max = Some(size.into().0);
        self
    }

    /// The file's contents must be in the given encoding.
    pub fn encoding(mut self, encoding: &str) -> Self {
        self.encoding = Some(encoding.to_string());
        self
    }

    /// Add additional rules.
    pub fn rules(mut self, rules: impl Into<RuleSet>) -> Self {
        self.custom.extend(rules.into().items);
        self
    }

    fn string_rules(&self) -> Vec<String> {
        let mut rules = vec!["file".to_string()];
        let (mimetypes, mimes): (Vec<&String>, Vec<&String>) = self.mimetypes.iter().partition(|t| t.contains('/'));
        if !mimetypes.is_empty() {
            rules.push(format!("mimetypes:{}", mimetypes.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",")));
        }
        if !mimes.is_empty() {
            rules.push(format!("mimes:{}", mimes.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",")));
        }
        if !self.extensions.is_empty() {
            rules.push(format!("extensions:{}", self.extensions.join(",")));
        }
        match (self.min, self.max) {
            (None, None) => {}
            (Some(min), None) => rules.push(format!("min:{}", num(min))),
            (None, Some(max)) => rules.push(format!("max:{}", num(max))),
            (Some(min), Some(max)) if min != max => rules.push(format!("between:{},{}", num(min), num(max))),
            (Some(size), Some(_)) => rules.push(format!("size:{}", num(size))),
        }
        if let Some(encoding) = &self.encoding {
            rules.push(format!("encoding:{encoding}"));
        }
        rules
    }

    /// The string rules this builder compiles to (custom rules excluded).
    pub fn to_rules(&self) -> Vec<String> {
        let mut rules = self.string_rules();
        for item in &self.custom {
            if let RuleItem::Str(rule) = item {
                rules.push(rule.clone());
            }
        }
        rules
    }
}

impl Conditionable for FileRule {}

impl IntoRuleItems for FileRule {
    fn into_rule_items(self) -> Vec<RuleItem> {
        let mut items = str_items(self.string_rules());
        items.extend(self.custom);
        items
    }
}

/// An image file rule (`File::image()`, `Rule::image_file()`).
#[derive(Clone, Debug)]
pub struct ImageFile {
    file: FileRule,
}

impl ImageFile {
    /// Create the rule, optionally allowing SVG images.
    pub fn new(allow_svg: bool) -> Self {
        Self {
            file: FileRule::new().rules(if allow_svg { "image:allow_svg" } else { "image" }),
        }
    }

    /// Constrain the image's dimensions.
    pub fn dimensions(mut self, dimensions: Dimensions) -> Self {
        self.file = self.file.rules(dimensions);
        self
    }

    /// Only allow the given client-provided extensions.
    pub fn extensions<S: AsRef<str>>(mut self, extensions: impl IntoIterator<Item = S>) -> Self {
        self.file = self.file.extensions(extensions);
        self
    }

    /// The image must be exactly this size.
    pub fn size(mut self, size: impl Into<FileSize>) -> Self {
        self.file = self.file.size(size);
        self
    }

    /// The image size must be between the given sizes.
    pub fn between(mut self, min: impl Into<FileSize>, max: impl Into<FileSize>) -> Self {
        self.file = self.file.between(min, max);
        self
    }

    /// The minimum image size.
    pub fn min(mut self, size: impl Into<FileSize>) -> Self {
        self.file = self.file.min(size);
        self
    }

    /// The maximum image size.
    pub fn max(mut self, size: impl Into<FileSize>) -> Self {
        self.file = self.file.max(size);
        self
    }

    /// Add additional rules.
    pub fn rules(mut self, rules: impl Into<RuleSet>) -> Self {
        self.file = self.file.rules(rules);
        self
    }

    /// The string rules this builder compiles to.
    pub fn to_rules(&self) -> Vec<String> {
        self.file.to_rules()
    }
}

impl Conditionable for ImageFile {}

impl IntoRuleItems for ImageFile {
    fn into_rule_items(self) -> Vec<RuleItem> {
        self.file.into_rule_items()
    }
}

// ----------------------------------------------------------------------
// Enums
// ----------------------------------------------------------------------

/// A "backed" enum: one whose cases map to string or integer values, so it
/// can be validated with `Rule::enum_::<T>()`.
///
/// ```
/// use illuminate_validation::BackedEnum;
/// use illuminate_support::{json, Value};
///
/// #[derive(Clone, PartialEq)]
/// enum ServerStatus { Active, Inactive }
///
/// impl BackedEnum for ServerStatus {
///     fn cases() -> Vec<Self> {
///         vec![ServerStatus::Active, ServerStatus::Inactive]
///     }
///
///     fn value(&self) -> Value {
///         match self {
///             ServerStatus::Active => json!("active"),
///             ServerStatus::Inactive => json!("inactive"),
///         }
///     }
/// }
/// ```
pub trait BackedEnum: Sized + PartialEq + 'static {
    /// Every case of the enum.
    fn cases() -> Vec<Self>;

    /// The backing value of this case.
    fn value(&self) -> Value;
}

/// The field must hold a valid enum value (`Rule::enum_::<T>()`).
#[derive(Clone, Debug)]
pub struct Enum<T> {
    values: Vec<String>,
    only: Option<Vec<String>>,
    except: Vec<String>,
    _marker: PhantomData<fn() -> T>,
}

impl<T: BackedEnum> Enum<T> {
    /// Create the rule for the enum type.
    pub fn new() -> Self {
        Self {
            values: T::cases().iter().map(|c| c.value().to_string_lossy()).collect(),
            only: None,
            except: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// Only allow the given cases.
    pub fn only(mut self, cases: impl IntoIterator<Item = T>) -> Self {
        self.only = Some(cases.into_iter().map(|c| c.value().to_string_lossy()).collect());
        self
    }

    /// Allow every case except the given ones.
    pub fn except(mut self, cases: impl IntoIterator<Item = T>) -> Self {
        self.except = cases.into_iter().map(|c| c.value().to_string_lossy()).collect();
        self
    }

    fn allowed(&self) -> Vec<String> {
        match &self.only {
            Some(only) => only.clone(),
            None => self.values.iter().filter(|v| !self.except.contains(v)).cloned().collect(),
        }
    }
}

impl<T: BackedEnum> Default for Enum<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: BackedEnum> Conditionable for Enum<T> {}

impl<T: BackedEnum> std::fmt::Display for Enum<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "enum:{}", quote_values(&self.allowed()))
    }
}

impl<T: BackedEnum> IntoRuleItems for Enum<T> {
    fn into_rule_items(self) -> Vec<RuleItem> {
        vec![RuleItem::Str(self.to_string())]
    }
}

pub(crate) fn enum_values_rule<V: Into<Value>>(values: impl IntoIterator<Item = V>) -> String {
    format!("enum:{}", quote_values(&stringify(values)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_rules_quote_their_values() {
        assert_eq!(In::new(["a", "b\"c"]).to_string(), r#"in:"a","b""c""#);
        assert_eq!(NotIn::new([1, 2]).to_string(), r#"not_in:"1","2""#);
    }

    #[test]
    fn file_sizes_parse_units() {
        assert_eq!(FileSize::from("10mb"), FileSize(10_000.0));
        assert_eq!(FileSize::from("1.5kb"), FileSize(2.0));
        assert_eq!(FileSize::from(512u64), FileSize(512.0));
    }

    #[test]
    fn file_rules_compile() {
        assert_eq!(FileRule::new().size(512).to_rules(), vec!["file", "size:512"]);
        assert_eq!(
            FileRule::types(["image/*", "pdf"]).extensions(["PDF"]).to_rules(),
            vec!["file", "mimetypes:image/*", "mimes:pdf", "extensions:pdf"]
        );
    }
}
