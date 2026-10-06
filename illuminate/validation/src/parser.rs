//! Laravel's `ValidationRuleParser`: parse string rules into a name and its
//! parameters, and explode wildcard attributes (`items.*.name`) into the
//! concrete attributes present in the data.

use std::sync::Arc;

use indexmap::IndexMap;

use illuminate_support::{Str, Value};

use crate::data;
use crate::php::str_getcsv;
use crate::rule::ValidationRule;
use crate::rules::{DatabaseRule, RuleItem, RuleSet, Rules};

/// A rule ready to run against a concrete attribute.
#[derive(Clone)]
pub(crate) enum Compiled {
    /// A built-in (or extension) rule, by its studly name.
    Named {
        name: String,
        params: Vec<String>,
        raw: String,
    },
    /// A fluent `unique` / `exists` rule.
    Database(Arc<DatabaseRule>),
    /// A rule object.
    Custom(Arc<dyn ValidationRule>),
}

impl Compiled {
    /// The studly name of a built-in rule.
    pub(crate) fn name(&self) -> Option<&str> {
        match self {
            Compiled::Named { name, .. } => Some(name),
            Compiled::Database(rule) => Some(match rule.kind {
                crate::rules::DatabaseRuleKind::Unique => "Unique",
                crate::rules::DatabaseRuleKind::Exists => "Exists",
            }),
            Compiled::Custom(_) => None,
        }
    }

    /// The parameters of a built-in rule.
    pub(crate) fn params(&self) -> Vec<String> {
        match self {
            Compiled::Named { params, .. } => params.clone(),
            Compiled::Database(rule) => rule.params(),
            Compiled::Custom(_) => Vec::new(),
        }
    }

    /// The raw rule string as written (`"boolean"`, `"array"`).
    pub(crate) fn raw(&self) -> Option<&str> {
        match self {
            Compiled::Named { raw, .. } => Some(raw),
            _ => None,
        }
    }
}

/// Parse a string rule (`"max:255"`) into its studly name and parameters.
pub(crate) fn parse_string_rule(rule: &str) -> Option<(String, Vec<String>)> {
    let (name, params) = match rule.split_once(':') {
        Some((name, parameter)) => {
            let lower = name.trim().to_ascii_lowercase();
            let params = if matches!(lower.as_str(), "regex" | "not_regex" | "notregex") {
                vec![parameter.to_string()]
            } else {
                str_getcsv(parameter)
            };
            (name, params)
        }
        None => (rule, Vec::new()),
    };
    let name = Str::studly(name.trim());
    let name = match name.as_str() {
        "Int" => "Integer".to_string(),
        "Bool" => "Boolean".to_string(),
        _ => name,
    };
    if name.is_empty() { None } else { Some((name, params)) }
}

fn compile_items(items: &[RuleItem], attribute: &str, data: &Value, out: &mut Vec<Compiled>) {
    for item in items {
        match item {
            RuleItem::Str(rule) => {
                if let Some((name, params)) = parse_string_rule(rule) {
                    out.push(Compiled::Named {
                        name,
                        params,
                        raw: rule.trim().to_string(),
                    });
                }
            }
            RuleItem::Database(rule) => out.push(Compiled::Database(Arc::new(rule.clone()))),
            RuleItem::Custom(rule) => out.push(Compiled::Custom(rule.clone())),
            RuleItem::Conditional(conditional) => {
                let resolved = conditional.resolve(data);
                compile_items(&resolved.items, attribute, data, out);
            }
            RuleItem::Nested(nested) => {
                let value = data::get(data, attribute).cloned().unwrap_or(Value::Null);
                let resolved = nested.compile(&value, &data::unescape(attribute));
                compile_items(&resolved.items, attribute, data, out);
            }
        }
    }
}

/// Compile a rule set for one concrete attribute.
pub(crate) fn compile(set: &RuleSet, attribute: &str, data: &Value) -> Vec<Compiled> {
    let mut out = Vec::new();
    compile_items(&set.items, attribute, data, &mut out);
    out
}

/// The result of exploding a rule map against the data.
#[derive(Default)]
pub(crate) struct Exploded {
    pub(crate) rules: IndexMap<String, Vec<Compiled>>,
    pub(crate) implicit_attributes: IndexMap<String, Vec<String>>,
}

/// Explode wildcard rules into concrete attributes. Explicit attributes
/// keep their order; wildcard expansions follow, like Laravel.
pub(crate) fn explode(rules: &Rules, data: &Value) -> Exploded {
    let mut exploded = Exploded::default();
    for (attribute, set) in rules.iter() {
        if !attribute.contains('*') {
            exploded
                .rules
                .insert(attribute.clone(), compile(set, attribute, data));
        }
    }
    for (attribute, set) in rules.iter() {
        if attribute.contains('*') {
            explode_wildcard(&mut exploded, attribute, set, data);
        }
    }
    exploded
}

fn explode_wildcard(exploded: &mut Exploded, attribute: &str, set: &RuleSet, data: &Value) {
    let keys = data::expand_wildcard(attribute, data);
    let implicit = exploded.implicit_attributes.entry(attribute.to_string()).or_default();
    for key in &keys {
        if !implicit.contains(key) {
            implicit.push(key.clone());
        }
    }
    for key in keys {
        let compiled = compile(set, &key, data);
        exploded.rules.entry(key).or_default().extend(compiled);
    }
}

/// Explode a single attribute's rules (used by `sometimes`).
pub(crate) fn explode_one(attribute: &str, set: &RuleSet, data: &Value) -> Exploded {
    let mut exploded = Exploded::default();
    if attribute.contains('*') {
        explode_wildcard(&mut exploded, attribute, set, data);
    } else {
        exploded
            .rules
            .insert(attribute.to_string(), compile(set, attribute, data));
    }
    exploded
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn it_parses_string_rules() {
        assert_eq!(parse_string_rule("required"), Some(("Required".into(), vec![])));
        assert_eq!(parse_string_rule("max:255"), Some(("Max".into(), vec!["255".into()])));
        assert_eq!(
            parse_string_rule("required_if:role,admin,owner"),
            Some(("RequiredIf".into(), vec!["role".into(), "admin".into(), "owner".into()]))
        );
        assert_eq!(parse_string_rule("regex:/^a,b$/"), Some(("Regex".into(), vec!["/^a,b$/".into()])));
        assert_eq!(parse_string_rule("int"), Some(("Integer".into(), vec![])));
        assert_eq!(parse_string_rule(""), None);
    }

    #[test]
    fn it_explodes_wildcards_after_explicit_rules() {
        let rules = Rules::from([("items.*.name", "required"), ("title", "required|string")]);
        let data = json!({"items": [{"name": "a"}, {"name": ""}], "title": "x"});
        let exploded = explode(&rules, &data);
        let keys: Vec<&String> = exploded.rules.keys().collect();
        assert_eq!(keys, vec!["title", "items.0.name", "items.1.name"]);
        assert_eq!(exploded.implicit_attributes["items.*.name"], vec!["items.0.name", "items.1.name"]);
    }
}
