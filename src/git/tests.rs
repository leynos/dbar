//! Tests for git probing: project naming, snapshot parsing, and the typed
//! failure outcomes for a missing repository, malformed output, and command
//! failures.

use super::*;
use crate::command::{CommandError, CommandOutput, CommandSpec, MockCommandRunner};
use rstest::{fixture, rstest};
use std::collections::HashMap;
use std::io;

/// The directory every stubbed probe is rooted at.
const REPO_DIR: &str = "/tmp/repo";

/// Argument lists for the four probes `git_status` runs.
const REPOSITORY_ARGS: &[&str] = &["rev-parse", "--is-inside-work-tree"];
const BRANCH_ARGS: &[&str] = &["branch", "--show-current"];
const PORCELAIN_ARGS: &[&str] = &["status", "--porcelain"];
const UPSTREAM_ARGS: &[&str] = &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"];
/// Arguments for the project-name probe.
const ORIGIN_ARGS: &[&str] = &["remote", "get-url", "origin"];

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
    expect_single_failure_in(outcome.into_failures())
}

/// Assert a failure list holds exactly one entry and hand it back.
fn expect_single_failure_in(failures: Vec<GitProbeFailure>) -> GitProbeFailure {
    match <[GitProbeFailure; 1]>::try_from(failures) {
        Ok([failure]) => failure,
        Err(other) => panic!("expected exactly one failure, got {other:?}"),
    }
}

#[rstest]
#[case("git@github.com:owner/dbar.git", "dbar")]
#[case("https://github.com/owner/alpha", "alpha")]
// Only the suffix git itself appends is removed. A repository genuinely named
// `beta.git` is cloned from `beta.git.git`, and stripping repeatedly would
// render it as `beta`.
#[case("https://github.com/owner/beta.git.git", "beta.git")]
// The same name without the appended suffix keeps every character.
#[case("git@github.com:owner/gamma.git.git", "gamma.git")]
// A dotted name that merely ends in the four characters must survive intact.
#[case("https://github.com/owner/delta.github", "delta.github")]
fn project_name_prefers_origin(#[case] origin: &str, #[case] expected: &str) {
    let runner = Answers::default()
        .answering_in(Utf8Path::new("/tmp/demo"), ORIGIN_ARGS, origin)
        .build();
    let outcome = project_name(&runner, Utf8Path::new("/tmp/demo"));
    assert_eq!(outcome.name.as_ref(), expected);
    assert!(outcome.failure.is_none());
}

#[rstest]
fn project_name_treats_a_missing_origin_as_an_ordinary_case() {
    // The empty answer set answers every spec with a non-zero exit, which is
    // how git reports both "no such remote" and "not a repository".
    let runner = Answers::default().build();
    let outcome = project_name(&runner, Utf8Path::new("/tmp/demo"));
    assert_eq!(outcome.name.as_ref(), "demo");
    // git answered, so there is nothing to diagnose.
    assert!(outcome.into_failures().is_empty());
}

#[rstest]
fn project_name_reports_an_unrunnable_origin_probe() {
    // An I/O error is what a missing `git` binary looks like, as opposed to a
    // `git` that ran and reported no origin.
    let mut runner = MockCommandRunner::new();
    runner
        .expect_run()
        .times(1..)
        .returning(|_| Err(CommandError::Io(io::Error::other("no git"))));

    let outcome = project_name(&runner, Utf8Path::new("/tmp/demo"));
    // The rendered contract is unchanged: the directory name still wins ...
    assert_eq!(outcome.name.as_ref(), "demo");
    // ... but the failure behind it is no longer invisible.
    let failure = expect_single_failure_in(outcome.into_failures());
    assert!(matches!(
        failure,
        GitProbeFailure::CommandFailed {
            probe: GitProbe::OriginUrl,
            ..
        }
    ));
    assert!(failure.to_string().contains("remote get-url origin"));
}

#[rstest]
#[case::trailing_separator("https://github.com/")]
#[case::bare_separator("/")]
fn project_name_reports_an_origin_url_naming_nothing(#[case] origin: &str) {
    let runner = Answers::default()
        .answering_in(Utf8Path::new("/tmp/demo"), ORIGIN_ARGS, origin)
        .build();

    let outcome = project_name(&runner, Utf8Path::new("/tmp/demo"));
    assert_eq!(outcome.name.as_ref(), "demo");
    let failure = expect_single_failure_in(outcome.into_failures());
    assert!(matches!(
        failure,
        GitProbeFailure::MalformedOutput {
            probe: GitProbe::OriginUrl,
            ..
        }
    ));
}

#[rstest]
// The marker suffixed onto the project directory itself.
#[case::suffixed_marker("/tmp/repo.worktrees/feat", "repo")]
// The marker as its own directory: the segment before it ends in a separator,
// which must not be read as an empty project name.
#[case::marker_as_a_directory("/tmp/project/.worktrees/branch", "project")]
// Nothing precedes the marker, so there is no name to recover and the final
// path component stands in.
#[case::marker_at_the_root("/.worktrees/branch", "branch")]
#[case::marker_first("/tmp/.worktrees/branch", "tmp")]
fn project_name_falls_back_to_worktree_path(#[case] dir: &str, #[case] expected: &str) {
    let runner = Answers::default().build();
    let outcome = project_name(&runner, Utf8Path::new(dir));
    assert_eq!(outcome.name.as_ref(), expected);
    assert!(outcome.failure.is_none());
}

#[rstest]
fn git_status_parses_porcelain_and_counts(healthy_runner: Answers) {
    let runner = healthy_runner
        .answering(PORCELAIN_ARGS, "MM file.txt\n")
        .answering(UPSTREAM_ARGS, "1\t2")
        .build();

    let report = expect_report(git_status(&runner, repo_dir()));
    // The trailing newline from `git` must be trimmed off the branch name.
    assert_eq!(
        report.status.branch.as_ref().map(BranchName::as_ref),
        Some("main")
    );
    assert!(report.status.dirty);
    assert!(report.status.staged);
    assert_eq!(report.status.ahead.value(), 2);
    assert_eq!(report.status.behind.value(), 1);
    assert!(report.degradations.is_empty());
}

#[rstest]
#[case::empty("")]
#[case::whitespace_only("  \n")]
fn git_status_reports_no_branch_when_head_is_detached(
    healthy_runner: Answers,
    #[case] branch_output: &str,
) {
    let runner = healthy_runner.answering(BRANCH_ARGS, branch_output).build();

    let report = expect_report(git_status(&runner, repo_dir()));
    // No name is invented: the `detached` label is the renderer's business, so
    // a real branch called `detached` stays distinguishable from this ...
    assert!(report.status.branch.is_none());
    // ... and this is a legitimate answer, not a degraded probe.
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
    // ... but the fixed failure category remains inspectable.
    let failure = expect_single_failure(outcome);
    assert!(matches!(
        &failure,
        GitProbeFailure::MalformedOutput {
            probe: GitProbe::Repository,
        }
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
    // The rendered contract is unchanged: with no branch the renderer still
    // draws "detached".
    assert!(report.status.branch.is_none());
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
        }]
    ));
}

#[rstest]
fn malformed_origin_diagnostics_never_retain_credentials() {
    let origin = "https://user:secret@example.test/";
    let runner = Answers::default()
        .answering_in(Utf8Path::new("/tmp/demo"), ORIGIN_ARGS, origin)
        .build();
    let failure =
        expect_single_failure_in(project_name(&runner, Utf8Path::new("/tmp/demo")).into_failures());
    assert!(!failure.to_string().contains("secret"));
    assert!(!format!("{failure:?}").contains("secret"));
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
