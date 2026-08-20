//! Tests for the branch-dependent half of the status boundary.
//!
//! These live apart from `tests.rs` because they need their own probe stubs,
//! and because that module is already at the line ceiling the repository lints
//! enforce.

use std::cell::Cell;
use std::collections::HashMap;

use camino::{Utf8Path, Utf8PathBuf};
use mockable::DefaultClock;
use rstest::rstest;

use super::build_status_report;
use crate::command::{CommandError, CommandOutput, CommandSpec, MockCommandRunner};
use crate::config::StatusArgs;
use crate::git::git_command;
use crate::github::{GitHubClient, GitHubError};
use crate::types::PrNumber;

/// The project directory every probe in these tests is rooted at.
const PROJECT_DIR: &str = "/projects/demo";

/// The glyph the renderer draws before the branch label.
const GLYPH_BRANCH: &str = "\u{f418}";

/// A GitHub client that must never be consulted, counting any call.
struct CountingGitHubClient {
    calls: Cell<usize>,
}

impl CountingGitHubClient {
    const fn new() -> Self {
        Self {
            calls: Cell::new(0),
        }
    }
}

impl GitHubClient for CountingGitHubClient {
    fn pr_number(
        &self,
        _project_dir: &Utf8Path,
        _branch: &str,
    ) -> Result<Option<PrNumber>, GitHubError> {
        self.calls.set(self.calls.get() + 1);
        Ok(Some(PrNumber::new("42")))
    }
}

/// Build the spec one `git` probe rooted at the project directory produces.
fn git_spec(args: &[&str]) -> CommandSpec {
    // Built through `git_command` rather than spelled out, so the stub keys
    // carry whatever hardening the probes carry. Re-spelling them here would
    // make every answer silently stop matching the moment a probe-wide option
    // is added, and the tests would then assert against a repository that
    // answered nothing.
    git_command(Utf8Path::new(PROJECT_DIR), args.iter().copied())
}

/// A runner answering a healthy repository whose `HEAD` is detached.
///
/// Every unlisted spec — the tmux queries and the `origin` lookup among them —
/// exits non-zero, exactly as a probe with no answer would.
fn detached_head_runner() -> MockCommandRunner {
    let answers: HashMap<CommandSpec, String> = [
        (git_spec(&["rev-parse", "--is-inside-work-tree"]), "true"),
        // git's documented way of saying "no current branch".
        (git_spec(&["branch", "--show-current"]), ""),
        (git_spec(&["status", "--porcelain"]), ""),
        (
            git_spec(&["rev-list", "--left-right", "--count", "@{upstream}...HEAD"]),
            "0\t0",
        ),
    ]
    .into_iter()
    .map(|(spec, stdout)| (spec, stdout.to_owned()))
    .collect();

    let mut runner = MockCommandRunner::new();
    runner.expect_run().returning(move |spec| {
        answers.get(spec).map_or(
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

#[rstest]
fn a_detached_head_renders_the_label_without_looking_up_a_pr() {
    let args = StatusArgs {
        project_dir: Some(Utf8PathBuf::from(PROJECT_DIR)),
        ..StatusArgs::default()
    };
    let clock = DefaultClock;
    let github = CountingGitHubClient::new();

    let report = build_status_report(&args, &detached_head_runner(), &clock, &github)
        .expect("a detached HEAD must not fail the command");

    // The rendered contract is unchanged: the branch segment still reads
    // "detached" ...
    assert!(report.line.contains(GLYPH_BRANCH));
    assert!(report.line.contains("detached"));
    // ... but no synthetic branch name reaches the lookup, so nothing is
    // queried and nothing can be cached under an invented key.
    assert_eq!(github.calls.get(), 0);
    assert!(report.diagnostics.pr.is_none());
    // A detached HEAD is an ordinary state, so no git probe is diagnosed.
    assert!(report.diagnostics.git.is_empty());
}
