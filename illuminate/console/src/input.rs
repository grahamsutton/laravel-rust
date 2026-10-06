//! Command input: argument and option definitions, and binding the raw
//! command line (or an array of parameters) against them.
//!
//! ```
//! use illuminate_console::input::{Input, InputValue};
//! use illuminate_console::Parser;
//!
//! let signature = Parser::parse("mail:send {user} {--queue=default}").unwrap();
//! let input = Input::from_tokens(["1", "--queue=emails"], &signature.definition()).unwrap();
//!
//! assert_eq!(input.argument("user"), Some(InputValue::from("1")));
//! assert_eq!(input.option("queue"), Some(InputValue::from("emails")));
//! ```

use illuminate_support::Value;
use indexmap::IndexMap;

/// Thrown when the given input does not satisfy a command's definition,
/// such as a missing argument or an unknown option.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct InvalidInputException {
    pub message: String,
}

impl InvalidInputException {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Thrown when a command signature or definition is malformed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct InvalidDefinitionException {
    pub message: String,
}

impl InvalidDefinitionException {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

// ----------------------------------------------------------------------
// Values
// ----------------------------------------------------------------------

/// The value of an argument or option.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum InputValue {
    /// No value (PHP's `null`).
    #[default]
    Null,
    /// A flag: `--force` is `true` when given, `false` otherwise.
    Bool(bool),
    /// A single value.
    String(String),
    /// Several values, for `{user*}` arguments and `{--id=*}` options.
    Array(Vec<String>),
}

impl InputValue {
    /// The value as a string: flags become `"1"`, arrays are comma separated,
    /// and `null` / `false` become `None`.
    pub fn as_string(&self) -> Option<String> {
        match self {
            InputValue::Null | InputValue::Bool(false) => None,
            InputValue::Bool(true) => Some("1".to_string()),
            InputValue::String(value) => Some(value.clone()),
            InputValue::Array(values) => Some(values.join(",")),
        }
    }

    /// The value as a boolean, using PHP's truthiness rules.
    pub fn as_bool(&self) -> bool {
        match self {
            InputValue::Null => false,
            InputValue::Bool(value) => *value,
            InputValue::String(value) => {
                !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
            }
            InputValue::Array(values) => !values.is_empty(),
        }
    }

    /// The value as a list of strings.
    pub fn as_list(&self) -> Vec<String> {
        match self {
            InputValue::Null | InputValue::Bool(false) => Vec::new(),
            InputValue::Bool(true) => vec!["1".to_string()],
            InputValue::String(value) => vec![value.clone()],
            InputValue::Array(values) => values.clone(),
        }
    }

    /// Determine if the value is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, InputValue::Null)
    }

    /// Convert the value into a JSON-like [`Value`].
    pub fn to_value(&self) -> Value {
        match self {
            InputValue::Null => Value::Null,
            InputValue::Bool(value) => Value::Bool(*value),
            InputValue::String(value) => Value::String(value.clone()),
            InputValue::Array(values) => {
                Value::Array(values.iter().cloned().map(Value::String).collect())
            }
        }
    }

    fn display_default(&self) -> String {
        serde_json::to_string(&self.to_value()).unwrap_or_default()
    }
}

impl From<&str> for InputValue {
    fn from(value: &str) -> Self {
        InputValue::String(value.to_string())
    }
}

impl From<String> for InputValue {
    fn from(value: String) -> Self {
        InputValue::String(value)
    }
}

impl From<bool> for InputValue {
    fn from(value: bool) -> Self {
        InputValue::Bool(value)
    }
}

impl From<Vec<String>> for InputValue {
    fn from(values: Vec<String>) -> Self {
        InputValue::Array(values)
    }
}

impl From<Vec<&str>> for InputValue {
    fn from(values: Vec<&str>) -> Self {
        InputValue::Array(values.into_iter().map(String::from).collect())
    }
}

impl From<Option<String>> for InputValue {
    fn from(value: Option<String>) -> Self {
        value.map(InputValue::String).unwrap_or(InputValue::Null)
    }
}

// ----------------------------------------------------------------------
// Definitions
// ----------------------------------------------------------------------

/// An argument a command accepts, such as `{user}` or `{user?*}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputArgument {
    /// The argument's name.
    pub name: String,
    /// Whether the argument must be given.
    pub required: bool,
    /// Whether the argument accepts several values.
    pub array: bool,
    /// The argument's description, shown by `help`.
    pub description: String,
    /// The default value.
    pub default: InputValue,
}

impl InputArgument {
    /// A required argument.
    pub fn required(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            required: true,
            array: false,
            description: String::new(),
            default: InputValue::Null,
        }
    }

    /// An optional argument.
    pub fn optional(name: impl Into<String>) -> Self {
        Self {
            required: false,
            ..Self::required(name)
        }
    }

    /// Accept several values.
    pub fn array(mut self) -> Self {
        self.array = true;
        if self.default.is_null() {
            self.default = InputValue::Array(Vec::new());
        }
        self
    }

    /// Describe the argument.
    pub fn describe(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Give the argument a default value.
    pub fn default_value(mut self, default: impl Into<InputValue>) -> Self {
        self.default = default.into();
        self
    }
}

/// Whether (and how) an option accepts a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionMode {
    /// A switch: `--force`.
    None,
    /// A value must be given: `--queue=default`.
    Required,
    /// A value may be given: `--queue` or `--queue=default`.
    Optional,
    /// A switch that may be negated: `--ansi` / `--no-ansi`.
    Negatable,
}

/// An option a command accepts, such as `{--queue=}` or `{--Q|queue}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputOption {
    /// The option's name (without the leading dashes).
    pub name: String,
    /// The option's shortcut(s), separated by `|` (e.g. `Q` or `v|vv|vvv`).
    pub shortcut: Option<String>,
    /// Whether the option accepts a value.
    pub mode: OptionMode,
    /// Whether the option accepts several values.
    pub array: bool,
    /// The option's description, shown by `help`.
    pub description: String,
    /// The default value.
    pub default: InputValue,
}

