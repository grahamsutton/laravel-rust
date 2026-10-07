use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use illuminate_config::Repository;
use illuminate_console::prelude::*;
use illuminate_console::scheduling::{EventMutex, InMemoryEventMutex};
use illuminate_console::testing::artisan;
use illuminate_console::{ConsoleServiceProvider, Output};
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_support::{Carbon, json};

fn container() -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(
        json!({"app": {"env": "testing", "timezone": "UTC"}}),
    ));
    ConsoleServiceProvider.register(&container);
    (container, guard)
}

#[tokio::test]
async fn the_facade_registers_events_in_the_container() {
    let (_container, _guard) = container();

    Schedule::command("inspire").hourly();
    Schedule::command_with("emails:send", ["Taylor", "--force"]).daily();
    Schedule::exec("node /home/forge/script.js").daily_at("13:00");
    Schedule::call(|| async {}).weekly_on(Schedule::MONDAY, "8:00");

    let events = Schedule::events();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].expression(), "0 * * * *");
    assert_eq!(
        events[1].get_command().as_deref(),
        Some("emails:send Taylor --force")
    );
    assert_eq!(events[2].expression(), "0 13 * * *");
    assert_eq!(events[3].expression(), "0 8 * * 1");
    assert_eq!(events[0].get_timezone().as_deref(), Some("UTC"));

    // Closure commands may be scheduled directly...
    Artisan::command("delete:recent-users", |_| async { Ok(()) })
        .purpose("Delete recent users")
        .schedule()
        .daily();
    assert_eq!(
        Schedule::events()[4].get_command().as_deref(),
        Some("delete:recent-users")
    );
}

#[tokio::test]
async fn schedule_run_runs_due_events() {
    let (_container, _guard) = container();
    let runs = Arc::new(AtomicUsize::new(0));

    Artisan::command("inspire", |cmd| async move {
        cmd.line("Simplicity is the ultimate sophistication.");
        Ok(())
    });

    let counter = runs.clone();
    Schedule::call(move || {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    })
    .name("Count runs");

    Schedule::command("inspire").every_minute();
    Schedule::command("inspire").environments(["production"]);
    Schedule::command("inspire").every_minute().when(|| false);

    let result = artisan("schedule:run").assert_successful().await;

    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert!(result.output.contains(" Running [Count runs] "));
    assert!(result.output.contains(" Running [artisan inspire] "));
    assert_eq!(result.output.matches(" DONE\n").count(), 2);
    assert!(result.output.contains("  ⇂ artisan inspire\n"));
}

#[tokio::test]
async fn schedule_run_says_when_nothing_is_due() {
    let (_container, _guard) = container();

    artisan("schedule:run")
        .expects_output_to_contain("INFO  No scheduled commands are ready to run.")
        .await;

    artisan("schedule:run --whisper")
        .doesnt_expect_any_output()
        .await;
}

