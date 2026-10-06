//! Error messages: Laravel's English validation lines, the pluggable
//! [`MessageResolver`] (the translator's hook), and the logic that picks a
//! message for a failed rule and fills in its placeholders.

use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;

use illuminate_support::{Str, Value, ValueExt, json};

use crate::data;
use crate::date;
use crate::php::is_numeric;
use crate::validator::{NUMERIC_RULES, SIZE_RULES, Validator};

/// Resolves validation language lines. The translation component binds an
/// implementation as `dyn MessageResolver` in the container (or registers
/// one with `Validator::resolve_messages_using`) to localize messages.
///
/// Keys follow Laravel's `lang/xx/validation.php` file: `validation.required`,
/// `validation.between.string`, `validation.custom` (attribute-specific
/// messages), `validation.attributes` (attribute names), and
/// `validation.values.{attribute}.{value}`. Lines that aren't found fall
/// back to the built-in English messages.
///
/// ```
/// use illuminate_validation::MessageResolver;
/// use illuminate_support::{json, Value};
///
/// struct Dutch;
///
/// impl MessageResolver for Dutch {
///     fn get(&self, key: &str) -> Option<Value> {
///         (key == "validation.required").then(|| json!(":attribute is verplicht."))
///     }
/// }
/// ```
pub trait MessageResolver: Send + Sync {
    /// Get the language line (a string, or an object for nested groups).
    fn get(&self, key: &str) -> Option<Value>;
}

/// Laravel's English validation messages (`lang/en/validation.php`).
#[derive(Clone, Copy, Debug, Default)]
pub struct EnglishMessages;

impl MessageResolver for EnglishMessages {
    fn get(&self, key: &str) -> Option<Value> {
        let key = key.strip_prefix("validation.")?;
        LINES.dot(key).cloned()
    }
}

