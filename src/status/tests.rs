//! Tests for the status boundary: the cache read/write it performs around the
//! PR policy, the typed report it produces for every failure path, clock
//! rendering, and the assembled status line's diagnostics.
//!
//! The sibling `retention_tests` module covers the explicit retention sweep,
//! and shares the stubs and helpers below.

use super::*;
use crate::cache::{self, FileCacheStorage};
use crate::command::CommandFailure;
use crate::command::{CommandError, MockCommandRunner};
use crate::config::ConfigCacheTtlSeconds;
use crate::github::GitHubError;
use crate::types::{CacheTtlSeconds, PrNumber};
use camino::Utf8Path;
use mockable::DefaultClock;
use rstest::{fixture, rstest};
use std::cell::Cell;
use std::io;
use tempfile::TempDir;

/// What a stubbed GitHub lookup should answer with.
#[derive(Debug, Clone, Copy)]
pub(super) enum Reply {
    /// Return this PR number.
    Found(&'static str),
    /// Report that the branch has no PR.
    NoPr,
    /// Fail, standing in for a network or rate-limit error.
    Failure,
}

/// A GitHub client with a canned answer that counts how often it is consulted.
pub(super) struct StubGitHubClient {
    reply: Reply,
    pub(super) calls: Cell<usize>,
}

impl StubGitHubClient {
    pub(super) const fn new(reply: Reply) -> Self {
        Self {
            reply,
            calls: Cell::new(0),
        }
    }
}

impl GitHubClient for StubGitHubClient {
    fn pr_number(
        &self,
        _project_dir: &Utf8Path,
        _branch: &str,
    ) -> Result<Option<PrNumber>, GitHubError> {
        self.calls.set(self.calls.get() + 1);
        match self.reply {
            Reply::Found(value) => Ok(Some(PrNumber::new(value))),
            Reply::NoPr => Ok(None),
            Reply::Failure => Err(GitHubError::Command(CommandFailure::ExitStatus(1))),
        }
    }
}

/// A runner that fails every command, standing in for missing binaries.
///
/// `times(1..)` rather than a bare `returning`: the point of the test that uses
/// it is that the probes really are attempted and their failures absorbed, so a
/// status line assembled without running anything must not pass.
pub(super) fn failing_runner() -> MockCommandRunner {
    let mut runner = MockCommandRunner::new();
    runner
        .expect_run()
        .times(1..)
        .returning(|_| Err(CommandError::NonZero { status: Some(1) }));
    runner
}

/// The project directory every PR lookup in these tests is scoped to.
pub(super) const PROJECT_DIR: &str = "/projects/demo";

/// The branch every PR lookup in these tests is scoped to.
pub(super) const BRANCH: &str = "pr/7";

/// A temporary directory and its UTF-8 path, used as the cache root.
///
/// Both halves are returned because dropping the [`TempDir`] deletes the
/// directory, so a test must hold the guard for as long as it uses the path.
/// A tuple keeps that ownership requirement visible at every call site.
#[fixture]
pub(super) fn cache_root() -> io::Result<(TempDir, Utf8PathBuf)> {
    let dir = TempDir::new()?;
    let path = utf8_path(&dir)?;
    Ok((dir, path))
}

/// Build status arguments pointing at the given cache directory.
pub(super) fn args_with_cache(cache_dir: &Utf8Path) -> StatusArgs {
    StatusArgs {
        cache_dir: Some(cache_dir.to_path_buf()),
        pr_cache_ttl_seconds: Some(ConfigCacheTtlSeconds::new(60)),
        ..StatusArgs::default()
    }
}

/// Convert a temporary directory into a UTF-8 path.
pub(super) fn utf8_path(temp_dir: &TempDir) -> io::Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "temp dir path is not UTF-8"))
}

/// Write raw bytes at a cache path, bypassing the cache's own encoding.
pub(super) fn write_raw(path: &Utf8Path, contents: &str) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    cap_std::fs_utf8::Dir::open_ambient_dir(parent, cap_std::ambient_authority())
        .and_then(|dir| dir.write(name, contents))
}

/// Report whether a path exists, without reaching for `std::fs`.
pub(super) fn exists(path: &Utf8Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let Some(name) = path.file_name() else {
        return false;
    };
    cap_std::fs_utf8::Dir::open_ambient_dir(parent, cap_std::ambient_authority())
        .is_ok_and(|dir| dir.metadata(name).is_ok())
}

/// Run a PR lookup against the given cache directory and GitHub stub.
pub(super) fn lookup(
    args: &StatusArgs,
    github: &dyn GitHubClient,
    clock: &dyn Clock,
) -> PrLookupReport {
    resolve_pr_number(
        &PrLookup {
            cache_dir: args.cache_dir.clone(),
            ttl: args.pr_cache_ttl_or_default(),
            clock,
            github,
            project_dir: Utf8Path::new(PROJECT_DIR),
            branch: BRANCH,
        },
        &FileCacheStorage,
    )
}

