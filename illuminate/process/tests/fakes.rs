//! Faking processes, and the process assertions.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_process::{
    Command, Factory, FakeProcess, OutOfBoundsException, OutputType, PendingProcess, Process,
    ProcessFailedException, ProcessServiceProvider,
};
use illuminate_support::error::RuntimeException;

/// Each test gets its own container (and so its own process factory).
fn container() -> LocalInstanceGuard {
    let container = Arc::new(Container::new());
    ProcessServiceProvider.register(&container);
    Container::set_local_instance(container)
}

#[tokio::test]
async fn basic_process_fake() {
    let _guard = container();
    Process::fake();

    let result = Process::run("ls -la").await.unwrap();

    assert_eq!(result.output(), "");
    assert_eq!(result.error_output(), "");
    assert_eq!(result.exit_code(), Some(0));
    assert!(result.successful());
    assert_eq!(result.command(), "ls -la");
    assert!(Process::is_recording());
}

#[tokio::test]
async fn fakes_are_isolated_per_container() {
    let _guard = container();
    Process::fake();
    assert!(Process::is_recording());

    {
        let _inner = container();
        assert!(!Process::is_recording());
        assert_eq!(Process::run("echo real").await.unwrap().output(), "real\n");
    }

    assert!(Process::is_recording());
}

#[tokio::test]
async fn multi_line_commands_can_be_faked() {
    let _guard = container();
    Process::prevent_stray_processes();
    Process::fake_commands([("*", "The output")]);

    let result = Process::run(
        "git clone --depth 1 \\\n      --single-branch \\\n      --branch main \\\n      git://some-url .",
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.output(), "The output\n");

    let factory = Factory::new();
    factory.prevent_stray_processes();
    factory.fake_commands([
        ("*--branch main*", "not this one"),
        ("*--branch develop*", "yes thank you"),
    ]);

    let result = factory
        .run("git clone --depth 1 \\\n      --single-branch \\\n      --branch develop \\\n      git://some-url .")
        .await
        .unwrap();
    assert_eq!(result.output(), "yes thank you\n");
}

#[tokio::test]
async fn fake_exit_codes() {
    let _guard = container();

    Process::fake_using(|_| Process::result("test output", "", 1));
    assert!(!Process::run("ls -la").await.unwrap().successful());

    Process::fake_commands([("ls -la", 1)]);
    let result = Process::run("ls -la").await.unwrap();
    assert_eq!(result.exit_code(), Some(1));
    assert!(!result.successful());
}

#[tokio::test]
async fn fakes_with_custom_output() {
    let factory = Factory::new();

    factory.fake_using(|_| Process::result("test output", "", 0));
    assert_eq!(
        factory.run("ls -la").await.unwrap().output(),
        "test output\n"
    );

    factory.fake_using(|_| Process::result(["line 1", "line 2"], "", 0));
    assert_eq!(
        factory.run("ls -la").await.unwrap().output(),
        "line 1\nline 2\n"
    );

    factory.fake_using(|_| Process::result(["line 1", "", "line 2"], "", 0));
    assert_eq!(
        factory.run("ls -la").await.unwrap().output(),
        "line 1\n\nline 2\n"
    );

    factory.fake_using(|_| "test output");
    assert_eq!(
        factory.run("ls -la").await.unwrap().output(),
        "test output\n"
    );

    factory.fake_using(|_| vec!["line 1", "line 2"]);
    assert_eq!(
        factory.run("ls -la").await.unwrap().output(),
        "line 1\nline 2\n"
    );

    factory.fake_using(|_| vec!["line 1", "", "line 2"]);
    assert_eq!(
        factory.run("ls -la").await.unwrap().output(),
        "line 1\n\nline 2\n"
    );

    factory.fake_using(|_| Process::describe().output("line 1").output("line 2"));
    assert_eq!(
        factory.run("ls -la").await.unwrap().output(),
        "line 1\nline 2\n"
    );

    factory.fake_using(|_| {
        Process::describe()
            .output("line 1")
            .output("")
            .output("line 2")
    });
    assert_eq!(
        factory.run("ls -la").await.unwrap().output(),
        "line 1\n\nline 2\n"
    );
}