static LINES: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "accepted": "The :attribute field must be accepted.",
        "accepted_if": "The :attribute field must be accepted when :other is :value.",
        "active_url": "The :attribute field must be a valid URL.",
        "after": "The :attribute field must be a date after :date.",
        "after_or_equal": "The :attribute field must be a date after or equal to :date.",
        "alpha": "The :attribute field must only contain letters.",
        "alpha_dash": "The :attribute field must only contain letters, numbers, dashes, and underscores.",
        "alpha_num": "The :attribute field must only contain letters and numbers.",
        "any_of": "The :attribute field is invalid.",
        "array": "The :attribute field must be an array.",
        "array_keys": "The :attribute field must only contain the following keys: :values.",
        "ascii": "The :attribute field must only contain single-byte alphanumeric characters and symbols.",
        "base64": "The :attribute field must be a valid Base64 string.",
        "before": "The :attribute field must be a date before :date.",
        "before_or_equal": "The :attribute field must be a date before or equal to :date.",
        "between": {
            "array": "The :attribute field must have between :min and :max items.",
            "file": "The :attribute field must be between :min and :max kilobytes.",
            "numeric": "The :attribute field must be between :min and :max.",
            "string": "The :attribute field must be between :min and :max characters.",
        },
        "boolean": "The :attribute field must be true or false.",
        "can": "The :attribute field contains an unauthorized value.",
        "confirmed": "The :attribute field confirmation does not match.",
        "contains": "The :attribute field is missing a required value.",
        "current_password": "The password is incorrect.",
        "date": "The :attribute field must be a valid date.",
        "date_equals": "The :attribute field must be a date equal to :date.",
        "date_format": "The :attribute field must match the format :format.",
        "decimal": "The :attribute field must have :decimal decimal places.",
        "declined": "The :attribute field must be declined.",
        "declined_if": "The :attribute field must be declined when :other is :value.",
        "different": "The :attribute field and :other must be different.",
        "digits": "The :attribute field must be :digits digits.",
        "digits_between": "The :attribute field must be between :min and :max digits.",
        "dimensions": "The :attribute field has invalid image dimensions.",
        "distinct": "The :attribute field has a duplicate value.",
        "doesnt_contain": "The :attribute field must not contain any of the following: :values.",
        "doesnt_end_with": "The :attribute field must not end with one of the following: :values.",
        "doesnt_start_with": "The :attribute field must not start with one of the following: :values.",
        "email": "The :attribute field must be a valid email address.",
        "encoding": "The :attribute field must be encoded in :encoding.",
        "ends_with": "The :attribute field must end with one of the following: :values.",
        "enum": "The selected :attribute is invalid.",
        "exists": "The selected :attribute is invalid.",
        "extensions": "The :attribute field must have one of the following extensions: :values.",
        "file": "The :attribute field must be a file.",
        "filled": "The :attribute field must have a value.",
        "gt": {
            "array": "The :attribute field must have more than :value items.",
            "file": "The :attribute field must be greater than :value kilobytes.",
            "numeric": "The :attribute field must be greater than :value.",
            "string": "The :attribute field must be greater than :value characters.",
        },
        "gte": {
            "array": "The :attribute field must have :value items or more.",
            "file": "The :attribute field must be greater than or equal to :value kilobytes.",
            "numeric": "The :attribute field must be greater than or equal to :value.",
            "string": "The :attribute field must be greater than or equal to :value characters.",
        },
        "hex_color": "The :attribute field must be a valid hexadecimal color.",
        "image": "The :attribute field must be an image.",
        "in": "The selected :attribute is invalid.",
        "in_array": "The :attribute field must exist in :other.",
        "in_array_keys": "The :attribute field must contain at least one of the following keys: :values.",
        "integer": "The :attribute field must be an integer.",
        "ip": "The :attribute field must be a valid IP address.",
        "ipv4": "The :attribute field must be a valid IPv4 address.",
        "ipv6": "The :attribute field must be a valid IPv6 address.",
        "json": "The :attribute field must be a valid JSON string.",
        "list": "The :attribute field must be a list.",
        "lowercase": "The :attribute field must be lowercase.",
        "lt": {
            "array": "The :attribute field must have less than :value items.",
            "file": "The :attribute field must be less than :value kilobytes.",
            "numeric": "The :attribute field must be less than :value.",
            "string": "The :attribute field must be less than :value characters.",
        },
        "lte": {
            "array": "The :attribute field must not have more than :value items.",
            "file": "The :attribute field must be less than or equal to :value kilobytes.",
            "numeric": "The :attribute field must be less than or equal to :value.",
            "string": "The :attribute field must be less than or equal to :value characters.",
        },
        "mac_address": "The :attribute field must be a valid MAC address.",
        "max": {
            "array": "The :attribute field must not have more than :max items.",
            "file": "The :attribute field must not be greater than :max kilobytes.",
            "numeric": "The :attribute field must not be greater than :max.",
            "string": "The :attribute field must not be greater than :max characters.",
        },
        "max_digits": "The :attribute field must not have more than :max digits.",
        "mimes": "The :attribute field must be a file of type: :values.",
        "mimetypes": "The :attribute field must be a file of type: :values.",
        "min": {
            "array": "The :attribute field must have at least :min items.",
            "file": "The :attribute field must be at least :min kilobytes.",
            "numeric": "The :attribute field must be at least :min.",
            "string": "The :attribute field must be at least :min characters.",
        },
        "min_digits": "The :attribute field must have at least :min digits.",
        "missing": "The :attribute field must be missing.",
        "missing_if": "The :attribute field must be missing when :other is :value.",
        "missing_unless": "The :attribute field must be missing unless :other is :value.",
        "missing_with": "The :attribute field must be missing when :values is present.",
        "missing_with_all": "The :attribute field must be missing when :values are present.",
        "multiple_of": "The :attribute field must be a multiple of :value.",
        "not_in": "The selected :attribute is invalid.",
        "not_regex": "The :attribute field format is invalid.",
        "numeric": "The :attribute field must be a number.",
        "password": {
            "letters": "The :attribute field must contain at least one letter.",
            "mixed": "The :attribute field must contain at least one uppercase and one lowercase letter.",
            "numbers": "The :attribute field must contain at least one number.",
            "symbols": "The :attribute field must contain at least one symbol.",
            "uncompromised": "The given :attribute has appeared in a data leak. Please choose a different :attribute.",
        },
        "present": "The :attribute field must be present.",
        "present_if": "The :attribute field must be present when :other is :value.",
        "present_unless": "The :attribute field must be present unless :other is :value.",
        "present_with": "The :attribute field must be present when :values is present.",
        "present_with_all": "The :attribute field must be present when :values are present.",
        "prohibited": "The :attribute field is prohibited.",
        "prohibited_if": "The :attribute field is prohibited when :other is :value.",
        "prohibited_if_accepted": "The :attribute field is prohibited when :other is accepted.",
        "prohibited_if_declined": "The :attribute field is prohibited when :other is declined.",
        "prohibited_unless": "The :attribute field is prohibited unless :other is in :values.",
        "prohibits": "The :attribute field prohibits :other from being present.",
        "regex": "The :attribute field format is invalid.",
        "required": "The :attribute field is required.",
        "required_array_keys": "The :attribute field must contain entries for: :values.",
        "required_if": "The :attribute field is required when :other is :value.",
        "required_if_accepted": "The :attribute field is required when :other is accepted.",
        "required_if_declined": "The :attribute field is required when :other is declined.",
        "required_unless": "The :attribute field is required unless :other is in :values.",
        "required_with": "The :attribute field is required when :values is present.",
        "required_with_all": "The :attribute field is required when :values are present.",
        "required_without": "The :attribute field is required when :values is not present.",
        "required_without_all": "The :attribute field is required when none of :values are present.",
        "same": "The :attribute field must match :other.",
        "size": {
            "array": "The :attribute field must contain :size items.",
            "file": "The :attribute field must be :size kilobytes.",
            "numeric": "The :attribute field must be :size.",
            "string": "The :attribute field must be :size characters.",
        },
        "starts_with": "The :attribute field must start with one of the following: :values.",
        "string": "The :attribute field must be a string.",
        "timezone": "The :attribute field must be a valid timezone.",
        "unique": "The :attribute has already been taken.",
        "uploaded": "The :attribute failed to upload.",
        "uppercase": "The :attribute field must be uppercase.",
        "url": "The :attribute field must be a valid URL.",
        "ulid": "The :attribute field must be a valid ULID.",
        "uuid": "The :attribute field must be a valid UUID.",
        "custom": {},
        "attributes": {},
    })
});

