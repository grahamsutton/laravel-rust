//! The `AnyOf` rule: the value must satisfy at least one of several rule sets.

use async_trait::async_trait;

use illuminate_support::{Map, Value};

use super::{RuleSet, Rules};
use crate::rule::{FailCallback, ValidationContext, ValidationRule};

#[derive(Clone, Debug)]
enum Alternative {
    /// Rules for the value itself.
    Set(RuleSet),
    /// Rules keyed by the value's own keys (for array values).
    Keyed(Rules),
}

/// The field must pass any one of the given rule sets (`Rule::any_of`).
///
/// ```
/// use illuminate_validation::Rule;
///
/// // Either an e-mail address or an alpha-dash username of 6+ characters.
/// let rule = Rule::any_of(["string|email", "string|alpha_dash|min:6"]);
/// ```
#[derive(Clone, Debug)]
pub struct AnyOf {
    alternatives: Vec<Alternative>,
}

impl AnyOf {
    /// Create the rule from rule sets that apply to the value itself.
    pub fn new<T: Into<RuleSet>>(alternatives: impl IntoIterator<Item = T>) -> Self {
        Self {
            alternatives: alternatives.into_iter().map(|a| Alternative::Set(a.into())).collect(),
        }
    }

    /// Create the rule from keyed rule sets, validated against the keys of
    /// an array value.
    pub fn keyed<T: Into<Rules>>(alternatives: impl IntoIterator<Item = T>) -> Self {
        Self {
            alternatives: alternatives.into_iter().map(|a| Alternative::Keyed(a.into())).collect(),
        }
    }
}

#[async_trait]
impl ValidationRule for AnyOf {
    async fn validate_with(
        &self,
        _attribute: &str,
        value: &Value,
        context: &ValidationContext<'_>,
        fail: &mut FailCallback<'_>,
    ) {
        for alternative in &self.alternatives {
            let (data, rules) = match alternative {
                Alternative::Set(set) => {
                    let mut data = Map::new();
                    data.insert("0".into(), value.clone());
                    (Value::Object(data), Rules::new().rule("0", set.clone()))
                }
                Alternative::Keyed(rules) => {
                    let data = if value.is_object() {
                        value.clone()
                    } else {
                        let mut data = Map::new();
                        data.insert("0".into(), value.clone());
                        Value::Object(data)
                    };
                    (data, rules.clone())
                }
            };
            let mut validator = context.validator().nested(data, rules);
            match validator.try_passes().await {
                Ok(true) => return,
                Ok(false) => {}
                Err(error) => {
                    context.abort(error);
                    return;
                }
            }
        }
        let message = context
            .validator()
            .language_line("validation.any_of")
            .unwrap_or_else(|| "The :attribute field is invalid.".to_string());
        fail(&message);
    }

    fn name(&self) -> String {
        "AnyOf".to_string()
    }
}
