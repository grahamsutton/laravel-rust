//! Rule definitions: the [`Rules`] map, the [`RuleSet`] for a single
//! attribute, and the fluent rule builders.

mod any_of;
mod builders;
mod database;
mod facade;
mod password;

use std::sync::Arc;

use indexmap::IndexMap;

use illuminate_support::Value;

use crate::rule::{ClosureRule, ValidationRule};

pub use any_of::AnyOf;
pub use builders::{
    ArrayKeys, ArrayRule, BackedEnum, Contains, DateArg, DateRule, Dimensions, DoesntContain, EmailRule, Enum,
    FileRule, FileSize, ImageFile, In, NotIn, NumericRule, StringRule,
};
pub use database::{Condition, DatabaseRule, DatabaseRuleKind, Exists, Unique};
pub use facade::Rule;
pub use password::{Password, PasswordDefaults};

/// A single validation rule: a string (`"max:255"`), a database rule, a
/// rule object or closure, or a conditional / nested rule.
#[derive(Clone)]
pub enum RuleItem {
    /// A string rule, such as `required` or `max:255`.
    Str(String),
    /// A fluent `unique` / `exists` rule.
    Database(DatabaseRule),
    /// A custom rule object or closure.
    Custom(Arc<dyn ValidationRule>),
    /// Rules that only apply when a condition holds (`Rule::when`).
    Conditional(ConditionalRules),
    /// Rules computed for each concrete attribute (`Rule::for_each`).
    Nested(NestedRules),
}

impl std::fmt::Debug for RuleItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleItem::Str(rule) => write!(f, "{rule:?}"),
            RuleItem::Database(rule) => write!(f, "{rule:?}"),
            RuleItem::Custom(rule) => write!(f, "Custom({})", rule.name()),
            RuleItem::Conditional(_) => f.write_str("Conditional"),
            RuleItem::Nested(_) => f.write_str("Nested"),
        }
    }
}

/// The list of rules for a single attribute.
///
/// A `&str` is split on `|` (`"required|email|max:255"`); arrays and
/// vectors keep each element as a single rule (use them for `regex`
/// patterns containing a `|`).
#[derive(Clone, Debug, Default)]
pub struct RuleSet {
    pub(crate) items: Vec<RuleItem>,
}

impl RuleSet {
    /// Create an empty rule set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a rule set from individual items.
    pub fn from_items(items: Vec<RuleItem>) -> Self {
        Self { items }
    }

    /// Add a rule to the set.
    pub fn push(mut self, rule: impl IntoRuleItems) -> Self {
        self.items.extend(rule.into_rule_items());
        self
    }

    /// Append the rules of another set.
    pub fn extend(&mut self, other: RuleSet) {
        self.items.extend(other.items);
    }

    /// The rules in the set.
    pub fn items(&self) -> &[RuleItem] {
        &self.items
    }

    /// Determine if the set has no rules.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The number of rules in the set.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    fn split(rules: &str) -> Self {
        Self {
            items: rules
                .split('|')
                .filter(|r| !r.is_empty())
                .map(|r| RuleItem::Str(r.to_string()))
                .collect(),
        }
    }
}

/// Conversion into one or more rules — implemented for strings, rule
/// builders, rule objects, closures and rule sets.
pub trait IntoRuleItems {
    /// Convert into rule items.
    fn into_rule_items(self) -> Vec<RuleItem>;
}

impl IntoRuleItems for &str {
    fn into_rule_items(self) -> Vec<RuleItem> {
        vec![RuleItem::Str(self.to_string())]
    }
}

impl IntoRuleItems for String {
    fn into_rule_items(self) -> Vec<RuleItem> {
        vec![RuleItem::Str(self)]
    }
}

impl IntoRuleItems for &String {
    fn into_rule_items(self) -> Vec<RuleItem> {
        vec![RuleItem::Str(self.clone())]
    }
}

impl IntoRuleItems for RuleItem {
    fn into_rule_items(self) -> Vec<RuleItem> {
        vec![self]
    }
}

impl IntoRuleItems for RuleSet {
    fn into_rule_items(self) -> Vec<RuleItem> {
        self.items
    }
}