/// Compile a message key pattern (`items.*.name`) into a regex where `*`
/// matches a single segment.
fn wildcard_regex(pattern: &str) -> Option<Regex> {
    let escaped = regex::escape(pattern).replace("\\*", "([^.]*)");
    Regex::new(&format!("^{escaped}$")).ok()
}

fn wildcard_match(pattern: &str, key: &str) -> bool {
    wildcard_regex(pattern).is_some_and(|r| r.is_match(key))
}

/// Replace `:placeholder`, `:PLACEHOLDER` and `:Placeholder`.
fn replace_keeping_case(message: &str, mapping: &[(&str, String)]) -> String {
    let mut message = message.to_string();
    for (placeholder, value) in mapping {
        message = message
            .replace(&format!(":{placeholder}"), value)
            .replace(&format!(":{}", Str::upper(placeholder)), &Str::upper(value))
            .replace(&format!(":{}", Str::ucfirst(placeholder)), &Str::ucfirst(value));
    }
    message
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

fn position_word(n: usize) -> &'static str {
    match n {
        1 => "first",
        2 => "second",
        3 => "third",
        4 => "fourth",
        5 => "fifth",
        6 => "sixth",
        7 => "seventh",
        8 => "eighth",
        9 => "ninth",
        10 => "tenth",
        _ => "other",
    }
}

fn str_ireplace(message: &str, search: &str, replace: &str) -> String {
    let pattern = Regex::new(&format!("(?i){}", regex::escape(search))).expect("escaped pattern");
    pattern.replace_all(message, regex::NoExpand(replace)).into_owned()
}

impl Validator {
    // ------------------------------------------------------------------
    // Language lines
    // ------------------------------------------------------------------

    /// Look up a validation language line: the registered resolver first,
    /// then the built-in English messages.
    pub fn language_line(&self, key: &str) -> Option<String> {
        self.language_value(key).and_then(|v| v.as_str().map(str::to_string))
    }

