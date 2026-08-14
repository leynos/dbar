//! Tests for git probing: project naming, snapshot parsing, and the typed
//! failure outcomes for a missing repository, malformed output, and command
//! failures.

use super::*;
use crate::command::{CommandError, CommandOutput, CommandSpec, MockCommandRunner};
use rstest::{fixture, rstest};
use std::collections::HashMap;

/// The directory every stubbed probe is rooted at.
const REPO_DIR: &str = "/tmp/repo";

/// Argument lists for the four probes `git_status` runs.
const REPOSITORY_ARGS: &[&str] = &["rev-parse", "--is-inside-work-tree"];
const BRANCH_ARGS: &[&str] = &["branch", "--show-current"];
const PORCELAIN_ARGS: &[&str] = &["status", "--porcelain"];
const UPSTREAM_ARGS: &[&str] = &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"];

/// The canned answers a test wants a [`MockCommandRunner`] to give, keyed by
/// the exact spec each probe is expected to build.
///
/// `git_status` runs several probes whose answers a test refines one at a
/// time, so the expectation is a single `expect_run` whose closure switches on
/// the spec: separate per-spec expectations would be matched in declaration
/// order, and the fixture's defaults are declared before the overrides that
/// are meant to replace them.
#[derive(Default)]
struct Answers {
    outputs: HashMap<CommandSpec, String>,
}

impl Answers {
    /// Answer one probe rooted at `project_dir` with `stdout`, replacing any
    /// previous answer for the same spec.
    fn answering_in(mut self, project_dir: &Utf8Path, args: &[&str], stdout: &str) -> Self {
        self.outputs.insert(
            git_command(project_dir, args.iter().copied()),
            stdout.to_owned(),
        );
        self
    }

    /// Answer one probe rooted at [`REPO_DIR`] with `stdout`.
    fn answering(self, args: &[&str], stdout: &str) -> Self {
        self.answering_in(Utf8Path::new(REPO_DIR), args, stdout)
    }

    /// Drop one probe's answer so the runner reports a command failure.
    fn failing(mut self, args: &[&str]) -> Self {
        self.outputs
            .remove(&git_command(Utf8Path::new(REPO_DIR), args.iter().copied()));
        self
    }

    /// Build a mock runner that serves these answers and fails every other
    /// spec, exactly as a `git` that cannot answer the probe would.
    fn build(self) -> MockCommandRunner {
        let outputs = self.outputs;
        let mut runner = MockCommandRunner::new();
        runner.expect_run().returning(move |spec| {
            outputs.get(spec).map_or(
                Err(CommandError::NonZero {
                    status: Some(1),
                    stderr: String::new(),
                }),
                |stdout| {
                    Ok(CommandOutput {
                        stdout: stdout.clone(),
                    })
                },
            )
        });
        runner
    }
}

/// Answers covering all four probes with healthy defaults.
#[fixture]
fn healthy_runner() -> Answers {
    Answers::default()
        .answering(REPOSITORY_ARGS, "true")
        .answering(BRANCH_ARGS, "main\n")
        .answering(PORCELAIN_ARGS, "")
        .answering(UPSTREAM_ARGS, "0\t0")
}

/// The project directory the fixture's probes are rooted at.
fn repo_dir() -> &'static Utf8Path {
    Utf8Path::new(REPO_DIR)
}

/// Assert an outcome is `Available` and hand back the report.
fn expect_report(outcome: GitStatusOutcome) -> GitStatusReport {
    match outcome {
        GitStatusOutcome::Available(report) => report,
        other => panic!("expected an available snapshot, got {other:?}"),
    }
}

/// Assert an outcome carries exactly one failure and hand it back.
fn expect_single_failure(outcome: GitStatusOutcome) -> GitProbeFailure {
    let failures = outcome.into_failures();
    match <[GitProbeFailure; 1]>::try_from(failures) {
        Ok([failure]) => failure,
        Err(other) => panic!("expected exactly one failure, got {other:?}"),
    }
}

#[rstest]
#[case("git@github.com:owner/dbar.git", "dbar")]
#[case("https://github.com/owner/alpha", "alpha")]
fn project_name_prefers_origin(#[case] origin: &str, #[case] expected: &str) {
    let runner = Answers::default()
        .answering_in(
            Utf8Path::new("/tmp/demo"),
            &["remote", "get-url", "origin"],
            origin,
        )
        .build();
    let name = project_name(&runner, Utf8Path::new("/tmp/demo"));
    assert_eq!(name.as_ref(), expected);
}

#[test]
fn project_name_falls_back_to_worktree_path() {
    let runner = Answers::default().build();
    let name = project_name(&runner, Utf8Path::new("/tmp/repo.worktrees/feat"));
    assert_eq!(name.as_ref(), "repo");
}

#[rstest]
fn git_status_parses_porcelain_and_counts(healthy_runner: Answers) {
    let runner = healthy_runner
        .answering(PORCELAIN_ARGS, "MM file.txt\n")
        .answering(UPSTREAM_ARGS, "1\t2")
        .build();

    let report = expect_report(git_status(&runner, repo_dir()));
    // The trailing newline from `git` must be trimmed off the branch name.
    assert_eq!(report.status.branch.as_ref(), "main");
    assert!(report.status.dirty);
    assert!(report.status.staged);
    assert_eq!(report.status.ahead.value(), 2);
    assert_eq!(report.status.behind.value(), 1);
    assert!(report.degradations.is_empty());
}