impl IntoRuleItems for Arc<dyn ValidationRule> {
    fn into_rule_items(self) -> Vec<RuleItem> {
        vec![RuleItem::Custom(self)]
    }
}

impl<T: ValidationRule + 'static> IntoRuleItems for T {
    fn into_rule_items(self) -> Vec<RuleItem> {
        vec![RuleItem::Custom(Arc::new(self))]
    }
}

impl From<&str> for RuleSet {
    fn from(rules: &str) -> Self {
        RuleSet::split(rules)
    }
}

impl From<String> for RuleSet {
    fn from(rules: String) -> Self {
        RuleSet::split(&rules)
    }
}

impl From<&String> for RuleSet {
    fn from(rules: &String) -> Self {
        RuleSet::split(rules)
    }
}

impl From<RuleItem> for RuleSet {
    fn from(item: RuleItem) -> Self {
        RuleSet { items: vec![item] }
    }
}

impl From<Arc<dyn ValidationRule>> for RuleSet {
    fn from(rule: Arc<dyn ValidationRule>) -> Self {
        RuleSet::from_items(rule.into_rule_items())
    }
}

impl<T: IntoRuleItems, const N: usize> From<[T; N]> for RuleSet {
    fn from(rules: [T; N]) -> Self {
        RuleSet {
            items: rules.into_iter().flat_map(IntoRuleItems::into_rule_items).collect(),
        }
    }
}

impl<T: IntoRuleItems> From<Vec<T>> for RuleSet {
    fn from(rules: Vec<T>) -> Self {
        RuleSet {
            items: rules.into_iter().flat_map(IntoRuleItems::into_rule_items).collect(),
        }
    }
}

impl<T: ValidationRule + 'static> From<T> for RuleSet {
    fn from(rule: T) -> Self {
        RuleSet::from_items(rule.into_rule_items())
    }
}

macro_rules! rule_set_from_builders {
    ($($builder:ty),* $(,)?) => {
        $(
            impl From<$builder> for RuleSet {
                fn from(rule: $builder) -> Self {
                    RuleSet::from_items(IntoRuleItems::into_rule_items(rule))
                }
            }
        )*
    };
}

rule_set_from_builders!(
    In,
    NotIn,
    Contains,
    DoesntContain,
    ArrayRule,
    ArrayKeys,
    Dimensions,
    DateRule,
    NumericRule,
    StringRule,
    EmailRule,
    FileRule,
    ImageFile,
    DatabaseRule,
    ConditionalRules,
    NestedRules,
);

impl<T: BackedEnum> From<Enum<T>> for RuleSet {
    fn from(rule: Enum<T>) -> Self {
        RuleSet::from_items(rule.into_rule_items())
    }
}

/// The validation rules for a set of attributes, in order.
///
/// ```
/// use illuminate_validation::{Rules, Rule, rules};
///
/// // From pairs of strings...
/// let simple = Rules::from([("email", "required|email"), ("name", "required")]);
/// assert_eq!(simple.len(), 2);
///
/// // ...or with the `rules!` macro, which mixes strings and rule objects.
/// let rich = rules! {
///     "email" => "required|email|max:255",
///     "tags.*" => ["string", Rule::in_(["a", "b"])],
/// };
/// assert!(rich.contains_key("tags.*"));
/// ```
#[derive(Clone, Debug, Default)]
pub struct Rules {
    rules: IndexMap<String, RuleSet>,
}

impl Rules {
    /// Create an empty rule map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace) the rules for an attribute, fluently.
    pub fn rule(mut self, attribute: impl Into<String>, rules: impl Into<RuleSet>) -> Self {
        self.insert(attribute, rules);
        self
    }

    /// Add (or replace) the rules for an attribute.
    pub fn insert(&mut self, attribute: impl Into<String>, rules: impl Into<RuleSet>) {
        self.rules.insert(attribute.into(), rules.into());
    }

    /// Append rules to an attribute, keeping any it already has.
    pub fn append(&mut self, attribute: impl Into<String>, rules: impl Into<RuleSet>) {
        self.rules.entry(attribute.into()).or_default().extend(rules.into());
    }