    pub(crate) fn language_value(&self, key: &str) -> Option<Value> {
        if let Some(resolver) = &self.resolver {
            if let Some(value) = resolver.get(key) {
                return Some(value);
            }
        }
        EnglishMessages.get(key)
    }

    // ------------------------------------------------------------------
    // Choosing the message
    // ------------------------------------------------------------------

    /// The type of the attribute, for size messages: `numeric`, `array`,
    /// `file` or `string`.
    pub(crate) fn attribute_type(&self, attribute: &str, extra_numeric: bool) -> &'static str {
        if extra_numeric || self.has_rule(attribute, NUMERIC_RULES) {
            "numeric"
        } else if self.has_rule(attribute, &["Array", "List"]) {
            "array"
        } else if self.value(attribute).is_some_and(data::is_file) {
            "file"
        } else {
            "string"
        }
    }

    /// Get the message for a failed rule (`FormatsMessages::getMessage`).
    pub(crate) fn get_message(&self, attribute: &str, rule: &str, extra_numeric: bool) -> String {
        let display = data::unescape(attribute);
        let lower_rule = Str::snake(rule);
        let is_size_rule = SIZE_RULES.contains(&rule);
        let attribute_type = self.attribute_type(attribute, extra_numeric);

        // Inline messages given to the validator.
        if let Some(inline) = self.get_from_local_array(&display, &lower_rule, &self.custom_messages, attribute_type) {
            match inline {
                Value::String(message) => return message,
                Value::Object(map) if is_size_rule => {
                    if let Some(message) = map.get(attribute_type).and_then(Value::as_str) {
                        return message.to_string();
                    }
                }
                _ => {}
            }
        }

        // Custom messages from the language files.
        let custom_key = format!("validation.custom.{display}.{lower_rule}");
        let keys = if is_size_rule {
            vec![format!("{custom_key}.{attribute_type}"), custom_key]
        } else {
            vec![custom_key]
        };
        if let Some(message) = self.custom_message_from_translator(&keys) {
            return message;
        }

        if is_size_rule {
            let key = format!("validation.{lower_rule}.{attribute_type}");
            return self.language_line(&key).unwrap_or(key);
        }

        let key = format!("validation.{lower_rule}");
        if let Some(line) = self.language_line(&key) {
            return line;
        }

        if let Some(Value::String(message)) =
            self.get_from_local_array(&display, &lower_rule, &self.extensions.fallback_messages, attribute_type)
        {
            return message;
        }

        key
    }

    /// Find a message in a local array (custom or fallback messages),
    /// supporting `attribute.rule`, `rule`, `attribute` and wildcard keys.
    pub(crate) fn get_from_local_array(
        &self,
        attribute: &str,
        lower_rule: &str,
        source: &IndexMap<String, Value>,
        attribute_type: &str,
    ) -> Option<Value> {
        let mut keys = vec![format!("{attribute}.{lower_rule}"), lower_rule.to_string(), attribute.to_string()];
        if attribute_type != "file" {
            let short = format!("{attribute}.{}", Str::snake(&Str::class_basename(lower_rule)));
            if !keys.contains(&short) {
                keys.push(short);
            }
        }

        for key in &keys {
            for (source_key, message) in source {
                if source_key.contains('*') {
                    if wildcard_match(source_key, key) {
                        return match message {
                            Value::Object(map) => map.get(lower_rule).cloned(),
                            other => Some(other.clone()),
                        };
                    }
                    continue;
                }
                if source_key == key {
                    if source_key == attribute {
                        if let Value::Object(map) = message {
                            return map.get(lower_rule).cloned();
                        }
                    }
                    return Some(message.clone());
                }
            }
        }
        None
    }

    fn custom_message_from_translator(&self, keys: &[String]) -> Option<String> {
        for key in keys {
            if let Some(message) = self.language_line(key) {
                return Some(message);
            }
            let short_key = key.strip_prefix("validation.custom.").unwrap_or(key);
            if let Some(custom) = self.language_value("validation.custom") {
                let mut flattened = Vec::new();
                data::flatten(&custom, "", &mut flattened);
                for (flat_key, message) in flattened {
                    let flat_key = data::unescape(&flat_key);
                    let matches = flat_key == short_key || (flat_key.contains('*') && Str::is(&flat_key, short_key));
                    if matches {
                        if let Some(message) = message.as_str() {
                            return Some(message.to_string());
                        }
                    }
                }
            }
        }
        None
    }

    // ------------------------------------------------------------------
    // Attribute and value names
    // ------------------------------------------------------------------

    /// The wildcard pattern an expanded attribute came from
    /// (`items.0.name` gives `items.*.name`).
    pub(crate) fn primary_attribute(&self, attribute: &str) -> String {
        for (pattern, expanded) in &self.implicit_attributes {
            if expanded.iter().any(|a| a == attribute) {
                return pattern.clone();
            }
        }
        attribute.to_string()
    }

    fn attribute_from_local_array(attribute: &str, source: &IndexMap<String, String>) -> Option<String> {
        if let Some(name) = source.get(attribute) {
            return Some(name.clone());
        }
        source
            .iter()
            .find(|(key, _)| key.contains('*') && wildcard_match(key, attribute))
            .map(|(_, name)| name.clone())
    }

    fn attribute_from_translations(&self, attribute: &str) -> Option<String> {
        let attributes = self.language_value("validation.attributes")?;
        let mut flattened = Vec::new();
        data::flatten(&attributes, "", &mut flattened);
        let source: IndexMap<String, String> = flattened
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|v| (data::unescape(&k), v.to_string())))
            .collect();
        Self::attribute_from_local_array(attribute, &source)
    }

    /// The human-friendly name of an attribute: a custom name, a translated
    /// name, the raw name for array items (`users.0.email`), or the
    /// attribute with underscores turned into spaces (`first name`).
    pub fn displayable_attribute(&self, attribute: &str) -> String {
        let primary = self.primary_attribute(attribute);
        let mut expected = vec![data::unescape(attribute)];
        if primary != attribute {
            expected.push(data::unescape(&primary));
        }
        for name in &expected {
            if let Some(custom) = Self::attribute_from_local_array(name, &self.custom_attributes) {
                return custom;
            }
            if let Some(translated) = self.attribute_from_translations(name) {
                return translated;
            }
        }
        let display = data::unescape(attribute);
        if self.implicit_attributes.contains_key(&primary) {
            return match &self.implicit_attributes_formatter {
                Some(formatter) => formatter(&display),
                None => display,
            };
        }
        Str::snake(&display).replace('_', " ")
    }

    /// The human-friendly representation of a value for an attribute.
    pub fn displayable_value(&self, attribute: &str, value: &Value) -> String {
        let display = data::unescape(attribute);
        let as_string = value.to_string_lossy();
        if let Some(custom) = self.custom_values.get(&display).and_then(|values| values.get(&as_string)) {
            return custom.clone();
        }
        if data::is_array(value) {
            return "array".to_string();
        }
        if let Some(line) = self.language_line(&format!("validation.values.{display}.{as_string}")) {
            return line;
        }
        match value {
            Value::Bool(true) => "true".to_string(),
            Value::Bool(false) => "false".to_string(),
            Value::Null => "empty".to_string(),
            _ => as_string,
        }
    }

    fn attribute_list(&self, attributes: &[String]) -> Vec<String> {
        attributes.iter().map(|a| self.displayable_attribute(a)).collect()
    }

    // ------------------------------------------------------------------
    // Replacements
    // ------------------------------------------------------------------

    /// Replace all placeholders in a message (`makeReplacements`).
    pub(crate) fn make_replacements(
        &self,
        message: &str,
        attribute: &str,
        rule: &str,
        parameters: &[String],
        extra_numeric: bool,
    ) -> String {
        let display = self.displayable_attribute(attribute);
        let mut message = message
            .replace(":attribute", &display)
            .replace(":ATTRIBUTE", &Str::upper(&display))
            .replace(":Attribute", &Str::ucfirst(&display));

        if message.contains(":input") {
            let value = self.value(attribute).cloned().unwrap_or(Value::Null);
            if !matches!(value, Value::Array(_) | Value::Object(_)) {
                message = message.replace(":input", &self.displayable_value(attribute, &value));
            }
        }

        message = self.replace_index_or_position(&message, attribute, "index", |n| n.to_string());
        message = self.replace_index_or_position(&message, attribute, "position", |n| (n + 1).to_string());
        message = self.replace_index_or_position(&message, attribute, "ordinal-position", |n| ordinal(n + 1));

        let lower_rule = Str::snake(rule);
        if let Some(replacer) = self.extensions.replacers.get(&lower_rule) {
            return replacer(&message, &data::unescape(attribute), &lower_rule, parameters);
        }
        self.replace_builtin(message, attribute, rule, parameters, extra_numeric)
    }

    fn replace_index_or_position(
        &self,
        message: &str,
        attribute: &str,
        placeholder: &str,
        modifier: impl Fn(usize) -> String,
    ) -> String {
        let lower = message.to_lowercase();
        if !lower.contains(&format!(":{placeholder}")) && !lower.contains(&format!("-{placeholder}")) {
            return message.to_string();
        }
        let mut message = message.to_string();
        let mut numeric_index = 1;
        for segment in data::segments(attribute) {
            if let Ok(index) = segment.parse::<usize>() {
                let replacement = modifier(index);
                if numeric_index == 1 {
                    message = str_ireplace(&message, &format!(":{placeholder}"), &replacement);
                }
                message = str_ireplace(
                    &message,
                    &format!(":{}-{placeholder}", position_word(numeric_index)),
                    &replacement,
                );
                numeric_index += 1;
            }
        }
        message
    }

    fn replace_builtin(
        &self,
        message: String,
        attribute: &str,
        rule: &str,
        params: &[String],
        extra_numeric: bool,
    ) -> String {
        let p = |i: usize| params.get(i).cloned().unwrap_or_default();
        let joined_attributes = |separator: &str| self.attribute_list(params).join(separator);

        match rule {
            "AcceptedIf" | "DeclinedIf" | "MissingIf" | "PresentIf" | "RequiredIf" | "ProhibitedIf" => {
                let other_value = data::get(self.data(), &p(0)).cloned().unwrap_or(Value::Null);
                let value = self.displayable_value(&p(0), &other_value);
                let other = self.displayable_attribute(&p(0));
                replace_keeping_case(&message, &[("other", other), ("value", value)])
            }
            "Between" | "DigitsBetween" => message.replace(":min", &p(0)).replace(":max", &p(1)),
            "DateFormat" => message.replace(":format", &p(0)),
            "Decimal" => {
                let decimal = if params.len() > 1 { format!("{}-{}", p(0), p(1)) } else { p(0) };
                message.replace(":decimal", &decimal)
            }
            "Different" | "Same" | "InArray" | "RequiredIfAccepted" | "RequiredIfDeclined"
            | "ProhibitedIfAccepted" | "ProhibitedIfDeclined" => {
                replace_keeping_case(&message, &[("other", self.displayable_attribute(&p(0)))])
            }
            "Digits" => message.replace(":digits", &p(0)),
            "Encoding" => message.replace(":encoding", &p(0)),
            "Extensions" | "Mimes" | "Mimetypes" => message.replace(":values", &params.join(", ")),
            "Min" | "MinDigits" => message.replace(":min", &p(0)),
            "Max" | "MaxDigits" => message.replace(":max", &p(0)),
            "MissingUnless" | "PresentUnless" => message
                .replace(":other", &self.displayable_attribute(&p(0)))
                .replace(":value", &self.displayable_value(&p(0), &Value::String(p(1)))),
            "MissingWith" | "MissingWithAll" | "PresentWith" | "PresentWithAll" | "RequiredWith"
            | "RequiredWithAll" | "RequiredWithout" | "RequiredWithoutAll" => {
                let values = joined_attributes(" / ");
                let ucfirst: Vec<String> = self.attribute_list(params).iter().map(|a| Str::ucfirst(a)).collect();
                message
                    .replace(":values", &values)
                    .replace(":VALUES", &Str::upper(&values))
                    .replace(":Values", &ucfirst.join(" / "))
            }
            "MultipleOf" => message.replace(":value", &p(0)),
            "In" | "NotIn" | "InArrayKeys" | "RequiredArrayKeys" | "EndsWith" | "DoesntEndWith" | "StartsWith"
            | "DoesntStartWith" | "DoesntContain" => self.replace_in(&message, attribute, params),
            "ArrayKeys" => {
                let message = self.replace_in(&message, attribute, params);
                let unexpected: Vec<String> = match self.value(attribute) {
                    Some(value) if data::is_array(value) => data::keys(value)
                        .into_iter()
                        .map(|k| data::unescape(&k))
                        .filter(|k| !params.contains(k))
                        .map(|k| self.displayable_value(attribute, &Value::String(k)))
                        .collect(),
                    _ => Vec::new(),
                };
                replace_keeping_case(&message, &[("unexpected", unexpected.join(", "))])
            }
            "Size" => message.replace(":size", &p(0)),
            "Gt" | "Lt" | "Gte" | "Lte" => match data::get(self.data(), &p(0)) {
                None | Some(Value::Null) => message.replace(":value", &self.displayable_attribute(&p(0))),
                Some(value) => {
                    let size = self
                        .size_of(attribute, value, extra_numeric || is_numeric(value))
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    message.replace(":value", &size)
                }
            },
            "RequiredUnless" | "ProhibitedUnless" => {
                let other = self.displayable_attribute(&p(0));
                let values: Vec<String> = params
                    .iter()
                    .skip(1)
                    .map(|v| self.displayable_value(&p(0), &Value::String(v.clone())))
                    .collect();
                let joined = values.join(", ");
                let ucfirst: Vec<String> = values.iter().map(|v| Str::ucfirst(v)).collect();
                message
                    .replace(":other", &other)
                    .replace(":OTHER", &Str::upper(&other))
                    .replace(":Other", &Str::ucfirst(&other))
                    .replace(":values", &joined)
                    .replace(":VALUES", &Str::upper(&joined))
                    .replace(":Values", &ucfirst.join(", "))
            }
            "Prohibits" => {
                let others = joined_attributes(" / ");
                let ucfirst: Vec<String> = self.attribute_list(params).iter().map(|a| Str::ucfirst(a)).collect();
                message
                    .replace(":other", &others)
                    .replace(":OTHER", &Str::upper(&others))
                    .replace(":Other", &ucfirst.join(" / "))
            }
            "Before" | "BeforeOrEqual" | "After" | "AfterOrEqual" | "DateEquals" => {
                let date = if date::parse(&p(0)).is_none() || p(0).is_empty() {
                    self.displayable_attribute(&p(0))
                } else {
                    self.displayable_value(attribute, &Value::String(p(0)))
                };
                message.replace(":date", &date)
            }
            "Dimensions" => {
                let mut message = message;
                for param in params {
                    let (key, value) = param.split_once('=').unwrap_or((param.as_str(), ""));
                    message = message.replace(&format!(":{key}"), value);
                }
                message
            }
            _ => message,
        }
    }

    fn replace_in(&self, message: &str, attribute: &str, params: &[String]) -> String {
        let values: Vec<String> = params
            .iter()
            .map(|p| self.displayable_value(attribute, &Value::String(p.clone())))
            .collect();
        let joined = values.join(", ");
        let ucfirst: Vec<String> = values.iter().map(|v| Str::ucfirst(v)).collect();
        message
            .replace(":values", &joined)
            .replace(":VALUES", &Str::upper(&joined))
            .replace(":Values", &ucfirst.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_lines_resolve_nested_keys() {
        assert_eq!(
            EnglishMessages.get("validation.required"),
            Some(json!("The :attribute field is required."))
        );
        assert_eq!(
            EnglishMessages.get("validation.between.string"),
            Some(json!("The :attribute field must be between :min and :max characters."))
        );
        assert_eq!(EnglishMessages.get("validation.nope"), None);
        assert_eq!(EnglishMessages.get("required"), None);
    }

    #[test]
    fn placeholders_keep_their_case() {
        assert_eq!(
            replace_keeping_case(":other / :OTHER / :Other", &[("other", "first name".into())]),
            "first name / FIRST NAME / First name"
        );
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(12), "12th");
        assert_eq!(ordinal(23), "23rd");
    }
}
