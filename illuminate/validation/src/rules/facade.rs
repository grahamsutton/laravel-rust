//! The `Rule` class: static constructors for the fluent rule builders.

use illuminate_support::Value;

use super::builders::enum_values_rule;
use super::database::DatabaseRuleKind;
use super::{
    AnyOf, ArrayKeys, ArrayRule, BackedEnum, ConditionalRules, Contains, DatabaseRule, DateRule, Dimensions,
    DoesntContain, EmailRule, Enum, FileRule, ImageFile, In, IntoCondition, IntoDataCondition, NestedRules, NotIn,
    NumericRule, RuleItem, RuleSet, StringRule,
};
use crate::rule::{ClosureRule, FailCallback};

/// Fluent constructors for validation rules.
///
/// ```
/// use illuminate_validation::{Rule, rules};
///
/// let rules = rules! {
///     "zones" => ["required", Rule::in_(["first-zone", "second-zone"])],
///     "email" => ["required", Rule::unique("users", "email").ignore(1)],
///     "starts_at" => [Rule::date().after_today()],
///     "avatar" => [Rule::image_file().max(1024)],
/// };
/// assert_eq!(rules.len(), 4);
/// ```
pub struct Rule;

impl Rule {
    /// The field must be one of the given values.
    pub fn in_<V: Into<Value>>(values: impl IntoIterator<Item = V>) -> In {
        In::new(values)
    }

    /// The field must not be one of the given values.
    pub fn not_in<V: Into<Value>>(values: impl IntoIterator<Item = V>) -> NotIn {
        NotIn::new(values)
    }

    /// The array must contain all of the given values.
    pub fn contains<V: Into<Value>>(values: impl IntoIterator<Item = V>) -> Contains {
        Contains::new(values)
    }

    /// The array must not contain any of the given values.
    pub fn doesnt_contain<V: Into<Value>>(values: impl IntoIterator<Item = V>) -> DoesntContain {
        DoesntContain::new(values)
    }

    /// The field must be an array limited to the given keys.
    pub fn array<V: Into<Value>>(keys: impl IntoIterator<Item = V>) -> ArrayRule {
        ArrayRule::new(keys)
    }

    /// The field must be an array whose keys are all in the given list.
    pub fn array_keys<V: Into<Value>>(keys: impl IntoIterator<Item = V>) -> ArrayKeys {
        ArrayKeys::new(keys)
    }

    /// The value must not exist in the table's column. Pass `""` as the
    /// column to use the attribute's name.
    pub fn unique(table: &str, column: &str) -> DatabaseRule {
        DatabaseRule::new(DatabaseRuleKind::Unique, table, column)
    }

    /// The value must exist in the table's column. Pass `""` as the column
    /// to use the attribute's name.
    pub fn exists(table: &str, column: &str) -> DatabaseRule {
        DatabaseRule::new(DatabaseRuleKind::Exists, table, column)
    }

    /// The field is required when the condition (a `bool` or closure) holds.
    pub fn required_if(condition: impl IntoCondition) -> ConditionalRules {
        ConditionalRules::from_condition(condition.into_condition(), RuleSet::from("required"))
    }

    /// The field is required unless the condition holds.
    pub fn required_unless(condition: impl IntoCondition) -> ConditionalRules {
        ConditionalRules::from_condition(condition.into_condition().negate(), RuleSet::from("required"))
    }

    /// The field is excluded from the validated data when the condition holds.
    pub fn exclude_if(condition: impl IntoCondition) -> ConditionalRules {
        ConditionalRules::from_condition(condition.into_condition(), RuleSet::from("exclude"))
    }

    /// The field is excluded from the validated data unless the condition holds.
    pub fn exclude_unless(condition: impl IntoCondition) -> ConditionalRules {
        ConditionalRules::from_condition(condition.into_condition().negate(), RuleSet::from("exclude"))
    }

    /// The field is prohibited when the condition holds.
    pub fn prohibited_if(condition: impl IntoCondition) -> ConditionalRules {
        ConditionalRules::from_condition(condition.into_condition(), RuleSet::from("prohibited"))
    }

    /// The field is prohibited unless the condition holds.
    pub fn prohibited_unless(condition: impl IntoCondition) -> ConditionalRules {
        ConditionalRules::from_condition(condition.into_condition().negate(), RuleSet::from("prohibited"))
    }

    /// Apply `rules` when the condition (a `bool`, or a closure receiving
    /// the data) holds, and `default` otherwise.
    pub fn when(condition: impl IntoDataCondition, rules: impl Into<RuleSet>, default: impl Into<RuleSet>) -> ConditionalRules {
        ConditionalRules::new(condition, rules, default)
    }

    /// Apply `rules` unless the condition holds.
    pub fn unless(condition: impl IntoDataCondition, rules: impl Into<RuleSet>, default: impl Into<RuleSet>) -> ConditionalRules {
        ConditionalRules::new(condition, default, rules)
    }

    /// Compute rules for each concrete attribute from its value and name.
    pub fn for_each<R: Into<RuleSet>>(callback: impl Fn(&Value, &str) -> R + Send + Sync + 'static) -> NestedRules {
        NestedRules::new(callback)
    }

    /// The fluent date rule builder.
    pub fn date() -> DateRule {
        DateRule::new()
    }

    /// A date rule requiring the `Y-m-d H:i:s` format.
    pub fn date_time() -> DateRule {
        DateRule::new().format("Y-m-d H:i:s")
    }

    /// The fluent numeric rule builder.
    pub fn numeric() -> NumericRule {
        NumericRule::new()
    }

    /// The fluent string rule builder.
    pub fn string() -> StringRule {
        StringRule::new()
    }

    /// The fluent e-mail rule builder.
    pub fn email() -> EmailRule {
        EmailRule::new()
    }

    /// The fluent file rule builder.
    pub fn file() -> FileRule {
        FileRule::new()
    }

    /// An image file rule (SVGs are not allowed).
    pub fn image_file() -> ImageFile {
        ImageFile::new(false)
    }

    /// Image dimension constraints.
    pub fn dimensions() -> Dimensions {
        Dimensions::new()
    }

    /// The field must hold one of the backed enum's values.
    pub fn enum_<T: BackedEnum>() -> Enum<T> {
        Enum::new()
    }

    /// The field must hold one of the given enum values.
    pub fn enum_values<V: Into<Value>>(values: impl IntoIterator<Item = V>) -> RuleItem {
        RuleItem::Str(enum_values_rule(values))
    }

    /// The field must pass at least one of the given rule sets.
    pub fn any_of<T: Into<RuleSet>>(alternatives: impl IntoIterator<Item = T>) -> AnyOf {
        AnyOf::new(alternatives)
    }

    /// A rule defined by a closure receiving the attribute, its value, and
    /// the `fail` callback.
    ///
    /// ```
    /// use illuminate_validation::{Rule, rules};
    ///
    /// let rules = rules! {
    ///     "title" => ["required", Rule::closure(|attribute, value, fail| {
    ///         if value == "foo" {
    ///             fail(&format!("The {attribute} is invalid."));
    ///         }
    ///     })],
    /// };
    /// ```
    pub fn closure(callback: impl Fn(&str, &Value, &mut FailCallback<'_>) + Send + Sync + 'static) -> ClosureRule {
        ClosureRule::new(callback)
    }
}
