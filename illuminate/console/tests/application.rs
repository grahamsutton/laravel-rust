use std::sync::{Arc, Mutex};

use illuminate_console::prelude::*;
use illuminate_console::{
    CommandNotFoundException, InvalidInputException, Output, Verbosity, async_trait,
};
use illuminate_support::json;

fn artisan() -> Arc<Application> {
    let app = Application::new();
    app.set_version("13.0.0");
    app
}

async fn run(app: &Application, tokens: &[&str]) -> (i32, String) {
    let output = Output::buffered();
    let code = app.run_with_output(tokens.iter().copied(), &output).await;
    (code, output.fetch())
}

fn sample_application() -> Arc<Application> {
    let app = artisan();
    app.command("inspire", |cmd| async move {
        cmd.line("Simplicity is the ultimate sophistication.");
        Ok(())
    })
    .purpose("Display an inspiring quote");
    app.command(
        "migrate {--force : Force the operation to run when in production}",
        |_| async { Ok(()) },
    )
    .purpose("Run the database migrations")
    .alias("mig");
    app.command("make:model {name : The name of the model}", |_| async {
        Ok(())
    })
    .purpose("Create a new Eloquent model class");
    app.command("make:migration {name}", |_| async { Ok(()) })
        .purpose("Create a new migration file");
    app.command("secret:thing", |_| async { Ok(()) }).hide();
    app
}

#[tokio::test]
async fn it_lists_commands_like_laravel() {
    let app = sample_application();
    let (code, output) = run(&app, &["list"]).await;

    assert_eq!(code, 0);
    assert_eq!(
        output,
        "Laravel Framework 13.0.0

Usage:
  command [options] [arguments]

Options:
  -h, --help            Display help for the given command. When no command is given display help for the list command
      --silent          Do not output any message
  -q, --quiet           Only errors are displayed. All other output is suppressed
  -V, --version         Display this application version
      --ansi|--no-ansi  Force (or disable --no-ansi) ANSI output
  -n, --no-interaction  Do not ask any interactive question
      --env[=ENV]       The environment the command should run under
  -v|vv|vvv, --verbose  Increase the verbosity of messages: 1 for normal output, 2 for more verbose output and 3 for debug

Available commands:
  help            Display help for a command
  inspire         Display an inspiring quote
  list            List commands
  migrate         [mig] Run the database migrations
 make
  make:migration  Create a new migration file
  make:model      Create a new Eloquent model class
"
    );
}

#[tokio::test]
async fn the_list_is_the_default_command() {
    let app = sample_application();
    let (code, output) = run(&app, &[]).await;
    assert_eq!(code, 0);
    assert!(output.starts_with("Laravel Framework 13.0.0\n"));
    assert!(output.contains("Available commands:"));
}

#[tokio::test]
async fn it_colors_the_list_when_decorated() {
    let app = sample_application();
    let (_, output) = run(&app, &["list", "--ansi"]).await;
    assert!(output.starts_with("Laravel Framework \x1b[32m13.0.0\x1b[39m\n"));
    assert!(output.contains("\x1b[33mUsage:\x1b[39m"));
    assert!(output.contains("  \x1b[32minspire\x1b[39m"));
    assert!(output.contains(" \x1b[33mmake\x1b[39m"));
}

#[tokio::test]
async fn it_lists_a_namespace() {
    let app = sample_application();
    let (code, output) = run(&app, &["list", "make"]).await;
    assert_eq!(code, 0);
    assert!(output.contains("Available commands for the \"make\" namespace:\n  make:migration  Create a new migration file\n  make:model      Create a new Eloquent model class\n"));
    assert!(!output.contains("inspire"));

    // Namespaces may be abbreviated...
    let (_, output) = run(&app, &["list", "ma"]).await;
    assert!(output.contains("make:model"));
}

