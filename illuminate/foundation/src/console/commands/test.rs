//! `test` — run the application's tests, the way `php artisan test` does.
//!
//! Cargo builds the test binaries (its progress and any compiler errors are
//! shown as usual), then each binary runs and its results are printed the
//! way Laravel's test runner prints them:
//!
//! ```text
//!    PASS  Tests\Feature\PodcastTest
//!   ✓ podcasts are listed
//!   ✓ podcasts can be queued for processing
//!
//!   Tests:    2 passed
//!   Duration: 0.12s
//! ```

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;

use illuminate_console::{Command, Console, Output, async_trait};
use illuminate_support::{Result, Str};
use serde_json::Value;

use crate::application::Application;

pub struct TestCommand;

#[async_trait]
impl Command for TestCommand {
    fn signature(&self) -> &str {
        "test
            {--filter= : Only run the tests whose names contain the given string}
            {--release : Build and run the tests with optimizations}"
    }

    fn description(&self) -> &str {
        "Run the application tests"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let base = PathBuf::from(Application::current().base_path(""));
        let output = cmd.output().clone();

        let Some(binaries) = build(&base, &output, cmd.option_bool("release")).await? else {
            return cmd.exit(1);
        };

        let filter = cmd.option("filter").filter(|filter| !filter.is_empty());
        let started = Instant::now();
        let mut totals = Totals::default();
        let mut failures = Vec::new();
        for binary in &binaries {
            let run = run(binary, &base, filter.as_deref()).await?;
            if run.tests.is_empty() && run.crashed.is_none() {
                continue;
            }
            print_suites(&output, binary, &run, &mut totals);
            failures.extend(run.failures.into_iter().map(|(name, details)| (binary.suite(&name), describe(&name), details)));
            if let Some(crash) = run.crashed {
                totals.failed += 1;
                failures.push((binary.suite(""), "crashed".to_string(), crash));
            }
        }

        for (suite, test, details) in &failures {
            let rule = "─".repeat(output.width().min(150).saturating_sub(4));
            output.writeln(format!("  <fg=gray>{rule}</>"));
            output.writeln(format!(
                "  <fg=white;bg=red;options=bold> FAILED </> {} <fg=gray>></> {}",
                escape(suite),
                escape(test)
            ));
            for line in details.lines() {
                output.writeln(format!("  {}", escape(line)));
            }
            output.new_line(1);
        }

        output.new_line(1);
        output.writeln(format!("  <fg=gray>Tests:</>    {}", totals.summary()));
        output.writeln(format!("  <fg=gray>Duration:</> {:.2}s", started.elapsed().as_secs_f64()));
        output.new_line(1);

        if totals.failed > 0 { cmd.exit(1) } else { Ok(()) }
    }
}

/// A compiled test binary.
#[derive(Debug, Clone, PartialEq)]
struct TestBinary {
    /// The target's name (`feature`, `unit`, or the crate's name).
    target: String,
    /// The target's kind (`lib`, `bin`, or `test`).
    kind: String,
    executable: PathBuf,
}

impl TestBinary {
    /// The suite a test belongs to, named like a PHPUnit test class:
    /// `podcast_test::podcasts_are_listed` in `tests/feature` belongs to
    /// `Tests\Feature\PodcastTest`.
    fn suite(&self, test: &str) -> String {
        let mut parts: Vec<String> = match self.kind.as_str() {
            "test" => vec!["Tests".into(), Str::studly(&self.target)],
            _ => vec![Str::studly(&self.target)],
        };
        let module = test.rsplit_once("::").map_or("", |(module, _)| module);
        parts.extend(module.split("::").filter(|part| !part.is_empty()).map(Str::studly));
        parts.join("\\")
    }
}

/// Build the test binaries, showing cargo's progress and diagnostics.
/// Returns `None` when the build fails.
async fn build(base: &Path, output: &Output, release: bool) -> Result<Option<Vec<TestBinary>>> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut command = tokio::process::Command::new(cargo);
    command
        .current_dir(base)
        .args(["test", "--no-run", "--message-format=json-render-diagnostics"])
        .arg(if output.is_decorated() { "--color=always" } else { "--color=never" })
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if release {
        command.arg("--release");
    }
    let result = command.output().await?;
    if !result.status.success() {
        return Ok(None);
    }
    Ok(Some(test_binaries(&String::from_utf8_lossy(&result.stdout))))
}

