use std::sync::Arc;

use illuminate_console::prelude::*;
use illuminate_console::prompts;
use illuminate_console::testing::{PendingCommand, artisan};
use illuminate_console::{ConsoleServiceProvider, Output};
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_support::json;

fn container() -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    ConsoleServiceProvider.register(&container);
    (container, guard)
}

fn register_question_command() {
    Artisan::command("question", |cmd| async move {
        let name = cmd.ask("What is your name?");
        let language = cmd.choice(
            "Which language do you prefer?",
            ["PHP", "Ruby", "Python"],
            None,
        );
        cmd.line(format!("Your name is {name} and you prefer {language}."));
        Ok(())
    });
}

#[tokio::test]
async fn it_expects_questions_and_output() {
    let (_container, _guard) = container();
    register_question_command();

    let result = artisan("question")
        .expects_question("What is your name?", "Taylor Otwell")
        .expects_question("Which language do you prefer?", "PHP")
        .expects_output("Your name is Taylor Otwell and you prefer PHP.")
        .doesnt_expect_output("Your name is Taylor Otwell and you prefer Ruby.")
        .assert_exit_code(0)
        .await;

    assert_eq!(result.exit_code, 0);
    assert_eq!(
        result.output,
        "Your name is Taylor Otwell and you prefer PHP.\n"
    );
}

#[tokio::test]
async fn it_expects_choices() {
    let (_container, _guard) = container();
    register_question_command();

    artisan("question")
        .expects_question("What is your name?", "Taylor")
        .expects_choice(
            "Which language do you prefer?",
            "Ruby",
            ["Python", "PHP", "Ruby"],
        )
        .expects_output_to_contain("you prefer Ruby")
        .assert_successful()
        .await;
}

#[tokio::test]
#[should_panic(expected = "Question \"Which language do you prefer?\" has different options.")]
async fn it_fails_when_choices_differ() {
    let (_container, _guard) = container();
    register_question_command();

    artisan("question")
        .expects_question("What is your name?", "Taylor")
        .expects_choice_strict(
            "Which language do you prefer?",
            "PHP",
            ["Python", "PHP", "Ruby"],
        )
        .await;
}

#[tokio::test]
#[should_panic(expected = "Question \"What is your age?\" was not asked.")]
async fn it_fails_when_a_question_is_not_asked() {
    let (_container, _guard) = container();
    register_question_command();

    artisan("question")
        .expects_question("What is your name?", "Taylor")
        .expects_question("Which language do you prefer?", "PHP")
        .expects_question("What is your age?", "40")
        .await;
}

#[tokio::test]
#[should_panic(expected = "Unexpected question \"What is your name?\" was asked.")]
async fn it_fails_when_an_unexpected_question_is_asked() {
    let (_container, _guard) = container();
    register_question_command();

    artisan("question").await;
}

#[tokio::test]
#[should_panic(expected = "Output \"Goodbye\" was not printed.")]
async fn it_fails_when_output_is_missing() {
    let (_container, _guard) = container();
    Artisan::command("hello", |cmd| async move {
        cmd.line("Hello");
        Ok(())
    });

    artisan("hello").expects_output("Goodbye").await;
}

#[tokio::test]
#[should_panic(expected = "Output \"Hello\" was printed.")]
async fn it_fails_when_unexpected_output_is_printed() {
    let (_container, _guard) = container();
    Artisan::command("hello", |cmd| async move {
        cmd.line("Hello");
        Ok(())
    });

    artisan("hello")
        .doesnt_expect_output_to_contain("Hello")
        .await;
}

#[tokio::test]
#[should_panic(expected = "Expected status code 0 but received 1.")]
async fn it_asserts_exit_codes() {
    let (_container, _guard) = container();
    Artisan::command("broken", |cmd| async move { cmd.fail("Broken.") });

    artisan("broken").assert_successful().await;
}

#[tokio::test]
async fn it_asserts_failures_and_confirmations() {
    let (_container, _guard) = container();
    Artisan::command("module:import", |cmd| async move {
        if !cmd.confirm("Do you really wish to run this command?", false) {
            return cmd.exit(1);
        }
        cmd.info("Imported.");
        Ok(())
    });

    artisan("module:import")
        .expects_confirmation("Do you really wish to run this command?", "no")
        .assert_exit_code(1)
        .await;

    artisan("module:import")
        .expects_confirmation("Do you really wish to run this command?", "yes")
        .expects_output("Imported.")
        .assert_ok()
        .await;

    artisan("module:import")
        .expects_confirmation("Do you really wish to run this command?", "no")
        .assert_failed()
        .assert_not_exit_code(0)
        .await;
}