#[tokio::test]
async fn it_lists_raw_and_json() {
    let app = sample_application();
    let (_, output) = run(&app, &["list", "--raw"]).await;
    assert!(output.contains("inspire          Display an inspiring quote\n"));
    assert!(!output.contains("Usage:"));

    let (code, output) = run(&app, &["list", "--format=json"]).await;
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["application"]["version"], json!("13.0.0"));
    assert!(
        value["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == json!("make:model"))
    );

    let (code, output) = run(&app, &["list", "--format=xml"]).await;
    assert_eq!(code, 2);
    assert!(output.contains("Unsupported format \"xml\"."));
}

#[tokio::test]
async fn it_describes_commands() {
    let app = artisan();
    app.command(
        "mail:send {user : The ID of the user} {names?* : Extra names} {--Q|queue=default : The queue to use} {--id=* : The IDs}",
        |_| async { Ok(()) },
    )
    .purpose("Send a marketing email to a user")
    .with_help("Sends the <info>%command.name%</info> email.");

    let (code, output) = run(&app, &["help", "mail:send"]).await;
    assert_eq!(code, 0);
    assert_eq!(
        output,
        "Description:
  Send a marketing email to a user

Usage:
  mail:send [options] [--] <user> [<names>...]

Arguments:
  user                  The ID of the user
  names                 Extra names

Options:
  -Q, --queue[=QUEUE]   The queue to use [default: \"default\"]
      --id[=ID]         The IDs (multiple values allowed)
  -h, --help            Display help for the given command. When no command is given display help for the list command
      --silent          Do not output any message
  -q, --quiet           Only errors are displayed. All other output is suppressed
  -V, --version         Display this application version
      --ansi|--no-ansi  Force (or disable --no-ansi) ANSI output
  -n, --no-interaction  Do not ask any interactive question
      --env[=ENV]       The environment the command should run under
  -v|vv|vvv, --verbose  Increase the verbosity of messages: 1 for normal output, 2 for more verbose output and 3 for debug

Help:
  Sends the mail:send email.
"
    );

    // The --help flag works too...
    let (code, flagged) = run(&app, &["mail:send", "--help"]).await;
    assert_eq!(code, 0);
    assert_eq!(flagged, output);

    // ...and "help" alone describes itself.
    let (_, output) = run(&app, &["help"]).await;
    assert!(output.contains("  help [options] [--] [<command_name>]"));
    assert!(output.contains("The help command displays help for a given command:"));
    assert!(output.contains("  artisan help list"));

    // --help without a command describes the list command.
    let (_, output) = run(&app, &["--help"]).await;
    assert!(output.contains("  list [options] [--] [<namespace>]"));
}

#[tokio::test]
async fn it_displays_the_version() {
    let app = artisan();
    let (code, output) = run(&app, &["--version"]).await;
    assert_eq!(code, 0);
    assert_eq!(output, "Laravel Framework 13.0.0\n");

    let (_, output) = run(&app, &["-V"]).await;
    assert_eq!(output, "Laravel Framework 13.0.0\n");

    app.set_name("Laravel Rust");
    assert_eq!(app.long_version(), "Laravel Rust <info>13.0.0</info>");
}

#[tokio::test]
async fn it_suggests_alternatives_for_unknown_commands() {
    let app = sample_application();
    let (code, output) = run(&app, &["make:modle"]).await;

    assert_eq!(code, 1, "{output}");
    assert_eq!(
        output,
        "\n   ERROR  Command \"make:modle\" is not defined. Did you mean one of these?\n\n  ⇂ make:migration\n  ⇂ make:model\n\n"
    );

    let (code, output) = run(&app, &["mke:model"]).await;
    assert_eq!(code, 1);
    assert_eq!(
        output,
        "\n   ERROR  There are no commands defined in the \"mke\" namespace. Did you mean one of these?\n\n  ⇂ make\n\n"
    );

    // Abbreviations of each segment are resolved, like Symfony does...
    let (code, _) = run(&app, &["mak:model", "User"]).await;
    assert_eq!(code, 0);

    let (code, output) = run(&app, &["inspir3"]).await;
    assert_eq!(code, 1);
    assert!(
        output.contains("ERROR  Command \"inspir3\" is not defined. Did you mean one of these?")
    );
    assert!(output.contains("⇂ inspire"));

    let (code, output) = run(&app, &["zzzzzz"]).await;
    assert_eq!(code, 1);
    assert_eq!(output, "\n   ERROR  Command \"zzzzzz\" is not defined.\n\n");

    let (_, output) = run(&app, &["nope:model"]).await;
    assert!(output.contains("There are no commands defined in the \"nope\" namespace."));
}

#[tokio::test]
async fn it_finds_abbreviated_commands() {
    let app = sample_application();
    let (code, output) = run(&app, &["insp"]).await;
    assert_eq!(code, 0);
    assert_eq!(output, "Simplicity is the ultimate sophistication.\n");

    let (code, _) = run(&app, &["m:mo", "User"]).await;
    assert_eq!(code, 0);

    assert!(app.find("make:mod").is_ok());
    assert!(app.find("mig").is_ok());

    // A bare namespace is not a command...
    let error = app.find("make").err().unwrap();
    assert_eq!(
        error.message,
        "Command \"make\" is not defined.\n\nDid you mean one of these?\n    make:migration\n    make:model"
    );
    assert_eq!(error.alternatives, vec!["make:migration", "make:model"]);

    let error = app.find("make:m").err().unwrap();
    assert!(
        error
            .message
            .starts_with("Command \"make:m\" is ambiguous.\nDid you mean one of these?\n")
    );
    assert_eq!(error.alternatives, vec!["make:model", "make:migration"]);

    let (code, output) = run(&app, &["make:m", "x"]).await;
    assert_eq!(code, 1);
    assert!(output.contains("ERROR  Command \"make:m\" is ambiguous. Did you mean one of these?"));

    // Hidden commands may be run by their exact name, but are never suggested.
    assert!(app.find("secret:thing").is_ok());
    assert!(app.find("secret:th").is_err());
}

#[tokio::test]
async fn it_reports_invalid_input() {
    let app = artisan();
    app.command("mail:send {user} {id}", |_| async { Ok(()) });

    let (code, output) = run(&app, &["mail:send"]).await;
    assert_eq!(code, 2);
    assert_eq!(
        output,
        "\n                                               \n  Not enough arguments (missing: \"user, id\").  \n                                               \n\n"
    );

    let (code, output) = run(&app, &["mail:send", "1", "2", "--nope"]).await;
    assert_eq!(code, 2);
    assert!(output.contains("The \"--nope\" option does not exist."));

    let (code, output) = run(&app, &["mail:send", "1", "2", "3"]).await;
    assert_eq!(code, 2);
    assert!(output.contains(
        "Too many arguments to \"mail:send\" command, expected arguments \"user\" \"id\"."
    ));
}

#[tokio::test]
async fn it_renders_command_errors() {
    let app = artisan();
    let reported = Arc::new(Mutex::new(Vec::new()));
    let log = reported.clone();
    app.report_exceptions_using(move |error| log.lock().unwrap().push(error.to_string()));

    app.command("explode", |_| async {
        Err(illuminate_support::error::error!("Something broke"))
    });

    let (code, output) = run(&app, &["explode"]).await;
    assert_eq!(code, 1);
    assert!(output.contains("  Something broke  "));
    assert_eq!(
        *reported.lock().unwrap(),
        vec!["Something broke".to_string()]
    );

    // Calling the command programmatically returns the error...
    let error = app.call("explode", ()).await.unwrap_err();
    assert_eq!(error.to_string(), "Something broke");
}

#[tokio::test]
async fn commands_may_fail_or_exit_with_a_code() {
    let app = artisan();
    app.command(
        "fail",
        |cmd| async move { cmd.fail("Something went wrong.") },
    );
    app.command("exit {code}", |cmd| async move {
        let code: i32 = cmd.argument("code").unwrap().parse()?;
        cmd.exit(code)
    });

    let (code, output) = run(&app, &["fail"]).await;
    assert_eq!(code, 1);
    assert_eq!(output, "\n   ERROR  Something went wrong.\n\n");

    assert_eq!(app.call("exit 3", ()).await.unwrap(), 3);
    assert_eq!(app.call("exit 0", ()).await.unwrap(), 0);
    assert_eq!(app.call("fail", ()).await.unwrap(), 1);
}

#[tokio::test]
async fn it_calls_commands_with_parameters() {
    let app = artisan();
    app.command(
        "mail:send {user} {--queue=} {--id=*} {--force}",
        |cmd| async move {
            cmd.line(format!(
                "user={} queue={} ids={} force={}",
                cmd.argument("user").unwrap(),
                cmd.option("queue").unwrap_or_else(|| "none".into()),
                cmd.option_list("id").join("|"),
                cmd.option_bool("force"),
            ));
            Ok(())
        },
    );

    app.call("mail:send", json!({"user": 1, "--queue": "default"}))
        .await
        .unwrap();
    assert_eq!(app.output(), "user=1 queue=default ids= force=false\n");

    app.call(
        "mail:send",
        json!({"user": "taylor", "--id": [5, 13], "--force": true}),
    )
    .await
    .unwrap();
    assert_eq!(app.output(), "user=taylor queue=none ids=5|13 force=true\n");

    app.call("mail:send 2 --queue=high --id=1 --id 2", ())
        .await
        .unwrap();
    assert_eq!(app.output(), "user=2 queue=high ids=1|2 force=false\n");

    app.call("mail:send", [("user", "3"), ("--queue", "low")])
        .await
        .unwrap();
    assert_eq!(app.output(), "user=3 queue=low ids= force=false\n");

    app.call("mail:send", ["4", "--force"]).await.unwrap();
    assert_eq!(app.output(), "user=4 queue=none ids= force=true\n");

    app.call("mail:send 'Taylor Otwell' --queue=\"high priority\"", ())
        .await
        .unwrap();
    assert_eq!(
        app.output(),
        "user=Taylor Otwell queue=high priority ids= force=false\n"
    );

    let error = app.call("nope", ()).await.unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<CommandNotFoundException>()
            .unwrap()
            .message,
        "The command \"nope\" does not exist."
    );

    let error = app.call("mail:send", ()).await.unwrap_err();
    assert!(error.downcast_ref::<InvalidInputException>().is_some());
}