impl InputOption {
    /// Create an option.
    pub fn new(
        name: impl Into<String>,
        shortcut: Option<&str>,
        mode: OptionMode,
        description: impl Into<String>,
    ) -> Self {
        let name: String = name.into();
        let shortcut = shortcut.and_then(|shortcut| {
            let parts: Vec<&str> = shortcut
                .trim_start_matches('-')
                .split('|')
                .map(|part| part.trim_start_matches('-'))
                .filter(|part| !part.is_empty())
                .collect();
            (!parts.is_empty()).then(|| parts.join("|"))
        });

        let default = match mode {
            OptionMode::None => InputValue::Bool(false),
            _ => InputValue::Null,
        };

        Self {
            name: name.trim_start_matches("--").to_string(),
            shortcut,
            mode,
            array: false,
            description: description.into(),
            default,
        }
    }

    /// A switch option, such as `--force`.
    pub fn flag(name: impl Into<String>) -> Self {
        Self::new(name, None, OptionMode::None, "")
    }

    /// An option that may receive a value.
    pub fn value(name: impl Into<String>) -> Self {
        Self::new(name, None, OptionMode::Optional, "")
    }

    /// Accept several values.
    pub fn array(mut self) -> Self {
        self.array = true;
        if self.default.is_null() {
            self.default = InputValue::Array(Vec::new());
        }
        self
    }

    /// Set the shortcut.
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Self::new("x", Some(&shortcut.into()), OptionMode::None, "").shortcut;
        self
    }

    /// Describe the option.
    pub fn describe(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Give the option a default value.
    pub fn default_value(mut self, default: impl Into<InputValue>) -> Self {
        self.default = default.into();
        self
    }

    /// Determine if the option accepts a value.
    pub fn accepts_value(&self) -> bool {
        matches!(self.mode, OptionMode::Required | OptionMode::Optional)
    }

    /// Determine if the option's value is optional.
    pub fn is_value_optional(&self) -> bool {
        self.mode == OptionMode::Optional
    }

    /// The option's individual shortcuts.
    pub fn shortcuts(&self) -> Vec<&str> {
        self.shortcut
            .as_deref()
            .map(|s| s.split('|').collect())
            .unwrap_or_default()
    }
}

/// The arguments and options a command accepts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputDefinition {
    arguments: Vec<InputArgument>,
    options: Vec<InputOption>,
}

impl InputDefinition {
    /// Create a definition from the given arguments and options.
    pub fn new(
        arguments: Vec<InputArgument>,
        options: Vec<InputOption>,
    ) -> Result<Self, InvalidDefinitionException> {
        let mut definition = Self::default();
        for argument in arguments {
            definition.add_argument(argument)?;
        }
        for option in options {
            definition.add_option(option)?;
        }
        Ok(definition)
    }

    /// Add an argument.
    pub fn add_argument(
        &mut self,
        argument: InputArgument,
    ) -> Result<(), InvalidDefinitionException> {
        if self.argument(&argument.name).is_some() {
            return Err(InvalidDefinitionException::new(format!(
                "An argument with name \"{}\" already exists.",
                argument.name
            )));
        }

        if let Some(last) = self.arguments.last() {
            if last.array {
                return Err(InvalidDefinitionException::new(format!(
                    "Cannot add a required argument \"{}\" after an array argument \"{}\".",
                    argument.name, last.name
                )));
            }

            if argument.required && !last.required {
                return Err(InvalidDefinitionException::new(format!(
                    "Cannot add a required argument \"{}\" after an optional one \"{}\".",
                    argument.name, last.name
                )));
            }
        }

        self.arguments.push(argument);
        Ok(())
    }

    /// Add an option.
    pub fn add_option(&mut self, option: InputOption) -> Result<(), InvalidDefinitionException> {
        if option.name.is_empty() {
            return Err(InvalidDefinitionException::new(
                "An option name cannot be empty.",
            ));
        }

        if let Some(existing) = self.option(&option.name) {
            if *existing != option {
                return Err(InvalidDefinitionException::new(format!(
                    "An option named \"{}\" already exists.",
                    option.name
                )));
            }
            return Ok(());
        }

        if self.negation(&option.name).is_some() {
            return Err(InvalidDefinitionException::new(format!(
                "An option named \"{}\" already exists.",
                option.name
            )));
        }

        for shortcut in option.shortcuts() {
            if self.option_for_shortcut(shortcut).is_some() {
                return Err(InvalidDefinitionException::new(format!(
                    "An option with shortcut \"{shortcut}\" already exists."
                )));
            }
        }

        self.options.push(option);
        Ok(())
    }

    /// The defined arguments, in order.
    pub fn arguments(&self) -> &[InputArgument] {
        &self.arguments
    }

    /// The defined options, in order.
    pub fn options(&self) -> &[InputOption] {
        &self.options
    }

    /// Find an argument by name.
    pub fn argument(&self, name: &str) -> Option<&InputArgument> {
        self.arguments.iter().find(|argument| argument.name == name)
    }

    /// Find an option by name.
    pub fn option(&self, name: &str) -> Option<&InputOption> {
        self.options.iter().find(|option| option.name == name)
    }

    /// Find the option with the given shortcut.
    pub fn option_for_shortcut(&self, shortcut: &str) -> Option<&InputOption> {
        self.options
            .iter()
            .find(|option| option.shortcuts().contains(&shortcut))
    }

    /// Find the negatable option negated by the given name (`no-ansi` → `ansi`).
    pub fn negation(&self, name: &str) -> Option<&InputOption> {
        let name = name.strip_prefix("no-")?;
        self.option(name)
            .filter(|option| option.mode == OptionMode::Negatable)
    }