/// The test binaries in cargo's JSON messages: library and binary unit
/// tests first, then the integration tests, `unit` before the others (the
/// order Laravel runs its Unit and Feature suites in).
fn test_binaries(messages: &str) -> Vec<TestBinary> {
    let mut binaries: Vec<TestBinary> = messages
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact" && message["profile"]["test"] == true)
        .filter_map(|message| {
            Some(TestBinary {
                target: message["target"]["name"].as_str()?.to_string(),
                kind: message["target"]["kind"][0].as_str()?.to_string(),
                executable: PathBuf::from(message["executable"].as_str()?),
            })
        })
        .collect();
    binaries.sort_by_key(|binary| match (binary.kind.as_str(), binary.target.as_str()) {
        ("lib", _) => (0, String::new()),
        ("bin", name) => (1, name.to_string()),
        ("test", "unit") => (2, String::new()),
        (_, name) => (3, name.to_string()),
    });
    binaries.dedup_by(|a, b| a.executable == b.executable);
    binaries
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Outcome {
    Passed,
    Failed,
    Ignored,
}

/// The results of running one test binary.
#[derive(Debug, Default)]
struct Run {
    /// Every test, in the order it finished.
    tests: Vec<(String, Outcome)>,
    /// The captured output of each failed test.
    failures: Vec<(String, String)>,
    /// What the binary printed when it died without reporting results.
    crashed: Option<String>,
}

async fn run(binary: &TestBinary, base: &Path, filter: Option<&str>) -> Result<Run> {
    let mut command = tokio::process::Command::new(&binary.executable);
    command.current_dir(base).args(["--format=pretty", "--color=never"]);
    if let Some(filter) = filter {
        command.arg(filter);
    }
    let result = command.stdin(Stdio::null()).output().await?;
    let stdout = String::from_utf8_lossy(&result.stdout);
    let mut run = parse(&stdout);
    if !result.status.success() && run.tests.iter().all(|(_, outcome)| *outcome != Outcome::Failed) {
        let stderr = String::from_utf8_lossy(&result.stderr);
        run.crashed = Some(format!("{}{stderr}", stdout.trim_end()).trim().to_string());
    }
    Ok(run)
}

/// Parse libtest's "pretty" output.
fn parse(stdout: &str) -> Run {
    let mut run = Run::default();
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix("test ") else { continue };
        let Some((name, result)) = rest.rsplit_once(" ... ") else { continue };
        let outcome = match result.trim() {
            "ok" => Outcome::Passed,
            "FAILED" => Outcome::Failed,
            result if result.starts_with("ignored") => Outcome::Ignored,
            _ => continue,
        };
        run.tests.push((name.to_string(), outcome));
    }

    // ---- name stdout ----
    // ...captured output...
    let mut current: Option<(String, Vec<&str>)> = None;
    for line in stdout.lines() {
        if let Some(name) = line.strip_prefix("---- ").and_then(|rest| rest.strip_suffix(" stdout ----")) {
            if let Some((name, lines)) = current.take() {
                run.failures.push((name, lines.join("\n").trim().to_string()));
            }
            current = Some((name.to_string(), Vec::new()));
        } else if line == "failures:" || line.starts_with("test result:") {
            if let Some((name, lines)) = current.take() {
                run.failures.push((name, lines.join("\n").trim().to_string()));
            }
        } else if let Some((_, lines)) = current.as_mut() {
            lines.push(line);
        }
    }
    if let Some((name, lines)) = current.take() {
        run.failures.push((name, lines.join("\n").trim().to_string()));
    }
    run
}

#[derive(Debug, Default)]
struct Totals {
    passed: usize,
    failed: usize,
    skipped: usize,
}

impl Totals {
    fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.failed > 0 {
            parts.push(format!("<fg=red;options=bold>{} failed</>", self.failed));
        }
        if self.skipped > 0 {
            parts.push(format!("<fg=yellow;options=bold>{} skipped</>", self.skipped));
        }
        if self.passed > 0 || parts.is_empty() {
            parts.push(format!("<fg=green;options=bold>{} passed</>", self.passed));
        }
        parts.join("<fg=gray>,</> ")
    }
}