#[tokio::test]
async fn it_exposes_arguments_and_options() {
    let app = artisan();
    app.command(
        "inspect {user} {names?*} {--Q|queue=default} {--force}",
        |cmd| async move {
            assert_eq!(cmd.name(), "inspect");
            assert_eq!(cmd.argument("user").as_deref(), Some("taylor"));
            assert_eq!(cmd.argument_list("names"), vec!["a", "b"]);
            assert_eq!(cmd.argument("names").as_deref(), Some("a,b"));
            assert_eq!(cmd.argument("missing"), None);
            assert!(cmd.has_argument("user"));
            assert!(!cmd.has_argument("missing"));
            assert_eq!(cmd.option("queue").as_deref(), Some("default"));
            assert!(!cmd.option_bool("force"));
            assert_eq!(cmd.option("force"), None);
            assert!(cmd.has_option("force"));
            assert!(cmd.has_option("verbose"));

            let arguments = cmd.arguments();
            assert_eq!(arguments["command"], json!("inspect"));
            assert_eq!(arguments["names"], json!(["a", "b"]));

            let options = cmd.options();
            assert_eq!(options["queue"], json!("default"));
            assert_eq!(options["force"], json!(false));
            assert_eq!(options["env"], json!(null));

            cmd.set_option("force", true)?;
            assert!(cmd.option_bool("force"));
            Ok(())
        },
    );

    assert_eq!(app.call("inspect taylor a b", ()).await.unwrap(), 0);
}

