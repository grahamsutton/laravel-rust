//! `dev`, `dev:list`, and the process runner behind them.

use std::future::pending;
use std::time::{Duration, Instant};

use illuminate_console::Output;
use illuminate_foundation::Application;
use illuminate_foundation::console::commands::{
    DevCommandColor, DevCommandPriority, DevCommands, DevProcess, DevProcessCommand, DevProcessRunner,
};
use illuminate_foundation::testing::TestApp;
use illuminate_support::Value;

fn shell(name: &str, command: &str) -> DevProcess {
    DevProcess {
        name: name.into(),
        command: DevProcessCommand::Shell(command.into()),
        color: Some(DevCommandColor::Blue.value().into()),
        source: String::new(),
        priority: DevCommandPriority::Userland,
    }
}

#[tokio::test]
async fn processes_run_side_by_side_with_prefixed_output() {
    let output = Output::buffered();
    let code = DevProcessRunner::new(vec![shell("one", "echo hello"), shell("two", "echo world; echo again")])
        .run(&output, pending())
        .await;

    assert_eq!(code, 0);
    let output = output.fetch();
    assert!(output.contains("[one] hello\n"), "{output}");
    assert!(output.contains("[two] world\n"), "{output}");
    assert!(output.contains("[two] again\n"), "{output}");
    assert!(output.contains("[one] echo hello exited with code 0"), "{output}");
}

#[tokio::test]
async fn artisan_commands_run_through_the_artisan_binary() {
    let output = Output::buffered();
    let queue = DevProcess {
        command: DevProcessCommand::Artisan("queue:listen --tries=1 'two words'".into()),
        ..shell("queue", "")
    };
    let code = DevProcessRunner::new(vec![queue]).artisan_binary("echo").run(&output, pending()).await;

    assert_eq!(code, 0);
    assert!(output.fetch().contains("[queue] queue:listen --tries=1 two words\n"));
}

#[tokio::test]
async fn crashed_processes_are_restarted() {
    let output = Output::buffered();
    let code = DevProcessRunner::new(vec![shell("worker", "echo attempt; exit 3")])
        .restart_tries(2, Duration::from_millis(10))
        .run(&output, pending())
        .await;

    assert_eq!(code, 1);
    let output = output.fetch();
    assert_eq!(output.matches("[worker] attempt").count(), 3, "{output}");
    assert_eq!(output.matches("[worker] echo attempt; exit 3 restarted").count(), 2, "{output}");
    assert!(output.contains("exited with code 3"), "{output}");
}

#[tokio::test]
async fn a_restarted_process_can_recover() {
    let dir = tempfile::tempdir().unwrap();
    let output = Output::buffered();
    let code = DevProcessRunner::new(vec![shell(
        "flaky",
        "if [ -f crashed ]; then echo recovered; else touch crashed; exit 1; fi",
    )])
    .working_directory(dir.path())
    .restart_tries(5, Duration::from_millis(10))
    .run(&output, pending())
    .await;

    assert_eq!(code, 0);
    let output = output.fetch();
    assert!(output.contains("[flaky] recovered"), "{output}");
    assert_eq!(output.matches("restarted").count(), 1, "{output}");
}

#[tokio::test]
async fn without_restarting_a_failure_stops_every_process() {
    let started = Instant::now();
    let output = Output::buffered();
    let code = DevProcessRunner::new(vec![shell("server", "exec sleep 30"), shell("broken", "sleep 0.2; exit 2")])
        .restart(false)
        .run(&output, pending())
        .await;

    assert_eq!(code, 1);
    assert!(started.elapsed() < Duration::from_secs(10));
    let output = output.fetch();
    assert!(output.contains("[broken] sleep 0.2; exit 2 exited with code 2"), "{output}");
    assert!(output.contains("Sending SIGTERM to other processes"), "{output}");
}

#[tokio::test]
async fn every_process_is_stopped_on_shutdown() {
    let started = Instant::now();
    let output = Output::buffered();
    let code = DevProcessRunner::new(vec![shell("server", "echo booted; exec sleep 30"), shell("vite", "exec sleep 30")])
        .run(&output, tokio::time::sleep(Duration::from_millis(300)))
        .await;

    assert_eq!(code, 0);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(output.fetch().contains("[server] booted"));
}

