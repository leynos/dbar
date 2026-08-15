//! Tests for the status boundary: the cache read/write it performs around the
//! PR policy, the typed report it produces for every failure path, clock
//! rendering, and the assembled status line's diagnostics.

use super::*;
use crate::command::{CommandError, CommandOutput, CommandSpec};
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
enum Reply {
    /// Return this PR number.
    Found(&'static str),
    /// Report that the branch has no PR.
    NoPr,
    /// Fail, standing in for a network or rate-limit error.
    Failure,
}

/// A GitHub client with a canned answer that counts how often it is consulted.
struct StubGitHubClient {
    reply: Reply,
    calls: Cell<usize>,
}

impl StubGitHubClient {
    const fn new(reply: Reply) -> Self {
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
            Reply::Failure => Err(GitHubError::Command(CommandError::NonZero {
                status: Some(1),
                stderr: "gh failed".to_owned(),
            })),
        }
    }
}

/// A runner that fails every command, standing in for missing binaries.
struct FailingRunner;

impl CommandRunner for FailingRunner {
    fn run(&self, _spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
        Err(CommandError::NonZero {
            status: Some(1),
            stderr: String::new(),
        })
    }
}

/// The project directory every PR lookup in these tests is scoped to.
const PROJECT_DIR: &str = "/projects/demo";

/// The branch every PR lookup in these tests is scoped to.
const BRANCH: &str = "pr/7";

/// A temporary directory used as the cache root.
#[fixture]
fn cache_root() -> io::Result<TempDir> {
    TempDir::new()
}

/// Build status arguments pointing at the given cache directory.
fn args_with_cache(cache_dir: &Utf8Path) -> StatusArgs {
    StatusArgs {
        cache_dir: Some(cache_dir.to_path_buf()),
        pr_cache_ttl_seconds: Some(CacheTtlSeconds::new(60)),
        ..StatusArgs::default()
    }
}

/// Convert a temporary directory into a UTF-8 path.
fn utf8_path(temp_dir: &TempDir) -> io::Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "temp dir path is not UTF-8"))
}

/// Write raw bytes at a cache path, bypassing the cache's own encoding.
fn write_raw(path: &Utf8Path, contents: &str) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    cap_std::fs_utf8::Dir::open_ambient_dir(parent, cap_std::ambient_authority())
        .and_then(|dir| dir.write(name, contents))
}

/// Report whether a path exists, without reaching for `std::fs`.
fn exists(path: &Utf8Path) -> bool {
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
fn lookup(args: &StatusArgs, github: &dyn GitHubClient, clock: &dyn Clock) -> PrLookupReport {
    resolve_pr_number(&PrLookup {
        args,
        clock,
        github,
        project_dir: Utf8Path::new(PROJECT_DIR),
        branch: BRANCH,
    })
}

/// Render a report's PR number for comparison.
fn rendered(report: &PrLookupReport) -> Option<String> {
    report.pr_number.as_ref().map(ToString::to_string)
}

#[rstest]
fn a_fresh_cache_entry_short_circuits_the_lookup(cache_root: io::Result<TempDir>) {
    let temp_dir = cache_root.expect("temp dir");
    let cache_dir = utf8_path(&temp_dir).expect("cache dir");
    let clock = DefaultClock;
    let path = pr_cache_path(&cache_dir, BRANCH, Utf8Path::new(PROJECT_DIR));
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
fn an_empty_cache_entry_records_that_there_is_no_pr(cache_root: io::Result<TempDir>) {
    let temp_dir = cache_root.expect("temp dir");
    let cache_dir = utf8_path(&temp_dir).expect("cache dir");
    let clock = DefaultClock;
    let path = pr_cache_path(&cache_dir, BRANCH, Utf8Path::new(PROJECT_DIR));
    cache::store_cached_value(&path, &clock, "").expect("seed cache");

    let github = StubGitHubClient::new(Reply::Found("99"));
    let report = lookup(&args_with_cache(&cache_dir), &github, &clock);

    assert!(report.pr_number.is_none());
    assert!(matches!(report.cache, CacheOutcome::Hit));
    assert_eq!(github.calls.get(), 0);
}

#[rstest]
fn a_cache_miss_consults_github_and_stores_the_answer(cache_root: io::Result<TempDir>) {
    let temp_dir = cache_root.expect("temp dir");
    let cache_dir = utf8_path(&temp_dir).expect("cache dir");
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
        BRANCH,
        Utf8Path::new(PROJECT_DIR)
    )));
}

