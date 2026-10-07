//! Real processes, using portable POSIX commands.
#![cfg(unix)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use illuminate_container::Container;
use illuminate_process::signal::{SIGKILL, SIGTERM};
use illuminate_process::{OutputType, Process, ProcessFailedException, ProcessTimedOutException};
use illuminate_support::CarbonInterval;
use illuminate_support::error::RuntimeException;

/// Each test gets its own container (and so its own process factory).
fn container() -> illuminate_container::LocalInstanceGuard {
    Container::set_local_instance(Arc::new(Container::new()))
}

#[tokio::test]
async fn successful_processes() {
    let _guard = container();
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("ProcessTest.php"), "").unwrap();

    let result = Process::path(directory.path()).run("ls").await.unwrap();

    assert!(result.successful());
    assert!(!result.failed());
    assert_eq!(result.exit_code(), Some(0));
    assert!(result.output().contains("ProcessTest.php"));
    assert_eq!(result.error_output(), "");
    assert_eq!(result.command(), "ls");

    let result = result.throw().unwrap().throw_if(true).unwrap();
    assert!(result.see_in_output("ProcessTest.php"));
}

#[tokio::test]
async fn argument_lists_run_without_a_shell() {
    let _guard = container();

    let result = Process::run(["printf", "%s-%s", "Hello World", "$HOME"])
        .await
        .unwrap();

    assert_eq!(result.output(), "Hello World-$HOME");
    assert_eq!(result.command(), "printf %s-%s 'Hello World' '$HOME'");
}

#[tokio::test]
async fn processes_can_have_error_output() {
    let _guard = container();

    let result = Process::run("echo \"Hello World\" >&2; exit 1;")
        .await
        .unwrap();

    assert!(result.failed());
    assert_eq!(result.exit_code(), Some(1));
    assert_eq!(result.output(), "");
    assert_eq!(result.error_output(), "Hello World\n");
    assert!(result.see_in_error_output("Hello"));
}

#[tokio::test]
async fn quiet_processes_return_empty_output() {
    let _guard = container();

    let result = Process::quietly()
        .run("echo \"Hello World\"; echo \"Hello World\" >&2; exit 1;")
        .await
        .unwrap();

    assert!(result.failed());
    assert_eq!(result.exit_code(), Some(1));
    assert_eq!(result.output(), "");
    assert_eq!(result.error_output(), "");
    assert!(!result.see_in_output("Hello World"));
    assert!(!result.see_in_error_output("Hello World"));
}

#[tokio::test]
async fn quiet_processes_can_throw() {
    let _guard = container();

    let result = Process::quietly()
        .run("echo \"Hello World\" >&2; exit 1;")
        .await
        .unwrap();
    let exception = result.clone().throw().unwrap_err();

    assert_eq!(
        exception.to_string(),
        "The command \"echo \"Hello World\" >&2; exit 1;\" failed.\n\nExit Code: 1"
    );
    assert_eq!(exception.code(), 1);
    assert_eq!(exception.result, result);
}

#[tokio::test]
async fn failed_processes_throw_with_their_output() {
    let _guard = container();

    let error = Process::run("exit 1;").await.unwrap().throw().unwrap_err();
    assert_eq!(
        error.to_string(),
        "The command \"exit 1;\" failed.\n\nExit Code: 1"
    );

    let error = Process::run("echo \"Hello World\" >&2; exit 1;")
        .await
        .unwrap()
        .throw()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "The command \"echo \"Hello World\" >&2; exit 1;\" failed.\n\nExit Code: 1\n\nError Output:\n================\nHello World\n"
    );

    let error = Process::run("echo \"Hello World\" >&1; exit 1;")
        .await
        .unwrap()
        .throw()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "The command \"echo \"Hello World\" >&1; exit 1;\" failed.\n\nExit Code: 1\n\nOutput:\n================\nHello World\n"
    );

    assert!(
        Process::run("exit 1")
            .await
            .unwrap()
            .throw_if(true)
            .is_err()
    );
    assert!(
        Process::run("exit 1")
            .await
            .unwrap()
            .throw_if(false)
            .is_ok()
    );
}

#[tokio::test]
async fn failures_convert_into_framework_errors() {
    let _guard = container();

    async fn deploy() -> illuminate_support::Result<String> {
        Ok(Process::run("echo nope >&2; exit 2")
            .await?
            .throw()?
            .output()
            .to_string())
    }

    let error = deploy().await.unwrap_err();
    let exception = error.downcast_ref::<ProcessFailedException>().unwrap();
    assert_eq!(exception.code(), 2);
    assert_eq!(exception.result.error_output(), "nope\n");
}