/// Render a report's PR number for comparison.
pub(super) fn rendered(report: &PrLookupReport) -> Option<String> {
    report.pr_number.as_ref().map(ToString::to_string)
}

#[rstest]
fn a_fresh_cache_entry_short_circuits_the_lookup(cache_root: io::Result<(TempDir, Utf8PathBuf)>) {
    let (_guard, cache_dir) = cache_root.expect("cache root");
    let clock = DefaultClock;
    let path = pr_cache_path(&cache_dir, Utf8Path::new(PROJECT_DIR), BRANCH);
    cache::store_cached_value(&path, &clock, "42").expect("seed cache");

    let github = StubGitHubClient::new(Reply::Found("99"));
    let report = lookup(&args_with_cache(&cache_dir), &github, &clock);

    assert_eq!(rendered(&report).as_deref(), Some("42"));
    assert!(matches!(report.cache, CacheOutcome::Hit));
    assert!(matches!(report.resolution, PrResolution::FromCache));
    assert!(matches!(
        report.write,
        CacheWriteOutcome::Skipped(PersistSkipReason::ServedFromCache)
    ));
    // A hit must not consult GitHub at all.
    assert_eq!(github.calls.get(), 0);
    assert!(report.describe_failures().is_empty());
}

#[rstest]
fn an_empty_cache_entry_records_that_there_is_no_pr(
    cache_root: io::Result<(TempDir, Utf8PathBuf)>,
) {
    let (_guard, cache_dir) = cache_root.expect("cache root");
    let clock = DefaultClock;
    let path = pr_cache_path(&cache_dir, Utf8Path::new(PROJECT_DIR), BRANCH);
    cache::store_cached_value(&path, &clock, "").expect("seed cache");

    let github = StubGitHubClient::new(Reply::Found("99"));
    let report = lookup(&args_with_cache(&cache_dir), &github, &clock);

    assert!(report.pr_number.is_none());
    assert!(matches!(report.cache, CacheOutcome::Hit));
    assert_eq!(github.calls.get(), 0);
}

#[rstest]
fn a_cache_miss_consults_github_and_stores_the_answer(
    cache_root: io::Result<(TempDir, Utf8PathBuf)>,
) {
    let (_guard, cache_dir) = cache_root.expect("cache root");
    let clock = DefaultClock;
    let github = StubGitHubClient::new(Reply::Found("42"));

    let report = lookup(&args_with_cache(&cache_dir), &github, &clock);

    assert_eq!(rendered(&report).as_deref(), Some("42"));
    assert!(matches!(report.cache, CacheOutcome::Miss));
    assert!(matches!(report.resolution, PrResolution::GitHub));
    assert!(matches!(report.write, CacheWriteOutcome::Stored));
    assert_eq!(github.calls.get(), 1);
    assert!(exists(&pr_cache_path(
        &cache_dir,
        Utf8Path::new(PROJECT_DIR),
        BRANCH
    )));
}

#[rstest]
fn a_branch_fallback_is_cached_when_github_reports_no_pr(
    cache_root: io::Result<(TempDir, Utf8PathBuf)>,
) {
    let (_guard, cache_dir) = cache_root.expect("cache root");
    let clock = DefaultClock;
    let github = StubGitHubClient::new(Reply::NoPr);

    let report = lookup(&args_with_cache(&cache_dir), &github, &clock);

    assert_eq!(rendered(&report).as_deref(), Some("7"));
    assert!(matches!(report.resolution, PrResolution::BranchFallback));
    assert!(matches!(report.write, CacheWriteOutcome::Stored));
}

#[rstest]
fn a_corrupt_cache_entry_is_reported_and_treated_as_a_miss(
    cache_root: io::Result<(TempDir, Utf8PathBuf)>,
) {
    let (_guard, cache_dir) = cache_root.expect("cache root");
    let clock = DefaultClock;
    let path = pr_cache_path(&cache_dir, Utf8Path::new(PROJECT_DIR), BRANCH);
    write_raw(&path, "{ not json").expect("write corrupt entry");

    let github = StubGitHubClient::new(Reply::Found("42"));
    let report = lookup(&args_with_cache(&cache_dir), &github, &clock);

    // The rendered contract is unchanged: the lookup proceeds as for a miss.
    assert_eq!(rendered(&report).as_deref(), Some("42"));
    assert_eq!(github.calls.get(), 1);
    // The read failure is carried into the report rather than discarded.
    assert!(matches!(report.cache, CacheOutcome::ReadFailed(_)));
    assert!(
        report
            .describe_failures()
            .iter()
            .any(|line| line.contains("cache read failed"))
    );
}