#[tokio::test]
async fn fakes_with_error_output() {
    let factory = Factory::new();

    factory.fake_using(|_| Process::result("standard output", "error output", 0));
    let result = factory.run("ls -la").await.unwrap();
    assert_eq!(result.output(), "standard output\n");
    assert_eq!(result.error_output(), "error output\n");

    factory.fake_using(|_| Process::result("standard output", ["line 1", "line 2"], 0));
    assert_eq!(
        factory.run("ls -la").await.unwrap().error_output(),
        "line 1\nline 2\n"
    );

    factory.fake_using(|_| {
        Process::describe()
            .output("standard output")
            .error_output("error output")
    });
    let result = factory.run("ls -la").await.unwrap();
    assert_eq!(result.output(), "standard output\n");
    assert_eq!(result.error_output(), "error output\n");
}

#[tokio::test]
async fn fakes_can_be_customized_per_command() {
    let _guard = container();
    Process::fake_commands([("ls *", "ls command"), ("cat *", "cat command")]);

    assert_eq!(
        Process::run("ls -la").await.unwrap().output(),
        "ls command\n"
    );
    assert_eq!(
        Process::run("cat composer.json").await.unwrap().output(),
        "cat command\n"
    );
}

#[tokio::test]
async fn later_fakes_for_the_same_pattern_replace_earlier_ones() {
    let _guard = container();
    Process::fake_commands([("ls *", "first")]);
    Process::fake_commands([("ls *", "second")]);

    assert_eq!(Process::run("ls -la").await.unwrap().output(), "second\n");
}

#[tokio::test]
async fn fake_handlers_receive_the_pending_process() {
    let _guard = container();
    Process::fake_command_using("deploy *", |process: &PendingProcess| {
        format!(
            "deploying from {}",
            process.path.as_ref().unwrap().display()
        )
    });

    let result = Process::path("/var/www")
        .run("deploy production")
        .await
        .unwrap();

    assert_eq!(result.output(), "deploying from /var/www\n");
}

#[tokio::test]
async fn fake_sequences() {
    let _guard = container();
    Process::fake_commands([
        (
            "ls *",
            FakeProcess::from(
                Process::sequence()
                    .push("ls command 1")
                    .push("ls command 2"),
            ),
        ),
        ("cat *", "cat command".into()),
    ]);

    assert_eq!(
        Process::run("ls -la").await.unwrap().output(),
        "ls command 1\n"
    );
    assert_eq!(
        Process::run("ls -la").await.unwrap().output(),
        "ls command 2\n"
    );
    assert_eq!(
        Process::run("cat composer.json").await.unwrap().output(),
        "cat command\n"
    );
}

#[tokio::test]
async fn fake_sequences_can_return_empty_results_when_empty() {
    let _guard = container();
    Process::fake_commands([(
        "ls *",
        Process::sequence()
            .push("ls command 1")
            .push("ls command 2")
            .dont_fail_when_empty(),
    )]);

    assert_eq!(
        Process::run("ls -la").await.unwrap().output(),
        "ls command 1\n"
    );
    assert_eq!(
        Process::run("ls -la").await.unwrap().output(),
        "ls command 2\n"
    );
    assert_eq!(Process::run("ls -la").await.unwrap().output(), "");
}

#[tokio::test]
async fn fake_sequences_throw_when_empty() {
    let _guard = container();
    Process::fake_commands([(
        "ls *",
        Process::sequence()
            .push("ls command 1")
            .push("ls command 2"),
    )]);

    Process::run("ls -la").await.unwrap();
    Process::run("ls -la").await.unwrap();
    let error = Process::run("ls -la").await.unwrap_err();

    assert!(error.is::<OutOfBoundsException>());
    assert_eq!(
        error.to_string(),
        "A process was invoked, but the process result sequence is empty."
    );
}