#[tokio::test]
async fn processes_can_time_out() {
    let _guard = container();
    let started = Instant::now();

    let error = Process::timeout(0.5)
        .run("sleep 3; exit 1;")
        .await
        .unwrap_err();
    let exception = error.downcast_ref::<ProcessTimedOutException>().unwrap();

    assert_eq!(
        exception.to_string(),
        "The process \"sleep 3; exit 1;\" exceeded the timeout of 0.5 seconds."
    );
    assert!(exception.is_general_timeout());
    assert!(!exception.is_idle_timeout());
    assert_eq!(exception.exceeded_timeout(), Duration::from_millis(500));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn timeouts_may_be_carbon_intervals() {
    let _guard = container();

    let error = Process::timeout(CarbonInterval::milliseconds(300))
        .run("sleep 3")
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("exceeded the timeout of 0.3 seconds")
    );
}

#[tokio::test]
async fn idle_timeouts_throw_the_idle_exception() {
    let _guard = container();

    let error = Process::timeout(10)
        .idle_timeout(0.3)
        .run("sleep 5;")
        .await
        .unwrap_err();
    let exception = error.downcast_ref::<ProcessTimedOutException>().unwrap();

    assert!(exception.is_idle_timeout());
    assert_eq!(exception.exceeded_timeout(), Duration::from_millis(300));
    assert_eq!(
        exception.to_string(),
        "The process \"sleep 5;\" exceeded the idle timeout of 0.3 seconds."
    );
}

#[tokio::test]
async fn output_keeps_idle_processes_alive() {
    let _guard = container();

    let result = Process::idle_timeout(0.5)
        .run("for i in 1 2 3 4 5; do echo $i; sleep 0.2; done")
        .await
        .unwrap();

    assert_eq!(result.output(), "1\n2\n3\n4\n5\n");
}

#[tokio::test]
async fn timed_out_processes_still_expose_their_result() {
    let _guard = container();

    let error = Process::timeout(0.5)
        .run("echo \"Hello World\"; sleep 3;")
        .await
        .unwrap_err();
    let exception = error.downcast_ref::<ProcessTimedOutException>().unwrap();

    assert!(exception.result.output().contains("Hello World"));
    assert_eq!(exception.result.exit_code(), Some(128 + SIGKILL));
}

#[tokio::test]
async fn processes_may_run_forever() {
    let _guard = container();

    let mut pending = Process::forever();
    assert_eq!(pending.timeout, None);
    assert!(pending.timeout(1).run("true").await.unwrap().successful());
}

#[tokio::test]
async fn processes_can_use_standard_input() {
    let _guard = container();

    let result = Process::input("foobar").run("cat").await.unwrap();
    assert_eq!(result.output(), "foobar");

    let big = "x".repeat(300_000);
    let result = Process::input(big.clone()).run("cat").await.unwrap();
    assert_eq!(result.output().len(), big.len());
}

#[tokio::test]
async fn processes_receive_environment_variables() {
    let _guard = container();

    let result = Process::env([("TEST_VAR", "test_value"), ("OTHER_VAR", "other_value")])
        .run("printenv TEST_VAR OTHER_VAR")
        .await
        .unwrap();
    assert_eq!(result.output(), "test_value\nother_value\n");

    let result = Process::env([("HOME", false)])
        .run("echo ${HOME:-unset}")
        .await
        .unwrap();
    assert_eq!(result.output(), "unset\n");
}

#[tokio::test]
async fn missing_working_directories_are_reported() {
    let _guard = container();

    let error = Process::path("/this/path/does/not/exist")
        .run("ls")
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "The provided cwd \"/this/path/does/not/exist\" does not exist."
    );
    assert!(error.is::<RuntimeException>());
}

#[tokio::test]
async fn programs_that_cannot_launch_are_reported() {
    let _guard = container();

    let error = Process::run(["definitely-not-a-real-program-1234"])
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("Unable to launch process [definitely-not-a-real-program-1234]")
    );

    let result = Process::run("definitely-not-a-real-program-1234")
        .await
        .unwrap();
    assert_eq!(result.exit_code(), Some(127));
}