#[tokio::test]
async fn lines_can_be_timestamped() {
    let output = Output::buffered();
    DevProcessRunner::new(vec![shell("one", "echo hello")])
        .timestamps(true)
        .run(&output, pending())
        .await;

    let output = output.fetch();
    let line = output.lines().find(|line| line.ends_with("[one] hello")).expect(&output);
    let time = line.split(' ').next().unwrap();
    assert_eq!(time.len(), 8, "{line}");
    assert!(time.chars().all(|c| c.is_ascii_digit() || c == ':'), "{line}");
}

fn app() -> (TestApp, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("package.json"), r#"{"private": true, "scripts": {"dev": "vite", "build": "vite build"}}"#)
        .unwrap();
    std::fs::write(dir.path().join("yarn.lock"), "").unwrap();
    (TestApp::new(Application::configure_detached(dir.path())), dir)
}

async fn listed(app: &TestApp, command: &str) -> Vec<Value> {
    let result = app.artisan(command).run().await;
    serde_json::from_str(result.output.trim()).unwrap_or_else(|_| panic!("{}", result.output))
}

#[tokio::test]
async fn the_dev_command_runs_the_registered_processes() {
    let (app, _dir) = app();
    DevCommands::without_default_commands();
    DevCommands::register_as("echo hello from dev", "greeter").green();
    DevCommands::register("printf 'one\\ntwo\\n'");

    app.artisan("dev --no-restart")
        .expects_output_to_contain("[greeter] echo hello from dev")
        .expects_output_to_contain("[printf]  printf 'one\\ntwo\\n'")
        .expects_output_to_contain("[greeter] hello from dev")
        .expects_output_to_contain("[printf] one")
        .expects_output_to_contain("[printf] two")
        .assert_successful()
        .await;

    DevCommands::register_as("exit 4", "broken");
    app.artisan("dev --no-restart")
        .expects_output_to_contain("[broken] exit 4 exited with code 4")
        .assert_failed()
        .await;
}

#[tokio::test]
async fn the_framework_registers_the_default_processes() {
    let (app, _dir) = app();

    let processes = listed(&app, "dev:list --json").await;
    let names: Vec<&str> = processes.iter().map(|process| process["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["server", "queue", "vite"]);
    assert_eq!(processes[0]["command"], "artisan serve");
    assert_eq!(processes[1]["command"], "artisan queue:listen --tries=1 --timeout=0");
    assert_eq!(processes[2]["command"], "yarn run dev");
    assert_eq!(processes[0]["color"], DevCommandColor::Blue.value());
    assert_eq!(processes[1]["color"], DevCommandColor::Purple.value());
    assert_eq!(processes[2]["priority"], 0);

    // The application's commands win over the defaults, keeping their place.
    DevCommands::artisan_as("queue:work --tries=3", "queue").orange();
    DevCommands::artisan("horizon");
    DevCommands::node_exec_as("stripe listen", "stripe");
    let processes = listed(&app, "dev:list --json").await;
    assert_eq!(processes.len(), 5);
    assert_eq!(processes[1]["command"], "artisan queue:work --tries=3");
    assert_eq!(processes[1]["color"], DevCommandColor::Orange.value());
    assert_eq!(processes[1]["priority"], 2);
    assert!(processes[1]["source"].as_str().unwrap().contains("console_dev.rs:"));
    assert_eq!(processes[3]["name"], "horizon");
    assert_eq!(processes[4]["command"], "yarn dlx stripe listen");

    let filtered = listed(&app, "dev:list --json --filter=queue").await;
    assert_eq!(filtered.len(), 1);
    let result = app.artisan("dev:list --json --only-vendor").run().await;
    assert_eq!(result.output.trim(), "[]");
    assert_eq!(result.exit_code, 1);

    DevCommands::except(["queue"]);
    DevCommands::order(["vite", "horizon"]);
    let names: Vec<String> = listed(&app, "dev:list --json")
        .await
        .iter()
        .map(|process| process["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["vite", "horizon", "server", "stripe"]);

    DevCommands::only(["server"]);
    assert_eq!(listed(&app, "dev:list --json").await.len(), 1);
}

#[tokio::test]
async fn processes_are_only_added_for_node_projects_with_a_dev_script() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("package.json"), r#"{"scripts": {"build": "vite build"}}"#).unwrap();
    let app = TestApp::new(Application::configure_detached(dir.path()));

    let names: Vec<String> = listed(&app, "dev:list --json")
        .await
        .iter()
        .map(|process| process["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["server", "queue"]);

    DevCommands::without_default_commands();
    app.artisan("dev:list --json")
        .expects_output("[]")
        .assert_successful()
        .await;
    app.artisan("dev")
        .expects_output_to_contain("Your application doesn't have any dev processes.")
        .assert_successful()
        .await;
}