#[tokio::test]
async fn it_writes_styled_output() {
    let app = artisan();
    app.command("styles", |cmd| async move {
        cmd.info("Info");
        cmd.comment("Comment");
        cmd.question("Question");
        cmd.error("Error");
        cmd.warn("Warning");
        cmd.line("Line");
        cmd.new_line(1);
        cmd.alert("Alert");
        cmd.line_with("Verbose", None, Verbosity::Verbose);
        Ok(())
    });

    app.call("styles", ()).await.unwrap();
    assert_eq!(
        app.output(),
        "Info\nComment\nQuestion\nError\nWarning\nLine\n\n*****************\n*     Alert     *\n*****************\n\n"
    );

    app.call("styles -v", ()).await.unwrap();
    assert!(app.output().ends_with("Verbose\n"));

    let output = Output::buffered();
    app.call_with_output("styles", json!({"--ansi": true}), &output)
        .await
        .unwrap();
    let text = output.fetch();
    assert!(text.starts_with("\x1b[32mInfo\x1b[39m\n\x1b[33mComment\x1b[39m\n\x1b[30;46mQuestion\x1b[39;49m\n\x1b[37;41mError\x1b[39;49m\n\x1b[33mWarning\x1b[39m\n"));

    app.call("styles --quiet", ()).await.unwrap();
    assert_eq!(app.output(), "");
}