#[tokio::test]
async fn output_can_be_streamed_line_by_line() {
    let _guard = container();
    let mut lines = Vec::new();

    let result = Process::run_with_output(
        "echo one; echo two >&2; sleep 0.05; printf three",
        |kind, line| lines.push((kind, line.to_string())),
    )
    .await
    .unwrap();

    assert!(lines.contains(&(OutputType::Out, "one\n".to_string())));
    assert!(lines.contains(&(OutputType::Err, "two\n".to_string())));
    assert_eq!(lines.last(), Some(&(OutputType::Out, "three".to_string())));
    assert_eq!(lines.len(), 3);
    assert_eq!(result.output(), "one\nthree");
    assert_eq!(result.error_output(), "two\n");
}

#[tokio::test]
async fn streamed_output_arrives_while_the_process_runs() {
    let _guard = container();
    let started = Instant::now();
    let mut arrivals = Vec::new();

    Process::run_with_output("echo first; sleep 0.4; echo second", |_, line| {
        arrivals.push((line.to_string(), started.elapsed()));
    })
    .await
    .unwrap();

    assert_eq!(arrivals.len(), 2);
    assert!(arrivals[1].1 - arrivals[0].1 >= Duration::from_millis(300));
}

#[tokio::test]
async fn large_output_is_collected() {
    let _guard = container();

    let result = Process::run("yes | head -n 50000").await.unwrap();

    assert_eq!(result.output().len(), 100_000);
}

