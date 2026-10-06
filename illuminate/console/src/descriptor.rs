//! Text descriptions of the application and its commands, exactly like
//! Symfony's text descriptor (used by `list` and `help`).

use illuminate_support::{Value, json};

use crate::application::{Application, Registered};
use crate::formatter::OutputFormatter;
use crate::input::{InputArgument, InputDefinition, InputOption, OptionMode};

const GLOBAL_NAMESPACE: &str = "_global";

fn width(value: &str) -> usize {
    OutputFormatter::width(value)
}

fn indent_description(description: &str, total_width: usize) -> String {
    let indentation = format!("\n{}", " ".repeat(total_width + 4));
    let lines: Vec<&str> = description.lines().map(str::trim).collect();
    lines.join(&indentation)
}

fn total_width_for_options(options: &[InputOption]) -> usize {
    options
        .iter()
        .map(|option| {
            let shortcut_width = option.shortcut.as_deref().map(width).unwrap_or(0).max(1);
            let mut name_length = 1 + shortcut_width + 4 + width(&option.name);

            if option.mode == OptionMode::Negatable {
                name_length += 6 + width(&option.name);
            } else if option.accepts_value() {
                name_length += 1 + width(&option.name) + if option.is_value_optional() { 2 } else { 0 };
            }

            name_length
        })
        .max()
        .unwrap_or(0)
}

fn describe_argument(argument: &InputArgument, total_width: usize) -> String {
    let default = InputDefinition::default_display(&argument.default)
        .map(|default| format!("<comment> [default: {}]</comment>", OutputFormatter::escape(&default)))
        .unwrap_or_default();

    let spacing = total_width.saturating_sub(width(&argument.name));

    format!(
        "  <info>{}</info>  {}{}{}",
        argument.name,
        " ".repeat(spacing),
        indent_description(&argument.description, total_width),
        default
    )
}