#[tokio::test]
async fn sequences_work_with_environment_variables() {
    let _guard = container();
    Process::fake_commands([(
        "printenv TEST_VAR OTHER_VAR",
        Process::sequence()
            .push("test_value\nother_value")
            .push("new_test_value\nnew_other_value"),
    )]);

    let result = Process::env([("TEST_VAR", "test_value"), ("OTHER_VAR", "other_value")])
        .run("printenv TEST_VAR OTHER_VAR")
        .await
        .unwrap();
    assert_eq!(result.output(), "test_value\nother_value\n");

    let result = Process::env([
        ("TEST_VAR", "new_test_value"),
        ("OTHER_VAR", "new_other_value"),
    ])
    .run("printenv TEST_VAR OTHER_VAR")
    .await
    .unwrap();
    assert_eq!(result.output(), "new_test_value\nnew_other_value\n");

    Process::assert_ran_times_with(
        |process, _| {
            process
                .command
                .as_ref()
                .is_some_and(|command| command.to_string().contains("printenv TEST_VAR OTHER_VAR"))
        },
        2,
    );
    Process::assert_ran_with(|process, _| {
        process.environment.get("TEST_VAR") == Some(&Some("new_test_value".to_string()))
    });
}

#[tokio::test]
async fn stray_processes_can_be_prevented() {
    let _guard = container();
    Process::prevent_stray_processes();
    Process::fake_commands([("ls *", "ls command")]);

    let error = Process::run("cat composer.json").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Attempted process [cat composer.json] without a matching fake."
    );
    assert!(error.is::<RuntimeException>());

    let error = Process::run(["cat composer.json"]).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Attempted process ['cat composer.json'] without a matching fake."
    );

    assert!(Process::start("cat composer.json").is_err());
    assert!(Process::preventing_stray_processes());

    Process::allow_stray_processes();
    assert_eq!(
        Process::run("echo allowed").await.unwrap().output(),
        "allowed\n"
    );
}

#[tokio::test]
async fn stray_processes_run_by_default() {
    let _guard = container();
    Process::fake_commands([("cat *", "cat command")]);

    let result = Process::run("echo real").await.unwrap();

    assert_eq!(result.output(), "real\n");
    Process::assert_didnt_run("echo real");
}

#[tokio::test]
async fn preventing_stray_processes_needs_fakes() {
    let _guard = container();
    Process::prevent_stray_processes();

    assert_eq!(Process::run("echo real").await.unwrap().output(), "real\n");
}

#[tokio::test]
async fn fakes_can_throw() {
    let _guard = container();
    Process::fake_commands([(
        "cat me",
        FakeProcess::throw(RuntimeException::new("fake exception message")),
    )]);

    let error = Process::run("cat me").await.unwrap_err();

    assert_eq!(error.to_string(), "fake exception message");
    assert!(error.is::<RuntimeException>());
    assert!(Process::start("cat me").is_err());
}

#[tokio::test]
async fn fake_failures_throw_laravel_messages() {
    let _guard = container();

    Process::fake_using(|_| Process::result("", "", 1));
    let error = Process::run("exit 1;").await.unwrap().throw().unwrap_err();
    assert_eq!(
        error.to_string(),
        "The command \"exit 1;\" failed.\n\nExit Code: 1"
    );

    Process::fake_using(|_| Process::result("", "Hello World", 1));
    let error = Process::run("echo \"Hello World\" >&2; exit 1;")
        .await
        .unwrap()
        .throw()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "The command \"echo \"Hello World\" >&2; exit 1;\" failed.\n\nExit Code: 1\n\nError Output:\n================\nHello World\n"
    );

    Process::fake_using(|_| Process::result("Hello World", "", 1));
    let error = Process::run("echo \"Hello World\" >&1; exit 1;")
        .await
        .unwrap()
        .throw()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "The command \"echo \"Hello World\" >&1; exit 1;\" failed.\n\nExit Code: 1\n\nOutput:\n================\nHello World\n"
    );

    assert!(Process::run("ls").await.unwrap().throw_if(true).is_err());
    assert!(Process::run("ls").await.unwrap().throw_if(false).is_ok());
}