    /// Merge another rule map into this one (later rules are appended).
    pub fn merge(mut self, other: impl Into<Rules>) -> Self {
        for (attribute, rules) in other.into().rules {
            self.append(attribute, rules);
        }
        self
    }

    /// Get the rules for an attribute.
    pub fn get(&self, attribute: &str) -> Option<&RuleSet> {
        self.rules.get(attribute)
    }

    /// Determine if rules exist for the attribute.
    pub fn contains_key(&self, attribute: &str) -> bool {
        self.rules.contains_key(attribute)
    }

    /// Remove the rules for an attribute.
    pub fn remove(&mut self, attribute: &str) -> Option<RuleSet> {
        self.rules.shift_remove(attribute)
    }

    /// The attributes that have rules.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.rules.keys()
    }

    /// Iterate over the attributes and their rules.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &RuleSet)> {
        self.rules.iter()
    }

    /// The number of attributes with rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Determine if there are no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

impl IntoIterator for Rules {
    type Item = (String, RuleSet);
    type IntoIter = indexmap::map::IntoIter<String, RuleSet>;

    fn into_iter(self) -> Self::IntoIter {
        self.rules.into_iter()
    }
}

impl<K: Into<String>, V: Into<RuleSet>> FromIterator<(K, V)> for Rules {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut rules = Rules::new();
        for (attribute, set) in iter {
            rules.insert(attribute, set);
        }
        rules
    }
}

impl<K: Into<String>, V: Into<RuleSet>, const N: usize> From<[(K, V); N]> for Rules {
    fn from(pairs: [(K, V); N]) -> Self {
        pairs.into_iter().collect()
    }
}

impl<K: Into<String>, V: Into<RuleSet>> From<Vec<(K, V)>> for Rules {
    fn from(pairs: Vec<(K, V)>) -> Self {
        pairs.into_iter().collect()
    }
}

impl<K: Into<String>, V: Into<RuleSet>> From<IndexMap<K, V>> for Rules {
    fn from(map: IndexMap<K, V>) -> Self {
        map.into_iter().collect()
    }
}