#[tokio::test]
async fn it_expects_tables() {
    let (_container, _guard) = container();
    Artisan::command("users:all", |cmd| async move {
        cmd.table(
            ["ID", "Email"],
            [
                vec!["1", "taylor@example.com"],
                vec!["2", "abigail@example.com"],
            ],
        );
        Ok(())
    });

    artisan("users:all")
        .expects_table(
            ["ID", "Email"],
            [
                vec!["1", "taylor@example.com"],
                vec!["2", "abigail@example.com"],
            ],
        )
        .await;
}

#[tokio::test]
async fn it_expects_no_output() {
    let (_container, _guard) = container();
    Artisan::command("quiet", |_| async { Ok(()) });
    Artisan::command("loud", |cmd| async move {
        cmd.line("!");
        Ok(())
    });

    artisan("quiet").doesnt_expect_any_output().await;
    artisan("loud").expects_any_output().await;
}

#[tokio::test]
async fn it_passes_parameters() {
    let (_container, _guard) = container();
    Artisan::command("mail:send {user} {--queue=}", |cmd| async move {
        cmd.line(format!(
            "{}:{}",
            cmd.argument("user").unwrap(),
            cmd.option("queue").unwrap()
        ));
        Ok(())
    });

    artisan("mail:send")
        .with_args(json!({"user": 7, "--queue": "high"}))
        .expects_output("7:high")
        .await;

    artisan("mail:send 8 --queue=low")
        .expects_output("8:low")
        .await;

    // Usage errors are rendered and exit with 2...
    artisan("mail:send")
        .assert_exit_code(2)
        .expects_output_to_contain("Not enough arguments (missing: \"user\").")
        .await;

    // ...and unknown commands exit with 1.
    artisan("nope")
        .assert_exit_code(1)
        .expects_output_to_contain("The command \"nope\" does not exist.")
        .await;
}

#[tokio::test]
async fn prompts_answer_expected_questions() {
    let (_container, _guard) = container();
    Artisan::command("user:create", |cmd| async move {
        let name = prompts::text("What is your name?")
            .required(true)
            .prompt()?;
        let role =
            prompts::select("What role should the user have?", ["Member", "Owner"]).prompt()?;
        let permissions = prompts::multiselect("Permissions?", ["Read", "Write"]).prompt()?;
        let confirmed = prompts::confirm("Create the user?").prompt()?;
        let password = prompts::password("Password?").prompt()?;
        prompts::info("Creating...");
        cmd.line(format!(
            "{name}|{role}|{}|{confirmed}|{password}",
            permissions.join("+")
        ));
        Ok(())
    });

    artisan("user:create")
        .expects_question("What is your name?", "Taylor")
        .expects_choice(
            "What role should the user have?",
            "Owner",
            ["Member", "Owner"],
        )
        .expects_question("Permissions?", "Read,Write")
        .expects_confirmation("Create the user?", "yes")
        .expects_question("Password?", "secret")
        .expects_output_to_contain(" Creating...")
        .expects_output("Taylor|Owner|Read+Write|true|secret")
        .assert_successful()
        .await;
}

#[tokio::test]
async fn invalid_prompt_answers_fail_the_command() {
    let (_container, _guard) = container();
    Artisan::command("user:create", |cmd| async move {
        let name = prompts::text("What is your name?")
            .validate(|value| {
                (value.len() < 3).then(|| "The name must be at least 3 characters.".to_string())
            })
            .prompt()?;
        cmd.line(name);
        Ok(())
    });

    artisan("user:create")
        .expects_question("What is your name?", "Al")
        .expects_output_to_contain("ERROR  The name must be at least 3 characters.")
        .assert_failed()
        .await;

    // Non-interactive runs fail with the validation message too.
    let output = Output::buffered();
    let code = Artisan::call_with_output("user:create", (), &output)
        .await
        .unwrap();
    assert_eq!(code, 1);
    assert!(
        output
            .contents()
            .contains("ERROR  The name must be at least 3 characters.")
    );
}

#[tokio::test]
async fn it_tests_commands_against_a_specific_application() {
    let app = Application::new();
    app.command("ping", |cmd| async move {
        cmd.line("pong");
        Ok(())
    });

    let result = PendingCommand::new(app, "ping")
        .expects_output("pong")
        .await;
    assert_eq!(result.output, "pong\n");
}

#[tokio::test]
async fn the_provider_registers_the_framework_commands() {
    let (container, _guard) = container();
    let artisan = container.make::<Application>();

    assert!(artisan.has("list"));
    assert!(artisan.has("help"));
    assert!(artisan.has("schedule:run"));
    assert!(artisan.has("schedule:list"));
    assert!(artisan.has("schedule:work"));
    assert!(artisan.has("schedule:test"));
    assert!(Arc::ptr_eq(&artisan, &Artisan::application()));

    Artisan::register(illuminate_console::ListCommand);
    assert!(Artisan::has("list"));
    assert!(Artisan::all().contains_key("schedule:run"));
}
