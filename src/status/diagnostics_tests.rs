//! Coverage for the diagnostics an assembled status line carries.
//!
//! Separated from the cache-outcome cases in [`super::tests`] to stay under
//! the module line cap; the stubs and helpers are shared from there.

use super::tests::{Reply, StubGitHubClient, cache_root, failing_runner};
use super::*;
use camino::Utf8PathBuf;
use mockable::DefaultClock;
use rstest::rstest;
use std::io;
use tempfile::TempDir;

#[rstest]
fn a_status_line_survives_every_probe_failing(cache_root: io::Result<(TempDir, Utf8PathBuf)>) {
    let (_guard, project_dir) = cache_root.expect("cache root");
    let args = StatusArgs {
        project_dir: Some(project_dir),
        ..StatusArgs::default()
    };
    let clock = DefaultClock;
    let github = StubGitHubClient::new(Reply::Found("42"));

    let report = build_status_report(&args, &failing_runner(), &clock, &github)
        .expect("a failing probe must not fail the command");

    // The rendered contract: a project segment and nothing that needs git,
    // tmux, or a PR lookup.
    assert!(!report.line.is_empty());
    assert!(!report.line.contains("\u{f418}"));
    // Without a branch there is nothing to look up, so GitHub is untouched.
    assert_eq!(github.calls.get(), 0);
    assert!(report.diagnostics.pr.is_none());

    let described = report.diagnostics.describe_failures();
    assert!(described.iter().any(|line| line.contains("rev-parse")));
    assert!(
        described
            .iter()
            .any(|line| line.contains("display-message"))
    );
}

#[rstest]
fn diagnostics_are_empty_when_nothing_degrades() {
    assert!(StatusDiagnostics::default().describe_failures().is_empty());
}