    /// Merge another definition's options (and optionally arguments) into this one.
    pub(crate) fn merged_with(
        &self,
        application: &InputDefinition,
        merge_arguments: bool,
    ) -> InputDefinition {
        let mut merged = InputDefinition {
            arguments: Vec::new(),
            options: self.options.clone(),
        };

        for option in &application.options {
            if merged.option(&option.name).is_none() {
                let _ = merged.add_option(option.clone());
            }
        }

        if merge_arguments {
            merged.arguments = application.arguments.clone();
        }
        for argument in &self.arguments {
            if merged.argument(&argument.name).is_none() {
                merged.arguments.push(argument.clone());
            }
        }

        merged
    }

    /// The synopsis, e.g. `[options] [--] <user>`, as displayed by `help`.
    pub fn synopsis(&self, short: bool) -> String {
        let mut elements: Vec<String> = Vec::new();

        if short && !self.options.is_empty() {
            elements.push("[options]".to_string());
        } else if !short {
            for option in &self.options {
                let value = if option.accepts_value() {
                    let name = option.name.to_uppercase();
                    if option.is_value_optional() {
                        format!(" [{name}]")
                    } else {
                        format!(" {name}")
                    }
                } else {
                    String::new()
                };
                let shortcut = option
                    .shortcut
                    .as_ref()
                    .map(|s| format!("-{s}|"))
                    .unwrap_or_default();
                let negation = if option.mode == OptionMode::Negatable {
                    format!("|--no-{}", option.name)
                } else {
                    String::new()
                };
                elements.push(format!("[{shortcut}--{}{value}{negation}]", option.name));
            }
        }

        if !elements.is_empty() && !self.arguments.is_empty() {
            elements.push("[--]".to_string());
        }

        let mut tail = String::new();
        for argument in &self.arguments {
            let mut element = format!("<{}>", argument.name);
            if argument.array {
                element.push_str("...");
            }
            if !argument.required {
                element = format!("[{element}");
                tail.push(']');
            }
            elements.push(element);
        }

        format!("{}{}", elements.join(" "), tail)
    }

    pub(crate) fn default_display(value: &InputValue) -> Option<String> {
        match value {
            InputValue::Null => None,
            InputValue::Array(values) if values.is_empty() => None,
            other => Some(other.display_default()),
        }
    }
}

// ----------------------------------------------------------------------
// Parameters passed programmatically (Artisan::call)
// ----------------------------------------------------------------------

/// A parameter value passed to `Artisan::call`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgValue {
    /// No value.
    Null,
    /// A boolean, typically for switches: `("--force", true)`.
    Bool(bool),
    /// A single value.
    String(String),
    /// Several values: `("--id", vec!["5", "13"])`.
    Array(Vec<String>),
}

impl From<&str> for ArgValue {
    fn from(value: &str) -> Self {
        ArgValue::String(value.to_string())
    }
}

impl From<String> for ArgValue {
    fn from(value: String) -> Self {
        ArgValue::String(value)
    }
}

impl From<&String> for ArgValue {
    fn from(value: &String) -> Self {
        ArgValue::String(value.clone())
    }
}

impl From<bool> for ArgValue {
    fn from(value: bool) -> Self {
        ArgValue::Bool(value)
    }
}

macro_rules! arg_value_from_number {
    ($($ty:ty),*) => {
        $(impl From<$ty> for ArgValue {
            fn from(value: $ty) -> Self {
                ArgValue::String(value.to_string())
            }
        })*
    };
}

arg_value_from_number!(i8, i16, i32, i64, u8, u16, u32, u64, usize, isize, f32, f64);

impl From<Vec<&str>> for ArgValue {
    fn from(values: Vec<&str>) -> Self {
        ArgValue::Array(values.into_iter().map(String::from).collect())
    }
}

impl From<Vec<String>> for ArgValue {
    fn from(values: Vec<String>) -> Self {
        ArgValue::Array(values)
    }
}

impl<const N: usize> From<[&str; N]> for ArgValue {
    fn from(values: [&str; N]) -> Self {
        ArgValue::Array(values.into_iter().map(String::from).collect())
    }
}

impl<T: Into<ArgValue>> From<Option<T>> for ArgValue {
    fn from(value: Option<T>) -> Self {
        value.map(Into::into).unwrap_or(ArgValue::Null)
    }
}

impl From<Value> for ArgValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Null => ArgValue::Null,
            Value::Bool(value) => ArgValue::Bool(value),
            Value::String(value) => ArgValue::String(value),
            Value::Number(number) => ArgValue::String(number.to_string()),
            Value::Array(values) => ArgValue::Array(
                values
                    .into_iter()
                    .map(|value| match value {
                        Value::String(value) => value,
                        other => other.to_string(),
                    })
                    .collect(),
            ),
            object @ Value::Object(_) => ArgValue::String(object.to_string()),
        }
    }
}

/// The parameters passed to `Artisan::call` (and `Console::call`).
///
/// Use named parameters, exactly like Laravel's arrays:
///
/// ```
/// use illuminate_console::ArtisanArgs;
/// use illuminate_support::json;
///
/// let args: ArtisanArgs = json!({"user": 1, "--queue": "default", "--id": [5, 13]}).into();
/// let args: ArtisanArgs = [("user", "1"), ("--queue", "default")].into();
///
/// // ...or raw command line tokens:
/// let args: ArtisanArgs = ["Taylor", "--force"].into();
/// let args: ArtisanArgs = ().into();
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtisanArgs {
    pub(crate) tokens: Vec<String>,
    pub(crate) named: Vec<(String, ArgValue)>,
}

