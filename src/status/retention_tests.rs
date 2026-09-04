//! Coverage for the retention sweep this boundary performs explicitly.
//!
//! `cache::load_cached_value` never deletes anything, so the guarantee that
//! expired entries are reclaimed rests at the explicit refresh boundary.

use super::tests::{
    BRANCH, PROJECT_DIR, Reply, StubGitHubClient, args_with_cache, cache_root, exists, lookup,
    rendered, write_raw,
};
use super::*;
use crate::cache::{self, CacheFailure, CacheReader, CacheWriter, CachedValue};
use camino::{Utf8Path, Utf8PathBuf};
use mockable::DefaultClock;
use rstest::rstest;
use std::io;
use tempfile::TempDir;

/// A well-formed entry stamped at the epoch, and so expired against any TTL.
fn expired_payload() -> String {
    serde_json::json!({ "value": "1", "updated_at": 0 }).to_string()
}

/// A dbar-owned name that no test's own lookup would ever read.
///
/// Only the sweep can reclaim it, so its absence proves the sweep ran rather
/// than merely that the entry under lookup was overwritten.
const STALE_SIBLING: &str = "pr_00000000000000ab.json";

/// Run one refresh lookup through a storage double owned by this test module.
fn lookup_with_storage(
    args: &crate::config::StatusArgs,
    github: &dyn crate::github::GitHubClient,
    clock: &dyn mockable::Clock,
    cache: &dyn crate::cache::CacheStorage,
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
        cache,
    )
}

/// A storage double that proves a failed retention sweep does not hide a miss.
struct SweepFailingCache {
    dir: Utf8PathBuf,
}

impl CacheReader for SweepFailingCache {
    fn resolve_dir(&self, _override_dir: Option<Utf8PathBuf>) -> Result<Utf8PathBuf, CacheFailure> {
        Ok(self.dir.clone())
    }

    fn load(
        &self,
        _path: &Utf8Path,
        _clock: &dyn mockable::Clock,
        _ttl: crate::types::CacheTtlSeconds,
    ) -> Result<CachedValue, CacheFailure> {
        Ok(CachedValue::Missing)
    }
}

impl CacheWriter for SweepFailingCache {
    fn sweep(
        &self,
        _dir: &Utf8Path,
        _clock: &dyn mockable::Clock,
        _ttl: crate::types::CacheTtlSeconds,
    ) -> Result<(), CacheFailure> {
        Err(CacheFailure::Read)
    }

    fn store(
        &self,
        _path: &Utf8Path,
        _clock: &dyn mockable::Clock,
        _value: String,
    ) -> Result<(), CacheFailure> {
        Ok(())
    }
}

#[rstest]
fn an_expired_read_makes_the_boundary_sweep(cache_root: io::Result<(TempDir, Utf8PathBuf)>) {
    let (_guard, cache_dir) = cache_root.expect("cache root");
    let path = pr_cache_path(&cache_dir, Utf8Path::new(PROJECT_DIR), BRANCH);
    let sibling = cache_dir.join(STALE_SIBLING);
    write_raw(&path, &expired_payload()).expect("seed expired entry");
    write_raw(&sibling, &expired_payload()).expect("seed stale sibling");

    let github = StubGitHubClient::new(Reply::Found("42"));
    let clock = DefaultClock;
    let report = lookup(&args_with_cache(&cache_dir), &github, &clock);

    assert_eq!(
        rendered(&report).as_deref(),
        Some("42"),
        "an expired entry must fall through to a fresh lookup"
    );
    assert!(
        matches!(report.cache, CacheOutcome::Miss),
        "an expired entry is a miss, got {:?}",
        report.cache
    );
    assert!(
        !exists(&sibling),
        "the boundary must invoke retention on an expired read"
    );
    assert!(
        exists(&path),
        "the fresh lookup's answer must be written back"
    );
}

#[rstest]
fn a_fresh_read_still_sweeps_stale_siblings(cache_root: io::Result<(TempDir, Utf8PathBuf)>) {
    let (_guard, cache_dir) = cache_root.expect("cache root");
    let path = pr_cache_path(&cache_dir, Utf8Path::new(PROJECT_DIR), BRANCH);
    let sibling = cache_dir.join(STALE_SIBLING);
    let clock = DefaultClock;
    cache::store_cached_value(&path, &clock, "42").expect("seed fresh entry");
    write_raw(&sibling, &expired_payload()).expect("seed stale sibling");

    let github = StubGitHubClient::new(Reply::Failure);
    let report = lookup(&args_with_cache(&cache_dir), &github, &clock);

    assert!(
        matches!(report.cache, CacheOutcome::Hit),
        "a live entry must be served from cache, got {:?}",
        report.cache
    );
    assert!(
        !exists(&sibling),
        "every explicit refresh must reclaim stale siblings, including a cache hit"
    );
}

#[rstest]
fn a_missing_read_sweeps_stale_siblings(cache_root: io::Result<(TempDir, Utf8PathBuf)>) {
    let (_guard, cache_dir) = cache_root.expect("cache root");
    let sibling = cache_dir.join(STALE_SIBLING);
    write_raw(&sibling, &expired_payload()).expect("seed stale sibling");

    let github = StubGitHubClient::new(Reply::NoPr);
    let clock = DefaultClock;
    let report = lookup(&args_with_cache(&cache_dir), &github, &clock);

    assert!(matches!(report.cache, CacheOutcome::Miss));
    assert!(
        !exists(&sibling),
        "a cache miss must run the same bounded retention sweep"
    );
}

#[rstest]
fn a_failed_sweep_preserves_the_cache_miss(cache_root: io::Result<(TempDir, Utf8PathBuf)>) {
    let (_guard, cache_dir) = cache_root.expect("cache root");
    let cache = SweepFailingCache {
        dir: cache_dir.clone(),
    };
    let github = StubGitHubClient::new(Reply::Found("42"));
    let clock = DefaultClock;

    let report = lookup_with_storage(&args_with_cache(&cache_dir), &github, &clock, &cache);

    assert_eq!(rendered(&report).as_deref(), Some("42"));
    assert!(matches!(
        &report.cache,
        CacheOutcome::RetentionSweepFailed { current, failure: CacheFailure::Read }
            if matches!(current.as_ref(), CacheOutcome::Miss)
    ));
    assert_eq!(
        report.describe_failures(),
        ["PR cache retention sweep failed (read)"]
    );
}