#[tokio::test]
async fn background_children_do_not_hold_up_the_parent() {
    let _guard = container();
    let started = Instant::now();

    let result = Process::run("sleep 5 & echo started").await.unwrap();

    assert_eq!(result.output(), "started\n");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn signals_are_reported_as_exit_codes() {
    let _guard = container();

    let result = Process::run("kill -9 $$").await.unwrap();

    assert_eq!(result.exit_code(), Some(128 + SIGKILL));
    assert!(result.failed());
}

#[tokio::test]
async fn tty_processes_inherit_the_terminal() {
    let _guard = container();

    let result = Process::tty().run("true").await.unwrap();

    assert!(result.successful());
    assert_eq!(result.output(), "");
}

// ----------------------------------------------------------------------
// Asynchronous processes
// ----------------------------------------------------------------------

#[tokio::test]
async fn processes_can_be_started_in_the_background() {
    let _guard = container();

    let mut process = Process::start("echo started; sleep 0.3; echo finished").unwrap();

    assert!(process.id().is_some());
    assert!(process.running());
    assert_eq!(process.command(), "echo started; sleep 0.3; echo finished");

    let mut latest = String::new();
    while process.running() {
        latest.push_str(&process.latest_output());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    latest.push_str(&process.latest_output());

    assert_eq!(latest, "started\nfinished\n");
    assert_eq!(process.output(), "started\nfinished\n");
    assert_eq!(process.latest_output(), "");

    let result = process.wait().await.unwrap();
    assert!(result.successful());
    assert_eq!(result.output(), "started\nfinished\n");
    assert_eq!(process.id(), None);

    // Waiting again hands back the same result...
    assert_eq!(process.wait().await.unwrap(), result);
}

#[tokio::test]
async fn error_output_can_be_read_while_running() {
    let _guard = container();

    let mut process = Process::start("echo oops >&2; sleep 0.2").unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(process.latest_error_output(), "oops\n");
    assert_eq!(process.latest_error_output(), "");
    assert_eq!(process.error_output(), "oops\n");

    process.wait().await.unwrap();
}

#[tokio::test]
async fn output_can_be_retrieved_via_the_start_callback() {
    let _guard = container();
    let output = Arc::new(Mutex::new(Vec::new()));
    let captured = output.clone();

    let mut process = Process::start_with_output("echo one; echo two", move |_, line| {
        captured.lock().unwrap().push(line.to_string());
    })
    .unwrap();

    process.wait().await.unwrap();

    assert_eq!(*output.lock().unwrap(), vec!["one\n", "two\n"]);
}

#[tokio::test]
async fn output_can_be_retrieved_via_the_wait_callback() {
    let _guard = container();
    let mut output = Vec::new();

    let mut process = Process::start("echo one; echo two >&2").unwrap();
    let result = process
        .wait_with_output(|kind, line| output.push(format!("{kind}:{line}")))
        .await
        .unwrap();

    output.sort();
    assert_eq!(output, vec!["err:two\n", "out:one\n"]);
    assert!(result.successful());
}

#[tokio::test]
async fn waiting_can_stop_once_output_matches() {
    let _guard = container();

    let mut process = Process::start("echo Booting; echo Ready...; sleep 10").unwrap();
    let mut seen = Vec::new();

    let snapshot = process
        .wait_until(|_, output| {
            seen.push(output.to_string());
            output == "Ready...\n"
        })
        .await
        .unwrap();

    assert_eq!(seen, vec!["Booting\n", "Ready...\n"]);
    assert_eq!(snapshot.exit_code(), None);
    assert!(process.running());

    assert_eq!(process.stop().await, Some(128 + SIGTERM));
    assert!(!process.running());
}

#[tokio::test]
async fn processes_can_be_signalled() {
    let _guard = container();

    let mut process = Process::start("sleep 10").unwrap();
    process.signal(SIGTERM).unwrap();

    assert!(process.has_received_signal(SIGTERM));
    assert!(!process.has_received_signal(SIGKILL));

    let result = process.wait().await.unwrap();
    assert_eq!(result.exit_code(), Some(143));

    let error = process.signal(SIGTERM).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Cannot send signal on a non running process."
    );
}

#[tokio::test]
async fn processes_can_be_stopped() {
    let _guard = container();

    let mut process = Process::start("sleep 10").unwrap();
    assert!(process.running());

    let started = Instant::now();
    assert_eq!(process.stop().await, Some(143));
    assert!(!process.running());
    assert!(started.elapsed() < Duration::from_secs(5));

    // Processes that ignore the signal are killed after the timeout...
    let mut process = Process::start("trap '' TERM; echo ready; sleep 10").unwrap();
    process
        .wait_until(|_, line| line == "ready\n")
        .await
        .unwrap();
    assert_eq!(
        process.stop_with(Duration::from_millis(200), None).await,
        Some(128 + SIGKILL)
    );
}

#[tokio::test]
async fn started_processes_can_time_out() {
    let _guard = container();

    let mut process = Process::timeout(0.3).start("sleep 5").unwrap();
    assert!(process.ensure_not_timed_out().is_ok());

    let error = process.wait().await.unwrap_err();
    assert!(error.is::<ProcessTimedOutException>());
}

#[tokio::test]
async fn timeouts_can_be_checked_while_polling() {
    let _guard = container();

    let mut process = Process::timeout(0.2).start("sleep 5").unwrap();
    let mut exception = None;

    while process.running() {
        if let Err(timed_out) = process.ensure_not_timed_out() {
            exception = Some(timed_out);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let exception = exception.expect("the process should have timed out");
    assert!(exception.is_general_timeout());
    assert_eq!(exception.result.command(), "sleep 5");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn started_processes_can_be_awaited_on_other_tasks() {
    let _guard = container();

    let mut process = Process::start("echo spawned").unwrap();
    let result = tokio::spawn(async move { process.wait().await })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.output(), "spawned\n");
}

// ----------------------------------------------------------------------
// Pools
// ----------------------------------------------------------------------

#[tokio::test]
async fn pools_run_processes_concurrently() {
    let _guard = container();
    let started = Instant::now();

    let results = Process::concurrently(|pool| {
        pool.command("sleep 0.3; echo 1");
        pool.command("sleep 0.3; echo 2");
        pool.command("sleep 0.3; echo 3");
    })
    .await
    .unwrap();

    assert!(started.elapsed() < Duration::from_millis(900));
    assert!(results.successful());
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].output(), "1\n");
    assert_eq!(results["1"].output(), "2\n");
    assert_eq!(results[2].output(), "3\n");
}

#[tokio::test]
async fn pool_results_can_be_evaluated_by_name() {
    let _guard = container();
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("ProcessTest.php"), "").unwrap();

    let results = Process::pool(|pool| {
        pool.as_("first").path(directory.path()).command("ls");
        pool.as_("second").path(directory.path()).command("ls");
    })
    .run()
    .await
    .unwrap();

    assert!(results["first"].successful());
    assert!(results["second"].see_in_output("ProcessTest.php"));

    let iterated: Vec<(&str, bool)> = results
        .iter()
        .map(|(key, result)| (key, result.successful()))
        .collect();
    assert_eq!(iterated, vec![("first", true), ("second", true)]);
}

#[tokio::test]
async fn failed_pools_report_it() {
    let _guard = container();

    let results = Process::pool(|pool| {
        pool.command("true");
        pool.quietly().command("exit 1;");
    })
    .wait()
    .await
    .unwrap();

    assert!(results.failed());
    assert!(results[0].successful());
    assert!(results[1].clone().throw().is_err());
}

