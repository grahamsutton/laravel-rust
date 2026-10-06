//! The commands every console application has: `list` and `help`.

use async_trait::async_trait;
use illuminate_support::Result;

use crate::command::Command;
use crate::console::Console;
use crate::descriptor;
use crate::input::InvalidInputException;

/// Lists the application's commands.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListCommand;

#[async_trait]
impl Command for ListCommand {
    fn signature(&self) -> &str {
        "list
            {namespace? : The namespace name}
            {--raw : To output raw command list}
            {--format=txt : The output format (txt or json)}
            {--short : To skip describing commands' arguments}"
    }

    fn description(&self) -> &str {
        "List commands"
    }

    fn help(&self) -> &str {
        "The <info>%command.name%</info> command lists all commands:

  <info>%command.full_name%</info>

You can also display the commands for a specific namespace:

  <info>%command.full_name% test</info>

You can also output the information in other formats by using the <comment>--format</comment> option:

  <info>%command.full_name% --format=json</info>

It's also possible to get raw list of commands (useful for embedding command runner):

  <info>%command.full_name% --raw</info>"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let application = cmd.application();

        let namespace = match cmd.argument("namespace") {
            Some(namespace) => Some(application.find_namespace(&namespace)?),
            None => None,
        };

        match cmd.option("format").as_deref().unwrap_or("txt") {
            "txt" => cmd.output().write(descriptor::describe_application(
                &application,
                namespace.as_deref(),
                cmd.option_bool("raw"),
            )),
            "json" => cmd.output().writeln(descriptor::describe_application_json(
                &application,
                namespace.as_deref(),
            )),
            other => {
                return Err(
                    InvalidInputException::new(format!("Unsupported format \"{other}\".")).into(),
                );
            }
        }

        Ok(())
    }
}

/// Displays help for a command.
#[derive(Clone, Copy, Debug, Default)]
pub struct HelpCommand;

#[async_trait]
impl Command for HelpCommand {
    fn signature(&self) -> &str {
        "help
            {command_name=help : The command name}
            {--format=txt : The output format (txt or json)}
            {--raw : To output raw command help}"
    }

    fn description(&self) -> &str {
        "Display help for a command"
    }

    fn help(&self) -> &str {
        "The <info>%command.name%</info> command displays help for a given command:

  <info>%command.full_name% list</info>

You can also output the help in other formats by using the <comment>--format</comment> option:

  <info>%command.full_name% --format=json list</info>

To display the list of available commands, please use the <info>list</info> command."
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let application = cmd.application();
        let name = cmd
            .argument("command_name")
            .unwrap_or_else(|| "help".to_string());
        let registered = application.find_registered(&name)?;

        match cmd.option("format").as_deref().unwrap_or("txt") {
            "txt" => cmd
                .output()
                .write(descriptor::describe_command(&application, &registered)),
            "json" => cmd
                .output()
                .writeln(descriptor::describe_command_json(&application, &registered)),
            other => {
                return Err(
                    InvalidInputException::new(format!("Unsupported format \"{other}\".")).into(),
                );
            }
        }

        Ok(())
    }
}
