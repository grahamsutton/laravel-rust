//! Laravel's command signature parser.
//!
//! Signatures define a command's name, arguments and options in a single,
//! expressive, route-like syntax:
//!
//! ```
//! use illuminate_console::Parser;
//!
//! let signature = Parser::parse(
//!     "mail:send
//!         {user : The ID of the user}
//!         {--Q|queue=default : The queue to use}
//!         {--id=* : The message IDs}",
//! )
//! .unwrap();
//!
//! assert_eq!(signature.name, "mail:send");
//! assert_eq!(signature.arguments[0].name, "user");
//! assert_eq!(signature.arguments[0].description, "The ID of the user");
//! assert_eq!(signature.options[0].shortcut.as_deref(), Some("Q"));
//! assert!(signature.options[1].array);
//! ```

use std::sync::LazyLock;

use regex::Regex;

use crate::input::{InputArgument, InputDefinition, InputOption, InputValue, InvalidDefinitionException, OptionMode};

/// A parsed command signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    /// The command's name, e.g. `mail:send`.
    pub name: String,
    /// The command's arguments, in order.
    pub arguments: Vec<InputArgument>,
    /// The command's options, in order.
    pub options: Vec<InputOption>,
}

impl Signature {
    /// Build the input definition described by this signature.
    pub fn definition(&self) -> InputDefinition {
        InputDefinition::new(self.arguments.clone(), self.options.clone())
            .expect("the signature was validated when it was parsed")
    }
}

/// Parses Laravel's command signature syntax.
pub struct Parser;

static TOKENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\s*(.*?)\s*\}").unwrap());
static OPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-{2,}(.*)").unwrap());
static ARRAY_DEFAULT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+)=\*(.+)$").unwrap());
static VALUE_DEFAULT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+?)=(.+)$").unwrap());
static DESCRIPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+:\s+").unwrap());
static SHORTCUT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\|\s*").unwrap());
static LIST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r",\s?").unwrap());

impl Parser {
    /// Parse the given console command definition.
    pub fn parse(expression: &str) -> Result<Signature, InvalidDefinitionException> {
        let name = Self::name(expression)?;

        let mut arguments = Vec::new();
        let mut options = Vec::new();

        for captures in TOKENS.captures_iter(expression) {
            let token = captures.get(1).map(|m| m.as_str()).unwrap_or_default();

            match OPTION.captures(token) {
                Some(option) => options.push(Self::parse_option(&option[1])?),
                None => arguments.push(Self::parse_argument(token)?),
            }
        }

        // Validate the definition (argument ordering, duplicate options...).
        InputDefinition::new(arguments.clone(), options.clone())?;

        Ok(Signature {
            name,
            arguments,
            options,
        })
    }

    fn name(expression: &str) -> Result<String, InvalidDefinitionException> {
        let name = expression
            .split_whitespace()
            .next()
            .ok_or_else(|| InvalidDefinitionException::new("Unable to determine command name from signature."))?;

        let valid = !name.starts_with('{') && name.split(':').all(|part| !part.is_empty());
        if !valid {
            return Err(InvalidDefinitionException::new(format!("Command name \"{name}\" is invalid.")));
        }

        Ok(name.to_string())
    }

    /// Parse an argument expression, such as `user?*` or `user=foo`.
    pub fn parse_argument(token: &str) -> Result<InputArgument, InvalidDefinitionException> {
        let (token, description) = Self::extract_description(token);
        let trim = |value: &str, chars: &[char]| value.trim_matches(|c| chars.contains(&c)).to_string();

        let argument = if token.ends_with("?*") {
            InputArgument::optional(trim(&token, &['?', '*'])).array()
        } else if token.ends_with('*') {
            let mut argument = InputArgument::required(trim(&token, &['?', '*'])).array();
            argument.required = true;
            argument
        } else if token.ends_with('?') {
            InputArgument::optional(trim(&token, &['?']))
        } else if let Some(captures) = ARRAY_DEFAULT.captures(&token) {
            InputArgument::optional(&captures[1])
                .array()
                .default_value(Self::split_list(&captures[2]))
        } else if let Some(captures) = VALUE_DEFAULT.captures(&token) {
            InputArgument::optional(&captures[1]).default_value(InputValue::String(captures[2].to_string()))
        } else {
            InputArgument::required(token.clone())
        };

        if argument.name.is_empty() {
            return Err(InvalidDefinitionException::new("An argument name cannot be empty."));
        }

        Ok(argument.describe(description))
    }

    /// Parse an option expression (without the leading dashes), such as
    /// `Q|queue=default`.
    pub fn parse_option(token: &str) -> Result<InputOption, InvalidDefinitionException> {
        let (token, description) = Self::extract_description(token);

        let mut parts = SHORTCUT.splitn(&token, 2);
        let first = parts.next().unwrap_or_default().to_string();
        let (shortcut, token) = match parts.next() {
            Some(rest) => (Some(first), rest.to_string()),
            None => (None, first),
        };

        let trim = |value: &str, chars: &[char]| value.trim_matches(|c| chars.contains(&c)).to_string();
        let shortcut = shortcut.as_deref();

        let option = if token.ends_with('=') {
            InputOption::new(trim(&token, &['=']), shortcut, OptionMode::Optional, description)
        } else if token.ends_with("=*") {
            InputOption::new(trim(&token, &['=', '*']), shortcut, OptionMode::Optional, description).array()
        } else if let Some(captures) = ARRAY_DEFAULT.captures(&token) {
            InputOption::new(&captures[1], shortcut, OptionMode::Optional, description)
                .array()
                .default_value(Self::split_list(&captures[2]))
        } else if let Some(captures) = VALUE_DEFAULT.captures(&token) {
            InputOption::new(&captures[1], shortcut, OptionMode::Optional, description)
                .default_value(InputValue::String(captures[2].to_string()))
        } else {
            InputOption::new(token, shortcut, OptionMode::None, description)
        };

        if option.name.is_empty() {
            return Err(InvalidDefinitionException::new("An option name cannot be empty."));
        }

        Ok(option)
    }