#[rstest]
#[case::empty("")]
#[case::whitespace_only("  \n")]
fn git_status_reports_detached_when_no_branch_is_current(
    healthy_runner: Answers,
    #[case] branch_output: &str,
) {
    let runner = healthy_runner.answering(BRANCH_ARGS, branch_output).build();

    let report = expect_report(git_status(&runner, repo_dir()));
    // A detached HEAD reports no current branch, so the label falls back ...
    assert_eq!(report.status.branch.as_ref(), "detached");
    // ... but that is a legitimate answer, not a degraded probe.
    assert!(report.degradations.is_empty());
}

#[rstest]
fn git_status_reports_not_a_repository(healthy_runner: Answers) {
    let runner = healthy_runner.answering(REPOSITORY_ARGS, "false\n").build();

    let outcome = git_status(&runner, repo_dir());
    assert!(matches!(outcome, GitStatusOutcome::NotARepository));
    // The rendered contract: no git segment at all, and nothing to diagnose.
    assert!(outcome.status().is_none());
    assert!(outcome.into_failures().is_empty());
}

#[rstest]
fn git_status_reports_a_malformed_repository_probe(healthy_runner: Answers) {
    let runner = healthy_runner.answering(REPOSITORY_ARGS, "banana").build();

    let outcome = git_status(&runner, repo_dir());
    // The rendered contract is the same as "not a repository" ...
    assert!(outcome.status().is_none());
    // ... but the cause is now inspectable.
    let failure = expect_single_failure(outcome);
    assert!(matches!(
        &failure,
        GitProbeFailure::MalformedOutput {
            probe: GitProbe::Repository,
            output,
        } if output == "banana"
    ));
}

#[rstest]
fn git_status_reports_a_failed_repository_probe() {
    // An empty answer set fails every command, standing in for a missing `git`.
    let runner = Answers::default().build();
    let outcome = git_status(&runner, repo_dir());
    assert!(outcome.status().is_none());
    let failure = expect_single_failure(outcome);
    assert!(matches!(
        failure,
        GitProbeFailure::CommandFailed {
            probe: GitProbe::Repository,
            ..
        }
    ));
    // The message must name the probe so an operator can act on it.
    assert!(failure.to_string().contains("rev-parse"));
}

#[rstest]
fn git_status_reports_a_failed_branch_probe(healthy_runner: Answers) {
    let runner = healthy_runner.failing(BRANCH_ARGS).build();

    let report = expect_report(git_status(&runner, repo_dir()));
    // The rendered contract is unchanged: the branch still reads "detached".
    assert_eq!(report.status.branch.as_ref(), "detached");
    assert!(matches!(
        report.degradations.as_slice(),
        [GitProbeFailure::CommandFailed {
            probe: GitProbe::Branch,
            ..
        }]
    ));
}

#[rstest]
fn git_status_reports_a_failed_worktree_status_probe(healthy_runner: Answers) {
    let runner = healthy_runner.failing(PORCELAIN_ARGS).build();

    let report = expect_report(git_status(&runner, repo_dir()));
    assert!(!report.status.dirty);
    assert!(!report.status.staged);
    assert!(matches!(
        report.degradations.as_slice(),
        [GitProbeFailure::CommandFailed {
            probe: GitProbe::WorktreeStatus,
            ..
        }]
    ));
}

#[rstest]
fn git_status_reports_a_malformed_porcelain_line(healthy_runner: Answers) {
    // The single-character line cannot carry both status characters; the
    // well-formed line after it must still be classified.
    let runner = healthy_runner
        .answering(PORCELAIN_ARGS, "M\n M file.txt\n")
        .build();

    let report = expect_report(git_status(&runner, repo_dir()));
    assert!(report.status.dirty);
    assert!(!report.status.staged);
    assert!(matches!(
        report.degradations.as_slice(),
        [GitProbeFailure::MalformedOutput {
            probe: GitProbe::WorktreeStatus,
            output,
        }] if output == "M"
    ));
}

#[rstest]
fn git_status_reports_a_failed_upstream_probe(healthy_runner: Answers) {
    let runner = healthy_runner.failing(UPSTREAM_ARGS).build();

    let report = expect_report(git_status(&runner, repo_dir()));
    assert_eq!(report.status.ahead.value(), 0);
    assert_eq!(report.status.behind.value(), 0);
    assert!(matches!(
        report.degradations.as_slice(),
        [GitProbeFailure::CommandFailed {
            probe: GitProbe::UpstreamCounts,
            ..
        }]
    ));
}

#[rstest]
#[case::single_field("3")]
#[case::non_numeric("one\ttwo")]
#[case::empty("")]
fn git_status_reports_malformed_upstream_counts(healthy_runner: Answers, #[case] stdout: &str) {
    let runner = healthy_runner.answering(UPSTREAM_ARGS, stdout).build();

    let report = expect_report(git_status(&runner, repo_dir()));
    assert_eq!(report.status.ahead.value(), 0);
    assert_eq!(report.status.behind.value(), 0);
    assert!(matches!(
        report.degradations.as_slice(),
        [GitProbeFailure::MalformedOutput {
            probe: GitProbe::UpstreamCounts,
            ..
        }]
    ));
}