fn describe_option(option: &InputOption, total_width: usize) -> String {
    let default = if option.accepts_value() {
        InputDefinition::default_display(&option.default)
            .map(|default| format!("<comment> [default: {}]</comment>", OutputFormatter::escape(&default)))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let mut value = String::new();
    if option.accepts_value() {
        value = format!("={}", option.name.to_uppercase());
        if option.is_value_optional() {
            value = format!("[{value}]");
        }
    }

    let shortcut = match &option.shortcut {
        Some(shortcut) => format!("-{shortcut}, "),
        None => "    ".to_string(),
    };

    let name = if option.mode == OptionMode::Negatable {
        format!("--{0}|--no-{0}", option.name)
    } else {
        format!("--{}{}", option.name, value)
    };

    let synopsis = format!("{shortcut}{name}");
    let spacing = total_width.saturating_sub(width(&synopsis));

    format!(
        "  <info>{}</info>  {}{}{}{}",
        synopsis,
        " ".repeat(spacing),
        indent_description(&option.description, total_width),
        default,
        if option.array { "<comment> (multiple values allowed)</comment>" } else { "" }
    )
}

/// Describe an input definition's arguments and options.
pub(crate) fn describe_input_definition(definition: &InputDefinition) -> String {
    let mut total_width = total_width_for_options(definition.options());
    for argument in definition.arguments() {
        total_width = total_width.max(width(&argument.name));
    }

    let mut text = String::new();

    if !definition.arguments().is_empty() {
        text.push_str("<comment>Arguments:</comment>\n");
        for argument in definition.arguments() {
            text.push_str(&describe_argument(argument, total_width));
            text.push('\n');
        }
    }

    if !definition.arguments().is_empty() && !definition.options().is_empty() {
        text.push('\n');
    }

    if !definition.options().is_empty() {
        text.push_str("<comment>Options:</comment>");

        let (later, first): (Vec<&InputOption>, Vec<&InputOption>) = definition
            .options()
            .iter()
            .partition(|option| option.shortcut.as_deref().is_some_and(|s| s.chars().count() > 1));

        for option in first.into_iter().chain(later) {
            text.push('\n');
            text.push_str(&describe_option(option, total_width));
        }
    }

    text
}

/// The commands of the application, grouped by namespace.
pub(crate) struct ApplicationDescription {
    pub commands: Vec<std::sync::Arc<Registered>>,
    pub namespaces: Vec<(String, Vec<String>)>,
}

impl ApplicationDescription {
    pub(crate) fn new(application: &Application, namespace: Option<&str>) -> Self {
        let entries = application.registry_entries(namespace);

        let mut global: Vec<(String, std::sync::Arc<Registered>)> = Vec::new();
        let mut namespaced: std::collections::BTreeMap<String, Vec<(String, std::sync::Arc<Registered>)>> =
            std::collections::BTreeMap::new();

        for (name, registered) in entries {
            if registered.command.hidden() {
                continue;
            }
            let key = Application::extract_namespace(&name, Some(1));
            if key.is_empty() || key == GLOBAL_NAMESPACE {
                global.push((name, registered));
            } else {
                namespaced.entry(key).or_default().push((name, registered));
            }
        }

        let mut commands = Vec::new();
        let mut namespaces = Vec::new();

        let mut groups: Vec<(String, Vec<(String, std::sync::Arc<Registered>)>)> = Vec::new();
        if !global.is_empty() {
            groups.push((GLOBAL_NAMESPACE.to_string(), global));
        }
        groups.extend(namespaced);

        for (id, mut entries) in groups {
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut names = Vec::new();
            for (name, registered) in entries {
                if registered.name == name {
                    commands.push(registered);
                }
                names.push(name);
            }
            namespaces.push((id, names));
        }

        Self { commands, namespaces }
    }

    fn command(&self, name: &str) -> Option<&std::sync::Arc<Registered>> {
        self.commands.iter().find(|registered| registered.name == name)
    }
}

fn column_width(description: &ApplicationDescription, include_aliases: bool) -> usize {
    let mut widths = Vec::new();
    for registered in &description.commands {
        widths.push(width(&registered.name));
        if include_aliases {
            for alias in registered.command.aliases() {
                widths.push(width(alias));
            }
        }
    }
    widths.into_iter().max().map(|w| w + 2).unwrap_or(0)
}

/// Describe the whole application (the `list` command's output).
pub(crate) fn describe_application(application: &Application, namespace: Option<&str>, raw: bool) -> String {
    let description = ApplicationDescription::new(application, namespace);
    let mut text = String::new();

    if raw {
        let width = column_width(&description, true);
        for registered in &description.commands {
            text.push_str(&format!(
                "{:<width$} {}\n",
                registered.name,
                registered.command.description(),
            ));
        }
        return text;
    }

    let help = application.long_version();
    if !help.is_empty() {
        text.push_str(&format!("{help}\n\n"));
    }

    text.push_str("<comment>Usage:</comment>\n");
    text.push_str("  command [options] [arguments]\n\n");

    let options = InputDefinition::new(Vec::new(), Application::default_definition().options().to_vec())
        .unwrap_or_default();
    text.push_str(&describe_input_definition(&options));
    text.push_str("\n\n");

    let width = column_width(&description, false);

    match namespace {
        Some(namespace) => text.push_str(&format!(
            "<comment>Available commands for the \"{namespace}\" namespace:</comment>"
        )),
        None => text.push_str("<comment>Available commands:</comment>"),
    }

    for (id, names) in &description.namespaces {
        let names: Vec<&String> = names.iter().filter(|name| description.command(name).is_some()).collect();
        if names.is_empty() {
            continue;
        }

        if namespace.is_none() && id != GLOBAL_NAMESPACE {
            text.push('\n');
            text.push_str(&format!(" <comment>{id}</comment>"));
        }

        for name in names {
            let registered = description.command(name).expect("filtered above");
            let aliases = registered.command.aliases();
            let aliases = if aliases.is_empty() { String::new() } else { format!("[{}] ", aliases.join("|")) };
            let spacing = width.saturating_sub(self::width(name));

            text.push('\n');
            text.push_str(&format!(
                "  <info>{name}</info>{}{aliases}{}",
                " ".repeat(spacing),
                registered.command.description()
            ));
        }
    }

    text.push('\n');
    text
}

/// The processed help text of a command.
pub(crate) fn processed_help(application: &Application, registered: &Registered) -> String {
    let help = registered.command.help();
    let help = if help.is_empty() { registered.command.description() } else { help };

    help.replace("%command.name%", &registered.name)
        .replace("%command.full_name%", &format!("{} {}", application.binary(), registered.name))
}

/// The short synopsis of a command, e.g. `mail:send [options] [--] <user>`.
pub(crate) fn synopsis(registered: &Registered) -> String {
    format!("{} {}", registered.name, registered.definition.synopsis(true))
        .trim()
        .to_string()
}

/// Describe a single command (the `help` command's output).
pub(crate) fn describe_command(application: &Application, registered: &Registered) -> String {
    let mut text = String::new();
    let description = registered.command.description();

    if !description.is_empty() {
        text.push_str("<comment>Description:</comment>\n");
        text.push_str(&format!("  {description}\n\n"));
    }

    text.push_str("<comment>Usage:</comment>");
    let mut usages = vec![synopsis(registered)];
    usages.extend(registered.command.aliases().into_iter().map(String::from));
    usages.extend(
        registered
            .command
            .usages()
            .into_iter()
            .map(|usage| format!("{} {}", registered.name, usage)),
    );
    for usage in usages {
        text.push('\n');
        text.push_str(&format!("  {}", OutputFormatter::escape(&usage)));
    }
    text.push('\n');

    let definition = registered
        .definition
        .merged_with(&Application::default_definition(), false);

    if !definition.options().is_empty() || !definition.arguments().is_empty() {
        text.push('\n');
        text.push_str(&describe_input_definition(&definition));
        text.push('\n');
    }

    let help = processed_help(application, registered);
    if !help.is_empty() && help != description {
        text.push('\n');
        text.push_str("<comment>Help:</comment>\n");
        text.push_str(&format!("  {}\n", help.replace('\n', "\n  ")));
    }

    text
}

fn command_json(application: &Application, registered: &Registered) -> Value {
    json!({
        "name": registered.name,
        "hidden": registered.command.hidden(),
        "usage": std::iter::once(synopsis(registered))
            .chain(registered.command.aliases().into_iter().map(String::from))
            .collect::<Vec<_>>(),
        "description": registered.command.description(),
        "help": OutputFormatter::strip(&processed_help(application, registered)),
        "definition": {
            "arguments": registered.definition.arguments().iter().map(|argument| json!({
                "name": argument.name,
                "is_required": argument.required,
                "is_array": argument.array,
                "description": argument.description,
                "default": argument.default.to_value(),
            })).collect::<Vec<_>>(),
            "options": registered.definition.options().iter().map(|option| json!({
                "name": format!("--{}", option.name),
                "shortcut": option.shortcut.as_ref().map(|s| format!("-{}", s.replace('|', "|-"))).unwrap_or_default(),
                "accept_value": option.accepts_value(),
                "is_value_required": option.mode == OptionMode::Required,
                "is_multiple": option.array,
                "description": option.description,
                "default": option.default.to_value(),
            })).collect::<Vec<_>>(),
        },
    })
}

/// Describe the application as JSON.
pub(crate) fn describe_application_json(application: &Application, namespace: Option<&str>) -> String {
    let description = ApplicationDescription::new(application, namespace);

    let mut value = json!({
        "application": {
            "name": application.name(),
            "version": application.version(),
        },
        "commands": description
            .commands
            .iter()
            .map(|registered| command_json(application, registered))
            .collect::<Vec<_>>(),
    });

    match namespace {
        Some(namespace) => value["namespace"] = json!(namespace),
        None => {
            value["namespaces"] = description
                .namespaces
                .iter()
                .map(|(id, commands)| json!({"id": id, "commands": commands}))
                .collect();
        }
    }

    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// Describe a command as JSON.
pub(crate) fn describe_command_json(application: &Application, registered: &Registered) -> String {
    serde_json::to_string_pretty(&command_json(application, registered)).unwrap_or_default()
}