#[tokio::test]
async fn faked_output_is_handed_to_output_callbacks() {
    let _guard = container();
    Process::fake_using(|_| Process::result(["one", "two"], "oops", 0));
    let mut lines = Vec::new();

    Process::run_with_output("ls", |kind, line| lines.push((kind, line.to_string())))
        .await
        .unwrap();

    assert_eq!(
        lines,
        vec![
            (OutputType::Out, "one\n".to_string()),
            (OutputType::Out, "two\n".to_string()),
            (OutputType::Err, "oops\n".to_string()),
        ]
    );
}

#[tokio::test]
async fn fake_pools() {
    let _guard = container();
    Process::fake_commands([("cat *", Process::result("", "", 1))]);

    let results = Process::pool(|pool| {
        pool.command("echo real");
        pool.command("cat test");
    })
    .start()
    .unwrap()
    .wait()
    .await
    .unwrap();

    assert!(results[0].successful());
    assert_eq!(results[0].output(), "real\n");
    assert!(results[1].failed());
    assert!(results.failed());
}

#[tokio::test]
async fn fake_pools_can_be_stopped() {
    let _guard = container();
    Process::fake_using(|_| Process::describe().runs_for(10));

    let mut pool = Process::pool(|pool| {
        pool.command("sleep 100");
        pool.command("sleep 100");
    })
    .start()
    .unwrap();

    assert_eq!(pool.running().len(), 2);
    pool.stop().await;
    assert_eq!(pool.running().len(), 0);
}

#[tokio::test]
async fn fake_pipes() {
    let _guard = container();
    Process::fake_commands([("cat *", "Hello, world\nfoo\nbar")]);

    let result = Process::pipe(|pipe| {
        pipe.command("cat test");
        pipe.command("grep -i \"foo\"");
    })
    .await
    .unwrap();
    assert_eq!(result.output(), "foo\n");

    let result = Process::pipe_commands(["cat test", "grep -i \"foo\""])
        .await
        .unwrap();
    assert_eq!(result.output(), "foo\n");

    Process::assert_ran_times("cat test", 2);
}

#[tokio::test]
async fn failed_fake_pipes_stop() {
    let _guard = container();
    Process::fake_commands([("cat *", Process::result("", "", 1))]);

    let result = Process::pipe_commands(["cat test", "grep -i \"foo\""])
        .await
        .unwrap();

    assert!(result.failed());
    Process::assert_didnt_run("grep -i \"foo\"");
}

#[tokio::test]
async fn fake_pipes_receive_the_previous_output_as_input() {
    let _guard = container();
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let captured = inputs.clone();
    Process::fake_using(move |process| {
        let input = process
            .input
            .clone()
            .map(|input| String::from_utf8(input).unwrap());
        captured.lock().unwrap().push(input);
        "next"
    });

    Process::pipe_commands(["first", "second", "third"])
        .await
        .unwrap();

    assert_eq!(
        *inputs.lock().unwrap(),
        vec![None, Some("next\n".to_string()), Some("next\n".to_string())]
    );
}

// ----------------------------------------------------------------------
// Faking asynchronous processes
// ----------------------------------------------------------------------

#[tokio::test]
async fn fake_started_processes_play_back_their_description() {
    let _guard = container();
    Process::fake_commands([(
        "bash import.sh",
        Process::describe()
            .output("ONE")
            .output("TWO")
            .output("THREE")
            .runs_for(3),
    )]);

    let mut process = Process::start("bash import.sh").unwrap();
    let mut latest = Vec::new();
    let mut output = Vec::new();

    while process.running() {
        latest.push(process.latest_output());
        output.push(process.output());
    }

    assert_eq!(latest, vec!["ONE\n", "THREE\n", ""]);
    assert_eq!(
        output,
        vec!["ONE\nTWO\n", "ONE\nTWO\nTHREE\n", "ONE\nTWO\nTHREE\n"]
    );
    assert_eq!(process.id(), Some(1000));
    assert_eq!(process.command(), "bash import.sh");
}