#[tokio::test]
async fn commands_may_call_other_commands() {
    let app = artisan();
    app.command("child {name} {--loud}", |cmd| async move {
        let mut greeting = format!("Hello {}", cmd.argument("name").unwrap());
        if cmd.option_bool("loud") {
            greeting = greeting.to_uppercase();
        }
        cmd.line(greeting);
        cmd.line_with("(verbose)", None, Verbosity::Verbose);
        Ok(())
    });
    app.command("parent", |cmd| async move {
        let code = cmd
            .call("child", json!({"name": "Taylor", "--loud": true}))
            .await?;
        cmd.call("child Abigail", ()).await?;
        cmd.call_silently("child Silent", ()).await?;
        cmd.line(format!("child exited with {code}"));
        Ok(())
    });

    app.call("parent", ()).await.unwrap();
    assert_eq!(
        app.output(),
        "HELLO TAYLOR\nHello Abigail\nchild exited with 0\n"
    );

    // Verbosity is forwarded to the called commands...
    app.call("parent -v", ()).await.unwrap();
    assert!(app.output().contains("Hello Abigail\n(verbose)\n"));
}

#[tokio::test]
async fn it_renders_tables_and_progress_bars() {
    let app = artisan();
    app.command("users", |cmd| async move {
        cmd.table(
            ["ID", "Email"],
            [["1", "taylor@example.com"], ["2", "abigail@example.com"]],
        );
        let items = cmd.with_progress_bar(vec![1, 2, 3], |_, _| {});
        assert_eq!(items, vec![1, 2, 3]);
        Ok(())
    });

    app.call("users", ()).await.unwrap();
    let output = app.output();
    assert!(output.starts_with(
        "+----+---------------------+\n| ID | Email               |\n+----+---------------------+\n| 1  | taylor@example.com  |\n| 2  | abigail@example.com |\n+----+---------------------+\n"
    ));
    assert!(output.ends_with(" 3/3 [============================] 100%"));
}

#[tokio::test]
async fn it_asks_questions_with_fed_input() {
    let app = artisan();
    app.command("interview", |cmd| async move {
        let name = cmd.ask("What is your name?");
        let framework = cmd.ask_with_default("Favorite framework?", "Laravel");
        let password = cmd.secret("Password?");
        let sure = cmd.confirm("Are you sure?", false);
        let language = cmd.choice("Language?", ["PHP", "Rust"], Some(0));
        let many = cmd.choice_multiple("Tools?", ["Forge", "Vapor", "Envoyer"], &[]);
        let guess = cmd.anticipate("Editor?", ["PhpStorm", "Vim"]);
        cmd.line(format!(
            "{name}|{framework}|{password}|{sure}|{language}|{}|{guess}",
            many.join("+")
        ));
        Ok(())
    });

    let output = Output::buffered().with_input(["Taylor", "", "secret", "yes", "1", "0,2", "Vim"]);
    app.call_with_output("interview", (), &output)
        .await
        .unwrap();
    assert!(
        output
            .contents()
            .ends_with("Taylor|Laravel|secret|true|Rust|Forge+Envoyer|Vim\n")
    );

    // Without interaction the defaults are used...
    let output = Output::buffered().with_input(["ignored"]);
    app.call_with_output("interview --no-interaction", (), &output)
        .await
        .unwrap();
    assert_eq!(output.contents(), "|Laravel||false|PHP||\n");
}