#[tokio::test]
async fn schedule_run_reports_failures() {
    let (_container, _guard) = container();
    let reported = Arc::new(Mutex::new(Vec::new()));
    let log = reported.clone();
    Artisan::application()
        .report_exceptions_using(move |error| log.lock().unwrap().push(error.to_string()));

    Artisan::command("fails", |cmd| async move { cmd.exit(3) });
    Schedule::command("fails");
    Schedule::call(|| async { Err::<(), _>(std::io::Error::other("disk full")) }).name("Backup");

    let failures = Arc::new(AtomicUsize::new(0));
    let counter = failures.clone();
    Schedule::call(|| async { false }).on_failure(move || {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let result = artisan("schedule:run").await;

    assert_eq!(result.output.matches(" FAIL\n").count(), 3);
    assert_eq!(failures.load(Ordering::SeqCst), 1);
    let reported = reported.lock().unwrap();
    assert!(reported.contains(&"Scheduled command [fails] failed with exit code [3].".to_string()));
    assert!(reported.contains(&"disk full".to_string()));
}

#[tokio::test]
async fn events_run_hooks_and_prevent_overlapping() {
    let (_container, _guard) = container();
    let log = Arc::new(Mutex::new(Vec::<String>::new()));

    let (before, after, success) = (log.clone(), log.clone(), log.clone());
    let event = Schedule::call(|| async {})
        .name("hooks")
        .without_overlapping()
        .before(move || {
            let log = before.clone();
            async move { log.lock().unwrap().push("before".into()) }
        })
        .then(move || {
            let log = after.clone();
            async move { log.lock().unwrap().push("after".into()) }
        })
        .on_success(move || {
            let log = success.clone();
            async move { log.lock().unwrap().push("success".into()) }
        });

    event.run(&Artisan::application()).await.unwrap();
    assert_eq!(*log.lock().unwrap(), vec!["before", "after", "success"]);
    assert_eq!(event.exit_code(), Some(0));
    assert!(!event.has_mutex().await);

    // While the mutex is held, the event is skipped.
    let mutex = event.mutex();
    assert!(mutex.create(&event).await);
    assert!(!event.filters_pass(&Carbon::now()).await);
    event.run(&Artisan::application()).await.unwrap();
    assert!(event.skipped_because_overlapping());
    assert_eq!(log.lock().unwrap().len(), 3);

    mutex.forget(&event).await;
    assert!(event.filters_pass(&Carbon::now()).await);
}

#[tokio::test]
async fn background_events_finish_later() {
    let (_container, _guard) = container();
    let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
    let sender = Arc::new(Mutex::new(Some(sender)));

    let event = Schedule::call(|| async {})
        .run_in_background()
        .then(move || {
            let sender = sender.clone();
            async move {
                if let Some(sender) = sender.lock().unwrap().take() {
                    let _ = sender.send(());
                }
            }
        });

    event.run(&Artisan::application()).await.unwrap();
    receiver.await.unwrap();
    assert_eq!(event.exit_code(), Some(0));
}

#[tokio::test]
async fn exec_events_run_shell_commands() {
    let (_container, _guard) = container();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("output.log");

    let event = Schedule::exec("echo scheduled").send_output_to(&path);
    event.run(&Artisan::application()).await.unwrap();

    assert_eq!(event.exit_code(), Some(0));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "scheduled\n");

    let failing = Schedule::exec("exit 4");
    failing.run(&Artisan::application()).await.unwrap();
    assert_eq!(failing.exit_code(), Some(4));
}

#[tokio::test]
async fn command_events_append_output() {
    let (_container, _guard) = container();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("inspire.log");

    Artisan::command("inspire", |cmd| async move {
        cmd.line("Quote");
        Ok(())
    });

    let event = Schedule::command("inspire").append_output_to(&path);
    event.run(&Artisan::application()).await.unwrap();
    event.run(&Artisan::application()).await.unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Quote\nQuote\n");
}

#[tokio::test]
async fn schedule_list_displays_the_tasks() {
    let (_container, _guard) = container();

    Schedule::command("inspire --force").hourly();
    Schedule::call(|| async {}).daily();
    Schedule::call(|| async {})
        .daily_at("13:00")
        .description("Send the newsletter");

    let result = artisan("schedule:list").assert_successful().await;
    let lines: Vec<&str> = result.output.lines().collect();

    assert_eq!(lines[0], "");
    assert!(lines[1].starts_with("  0 *  * * *  artisan inspire --force ...."));
    assert!(lines[1].contains("... Next Due: "));
    assert!(lines[1].ends_with(" from now"));
    assert_eq!(lines[1].chars().count(), 78);
    assert!(lines[2].starts_with("  0 0  * * *  Closure at: "));
    assert!(lines[2].contains("scheduling.rs:"));
    assert!(lines[3].starts_with("  0 13 * * *  Send the newsletter ...."));
    assert_eq!(lines[4], "");

    let result = artisan("schedule:list -v").await;
    assert!(result.output.contains("⇁ Send the newsletter"));
    assert!(result.output.contains("Next Due: 20"));

    let result = artisan("schedule:list --json").await;
    let rows: serde_json::Value = serde_json::from_str(result.output.trim()).unwrap();
    assert_eq!(rows[0]["expression"], json!("0 * * * *"));
    assert_eq!(rows[0]["command"], json!("artisan inspire --force"));
    assert_eq!(rows[2]["description"], json!("Send the newsletter"));
    assert_eq!(rows[0]["timezone"], json!("UTC"));
}

#[tokio::test]
async fn schedule_list_handles_empty_schedules() {
    let (_container, _guard) = container();

    artisan("schedule:list")
        .expects_output_to_contain("INFO  No scheduled tasks have been defined.")
        .await;

    artisan("schedule:list --json").expects_output("[]").await;
}

#[tokio::test]
async fn schedule_list_filters_by_environment_and_sorts() {
    let (_container, _guard) = container();

    Schedule::command("yearly").yearly();
    Schedule::command("staging")
        .every_minute()
        .environments(["staging"]);

    // Events without environment constraints run everywhere...
    let result = artisan("schedule:list --environment=production").await;
    assert!(result.output.contains("artisan yearly"));
    assert!(!result.output.contains("artisan staging"));

    let result = artisan("schedule:list --next").await;
    let staging = result.output.find("artisan staging").unwrap();
    let yearly = result.output.find("artisan yearly").unwrap();
    assert!(staging < yearly);
}

#[tokio::test]
async fn schedule_test_runs_a_chosen_event() {
    let (_container, _guard) = container();
    let runs = Arc::new(AtomicUsize::new(0));

    Artisan::command("inspire", |cmd| async move {
        cmd.line("Quote");
        Ok(())
    });
    Schedule::command("inspire").yearly();

    let counter = runs.clone();
    Schedule::call(move || {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    })
    .name("Count");

    artisan("schedule:test --name=inspire")
        .expects_output_to_contain("Running [artisan inspire]")
        .expects_output_to_contain("  ⇂ artisan inspire")
        .assert_successful()
        .await;

    artisan("schedule:test")
        .expects_choice(
            "Which command would you like to run?",
            "Count",
            ["artisan inspire", "Count"],
        )
        .expects_output_to_contain("Running [Count]")
        .await;
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    artisan("schedule:test --name=nope")
        .expects_output_to_contain("No matching scheduled command found.")
        .await;
}

#[tokio::test]
async fn the_schedule_uses_bound_mutexes() {
    let container = Arc::new(Container::new());
    let _guard = Container::set_local_instance(container.clone());
    let mutex = Arc::new(InMemoryEventMutex::new());
    let shared: Arc<dyn EventMutex> = mutex.clone();
    container.instance_arc::<dyn EventMutex>(shared);
    ConsoleServiceProvider.register(&container);

    let event = Schedule::command("x").without_overlapping();
    assert!(mutex.create(&event).await);
    assert!(event.has_mutex().await);
}

#[tokio::test]
async fn schedule_work_is_registered() {
    let (_container, _guard) = container();
    let output = Output::buffered();
    let code = Artisan::call_with_output("help schedule:work", (), &output)
        .await
        .unwrap();
    assert_eq!(code, 0);
    assert!(output.contents().contains("Start the schedule worker"));
    assert!(
        output
            .contents()
            .contains("--run-output-file[=RUN-OUTPUT-FILE]")
    );
}

mod sub_minute_and_signals {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use illuminate_console::prelude::*;
    use illuminate_console::scheduling::ScheduleOutputMailer;
    use illuminate_console::scheduling::events::{
        ScheduledTaskFailed, ScheduledTaskFinished, ScheduledTaskSkipped, ScheduledTaskStarting,
    };
    use illuminate_console::testing::artisan;
    use illuminate_container::Container;
    use illuminate_events::Event;
    use illuminate_http_client::Http;
    use illuminate_support::{Carbon, Result, Sleep};

    use super::container;

    /// Freeze "now" at the start of a minute, and let fake sleeps advance it.
    fn at_the_start_of_a_minute() {
        Sleep::fake();
        Carbon::set_thread_test_now(Some(Carbon::parse("2024-03-11 13:00:00").unwrap()));
        Sleep::sync_with_carbon(true);
    }

    fn counter() -> (Arc<AtomicUsize>, impl Fn() -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static) {
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = runs.clone();
        (runs, move || {
            let counter = counter.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
            })
        })
    }

    #[tokio::test]
    async fn sub_minute_tasks_repeat_until_the_end_of_the_minute() {
        let (_container, _guard) = container();
        at_the_start_of_a_minute();
        let (runs, count) = counter();

        let event = Schedule::call(count).every_ten_seconds();
        assert!(event.is_repeatable());
        assert_eq!(event.repeat_seconds(), Some(10));
        assert_eq!(event.expression(), "* * * * *");

        artisan("schedule:run").assert_successful().await;

        // At :00, :10, :20, :30, :40 and :50.
        assert_eq!(runs.load(Ordering::SeqCst), 6);
        Carbon::set_thread_test_now(None);
        Sleep::stop_faking();
    }

    #[tokio::test]
    async fn the_schedule_can_be_interrupted() {
        let (_container, _guard) = container();
        at_the_start_of_a_minute();
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = runs.clone();

        Schedule::call(move || {
            let counter = counter.clone();
            async move {
                if counter.fetch_add(1, Ordering::SeqCst) == 2 {
                    artisan("schedule:interrupt")
                        .expects_output_to_contain("Broadcasting schedule interrupt signal.")
                        .await;
                }
            }
        })
        .every_second();

        artisan("schedule:run").assert_successful().await;

        assert_eq!(runs.load(Ordering::SeqCst), 3);
        Carbon::set_thread_test_now(None);
        Sleep::stop_faking();
    }

    #[tokio::test]
    async fn paused_schedules_skip_tasks() {
        let (_container, _guard) = container();
        let skipped = Arc::new(AtomicUsize::new(0));
        let log = skipped.clone();
        Event::listen(move |_: &ScheduledTaskSkipped| {
            log.fetch_add(1, Ordering::SeqCst);
            async {}
        });
        let (paused_runs, paused_count) = counter();
        let (eager_runs, eager_count) = counter();
        Schedule::call(paused_count);
        Schedule::call(eager_count).even_when_paused();

        artisan("schedule:pause")
            .expects_output_to_contain("Scheduled task processing has been paused.")
            .assert_successful()
            .await;
        artisan("schedule:run").await;

        assert_eq!(paused_runs.load(Ordering::SeqCst), 0);
        assert_eq!(eager_runs.load(Ordering::SeqCst), 1);
        assert_eq!(skipped.load(Ordering::SeqCst), 1);

        artisan("schedule:continue")
            .expects_output_to_contain("Scheduled task processing has resumed.")
            .await;
        artisan("schedule:run").await;
        assert_eq!(paused_runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn pausing_can_be_disabled() {
        let (_container, _guard) = container();
        Schedule::instance().without_interruption_polling();

        artisan("schedule:pause")
            .expects_output_to_contain("Schedule pausing is currently disabled.")
            .assert_failed()
            .await;
    }

    #[tokio::test]
    async fn output_is_handed_to_hooks() {
        let (_container, _guard) = container();
        let outputs = Arc::new(Mutex::new(Vec::new()));
        Artisan::command("inspire", |cmd| async move {
            cmd.line("Simplicity is the ultimate sophistication.");
            Ok(())
        });

        let log = outputs.clone();
        let failures = outputs.clone();
        Schedule::command("inspire")
            .then_with_output(move |output| {
                let log = log.clone();
                async move { log.lock().unwrap().push(format!("then: {}", output.trim())) }
            })
            .on_failure_with_output(move |output| {
                let log = failures.clone();
                async move { log.lock().unwrap().push(format!("failure: {output}")) }
            });

        artisan("schedule:run").await;

        assert_eq!(
            *outputs.lock().unwrap(),
            vec!["then: Simplicity is the ultimate sophistication."]
        );
    }

    #[tokio::test]
    async fn tasks_ping_urls() {
        let (_container, _guard) = container();
        Http::fake();
        Schedule::call(|| async {})
            .ping_before("https://beats.example.com/before")
            .then_ping("https://beats.example.com/after")
            .ping_on_success("https://beats.example.com/success")
            .ping_on_failure("https://beats.example.com/failure")
            .ping_before_if(false, "https://beats.example.com/never");

        artisan("schedule:run").await;

        Http::assert_sent_in_order(&[
            "https://beats.example.com/before",
            "https://beats.example.com/after",
            "https://beats.example.com/success",
        ]);
        Http::assert_sent_count(3);
    }

    #[derive(Default)]
    struct FakeMailer {
        sent: Mutex<Vec<(Vec<String>, String, String)>>,
    }

    #[async_trait::async_trait]
    impl ScheduleOutputMailer for FakeMailer {
        async fn send(&self, addresses: &[String], subject: &str, output: &str) -> Result<()> {
            self.sent
                .lock()
                .unwrap()
                .push((addresses.to_vec(), subject.to_string(), output.to_string()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn output_is_emailed() {
        let (container, _guard) = container();
        let mailer = Arc::new(FakeMailer::default());
        container.instance_arc::<dyn ScheduleOutputMailer>(mailer.clone());
        Artisan::command("report", |cmd| async move {
            cmd.line("42 new users.");
            Ok(())
        });
        Artisan::command("quiet", |_| async { Ok(()) });

        Schedule::command("report").email_output_to(["taylor@laravel.com"]);
        Schedule::command("report")
            .description("Daily report")
            .email_output_on_failure(["ops@laravel.com"]);
        Schedule::command("quiet").email_output_to(["taylor@laravel.com"]);

        artisan("schedule:run").await;

        let sent = mailer.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, vec!["taylor@laravel.com"]);
        assert_eq!(sent[0].1, "Scheduled Job Output For [report]");
        assert_eq!(sent[0].2.trim(), "42 new users.");
    }

    #[tokio::test]
    async fn task_events_are_dispatched() {
        let (_container, _guard) = container();
        let log = Arc::new(Mutex::new(Vec::new()));
        let (starting, finished, failed) = (log.clone(), log.clone(), log.clone());
        Event::listen(move |event: &ScheduledTaskStarting| {
            starting.lock().unwrap().push(format!("starting {}", event.task.summary_for_display()));
            async {}
        });
        Event::listen(move |event: &ScheduledTaskFinished| {
            finished.lock().unwrap().push(format!("finished {}", event.task.summary_for_display()));
            async {}
        });
        Event::listen(move |event: &ScheduledTaskFailed| {
            failed.lock().unwrap().push(format!("failed {}", event.exception));
            async {}
        });
        Artisan::application().report_exceptions_using(|_| {});
        Schedule::call(|| async {}).name("Prune");
        Schedule::call(|| async { Err::<(), _>(std::io::Error::other("disk full")) }).name("Backup");

        artisan("schedule:run").await;

        assert_eq!(
            *log.lock().unwrap(),
            vec!["starting Prune", "finished Prune", "starting Backup", "failed disk full"]
        );
    }

    #[tokio::test]
    async fn schedule_list_shows_repeating_tasks() {
        let (_container, _guard) = container();
        Schedule::command("inspire").every_thirty_seconds();

        let result = artisan("schedule:list").await;
        assert!(result.output.contains("30s * * * * *"), "{}", result.output);

        let result = artisan("schedule:list --json").await;
        assert!(result.output.contains(r#""repeat_seconds":30"#), "{}", result.output);
    }

    #[tokio::test]
    async fn schedule_clear_cache_deletes_mutexes() {
        let (_container, _guard) = container();
        let event = Schedule::command("inspire").without_overlapping();

        artisan("schedule:clear-cache")
            .expects_output_to_contain("No mutex files were found.")
            .await;

        assert!(event.mutex().create(&event).await);
        artisan("schedule:clear-cache")
            .expects_output_to_contain("Deleting mutex for [artisan inspire]")
            .await;
        assert!(!event.mutex().exists(&event).await);
        let _ = Container::get_instance();
    }
}