impl ArtisanArgs {
    /// No parameters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parameters given as raw command line tokens.
    pub fn tokens<I, S>(tokens: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            tokens: tokens.into_iter().map(Into::into).collect(),
            named: Vec::new(),
        }
    }

    /// Add a named parameter: an argument name, `--option` or `-shortcut`.
    pub fn with(mut self, key: impl Into<String>, value: impl Into<ArgValue>) -> Self {
        self.named.push((key.into(), value.into()));
        self
    }

    /// Determine if there are no parameters.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty() && self.named.is_empty()
    }

    /// The named parameters.
    pub fn named(&self) -> &[(String, ArgValue)] {
        &self.named
    }

    /// The raw tokens.
    pub fn raw_tokens(&self) -> &[String] {
        &self.tokens
    }

    /// The named parameters as command line tokens (unescaped).
    pub(crate) fn named_tokens(&self) -> Vec<String> {
        let mut parts = Vec::new();

        for (key, value) in &self.named {
            let is_option = key.starts_with('-');
            let is_long = key.starts_with("--");
            match value {
                ArgValue::Null => {
                    if is_option {
                        parts.push(key.clone());
                    }
                }
                ArgValue::Bool(true) => parts.push(if is_option {
                    key.clone()
                } else {
                    "1".to_string()
                }),
                ArgValue::Bool(false) => {}
                ArgValue::String(value) if is_long => parts.push(format!("{key}={value}")),
                ArgValue::String(value) if is_option => parts.push(format!("{key}{value}")),
                ArgValue::String(value) => parts.push(value.clone()),
                ArgValue::Array(values) => {
                    for value in values {
                        parts.push(if is_long {
                            format!("{key}={value}")
                        } else if is_option {
                            format!("{key}{value}")
                        } else {
                            value.clone()
                        });
                    }
                }
            }
        }

        parts
    }

    /// Render the parameters as a command line string.
    pub fn to_command_line(&self) -> String {
        let mut parts: Vec<String> = self.tokens.iter().map(|t| escape_token(t)).collect();

        for (key, value) in &self.named {
            let is_option = key.starts_with('-');
            match value {
                ArgValue::Null => {
                    if is_option {
                        parts.push(key.clone());
                    }
                }
                ArgValue::Bool(true) => parts.push(if is_option {
                    key.clone()
                } else {
                    "1".to_string()
                }),
                ArgValue::Bool(false) => {}
                ArgValue::String(value) => parts.push(if is_option {
                    format!("{key}={}", escape_token(value))
                } else {
                    escape_token(value)
                }),
                ArgValue::Array(values) => {
                    for value in values {
                        parts.push(if key.starts_with("--") {
                            format!("{key}={}", escape_token(value))
                        } else if is_option {
                            format!("{key} {}", escape_token(value))
                        } else {
                            escape_token(value)
                        });
                    }
                }
            }
        }

        parts.join(" ")
    }
}

fn escape_token(token: &str) -> String {
    let safe = !token.is_empty()
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_=:./,@%+".contains(c));

    if safe {
        token.to_string()
    } else {
        format!("'{}'", token.replace('\'', "'\\''"))
    }
}

impl From<()> for ArtisanArgs {
    fn from(_: ()) -> Self {
        Self::default()
    }
}

impl From<Value> for ArtisanArgs {
    fn from(value: Value) -> Self {
        match value {
            Value::Object(map) => Self {
                tokens: Vec::new(),
                named: map
                    .into_iter()
                    .map(|(key, value)| (key, ArgValue::from(value)))
                    .collect(),
            },
            Value::Array(values) => Self::tokens(values.into_iter().map(|value| match value {
                Value::String(value) => value,
                other => other.to_string(),
            })),
            Value::Null => Self::default(),
            other => Self::tokens([other.to_string()]),
        }
    }
}

impl<const N: usize> From<[&str; N]> for ArtisanArgs {
    fn from(tokens: [&str; N]) -> Self {
        Self::tokens(tokens)
    }
}

impl<const N: usize> From<[String; N]> for ArtisanArgs {
    fn from(tokens: [String; N]) -> Self {
        Self::tokens(tokens)
    }
}

impl From<Vec<&str>> for ArtisanArgs {
    fn from(tokens: Vec<&str>) -> Self {
        Self::tokens(tokens)
    }
}

impl From<Vec<String>> for ArtisanArgs {
    fn from(tokens: Vec<String>) -> Self {
        Self::tokens(tokens)
    }
}

impl From<&[&str]> for ArtisanArgs {
    fn from(tokens: &[&str]) -> Self {
        Self::tokens(tokens.iter().copied())
    }
}

impl<V: Into<ArgValue>, const N: usize> From<[(&str, V); N]> for ArtisanArgs {
    fn from(pairs: [(&str, V); N]) -> Self {
        Self {
            tokens: Vec::new(),
            named: pairs
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.into()))
                .collect(),
        }
    }
}

impl<V: Into<ArgValue>> From<Vec<(&str, V)>> for ArtisanArgs {
    fn from(pairs: Vec<(&str, V)>) -> Self {
        Self {
            tokens: Vec::new(),
            named: pairs
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.into()))
                .collect(),
        }
    }
}

impl<V: Into<ArgValue>> From<Vec<(String, V)>> for ArtisanArgs {
    fn from(pairs: Vec<(String, V)>) -> Self {
        Self {
            tokens: Vec::new(),
            named: pairs
                .into_iter()
                .map(|(key, value)| (key, value.into()))
                .collect(),
        }
    }
}

// ----------------------------------------------------------------------
// Bound input
// ----------------------------------------------------------------------

/// Arguments and options bound against a command's definition.
#[derive(Clone, Debug, Default)]
pub struct Input {
    definition: InputDefinition,
    arguments: IndexMap<String, InputValue>,
    options: IndexMap<String, InputValue>,
}

impl Input {
    /// Bind command line tokens (without the binary or command name, unless
    /// the definition declares a `command` argument) against a definition.
    pub fn from_tokens<I, S>(
        tokens: I,
        definition: &InputDefinition,
    ) -> Result<Self, InvalidInputException>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut input = Self {
            definition: definition.clone(),
            ..Default::default()
        };

        let mut tokens: std::collections::VecDeque<String> =
            tokens.into_iter().map(Into::into).collect();
        let mut parse_options = true;