    fn extract_description(token: &str) -> (String, String) {
        let token = token.trim();
        let mut parts = DESCRIPTION.splitn(token, 2);
        let name = parts.next().unwrap_or_default().to_string();

        match parts.next() {
            Some(description) => (name, description.to_string()),
            None => (token.to_string(), String::new()),
        }
    }

    fn split_list(value: &str) -> InputValue {
        InputValue::Array(LIST.split(value).map(String::from).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argument(signature: &str) -> InputArgument {
        Parser::parse(&format!("cmd {{{signature}}}")).unwrap().arguments.remove(0)
    }

    fn option(signature: &str) -> InputOption {
        Parser::parse(&format!("cmd {{{signature}}}")).unwrap().options.remove(0)
    }

    #[test]
    fn it_parses_the_name() {
        assert_eq!(Parser::parse("inspire").unwrap().name, "inspire");
        assert_eq!(Parser::parse("  mail:send {user}").unwrap().name, "mail:send");
        assert!(Parser::parse("   ").is_err());
        assert_eq!(
            Parser::parse("").unwrap_err().message,
            "Unable to determine command name from signature."
        );
        assert!(Parser::parse("mail::send").is_err());
    }

    #[test]
    fn it_parses_required_arguments() {
        let user = argument("user");
        assert_eq!(user.name, "user");
        assert!(user.required);
        assert!(!user.array);
        assert_eq!(user.default, InputValue::Null);
    }

    #[test]
    fn it_parses_optional_arguments() {
        let user = argument("user?");
        assert_eq!(user.name, "user");
        assert!(!user.required);
    }

    #[test]
    fn it_parses_arguments_with_defaults() {
        let user = argument("user=taylor");
        assert_eq!(user.name, "user");
        assert!(!user.required);
        assert_eq!(user.default, InputValue::from("taylor"));
    }

    #[test]
    fn it_parses_array_arguments() {
        let users = argument("user*");
        assert!(users.required);
        assert!(users.array);

        let users = argument("user?*");
        assert!(!users.required);
        assert!(users.array);

        let users = argument("user=*taylor,abigail");
        assert!(!users.required);
        assert!(users.array);
        assert_eq!(users.default, InputValue::from(vec!["taylor", "abigail"]));
    }

    #[test]
    fn it_parses_descriptions() {
        let user = argument("user : The ID of the user");
        assert_eq!(user.name, "user");
        assert_eq!(user.description, "The ID of the user");

        let queue = option("--queue= : Whether the job should be queued");
        assert_eq!(queue.name, "queue");
        assert_eq!(queue.description, "Whether the job should be queued");

        // A colon without surrounding spaces is part of the name...
        assert_eq!(argument("user:id").name, "user:id");
    }

    #[test]
    fn it_parses_switches() {
        let queue = option("--queue");
        assert_eq!(queue.name, "queue");
        assert_eq!(queue.mode, OptionMode::None);
        assert_eq!(queue.default, InputValue::Bool(false));
    }

    #[test]
    fn it_parses_value_options() {
        let queue = option("--queue=");
        assert_eq!(queue.mode, OptionMode::Optional);
        assert_eq!(queue.default, InputValue::Null);

        let queue = option("--queue=default");
        assert_eq!(queue.mode, OptionMode::Optional);
        assert_eq!(queue.default, InputValue::from("default"));
    }

    #[test]
    fn it_parses_shortcuts() {
        let queue = option("--Q|queue");
        assert_eq!(queue.name, "queue");
        assert_eq!(queue.shortcut.as_deref(), Some("Q"));

        let queue = option("--Q | queue=high");
        assert_eq!(queue.shortcut.as_deref(), Some("Q"));
        assert_eq!(queue.default, InputValue::from("high"));
    }

    #[test]
    fn it_parses_array_options() {
        let ids = option("--id=*");
        assert_eq!(ids.name, "id");
        assert!(ids.array);
        assert_eq!(ids.mode, OptionMode::Optional);
        assert_eq!(ids.default, InputValue::Array(vec![]));

        let ids = option("--id=*1,2, 3");
        assert_eq!(ids.default, InputValue::from(vec!["1", "2", "3"]));
    }

    #[test]
    fn it_parses_multi_line_signatures() {
        let signature = Parser::parse(
            "mail:send
                {user : The ID of the user}
                {--queue : Whether the job should be queued}",
        )
        .unwrap();

        assert_eq!(signature.name, "mail:send");
        assert_eq!(signature.arguments.len(), 1);
        assert_eq!(signature.options.len(), 1);
        assert_eq!(signature.options[0].description, "Whether the job should be queued");
    }

    #[test]
    fn it_rejects_invalid_definitions() {
        assert!(Parser::parse("cmd {user?} {id}").is_err());
        assert!(Parser::parse("cmd {--a} {--a=}").is_err());
    }
}