#[rstest]
fn a_failed_lookup_falls_back_without_writing_the_cache(
    cache_root: io::Result<(TempDir, Utf8PathBuf)>,
) {
    let (_guard, cache_dir) = cache_root.expect("cache root");
    let clock = DefaultClock;
    let github = StubGitHubClient::new(Reply::Failure);

    let report = lookup(&args_with_cache(&cache_dir), &github, &clock);

    // The branch fallback still applies, so a `pr/7` branch yields 7 ...
    assert_eq!(rendered(&report).as_deref(), Some("7"));
    // ... the lookup must actually have been attempted ...
    assert_eq!(github.calls.get(), 1);
    assert!(matches!(report.resolution, PrResolution::LookupFailed(_)));
    // ... and a failed lookup must not be cached for the whole TTL.
    assert!(matches!(
        report.write,
        CacheWriteOutcome::Skipped(PersistSkipReason::LookupFailed)
    ));
    assert!(!exists(&pr_cache_path(
        &cache_dir,
        Utf8Path::new(PROJECT_DIR),
        BRANCH
    )));
}

#[rstest]
fn an_unavailable_cache_directory_skips_every_cache_access() {
    let clock = DefaultClock;
    let github = StubGitHubClient::new(Reply::Found("42"));
    let args = StatusArgs::default();
    let report = resolve_without_cache(
        &PrLookup {
            cache_dir: args.cache_dir.clone(),
            ttl: args.pr_cache_ttl_or_default(),
            clock: &clock,
            github: &github,
            project_dir: Utf8Path::new(PROJECT_DIR),
            branch: BRANCH,
        },
        CacheOutcome::DirUnavailable(CacheFailure::DirectoryUnavailable),
        &FileCacheStorage,
    );

    assert_eq!(rendered(&report).as_deref(), Some("42"));
    assert!(matches!(report.cache, CacheOutcome::DirUnavailable(_)));
    assert!(matches!(
        report.write,
        CacheWriteOutcome::Skipped(PersistSkipReason::CacheUnavailable)
    ));
    assert!(
        report
            .describe_failures()
            .iter()
            .any(|line| line.contains("cache directory unavailable"))
    );
}

#[rstest]
fn a_failed_cache_write_is_reported(cache_root: io::Result<(TempDir, Utf8PathBuf)>) {
    let (_guard, cache_dir) = cache_root.expect("cache root");
    // A regular file cannot be a parent directory, so creating the entry's
    // parent fails and the write with it.
    let blocker = cache_dir.join("blocker");
    write_raw(&blocker, "not a directory").expect("write blocker");
    let path = blocker.join("nested").join("pr.json");

    let clock = DefaultClock;
    let github = StubGitHubClient::new(Reply::Found("42"));
    let args = args_with_cache(&cache_dir);
    let context = PrLookup {
        cache_dir: args.cache_dir.clone(),
        ttl: args.pr_cache_ttl_or_default(),
        clock: &clock,
        github: &github,
        project_dir: Utf8Path::new(PROJECT_DIR),
        branch: BRANCH,
    };

    let outcome = persist(
        &context,
        Some(&path),
        PersistRequest::Store("42".to_owned()),
        &FileCacheStorage,
    );
    assert!(matches!(
        outcome,
        CacheWriteOutcome::Failed(CacheFailure::Write)
    ));
}

#[rstest]
fn a_cache_io_failure_while_loading_is_classified_as_a_read(
    cache_root: io::Result<(TempDir, Utf8PathBuf)>,
) {
    let (_guard, cache_dir) = cache_root.expect("cache root");
    let blocker = cache_dir.join("not-a-directory");
    write_raw(&blocker, "blocker").expect("write cache blocker");
    let path = blocker.join("entry.json");
    let clock = DefaultClock;
    let failure = CacheReader::load(&FileCacheStorage, &path, &clock, CacheTtlSeconds::new(60))
        .expect_err("a missing cache parent cannot be opened");
    assert_eq!(failure, CacheFailure::Read);
}

#[rstest]
fn a_skipped_write_records_its_reason() {
    let clock = DefaultClock;
    let github = StubGitHubClient::new(Reply::NoPr);
    let args = StatusArgs::default();
    let context = PrLookup {
        cache_dir: args.cache_dir.clone(),
        ttl: args.pr_cache_ttl_or_default(),
        clock: &clock,
        github: &github,
        project_dir: Utf8Path::new(PROJECT_DIR),
        branch: BRANCH,
    };

    let skipped = persist(
        &context,
        None,
        PersistRequest::Skip(PersistSkipReason::ServedFromCache),
        &FileCacheStorage,
    );
    assert!(matches!(
        skipped,
        CacheWriteOutcome::Skipped(PersistSkipReason::ServedFromCache)
    ));

    // A store request with nowhere to store it is a skip, not a failure.
    let nowhere = persist(
        &context,
        None,
        PersistRequest::Store("42".to_owned()),
        &FileCacheStorage,
    );
    assert!(matches!(
        nowhere,
        CacheWriteOutcome::Skipped(PersistSkipReason::CacheUnavailable)
    ));
}