#[tokio::test]
async fn fake_started_processes_report_error_output() {
    let _guard = container();
    Process::fake_using(|_| {
        Process::describe()
            .output("First line of standard output")
            .error_output("First line of error output")
            .output("Second line of standard output")
            .exit_code(0)
            .iterations(3)
    });

    let mut process = Process::start("bash import.sh").unwrap();
    let mut errors = Vec::new();
    while process.running() {
        errors.push(process.latest_error_output());
    }

    assert_eq!(errors, vec!["First line of error output\n", "", ""]);
    assert_eq!(process.error_output(), "First line of error output\n");

    let result = process.wait().await.unwrap();
    assert_eq!(
        result.output(),
        "First line of standard output\nSecond line of standard output\n"
    );
}

#[tokio::test]
async fn fake_wait_until() {
    let _guard = container();
    Process::fake_using(|_| {
        Process::describe()
            .output("WAITING")
            .output("READY")
            .output("DONE")
            .runs_for(3)
    });

    let mut process = Process::start("bash serve.sh").unwrap();
    let mut seen = Vec::new();

    let result = process
        .wait_until(|_, output| {
            seen.push(output.to_string());
            output.contains("READY")
        })
        .await
        .unwrap();

    assert!(result.successful());
    assert_eq!(seen, vec!["WAITING\n", "READY\n"]);
}

#[tokio::test]
async fn fake_wait_until_with_error_output() {
    let _guard = container();
    Process::fake_using(|_| {
        Process::describe()
            .output("STDOUT")
            .error_output("ERROR1")
            .error_output("TARGET_ERROR")
            .output("MORE_STDOUT")
            .runs_for(4)
    });

    let mut process = Process::start("bash serve.sh").unwrap();
    let mut seen = Vec::new();

    process
        .wait_until(|kind, output| {
            seen.push((kind, output.to_string()));
            output.contains("TARGET_ERROR")
        })
        .await
        .unwrap();

    assert!(seen.contains(&(OutputType::Out, "STDOUT\n".to_string())));
    assert!(seen.contains(&(OutputType::Err, "ERROR1\n".to_string())));
    assert!(seen.contains(&(OutputType::Err, "TARGET_ERROR\n".to_string())));
}

#[tokio::test]
async fn fake_wait_until_that_never_matches() {
    let _guard = container();
    Process::fake_using(|_| {
        Process::describe()
            .output("LINE1")
            .output("LINE2")
            .output("LINE3")
            .runs_for(3)
    });

    let mut process = Process::start("bash serve.sh").unwrap();
    let mut seen = Vec::new();

    let result = process
        .wait_until(|_, output| {
            seen.push(output.to_string());
            false
        })
        .await
        .unwrap();

    assert!(result.successful());
    assert_eq!(seen, vec!["LINE1\n", "LINE2\n", "LINE3\n"]);
}

#[tokio::test]
async fn fake_wait_until_followed_by_wait() {
    let _guard = container();
    Process::fake_using(|_| {
        Process::describe()
            .output("FIRST")
            .output("SECOND")
            .output("THIRD")
            .runs_for(3)
    });

    let mut process = Process::start("bash serve.sh").unwrap();
    let mut until = Vec::new();
    let mut waited = Vec::new();

    process
        .wait_until(|_, output| {
            until.push(output.to_string());
            output.contains("FIRST")
        })
        .await
        .unwrap();
    let result = process
        .wait_with_output(|_, output| waited.push(output.to_string()))
        .await
        .unwrap();

    assert!(result.successful());
    assert_eq!(until, vec!["FIRST\n"]);
    assert_eq!(waited, vec!["SECOND\n", "THIRD\n"]);
}

