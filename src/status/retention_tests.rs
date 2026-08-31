//! Coverage for the retention sweep this boundary performs explicitly.
//!
//! `cache::load_cached_value` no longer deletes anything, so the guarantee
//! that expired entries are still reclaimed — on the same trigger as before —
//! now rests here, at the call site that decided to go upstream.

use super::tests::{
    BRANCH, PROJECT_DIR, Reply, StubGitHubClient, args_with_cache, cache_root, exists, lookup,
    rendered, write_raw,
};
use super::*;
use crate::cache;
use camino::Utf8Path;
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
fn a_fresh_read_sweeps_nothing(cache_root: io::Result<(TempDir, Utf8PathBuf)>) {
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
        exists(&sibling),
        "an ordinary cache hit must not list or reclaim anything"
    );
}