#[rstest]
fn a_branch_fallback_is_cached_when_github_reports_no_pr(cache_root: io::Result<TempDir>) {
    let temp_dir = cache_root.expect("temp dir");
    let cache_dir = utf8_path(&temp_dir).expect("cache dir");
    let clock = DefaultClock;
    let github = StubGitHubClient::new(Reply::NoPr);

    let report = lookup(&args_with_cache(&cache_dir), &github, &clock);

    assert_eq!(rendered(&report).as_deref(), Some("7"));
    assert!(matches!(report.resolution, PrResolution::BranchFallback));
    assert!(matches!(report.write, CacheWriteOutcome::Stored));
}

#[rstest]
fn a_corrupt_cache_entry_is_reported_and_treated_as_a_miss(cache_root: io::Result<TempDir>) {
    let temp_dir = cache_root.expect("temp dir");
    let cache_dir = utf8_path(&temp_dir).expect("cache dir");
    let clock = DefaultClock;
    let path = pr_cache_path(&cache_dir, BRANCH, Utf8Path::new(PROJECT_DIR));
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
fn a_failed_lookup_falls_back_without_writing_the_cache(cache_root: io::Result<TempDir>) {
    let temp_dir = cache_root.expect("temp dir");
    let cache_dir = utf8_path(&temp_dir).expect("cache dir");
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
        BRANCH,
        Utf8Path::new(PROJECT_DIR)
    )));
}

#[rstest]
fn an_unavailable_cache_directory_skips_every_cache_access() {
    let clock = DefaultClock;
    let github = StubGitHubClient::new(Reply::Found("42"));
    let args = StatusArgs::default();
    let report = resolve_without_cache(
        &PrLookup {
            args: &args,
            clock: &clock,
            github: &github,
            project_dir: Utf8Path::new(PROJECT_DIR),
            branch: BRANCH,
        },
        CacheOutcome::DirUnavailable(crate::cache::CacheError::MissingBaseDir),
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
fn a_failed_cache_write_is_reported(cache_root: io::Result<TempDir>) {
    let temp_dir = cache_root.expect("temp dir");
    let cache_dir = utf8_path(&temp_dir).expect("cache dir");
    // A regular file cannot be a parent directory, so creating the entry's
    // parent fails and the write with it.
    let blocker = cache_dir.join("blocker");
    write_raw(&blocker, "not a directory").expect("write blocker");
    let path = blocker.join("nested").join("pr.json");

    let clock = DefaultClock;
    let github = StubGitHubClient::new(Reply::Found("42"));
    let args = args_with_cache(&cache_dir);
    let context = PrLookup {
        args: &args,
        clock: &clock,
        github: &github,
        project_dir: Utf8Path::new(PROJECT_DIR),
        branch: BRANCH,
    };

    let outcome = persist(
        &context,
        Some(&path),
        PersistRequest::Store("42".to_owned()),
    );
    assert!(matches!(outcome, CacheWriteOutcome::Failed(_)));
}

#[rstest]
fn a_skipped_write_records_its_reason() {
    let clock = DefaultClock;
    let github = StubGitHubClient::new(Reply::NoPr);
    let args = StatusArgs::default();
    let context = PrLookup {
        args: &args,
        clock: &clock,
        github: &github,
        project_dir: Utf8Path::new(PROJECT_DIR),
        branch: BRANCH,
    };

    let skipped = persist(
        &context,
        None,
        PersistRequest::Skip(PersistSkipReason::ServedFromCache),
    );
    assert!(matches!(
        skipped,
        CacheWriteOutcome::Skipped(PersistSkipReason::ServedFromCache)
    ));

    // A store request with nowhere to store it is a skip, not a failure.
    let nowhere = persist(&context, None, PersistRequest::Store("42".to_owned()));
    assert!(matches!(
        nowhere,
        CacheWriteOutcome::Skipped(PersistSkipReason::CacheUnavailable)
    ));
}

#[rstest]
fn a_status_line_survives_every_probe_failing(cache_root: io::Result<TempDir>) {
    let temp_dir = cache_root.expect("temp dir");
    let project_dir = utf8_path(&temp_dir).expect("project dir");
    let args = StatusArgs {
        project_dir: Some(project_dir),
        ..StatusArgs::default()
    };
    let clock = DefaultClock;
    let github = StubGitHubClient::new(Reply::Found("42"));

    let report = build_status_report(&args, &FailingRunner, &clock, &github)
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