        while let Some(token) = tokens.pop_front() {
            if parse_options && token.is_empty() {
                input.parse_argument(token)?;
            } else if parse_options && token == "--" {
                parse_options = false;
            } else if parse_options && token.starts_with("--") {
                input.parse_long_option(&token, &mut tokens)?;
            } else if parse_options && token.starts_with('-') && token != "-" {
                input.parse_short_option(&token, &mut tokens)?;
            } else {
                input.parse_argument(token)?;
            }
        }

        Ok(input)
    }

    /// Bind named parameters (Laravel's `['user' => 1, '--queue' => 'x']`).
    pub fn from_named(
        parameters: &[(String, ArgValue)],
        definition: &InputDefinition,
    ) -> Result<Self, InvalidInputException> {
        let mut input = Self {
            definition: definition.clone(),
            ..Default::default()
        };

        for (key, value) in parameters {
            if key == "--" {
                break;
            }

            if let Some(name) = key.strip_prefix("--") {
                input.add_named_option(name, value)?;
            } else if let Some(shortcut) = key.strip_prefix('-') {
                let name = input
                    .definition
                    .option_for_shortcut(shortcut)
                    .map(|option| option.name.clone())
                    .ok_or_else(|| {
                        InvalidInputException::new(format!(
                            "The \"-{shortcut}\" option does not exist."
                        ))
                    })?;
                input.add_named_option(&name, value)?;
            } else {
                let argument = input.definition.argument(key).cloned().ok_or_else(|| {
                    InvalidInputException::new(format!("The \"{key}\" argument does not exist."))
                })?;

                let value = match value {
                    ArgValue::Null => InputValue::Null,
                    ArgValue::Bool(value) => {
                        InputValue::String(if *value { "1" } else { "" }.to_string())
                    }
                    ArgValue::String(value) if argument.array => {
                        InputValue::Array(vec![value.clone()])
                    }
                    ArgValue::String(value) => InputValue::String(value.clone()),
                    ArgValue::Array(values) if argument.array => InputValue::Array(values.clone()),
                    ArgValue::Array(values) => InputValue::String(values.join(",")),
                };
                input.arguments.insert(argument.name, value);
            }
        }

        Ok(input)
    }

    fn add_named_option(
        &mut self,
        name: &str,
        value: &ArgValue,
    ) -> Result<(), InvalidInputException> {
        let Some(option) = self.definition.option(name).cloned() else {
            if let Some(option) = self.definition.negation(name) {
                let name = option.name.clone();
                self.options.insert(name, InputValue::Bool(false));
                return Ok(());
            }
            return Err(InvalidInputException::new(format!(
                "The \"--{name}\" option does not exist."
            )));
        };

        let value = match (value, option.mode) {
            (ArgValue::Null, OptionMode::Required) => {
                return Err(InvalidInputException::new(format!(
                    "The \"--{name}\" option requires a value."
                )));
            }
            (ArgValue::Null, OptionMode::Optional) if option.array => InputValue::Array(Vec::new()),
            (ArgValue::Null, OptionMode::Optional) => InputValue::Null,
            (ArgValue::Null, _) => InputValue::Bool(true),
            (ArgValue::Bool(flag), OptionMode::None | OptionMode::Negatable) => {
                InputValue::Bool(*flag)
            }
            (other, OptionMode::None | OptionMode::Negatable) => {
                InputValue::Bool(arg_to_input(other).as_bool())
            }
            (ArgValue::Array(values), _) if option.array => InputValue::Array(values.clone()),
            (ArgValue::Array(values), _) => InputValue::String(values.join(",")),
            (ArgValue::String(value), _) if option.array => InputValue::Array(vec![value.clone()]),
            (ArgValue::String(value), _) => InputValue::String(value.clone()),
            (ArgValue::Bool(flag), _) if option.array => InputValue::Array(vec![flag.to_string()]),
            (ArgValue::Bool(flag), _) => InputValue::Bool(*flag),
        };

        self.options.insert(option.name, value);
        Ok(())
    }

    fn parse_argument(&mut self, token: String) -> Result<(), InvalidInputException> {
        let count = self.arguments.len();
        let arguments = self.definition.arguments().to_vec();

        if let Some(argument) = arguments.get(count) {
            let value = if argument.array {
                InputValue::Array(vec![token])
            } else {
                InputValue::String(token)
            };
            self.arguments.insert(argument.name.clone(), value);
            return Ok(());
        }

        if count > 0
            && let Some(last) = arguments.get(count - 1).filter(|argument| argument.array)
            && let Some(InputValue::Array(values)) = self.arguments.get_mut(&last.name)
        {
            values.push(token);
            return Ok(());
        }

        let mut all: Vec<&InputArgument> = arguments.iter().collect();
        let mut command_name = None;
        if all
            .first()
            .is_some_and(|argument| argument.name == "command")
        {
            command_name = self
                .arguments
                .get("command")
                .and_then(InputValue::as_string);
            all.remove(0);
        }

        let names: Vec<&str> = all.iter().map(|argument| argument.name.as_str()).collect();

        let message = match (names.is_empty(), command_name) {
            (false, Some(command)) => format!(
                "Too many arguments to \"{command}\" command, expected arguments \"{}\".",
                names.join("\" \"")
            ),
            (false, None) => format!(
                "Too many arguments, expected arguments \"{}\".",
                names.join("\" \"")
            ),
            (true, Some(command)) => {
                format!("No arguments expected for \"{command}\" command, got \"{token}\".")
            }
            (true, None) => format!("No arguments expected, got \"{token}\"."),
        };

        Err(InvalidInputException::new(message))
    }

    fn parse_long_option(
        &mut self,
        token: &str,
        rest: &mut std::collections::VecDeque<String>,
    ) -> Result<(), InvalidInputException> {
        let name = &token[2..];

        match name.split_once('=') {
            Some((name, value)) => {
                if name.is_empty() {
                    return Err(InvalidInputException::new(format!(
                        "The \"--{name}\" option does not exist."
                    )));
                }
                self.add_long_option(name, Some(value.to_string()), rest)
            }
            None => self.add_long_option(name, None, rest),
        }
    }

    fn parse_short_option(
        &mut self,
        token: &str,
        rest: &mut std::collections::VecDeque<String>,
    ) -> Result<(), InvalidInputException> {
        let name = &token[1..];
        let mut chars = name.chars();
        let first = chars.next().map(String::from).unwrap_or_default();

        if name.chars().count() > 1 {
            let takes_value = self
                .definition
                .option_for_shortcut(&first)
                .is_some_and(InputOption::accepts_value);

            if takes_value {
                // An option with a value (with no space): -Qdefault
                let value: String = chars.collect();
                return self.add_short_option(&first, Some(value), rest);
            }

            // A set of switches: -abc (verbosity shortcuts such as -vv first).
            if self.definition.option_for_shortcut(name).is_some() {
                return self.add_short_option(name, None, rest);
            }

            let all: Vec<char> = name.chars().collect();
            for (index, c) in all.iter().enumerate() {
                let shortcut = c.to_string();
                let Some(option) = self.definition.option_for_shortcut(&shortcut).cloned() else {
                    return Err(InvalidInputException::new(format!(
                        "The \"-{c}\" option does not exist."
                    )));
                };

                if option.accepts_value() {
                    let value = if index == all.len() - 1 {
                        None
                    } else {
                        Some(all[index + 1..].iter().collect())
                    };
                    return self.add_long_option(&option.name, value, rest);
                }

                self.add_long_option(&option.name, None, rest)?;
            }

            return Ok(());
        }

        self.add_short_option(&first, None, rest)
    }

    fn add_short_option(
        &mut self,
        shortcut: &str,
        value: Option<String>,
        rest: &mut std::collections::VecDeque<String>,
    ) -> Result<(), InvalidInputException> {
        let name = self
            .definition
            .option_for_shortcut(shortcut)
            .map(|option| option.name.clone())
            .ok_or_else(|| {
                InvalidInputException::new(format!("The \"-{shortcut}\" option does not exist."))
            })?;

        self.add_long_option(&name, value, rest)
    }

    fn add_long_option(
        &mut self,
        name: &str,
        mut value: Option<String>,
        rest: &mut std::collections::VecDeque<String>,
    ) -> Result<(), InvalidInputException> {
        let Some(option) = self.definition.option(name).cloned() else {
            if let Some(option) = self.definition.negation(name) {
                if value.is_some() {
                    return Err(InvalidInputException::new(format!(
                        "The \"--{name}\" option does not accept a value."
                    )));
                }
                let name = option.name.clone();
                self.options.insert(name, InputValue::Bool(false));
                return Ok(());
            }
            return Err(InvalidInputException::new(format!(
                "The \"--{name}\" option does not exist."
            )));
        };

        if value.is_some() && !option.accepts_value() {
            return Err(InvalidInputException::new(format!(
                "The \"--{name}\" option does not accept a value."
            )));
        }

        if value.as_deref().is_none_or(str::is_empty)
            && option.accepts_value()
            && let Some(next) = rest.front()
            && (next.is_empty() || !next.starts_with('-'))
        {
            value = rest.pop_front();
        }

        let value = match value {
            Some(value) => InputValue::String(value),
            None if option.mode == OptionMode::Required => {
                return Err(InvalidInputException::new(format!(
                    "The \"--{name}\" option requires a value."
                )));
            }
            None if !option.array && !option.is_value_optional() => InputValue::Bool(true),
            None => InputValue::Null,
        };

        if option.array {
            let entry = self
                .options
                .entry(option.name.clone())
                .or_insert_with(|| InputValue::Array(Vec::new()));
            if let InputValue::Array(values) = entry
                && let InputValue::String(value) = value
            {
                values.push(value);
            }
        } else {
            self.options.insert(option.name.clone(), value);
        }

        Ok(())
    }

    /// Ensure every required argument has been given.
    pub fn validate(&self) -> Result<(), InvalidInputException> {
        let missing: Vec<&str> = self
            .definition
            .arguments()
            .iter()
            .filter(|argument| argument.required && !self.arguments.contains_key(&argument.name))
            .map(|argument| argument.name.as_str())
            .collect();

        if missing.is_empty() {
            Ok(())
        } else {
            Err(InvalidInputException::new(format!(
                "Not enough arguments (missing: \"{}\").",
                missing.join(", ")
            )))
        }
    }

    /// The definition this input was bound against.
    pub fn definition(&self) -> &InputDefinition {
        &self.definition
    }

    /// The value of an argument (given or default). `None` if it isn't defined.
    pub fn argument(&self, name: &str) -> Option<InputValue> {
        let argument = self.definition.argument(name)?;
        Some(
            self.arguments
                .get(name)
                .cloned()
                .unwrap_or_else(|| argument.default.clone()),
        )
    }

    /// The value of an option (given or default). `None` if it isn't defined.
    pub fn option(&self, name: &str) -> Option<InputValue> {
        let option = self.definition.option(name)?;
        Some(
            self.options
                .get(name)
                .cloned()
                .unwrap_or_else(|| option.default.clone()),
        )
    }

    /// Determine if the argument was explicitly given.
    pub fn has_given_argument(&self, name: &str) -> bool {
        self.arguments.contains_key(name)
    }

    /// Determine if the option was explicitly given.
    pub fn has_given_option(&self, name: &str) -> bool {
        self.options.contains_key(name)
    }

    /// Every argument, with defaults applied.
    pub fn arguments(&self) -> IndexMap<String, InputValue> {
        self.definition
            .arguments()
            .iter()
            .map(|argument| {
                (
                    argument.name.clone(),
                    self.argument(&argument.name).unwrap_or_default(),
                )
            })
            .collect()
    }

    /// Every option, with defaults applied.
    pub fn options(&self) -> IndexMap<String, InputValue> {
        self.definition
            .options()
            .iter()
            .map(|option| {
                (
                    option.name.clone(),
                    self.option(&option.name).unwrap_or_default(),
                )
            })
            .collect()
    }

    /// Set an argument's value.
    pub fn set_argument(
        &mut self,
        name: &str,
        value: impl Into<InputValue>,
    ) -> Result<(), InvalidInputException> {
        if self.definition.argument(name).is_none() {
            return Err(InvalidInputException::new(format!(
                "The \"{name}\" argument does not exist."
            )));
        }
        self.arguments.insert(name.to_string(), value.into());
        Ok(())
    }

    /// Set an option's value.
    pub fn set_option(
        &mut self,
        name: &str,
        value: impl Into<InputValue>,
    ) -> Result<(), InvalidInputException> {
        if self.definition.option(name).is_none() {
            return Err(InvalidInputException::new(format!(
                "The \"--{name}\" option does not exist."
            )));
        }
        self.options.insert(name.to_string(), value.into());
        Ok(())
    }
}