/// Print a binary's tests, grouped into suites in the order they were
/// first seen, the way Laravel prints each test class.
fn print_suites(output: &Output, binary: &TestBinary, run: &Run, totals: &mut Totals) {
    let mut suites: Vec<(String, Vec<(String, Outcome)>)> = Vec::new();
    let mut tests = run.tests.clone();
    tests.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, outcome) in tests {
        let suite = binary.suite(&name);
        match suites.iter_mut().find(|(existing, _)| *existing == suite) {
            Some((_, tests)) => tests.push((name, outcome)),
            None => suites.push((suite, vec![(name, outcome)])),
        }
    }

    for (suite, tests) in suites {
        let failed = tests.iter().any(|(_, outcome)| *outcome == Outcome::Failed);
        output.new_line(1);
        if failed {
            output.writeln(format!("   <fg=white;bg=red;options=bold> FAIL </> {}", escape(&suite)));
        } else {
            output.writeln(format!("   <fg=black;bg=green;options=bold> PASS </> {}", escape(&suite)));
        }
        for (name, outcome) in tests {
            let description = escape(&describe(&name));
            match outcome {
                Outcome::Passed => {
                    totals.passed += 1;
                    output.writeln(format!("  <fg=green;options=bold>✓</> <fg=gray>{description}</>"));
                }
                Outcome::Failed => {
                    totals.failed += 1;
                    output.writeln(format!("  <fg=red;options=bold>⨯</> <fg=red>{description}</>"));
                }
                Outcome::Ignored => {
                    totals.skipped += 1;
                    output.writeln(format!("  <fg=yellow;options=bold>-</> <fg=gray>{description}</>"));
                }
            }
        }
    }
}

/// `podcast_test::test_podcasts_are_listed` → `podcasts are listed`.
fn describe(test: &str) -> String {
    let name = test.rsplit_once("::").map_or(test, |(_, name)| name);
    let name = name.strip_prefix("test_").unwrap_or(name);
    name.replace('_', " ")
}

fn escape(text: &str) -> String {
    illuminate_console::OutputFormatter::escape(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: &str = "
running 3 tests
test podcast_test::podcasts_are_listed ... ok
test podcast_test::a_valid_url_is_required ... FAILED
test example_test::test_the_application_returns_a_successful_response ... ignored, slow

failures:

---- podcast_test::a_valid_url_is_required stdout ----

thread 'podcast_test::a_valid_url_is_required' panicked at tests/feature/podcast_test.rs:12:9:
Expected response status code [302] but received 200.
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    podcast_test::a_valid_url_is_required

test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.10s
";

    #[test]
    fn libtest_output_is_parsed() {
        let run = parse(OUTPUT);
        assert_eq!(
            run.tests,
            vec![
                ("podcast_test::podcasts_are_listed".to_string(), Outcome::Passed),
                ("podcast_test::a_valid_url_is_required".to_string(), Outcome::Failed),
                (
                    "example_test::test_the_application_returns_a_successful_response".to_string(),
                    Outcome::Ignored
                ),
            ]
        );
        assert_eq!(run.failures.len(), 1);
        assert_eq!(run.failures[0].0, "podcast_test::a_valid_url_is_required");
        assert!(run.failures[0].1.starts_with("thread 'podcast_test::a_valid_url_is_required' panicked"));
        assert!(run.failures[0].1.contains("Expected response status code [302] but received 200."));
    }

    #[test]
    fn tests_are_named_like_laravel_tests() {
        let feature = TestBinary { target: "feature".into(), kind: "test".into(), executable: PathBuf::new() };
        assert_eq!(feature.suite("podcast_test::podcasts_are_listed"), "Tests\\Feature\\PodcastTest");
        assert_eq!(feature.suite("it_works"), "Tests\\Feature");
        let lib = TestBinary { target: "podcasts".into(), kind: "lib".into(), executable: PathBuf::new() };
        assert_eq!(lib.suite("app::models::tests::it_casts"), "Podcasts\\App\\Models\\Tests");

        assert_eq!(describe("example_test::test_the_application_returns_a_successful_response"), "the application returns a successful response");
        assert_eq!(describe("podcasts_are_listed"), "podcasts are listed");
    }

    #[test]
    fn test_binaries_are_read_from_cargo_messages() {
        let messages = [
            r#"{"reason":"compiler-artifact","target":{"name":"feature","kind":["test"]},"profile":{"test":true},"executable":"/t/feature-1"}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"podcasts","kind":["lib"]},"profile":{"test":false},"executable":null}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"unit","kind":["test"]},"profile":{"test":true},"executable":"/t/unit-1"}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"podcasts","kind":["lib"]},"profile":{"test":true},"executable":"/t/podcasts-1"}"#,
            r#"{"reason":"build-finished","success":true}"#,
        ]
        .join("\n");
        let targets: Vec<String> = test_binaries(&messages).into_iter().map(|binary| binary.target).collect();
        assert_eq!(targets, ["podcasts", "unit", "feature"]);
    }

    #[test]
    fn totals_are_summarised() {
        assert_eq!(Totals { passed: 4, failed: 0, skipped: 0 }.summary(), "<fg=green;options=bold>4 passed</>");
        assert_eq!(
            Totals { passed: 4, failed: 1, skipped: 1 }.summary(),
            "<fg=red;options=bold>1 failed</><fg=gray>,</> <fg=yellow;options=bold>1 skipped</><fg=gray>,</> <fg=green;options=bold>4 passed</>"
        );
    }
}