#[tokio::test]
async fn fake_wait_followed_by_wait_until() {
    let _guard = container();
    Process::fake_using(|_| {
        Process::describe()
            .output("FIRST")
            .output("SECOND")
            .output("THIRD")
            .runs_for(3)
    });

    let mut process = Process::start("bash serve.sh").unwrap();
    let mut waited = Vec::new();
    let mut until = Vec::new();

    process
        .wait_with_output(|_, output| waited.push(output.to_string()))
        .await
        .unwrap();
    let result = process
        .wait_until(|_, output| {
            until.push(output.to_string());
            true
        })
        .await
        .unwrap();

    assert!(result.successful());
    assert_eq!(waited.len(), 3);
    assert!(until.is_empty());
}

#[tokio::test]
async fn fake_start_callbacks_receive_output_as_the_process_runs() {
    let _guard = container();
    Process::fake_using(|_| {
        Process::describe()
            .output("FIRST")
            .output("SECOND")
            .output("THIRD")
            .runs_for(10)
    });

    let output = Arc::new(Mutex::new(Vec::new()));
    let captured = output.clone();
    let mut process = Process::start_with_output("sleep 100", move |_, line| {
        captured.lock().unwrap().push(line.to_string());
    })
    .unwrap();

    while process.running() {
        process.stop().await;
    }

    assert_eq!(*output.lock().unwrap(), vec!["FIRST\n"]);
}

#[tokio::test]
async fn fake_processes_can_be_stopped_and_signalled() {
    let _guard = container();
    Process::fake_using(|_| {
        Process::describe()
            .output("STARTED")
            .exit_code(143)
            .runs_for(10)
    });

    let mut process = Process::start("sleep 100").unwrap();

    assert!(process.running());
    process.signal(12).unwrap();
    assert!(process.has_received_signal(12));
    assert_eq!(process.stop().await, Some(143));
    assert!(!process.running());
}

#[tokio::test]
async fn fake_processes_never_time_out() {
    let _guard = container();
    Process::fake_using(|_| Process::describe().runs_for(10));

    let mut process = Process::timeout(Duration::from_millis(1))
        .start("sleep 100")
        .unwrap();
    std::thread::sleep(Duration::from_millis(5));

    assert!(process.ensure_not_timed_out().is_ok());
}

#[tokio::test]
async fn started_fakes_from_results_finish_immediately() {
    let _guard = container();
    Process::fake_commands([("composer *", Process::result("Installed", "Warning", 2))]);

    let mut process = Process::start("composer install").unwrap();

    assert!(!process.running());
    let result = process.wait().await.unwrap();
    assert_eq!(result.output(), "Installed\n");
    assert_eq!(result.error_output(), "Warning\n");
    assert_eq!(result.exit_code(), Some(2));
    Process::assert_ran_with(|_, result| result.exit_code() == Some(2));
}

// ----------------------------------------------------------------------
// Assertions
// ----------------------------------------------------------------------

#[tokio::test]
async fn basic_fake_assertions() {
    let _guard = container();
    Process::fake();

    Process::run("ls -la").await.unwrap();

    Process::assert_ran("ls -la");
    Process::assert_ran_with(|process, _| process.command == Some(Command::from("ls -la")));
    Process::assert_ran_with(|process, _| process.timeout == Some(Duration::from_secs(60)));
    Process::assert_ran_times("ls -la", 1);
    Process::assert_ran_times_with(|process, _| process.command == Some("ls -la".into()), 1);
    Process::assert_not_ran("cat foo");
    Process::assert_not_ran_with(|process, _| process.command == Some("cat foo".into()));
    Process::assert_didnt_run("cat foo");
    Process::assert_did_not_run("cat foo");
}

#[tokio::test]
async fn assertions_work_with_falsy_commands() {
    let _guard = container();
    Process::fake();

    Process::run("0").await.unwrap();
    Process::start("0").unwrap().wait().await.unwrap();

    Process::assert_ran("0");
    Process::assert_ran_times("0", 2);
    Process::assert_not_ran("ls -la");
}