fn arg_to_input(value: &ArgValue) -> InputValue {
    match value {
        ArgValue::Null => InputValue::Null,
        ArgValue::Bool(value) => InputValue::Bool(*value),
        ArgValue::String(value) => InputValue::String(value.clone()),
        ArgValue::Array(values) => InputValue::Array(values.clone()),
    }
}

// ----------------------------------------------------------------------
// Tokenizing command strings
// ----------------------------------------------------------------------

/// Split a command string into tokens the way a shell would, honoring
/// single quotes, double quotes and backslash escapes.
///
/// ```
/// use illuminate_console::input::tokenize;
///
/// assert_eq!(
///     tokenize(r#"mail:send 1 --subject="Hello world" 'it''s'"#),
///     vec!["mail:send", "1", "--subject=Hello world", "its"],
/// );
/// ```
pub fn tokenize(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if c == '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            Some(_) => current.push(c),
            None => {
                if c.is_whitespace() {
                    if in_token {
                        tokens.push(std::mem::take(&mut current));
                        in_token = false;
                    }
                    continue;
                }

                in_token = true;

                match c {
                    '"' | '\'' => quote = Some(c),
                    '\\' => {
                        if let Some(next) = chars.next() {
                            current.push(next);
                        }
                    }
                    _ => current.push(c),
                }
            }
        }
    }

    if in_token {
        tokens.push(current);
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition() -> InputDefinition {
        InputDefinition::new(
            vec![
                InputArgument::required("user").describe("The user"),
                InputArgument::optional("names").array(),
            ],
            vec![
                InputOption::flag("force"),
                InputOption::new("queue", Some("Q"), OptionMode::Optional, ""),
                InputOption::new("connection", None, OptionMode::Required, ""),
                InputOption::value("id").array(),
                InputOption::new("ansi", None, OptionMode::Negatable, ""),
                InputOption::new("verbose", Some("v|vv|vvv"), OptionMode::None, ""),
                InputOption::new("no-interaction", Some("n"), OptionMode::None, ""),
            ],
        )
        .unwrap()
    }

    #[test]
    fn it_binds_arguments() {
        let input = Input::from_tokens(["taylor", "a", "b"], &definition()).unwrap();
        assert_eq!(input.argument("user"), Some(InputValue::from("taylor")));
        assert_eq!(
            input.argument("names"),
            Some(InputValue::from(vec!["a", "b"]))
        );
        assert_eq!(input.argument("missing"), None);
    }

    #[test]
    fn it_binds_long_options() {
        let input = Input::from_tokens(
            [
                "u",
                "--force",
                "--queue=high",
                "--connection",
                "redis",
                "--id=1",
                "--id",
                "2",
            ],
            &definition(),
        )
        .unwrap();

        assert_eq!(input.option("force"), Some(InputValue::Bool(true)));
        assert_eq!(input.option("queue"), Some(InputValue::from("high")));
        assert_eq!(input.option("connection"), Some(InputValue::from("redis")));
        assert_eq!(input.option("id"), Some(InputValue::from(vec!["1", "2"])));
    }

    #[test]
    fn it_applies_defaults() {
        let input = Input::from_tokens(["u"], &definition()).unwrap();
        assert_eq!(input.option("force"), Some(InputValue::Bool(false)));
        assert_eq!(input.option("queue"), Some(InputValue::Null));
        assert_eq!(input.option("id"), Some(InputValue::Array(vec![])));
        assert_eq!(input.argument("names"), Some(InputValue::Array(vec![])));
        assert_eq!(input.option("ansi"), Some(InputValue::Null));
    }

    #[test]
    fn it_binds_short_options() {
        let input = Input::from_tokens(["u", "-Qhigh", "-vv", "-n"], &definition()).unwrap();
        assert_eq!(input.option("queue"), Some(InputValue::from("high")));
        assert_eq!(input.option("verbose"), Some(InputValue::Bool(true)));
        assert_eq!(input.option("no-interaction"), Some(InputValue::Bool(true)));

        let input = Input::from_tokens(["u", "-Q", "low"], &definition()).unwrap();
        assert_eq!(input.option("queue"), Some(InputValue::from("low")));

        let input = Input::from_tokens(["u", "-nv"], &definition()).unwrap();
        assert_eq!(input.option("no-interaction"), Some(InputValue::Bool(true)));
        assert_eq!(input.option("verbose"), Some(InputValue::Bool(true)));
    }

    #[test]
    fn it_binds_negations() {
        let input = Input::from_tokens(["u", "--no-ansi"], &definition()).unwrap();
        assert_eq!(input.option("ansi"), Some(InputValue::Bool(false)));
        let input = Input::from_tokens(["u", "--ansi"], &definition()).unwrap();
        assert_eq!(input.option("ansi"), Some(InputValue::Bool(true)));
    }

    #[test]
    fn double_dash_ends_options() {
        let input = Input::from_tokens(["--", "--force", "-x"], &definition()).unwrap();
        assert_eq!(input.argument("user"), Some(InputValue::from("--force")));
        assert_eq!(input.argument("names"), Some(InputValue::from(vec!["-x"])));
        assert_eq!(input.option("force"), Some(InputValue::Bool(false)));
    }

    #[test]
    fn it_reports_invalid_input() {
        let error = |tokens: &[&str]| {
            Input::from_tokens(tokens.iter().copied(), &definition())
                .unwrap_err()
                .message
        };

        assert_eq!(error(&["--nope"]), "The \"--nope\" option does not exist.");
        assert_eq!(error(&["-x"]), "The \"-x\" option does not exist.");
        assert_eq!(
            error(&["--force=1"]),
            "The \"--force\" option does not accept a value."
        );
        assert_eq!(
            error(&["--connection"]),
            "The \"--connection\" option requires a value."
        );
        assert_eq!(
            error(&["--no-ansi=1"]),
            "The \"--no-ansi\" option does not accept a value."
        );

        let input = Input::from_tokens(Vec::<String>::new(), &definition()).unwrap();
        assert_eq!(
            input.validate().unwrap_err().message,
            "Not enough arguments (missing: \"user\")."
        );

        let single = InputDefinition::new(vec![InputArgument::required("user")], vec![]).unwrap();
        let error = Input::from_tokens(["a", "b"], &single).unwrap_err();
        assert_eq!(
            error.message,
            "Too many arguments, expected arguments \"user\"."
        );

        let none = InputDefinition::default();
        let error = Input::from_tokens(["a"], &none).unwrap_err();
        assert_eq!(error.message, "No arguments expected, got \"a\".");

        let with_command =
            InputDefinition::new(vec![InputArgument::required("command")], vec![]).unwrap();
        let error = Input::from_tokens(["inspire", "extra"], &with_command).unwrap_err();
        assert_eq!(
            error.message,
            "No arguments expected for \"inspire\" command, got \"extra\"."
        );
    }

    #[test]
    fn it_binds_named_parameters() {
        let parameters = vec![
            ("user".to_string(), ArgValue::from(1)),
            ("--force".to_string(), ArgValue::from(true)),
            ("-Q".to_string(), ArgValue::from("high")),
            ("--id".to_string(), ArgValue::from(vec!["5", "13"])),
            ("--no-ansi".to_string(), ArgValue::Null),
        ];
        let input = Input::from_named(&parameters, &definition()).unwrap();
        assert_eq!(input.argument("user"), Some(InputValue::from("1")));
        assert_eq!(input.option("force"), Some(InputValue::Bool(true)));
        assert_eq!(input.option("queue"), Some(InputValue::from("high")));
        assert_eq!(input.option("id"), Some(InputValue::from(vec!["5", "13"])));
        assert_eq!(input.option("ansi"), Some(InputValue::Bool(false)));

        let error =
            Input::from_named(&[("nope".to_string(), ArgValue::Null)], &definition()).unwrap_err();
        assert_eq!(error.message, "The \"nope\" argument does not exist.");
    }

    #[test]
    fn it_validates_definitions() {
        let error = InputDefinition::new(
            vec![InputArgument::optional("a"), InputArgument::required("b")],
            vec![],
        )
        .unwrap_err();
        assert_eq!(
            error.message,
            "Cannot add a required argument \"b\" after an optional one \"a\"."
        );

        let error = InputDefinition::new(
            vec![
                InputArgument::optional("a").array(),
                InputArgument::optional("b"),
            ],
            vec![],
        )
        .unwrap_err();
        assert_eq!(
            error.message,
            "Cannot add a required argument \"b\" after an array argument \"a\"."
        );

        let error = InputDefinition::new(
            vec![],
            vec![
                InputOption::flag("a").shortcut("x"),
                InputOption::flag("b").shortcut("x"),
            ],
        )
        .unwrap_err();
        assert_eq!(
            error.message,
            "An option with shortcut \"x\" already exists."
        );
    }

    #[test]
    fn it_builds_synopses() {
        let definition = definition();
        assert_eq!(
            definition.synopsis(true),
            "[options] [--] <user> [<names>...]"
        );
        assert!(
            definition
                .synopsis(false)
                .starts_with("[--force] [-Q|--queue [QUEUE]] [--connection CONNECTION]")
        );
        assert!(definition.synopsis(false).contains("[--ansi|--no-ansi]"));
    }

    #[test]
    fn it_tokenizes_strings() {
        assert_eq!(tokenize("a  b\tc"), vec!["a", "b", "c"]);
        assert_eq!(
            tokenize("--name=\"Taylor Otwell\""),
            vec!["--name=Taylor Otwell"]
        );
        assert_eq!(tokenize("'single quoted' x"), vec!["single quoted", "x"]);
        assert_eq!(tokenize("a\\ b"), vec!["a b"]);
        assert_eq!(tokenize("\"\""), vec![""]);
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn artisan_args_convert_from_many_shapes() {
        let args: ArtisanArgs = illuminate_support::json!({"user": 1, "--force": true}).into();
        assert_eq!(args.named().len(), 2);
        assert_eq!(args.to_command_line(), "1 --force");

        let args: ArtisanArgs = ["Taylor", "--force"].into();
        assert_eq!(args.raw_tokens(), ["Taylor", "--force"]);

        let args: ArtisanArgs = [("--id", vec!["1", "2"])].into();
        assert_eq!(args.to_command_line(), "--id=1 --id=2");

        let args = ArtisanArgs::new()
            .with("name", "Taylor Otwell")
            .with("--queue", "high");
        assert_eq!(args.to_command_line(), "'Taylor Otwell' --queue=high");
        assert!(ArtisanArgs::from(()).is_empty());
    }
}
