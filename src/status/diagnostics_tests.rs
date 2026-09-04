//! Coverage for the diagnostics an assembled status line carries.
//!
//! Separated from the cache-outcome cases in [`super::tests`] to stay under
//! the module line cap; the stubs and helpers are shared from there.

use super::tests::{cache_root, failing_runner};
use super::*;
use crate::cache::FileCacheStorage;
use camino::Utf8PathBuf;
use mockable::DefaultClock;
use rstest::rstest;
use std::io;
use tempfile::TempDir;

#[rstest]
fn a_status_line_survives_every_probe_failing(cache_root: io::Result<(TempDir, Utf8PathBuf)>) {
    let (_guard, project_dir) = cache_root.expect("cache root");
    let args = StatusArgs {
        project_dir: Some(project_dir.clone()),
        ..StatusArgs::default()
    };
    let clock = DefaultClock;
    let runner = failing_runner();
    let dependencies = StatusDependencies {
        runner: &runner,
        clock: &clock,
        cache: &FileCacheStorage,
    };
    let report = build_status_report(&args, &project_dir, &dependencies)
        .expect("a failing probe must not fail the command");

    // The rendered contract: a project segment and nothing that needs git,
    // tmux, or a PR lookup.
    assert!(!report.line.is_empty());
    assert!(!report.line.contains("\u{f418}"));
    // Without a branch there is nothing to look up in the cache.
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