#[tokio::test]
async fn invoked_pools_can_be_inspected() {
    let _guard = container();

    let mut pool = Process::pool(|pool| {
        pool.as_("first").command("sleep 0.2");
        pool.as_("second").command("sleep 0.2");
    })
    .start()
    .unwrap();

    assert_eq!(pool.len(), 2);
    assert!(!pool.is_empty());
    assert_eq!(
        pool.iter().map(|(key, _)| key).collect::<Vec<_>>(),
        vec!["first", "second"]
    );
    assert!(pool.get("first").is_some());
    assert!(pool.get_mut("second").unwrap().id().is_some());
    assert_eq!(pool.running().len(), 2);

    let ids: Vec<Option<u32>> = pool
        .running()
        .into_iter()
        .map(|process| process.id())
        .collect();
    assert!(ids.iter().all(Option::is_some));

    while !pool.running().is_empty() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert!(pool.wait().await.unwrap().successful());
}

#[tokio::test]
async fn pools_can_receive_output_for_each_process() {
    let _guard = container();
    let output = Arc::new(Mutex::new(Vec::new()));
    let captured = output.clone();

    let results = Process::concurrently_with_output(
        |pool| {
            pool.as_("first").command("echo one");
            pool.as_("second").command("echo two >&2");
        },
        move |kind, line, key| {
            captured
                .lock()
                .unwrap()
                .push(format!("{key}:{kind}:{line}"))
        },
    )
    .await
    .unwrap();

    let mut output = output.lock().unwrap().clone();
    output.sort();
    assert_eq!(output, vec!["first:out:one\n", "second:err:two\n"]);
    assert_eq!(results["second"].error_output(), "two\n");
}

#[tokio::test]
async fn pools_can_be_signalled_and_stopped() {
    let _guard = container();

    let mut pool = Process::pool(|pool| {
        pool.command("sleep 10");
        pool.command("sleep 10");
    })
    .start()
    .unwrap();

    assert_eq!(pool.signal(SIGTERM).unwrap(), 2);
    let results = pool.wait().await.unwrap();
    assert_eq!(results[0].exit_code(), Some(143));
    assert_eq!(results[1].exit_code(), Some(143));

    let mut pool = Process::pool(|pool| {
        pool.command("sleep 10");
        pool.command("sleep 10");
    })
    .start()
    .unwrap();

    assert_eq!(pool.stop().await, 2);
    assert!(pool.running().is_empty());
}

#[tokio::test]
async fn pool_processes_need_commands() {
    let _guard = container();

    let error = Process::pool(|pool| {
        pool.path("/tmp");
    })
    .start()
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "A command must be specified before a process can run."
    );
}

// ----------------------------------------------------------------------
// Pipes
// ----------------------------------------------------------------------

#[tokio::test]
async fn pipes_feed_output_into_the_next_process() {
    let _guard = container();

    let result = Process::pipe(|pipe| {
        pipe.command("printf 'Hello, world\\nfoo\\nbar\\n'");
        pipe.command("grep -i \"foo\"");
    })
    .await
    .unwrap();

    assert_eq!(result.output(), "foo\n");
    assert_eq!(result.command(), "grep -i \"foo\"");
}

#[tokio::test]
async fn pipes_can_be_simple_lists_of_commands() {
    let _guard = container();

    let result = Process::pipe_commands(["printf 'b\\na\\nc\\n'", "sort", "head -n 2"])
        .await
        .unwrap();

    assert_eq!(result.output(), "a\nb\n");
}

#[tokio::test]
async fn pipes_stop_at_the_first_failure() {
    let _guard = container();
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("ran");

    let result = Process::pipe(|pipe| {
        pipe.command("echo partial; exit 3");
        pipe.command(format!("touch {}", marker.display()));
    })
    .await
    .unwrap();

    assert!(result.failed());
    assert_eq!(result.exit_code(), Some(3));
    assert!(!marker.exists());
}

#[tokio::test]
async fn pipes_hand_output_to_the_callback_with_keys() {
    let _guard = container();
    let mut seen = Vec::new();

    Process::pipe_with_output(
        |pipe| {
            pipe.as_("first").command("echo laravel");
            pipe.as_("second").command("tr a-z A-Z");
        },
        |kind, output, key| seen.push(format!("{key}:{kind}:{output}")),
    )
    .await
    .unwrap();

    assert_eq!(seen, vec!["first:out:laravel\n", "second:out:LARAVEL\n"]);
}

#[tokio::test]
async fn empty_pipes_are_rejected() {
    let _guard = container();

    let error = Process::pipe(|_| {}).await.unwrap_err();

    assert_eq!(
        error.to_string(),
        "A process pipe must contain at least one process."
    );
}
