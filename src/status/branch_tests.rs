//! Tests for the branch-dependent half of the status boundary.
//!
//! These live apart from `tests.rs` because they need their own probe stubs,
//! and because that module is already at the line ceiling the repository lints
//! enforce.

use std::collections::HashMap;

use camino::{Utf8Path, Utf8PathBuf};
use mockable::DefaultClock;
use rstest::rstest;
use tempfile::TempDir;

use super::build_status_report;
use super::pr_cache_path;
use super::tests::{args_with_cache, exists, utf8_path};
use crate::cache;
use crate::command::{CommandError, CommandOutput, CommandSpec, MockCommandRunner};
use crate::config::StatusArgs;
use crate::git::git_command;

/// The project directory every probe in these tests is rooted at.
const PROJECT_DIR: &str = "/projects/demo";

/// The glyph the renderer draws before the branch label.
const GLYPH_BRANCH: &str = "\u{f418}";

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
        answers
            .get(spec)
            .map_or(Err(CommandError::NonZero { status: Some(1) }), |stdout| {
                Ok(CommandOutput {
                    stdout: stdout.clone(),
                })
            })
    });
    runner
}

/// A healthy repository with a named branch and an origin-derived project.
fn named_branch_runner() -> MockCommandRunner {
    let answers: HashMap<CommandSpec, String> = [
        (git_spec(&["rev-parse", "--is-inside-work-tree"]), "true"),
        (
            git_spec(&["branch", "--show-current"]),
            "feature/cache-only",
        ),
        (git_spec(&["status", "--porcelain"]), ""),
        (
            git_spec(&["rev-list", "--left-right", "--count", "@{upstream}...HEAD"]),
            "0\t0",
        ),
        (
            git_spec(&["remote", "get-url", "origin"]),
            "https://github.com/acme/demo.git",
        ),
    ]
    .into_iter()
    .map(|(spec, stdout)| (spec, stdout.to_owned()))
    .collect();
    let mut runner = MockCommandRunner::new();
    runner.expect_run().returning(move |spec| {
        answers.get(spec).map_or_else(
            || Err(CommandError::NonZero { status: Some(1) }),
            |stdout| {
                Ok(CommandOutput {
                    stdout: stdout.clone(),
                })
            },
        )
    });
    runner
}

/// Complete tmux inputs avoid a live fallback while exercising status assembly.
fn status_args(cache_dir: &Utf8Path) -> StatusArgs {
    StatusArgs {
        session: Some("demo".to_owned()),
        window: Some("1".to_owned()),
        pane: Some("%0".to_owned()),
        socket: Some("/tmp/tmux.sock".to_owned()),
        ..args_with_cache(cache_dir)
    }
}

#[rstest]
fn a_detached_head_renders_the_label_without_looking_up_a_pr() {
    let args = StatusArgs {
        project_dir: Some(Utf8PathBuf::from(PROJECT_DIR)),
        ..StatusArgs::default()
    };
    let clock = DefaultClock;
    let report = build_status_report(
        &args,
        Utf8Path::new(PROJECT_DIR),
        &detached_head_runner(),
        &clock,
    )
    .expect("a detached HEAD must not fail the command");

    // The rendered contract is unchanged: the branch segment still reads
    // "detached" ...
    assert!(report.line.contains(GLYPH_BRANCH));
    assert!(report.line.contains("detached"));
    // ... but no synthetic branch name reaches the cache key, so no PR
    // segment is rendered under an invented branch name.
    assert!(report.diagnostics.pr.is_none());
    // A detached HEAD is an ordinary state, so no git probe is diagnosed.
    assert!(report.diagnostics.git.is_empty());
}

#[rstest]
fn status_cache_miss_is_read_only() {
    let cache_root = TempDir::new().expect("temporary cache root");
    let cache_dir = utf8_path(&cache_root).expect("UTF-8 cache path");
    let args = status_args(&cache_dir);
    let clock = DefaultClock;
    let report = build_status_report(
        &args,
        Utf8Path::new(PROJECT_DIR),
        &named_branch_runner(),
        &clock,
    )
    .expect("healthy status assembly");

    let cache_path = pr_cache_path(&cache_dir, Utf8Path::new(PROJECT_DIR), "feature/cache-only");
    assert!(
        report
            .diagnostics
            .pr
            .as_ref()
            .is_some_and(|pr| pr.pr_number.is_none())
    );
    assert!(!exists(&cache_path), "status must not write a cache miss");
}

#[rstest]
fn status_reads_a_preseeded_cache_entry_without_refreshing_it() {
    let cache_root = TempDir::new().expect("temporary cache root");
    let cache_dir = utf8_path(&cache_root).expect("UTF-8 cache path");
    let cache_path = pr_cache_path(&cache_dir, Utf8Path::new(PROJECT_DIR), "feature/cache-only");
    let clock = DefaultClock;
    cache::store_cached_value(&cache_path, &clock, "42").expect("seed cache");

    let report = build_status_report(
        &status_args(&cache_dir),
        Utf8Path::new(PROJECT_DIR),
        &named_branch_runner(),
        &clock,
    )
    .expect("healthy status assembly");

    assert_eq!(
        report
            .diagnostics
            .pr
            .as_ref()
            .and_then(|pr| pr.pr_number.as_ref())
            .map(ToString::to_string)
            .as_deref(),
        Some("42")
    );
    assert!(exists(&cache_path), "status must retain the seeded entry");
}