#[tokio::test]
async fn assertions_work_with_argument_lists() {
    let _guard = container();
    Process::fake();

    Process::run(["php", "artisan", "migrate"]).await.unwrap();

    Process::assert_ran(["php", "artisan", "migrate"]);
    Process::assert_ran_times(["php", "artisan", "migrate"], 1);
    Process::assert_not_ran(["php", "artisan", "migrate:rollback"]);
    Process::assert_didnt_run(["php", "artisan", "migrate:rollback"]);
    Process::assert_ran_in_order([["php", "artisan", "migrate"]]);
    Process::assert_didnt_run("php artisan migrate");
}

#[tokio::test]
async fn assertions_can_check_the_order_processes_ran_in() {
    let _guard = container();
    Process::fake();

    Process::run("git fetch").await.unwrap();
    Process::run("git reset --hard origin/main").await.unwrap();
    Process::run("composer install --no-dev").await.unwrap();

    Process::assert_ran_in_order([
        "git fetch",
        "git reset --hard origin/main",
        "composer install --no-dev",
    ]);
    assert_eq!(Process::recorded().len(), 3);
}

#[tokio::test]
#[should_panic(expected = "An expected process (#1) was not invoked.")]
async fn ran_in_order_fails_when_out_of_order() {
    let _guard = container();
    Process::fake();

    Process::run("composer install").await.unwrap();
    Process::run("git fetch").await.unwrap();

    Process::assert_ran_in_order(["git fetch", "composer install"]);
}

#[tokio::test]
#[should_panic(expected = "Expected 2 processes to run, but 1 ran.")]
async fn ran_in_order_fails_when_the_count_differs() {
    let _guard = container();
    Process::fake();

    Process::run("git fetch").await.unwrap();

    Process::assert_ran_in_order(["git fetch", "composer install"]);
}

#[tokio::test]
async fn nothing_ran() {
    let _guard = container();
    Process::fake();

    Process::assert_nothing_ran();
}

#[tokio::test]
#[should_panic(expected = "An expected process was not invoked.")]
async fn assert_ran_fails_when_the_process_did_not_run() {
    let _guard = container();
    Process::fake();

    Process::assert_ran("ls -la");
}

#[tokio::test]
#[should_panic(expected = "An expected process ran 1 times instead of 2 times.")]
async fn assert_ran_times_fails_with_the_wrong_count() {
    let _guard = container();
    Process::fake();
    Process::run("ls").await.unwrap();

    Process::assert_ran_times("ls", 2);
}

#[tokio::test]
#[should_panic(expected = "An unexpected process was invoked.")]
async fn assert_didnt_run_fails_when_the_process_ran() {
    let _guard = container();
    Process::fake();
    Process::run("rm -rf storage").await.unwrap();

    Process::assert_didnt_run("rm -rf storage");
}

#[tokio::test]
#[should_panic(expected = "An unexpected process was invoked.")]
async fn assert_nothing_ran_fails_when_something_ran() {
    let _guard = container();
    Process::fake();
    Process::run("ls").await.unwrap();

    Process::assert_nothing_ran();
}

#[tokio::test]
async fn real_processes_are_not_recorded() {
    let _guard = container();
    Process::fake_commands([("cat *", "faked")]);

    Process::run("echo real").await.unwrap();
    Process::run("cat file").await.unwrap();

    assert_eq!(Process::recorded().len(), 1);
    Process::assert_ran("cat file");
}

#[tokio::test]
async fn throw_callbacks_are_invoked_for_fakes() {
    let _guard = container();
    Process::fake_using(|_| 3);

    let mut code = None;
    let error: ProcessFailedException = Process::run("ls")
        .await
        .unwrap()
        .throw_with(|_, exception| code = Some(exception.code()))
        .unwrap_err();

    assert_eq!(code, Some(3));
    assert_eq!(error.result.command(), "ls");
}