/// Build a [`Rules`] map. Values may be `|`-delimited strings, single rule
/// builders / objects, or `[...]` lists that freely mix both.
///
/// ```
/// use illuminate_validation::{rules, Rule};
///
/// let rules = rules! {
///     "email" => "required|email|max:255",
///     "tags.*" => ["string", Rule::in_(["a", "b"])],
///     "avatar" => [Rule::file().max(1024)],
/// };
///
/// assert_eq!(rules.len(), 3);
/// ```
#[macro_export]
macro_rules! rules {
    () => { $crate::Rules::new() };
    ($($tokens:tt)+) => {{
        let mut rules = $crate::Rules::new();
        $crate::__rules_munch!(rules; $($tokens)+);
        rules
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __rules_munch {
    ($rules:ident;) => {};
    ($rules:ident; $key:expr => [$($rule:expr),* $(,)?] $(, $($rest:tt)*)?) => {
        {
            #[allow(unused_mut)]
            let mut items: ::std::vec::Vec<$crate::RuleItem> = ::std::vec::Vec::new();
            $( items.extend($crate::IntoRuleItems::into_rule_items($rule)); )*
            $rules.insert($key, $crate::RuleSet::from_items(items));
        }
        $crate::__rules_munch!($rules; $($($rest)*)?);
    };
    ($rules:ident; $key:expr => $value:expr $(, $($rest:tt)*)?) => {
        $rules.insert($key, $value);
        $crate::__rules_munch!($rules; $($($rest)*)?);
    };
}

// ----------------------------------------------------------------------
// Conditional & nested rules
// ----------------------------------------------------------------------

type DataPredicate = Arc<dyn Fn(&Value) -> bool + Send + Sync>;
type RuleFactory = Arc<dyn Fn(&Value) -> RuleSet + Send + Sync>;

/// A condition evaluated against the data under validation.
#[derive(Clone)]
pub enum DataCondition {
    /// A fixed condition.
    Bool(bool),
    /// A closure receiving all of the data under validation.
    Closure(DataPredicate),
}

impl DataCondition {
    pub(crate) fn evaluate(&self, data: &Value) -> bool {
        match self {
            DataCondition::Bool(b) => *b,
            DataCondition::Closure(f) => f(data),
        }
    }

    pub(crate) fn negate(self) -> Self {
        match self {
            DataCondition::Bool(b) => DataCondition::Bool(!b),
            DataCondition::Closure(f) => DataCondition::Closure(Arc::new(move |data| !f(data))),
        }
    }
}

/// Conversion into a [`DataCondition`]: a `bool`, or a closure receiving
/// the data under validation.
pub trait IntoDataCondition {
    /// Convert into a condition.
    fn into_data_condition(self) -> DataCondition;
}

impl IntoDataCondition for bool {
    fn into_data_condition(self) -> DataCondition {
        DataCondition::Bool(self)
    }
}

impl<F: Fn(&Value) -> bool + Send + Sync + 'static> IntoDataCondition for F {
    fn into_data_condition(self) -> DataCondition {
        DataCondition::Closure(Arc::new(self))
    }
}

/// Conversion into a condition for `Rule::required_if` and friends: a
/// `bool`, or a closure returning one.
pub trait IntoCondition {
    /// Convert into a condition.
    fn into_condition(self) -> DataCondition;
}

impl IntoCondition for bool {
    fn into_condition(self) -> DataCondition {
        DataCondition::Bool(self)
    }
}

impl<F: Fn() -> bool + Send + Sync + 'static> IntoCondition for F {
    fn into_condition(self) -> DataCondition {
        DataCondition::Closure(Arc::new(move |_| self()))
    }
}

/// Rules applied only when a condition passes (Laravel's `ConditionalRules`).
#[derive(Clone)]
pub struct ConditionalRules {
    condition: DataCondition,
    rules: RuleFactory,
    default: RuleFactory,
}

impl ConditionalRules {
    /// Create conditional rules.
    pub fn new(condition: impl IntoDataCondition, rules: impl Into<RuleSet>, default: impl Into<RuleSet>) -> Self {
        let (rules, default) = (rules.into(), default.into());
        Self {
            condition: condition.into_data_condition(),
            rules: Arc::new(move |_| rules.clone()),
            default: Arc::new(move |_| default.clone()),
        }
    }

    pub(crate) fn from_condition(condition: DataCondition, rules: RuleSet) -> Self {
        Self {
            condition,
            rules: Arc::new(move |_| rules.clone()),
            default: Arc::new(|_| RuleSet::new()),
        }
    }

    /// The rules that apply for the given data.
    pub(crate) fn resolve(&self, data: &Value) -> RuleSet {
        if self.condition.evaluate(data) {
            (self.rules)(data)
        } else {
            (self.default)(data)
        }
    }
}

impl IntoRuleItems for ConditionalRules {
    fn into_rule_items(self) -> Vec<RuleItem> {
        vec![RuleItem::Conditional(self)]
    }
}

/// Rules computed for every concrete attribute (Laravel's `NestedRules`).
#[derive(Clone)]
pub struct NestedRules {
    callback: Arc<dyn Fn(&Value, &str) -> RuleSet + Send + Sync>,
}

impl NestedRules {
    /// Create nested rules from a callback receiving the attribute's value
    /// and its fully expanded name.
    pub fn new<R: Into<RuleSet>>(callback: impl Fn(&Value, &str) -> R + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(move |value, attribute| callback(value, attribute).into()),
        }
    }

    pub(crate) fn compile(&self, value: &Value, attribute: &str) -> RuleSet {
        (self.callback)(value, attribute)
    }
}

impl IntoRuleItems for NestedRules {
    fn into_rule_items(self) -> Vec<RuleItem> {
        vec![RuleItem::Nested(self)]
    }
}

impl From<ClosureRule> for RuleItem {
    fn from(rule: ClosureRule) -> Self {
        RuleItem::Custom(Arc::new(rule))
    }
}

/// Quote rule parameters the way Laravel's `In` rule does (`"a","b"`).
pub(crate) fn quote_values(values: &[String]) -> String {
    values
        .iter()
        .map(|v| format!("\"{}\"", v.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(",")
}