struct PromptingCommand;

#[async_trait]
impl Command for PromptingCommand {
    fn signature(&self) -> &str {
        "greet {name} {title : The title of the person}"
    }

    fn prompts_for_missing_input(&self) -> bool {
        true
    }

    fn prompt_for_missing_arguments_using(&self) -> Vec<(&str, &str)> {
        vec![("name", "Who should we greet?")]
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        cmd.line(format!(
            "Hello {} {}",
            cmd.argument("title").unwrap(),
            cmd.argument("name").unwrap()
        ));
        Ok(())
    }
}

#[tokio::test]
async fn it_prompts_for_missing_arguments() {
    let app = artisan();
    app.add(PromptingCommand);

    let output = Output::buffered().with_input(["Taylor", "Mr."]);
    assert_eq!(app.call_with_output("greet", (), &output).await.unwrap(), 0);
    let text = output.contents();
    assert!(text.contains("Who should we greet?"));
    assert!(text.contains("What is the title of the person?"));
    assert!(text.ends_with("Hello Mr. Taylor\n"));

    // Without interaction, the arguments are simply missing.
    let error = app.call("greet", ()).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Not enough arguments (missing: \"name, title\")."
    );
}

#[tokio::test]
async fn closure_commands_can_be_updated_after_registration() {
    let app = artisan();
    app.command("hello", |_| async { Ok(()) })
        .purpose("Say hello")
        .alias("hi")
        .with_help("Greets the world.");

    let command = app.get("hello").unwrap();
    assert_eq!(command.description(), "Say hello");
    assert_eq!(command.help(), "Greets the world.");
    assert!(app.has("hi"));
    assert_eq!(app.all().len(), 3);

    app.command("hello", |_| async { Ok(()) }).hide();
    assert!(app.get("hello").unwrap().hidden());
    assert!(!app.has("hi"));

    app.forget("hello");
    assert!(!app.has("hello"));
}

#[test]
#[should_panic(expected = "Invalid signature")]
fn invalid_signatures_panic_on_registration() {
    let app = artisan();
    app.command("bad {a?} {b}", |_| async { Ok(()) });
}

#[tokio::test]
async fn it_runs_hooks_around_commands() {
    let app = artisan();
    let events = Arc::new(Mutex::new(Vec::new()));
    let (starting, finished) = (events.clone(), events.clone());
    app.on_command_starting(move |name| starting.lock().unwrap().push(format!("starting {name}")));
    app.on_command_finished(move |name, code| {
        finished
            .lock()
            .unwrap()
            .push(format!("finished {name} {code}"))
    });
    app.command("noop", |_| async { Ok(()) });

    app.call("noop", ()).await.unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        vec!["starting noop", "finished noop 0"]
    );
}

#[tokio::test]
async fn components_render_inside_commands() {
    let app = artisan();
    app.command("migrate", |cmd| async move {
        cmd.components().info("Running migrations.");
        cmd.components()
            .task("2014_10_12_000000_create_users_table", || async { Ok(()) })
            .await?;
        cmd.components().two_column_detail("Environment", "local");
        cmd.components().bullet_list(["One", "Two"]);
        cmd.components().warn("Careful");
        Ok(())
    });

    app.call("migrate", ()).await.unwrap();
    let output = app.output();
    assert!(output.starts_with(
        "\n   INFO  Running migrations.\n\n  2014_10_12_000000_create_users_table ...."
    ));
    assert!(output.contains("ms DONE\n"));
    assert!(output.contains("  Environment ......"));
    assert!(output.contains(" local\n"));
    assert!(output.contains("  ⇂ One\n  ⇂ Two\n"));
    assert!(output.ends_with("\n   WARN  Careful.\n\n"));
}
