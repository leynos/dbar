//! Round-trip, TTL-expiry, and retention-sweep tests for the PR cache layer.
use super::retention::{SWEEP_INSPECT_LIMIT, SWEEP_REMOVAL_LIMIT};
use super::*;
use camino::Utf8PathBuf;
use mockable::DefaultClock;
use rstest::{fixture, rstest};
use tempfile::TempDir;

/// A temporary directory and a cache-file path within it.
type CachePath = Result<(TempDir, Utf8PathBuf), CacheError>;

/// Create a temporary directory and a cache path with the given file name.
///
/// The `TempDir` is returned alongside the path so callers keep it alive
/// for the duration of the test.
#[fixture]
fn cache_path(#[default("cache.json")] name: &str) -> CachePath {
    let temp_dir = TempDir::new().map_err(CacheError::Io)?;
    let path = Utf8PathBuf::from_path_buf(temp_dir.path().join(name))
        .map_err(|_| CacheError::InvalidUtf8)?;
    Ok((temp_dir, path))
}

#[rstest]
fn cache_round_trip(cache_path: CachePath) {
    let (_temp_dir, path) = cache_path.expect("cache path");
    let clock = DefaultClock;
    store_cached_value(&path, &clock, "123").expect("write cache");
    let value = load_cached_value(&path, &clock, CacheTtlSeconds::new(60)).expect("read cache");
    assert_eq!(value.as_deref(), Some("123"));
}

#[rstest]
fn cache_expires_when_ttl_passed(#[with("expired.json")] cache_path: CachePath) {
    let (_temp_dir, path) = cache_path.expect("cache path");
    let payload_json = serde_json::json!({
        "value": "999",
        "updated_at": 0
    });
    let payload = payload_json.to_string();
    write(&path, &payload).expect("write cache");
    let clock = DefaultClock;
    let value = load_cached_value(&path, &clock, CacheTtlSeconds::new(1)).expect("read cache");
    assert!(value.is_none());
}

#[rstest]
fn concurrent_writes_never_expose_partial_json(cache_path: CachePath) {
    use std::sync::{Arc, Barrier};

    let (_temp_dir, path) = cache_path.expect("cache path");

    let writers: usize = 8;
    let barrier = Arc::new(Barrier::new(writers));
    let mut handles = Vec::new();
    for id in 0..writers {
        let writer_path = path.clone();
        let writer_barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let clock = DefaultClock;
            writer_barrier.wait();
            for _ in 0..20 {
                store_cached_value(&writer_path, &clock, id.to_string()).expect("store cache");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("writer thread");
    }

    // A reader must always see a complete, parseable value written by one of
    // the writers — never a truncated or interleaved JSON payload.
    let clock = DefaultClock;
    let value = load_cached_value(&path, &clock, CacheTtlSeconds::new(600))
        .expect("read must never see partial JSON");
    let observed = value.expect("a value should be present");
    assert!((0..writers).any(|id| observed == id.to_string()));
}

/// A cache payload of exactly `len` bytes that still parses as an entry.
fn padded_entry(len: usize, updated_at: u64) -> String {
    let bare = serde_json::json!({ "value": "", "updated_at": updated_at }).to_string();
    let padding = len.saturating_sub(bare.len());
    let value = "x".repeat(padding);
    serde_json::json!({ "value": value, "updated_at": updated_at }).to_string()
}

#[rstest]
fn load_refuses_an_entry_past_the_ceiling(#[with("oversized.json")] cache_path: CachePath) {
    let (_temp_dir, path) = cache_path.expect("cache path");
    let payload = padded_entry(MAX_ENTRY_BYTES + 1, 0);
    assert!(payload.len() > MAX_ENTRY_BYTES);
    write(&path, &payload).expect("write oversized entry");

    let clock = DefaultClock;
    let error = load_cached_value(&path, &clock, CacheTtlSeconds::new(600))
        .expect_err("an oversized entry must be refused");
    assert!(
        matches!(
            error,
            CacheError::EntryTooLarge { limit, .. } if limit == MAX_ENTRY_BYTES
        ),
        "expected a size error, got {error:?}"
    );
}

#[rstest]
fn load_accepts_an_entry_at_the_ceiling(#[with("at_limit.json")] cache_path: CachePath) {
    let (_temp_dir, path) = cache_path.expect("cache path");
    let clock = DefaultClock;
    let now = now_seconds(&clock).expect("clock reading");
    let payload = padded_entry(MAX_ENTRY_BYTES, now);
    assert_eq!(payload.len(), MAX_ENTRY_BYTES);
    write(&path, &payload).expect("write entry at the ceiling");

    let value = load_cached_value(&path, &clock, CacheTtlSeconds::new(600))
        .expect("an entry at the ceiling must be readable");
    assert_eq!(
        value.map(|entry| entry.len()),
        Some(MAX_ENTRY_BYTES - padded_entry(0, now).len())
    );
}

/// A dbar-owned cache file name whose sweep behaviour a test drives.
const TARGET_NAME: &str = "pr_0000000000000001.json";

/// A temporary directory retained for the lifetime of a test.
type Workspace = Result<(TempDir, Utf8PathBuf), CacheError>;

/// Create a temporary directory to seed cache entries into.
#[fixture]
fn workspace() -> Workspace {
    let temp_dir = TempDir::new().map_err(CacheError::Io)?;
    let dir = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf())
        .map_err(|_| CacheError::InvalidUtf8)?;
    Ok((temp_dir, dir))
}

/// The clock reading the sweep will compare entry timestamps against.
fn now_seconds(clock: &dyn Clock) -> Result<u64, CacheError> {
    to_epoch_seconds(clock.utc().timestamp())
}

/// Write `payload` verbatim to `dir/name`.
fn seed_raw(dir: &Utf8Path, name: &str, payload: &str) -> Result<Utf8PathBuf, CacheError> {
    let path = dir.join(name);
    write(&path, payload)?;
    Ok(path)
}

/// Write a well-formed cache entry stamped `updated_at` to `dir/name`.
fn seed_entry(dir: &Utf8Path, name: &str, updated_at: u64) -> Result<Utf8PathBuf, CacheError> {
    let payload = serde_json::json!({ "value": "1", "updated_at": updated_at }).to_string();
    seed_raw(dir, name, &payload)
}

/// The sorted names of everything currently in `dir`.
fn entry_names(dir: &Utf8Path) -> Result<Vec<String>, CacheError> {
    let handle = Dir::open_ambient_dir(dir, ambient_authority())?;
    let mut names = Vec::new();
    for entry in handle.entries()? {
        names.push(entry?.file_name()?);
    }
    names.sort();
    Ok(names)
}

/// Seed an expired target entry and read it, triggering one retention sweep.
///
/// Returns the load result so callers can confirm the expiry that drove the
/// sweep.
fn sweep_via_expired_read(dir: &Utf8Path) -> Result<Option<String>, CacheError> {
    let clock = DefaultClock;
    let target = seed_entry(dir, TARGET_NAME, 0)?;
    load_cached_value(&target, &clock, CacheTtlSeconds::new(1))
}

#[rstest]
fn sweep_removes_expired_owned_entries(workspace: Workspace) {
    let (_temp_dir, dir) = workspace.expect("workspace");
    seed_entry(&dir, "pr_00000000000000ab.json", 0).expect("seed expired entry");
    let expired = sweep_via_expired_read(&dir).expect("expired read");
    assert!(
        expired.is_none(),
        "an epoch-stamped entry must read as expired"
    );
    assert_eq!(
        entry_names(&dir).expect("list directory"),
        Vec::<String>::new()
    );
}

#[rstest]
fn sweep_keeps_unexpired_owned_entries(workspace: Workspace) {
    let (_temp_dir, dir) = workspace.expect("workspace");
    let clock = DefaultClock;
    let now = now_seconds(&clock).expect("clock reading");
    seed_entry(&dir, "pr_00000000000000ab.json", now).expect("seed fresh entry");
    sweep_via_expired_read(&dir).expect("expired read");
    assert_eq!(
        entry_names(&dir).expect("list directory"),
        vec!["pr_00000000000000ab.json"]
    );
}

#[rstest]
fn sweep_leaves_unrelated_files_untouched(workspace: Workspace) {
    let (_temp_dir, dir) = workspace.expect("workspace");
    // Every decoy holds an expired payload, so only the ownership predicate
    // stands between it and removal.
    let decoys = [
        "pr_notahexdigits0.json",   // right length, not hex
        "pr_0123456789ABCDEF.json", // uppercase hex
        "pr_deadbeef.json",         // too few digits
        "pr_0123456789abcdef.json.tmp",
        "pr_0123456789abcdef.jsonx",
        "0123456789abcdef.json", // missing prefix
        "README.md",
    ];
    for name in decoys {
        seed_entry(&dir, name, 0).expect("seed decoy");
    }
    // A directory named exactly like an owned entry must survive too.
    let nested = dir.join("pr_00000000000000cd.json");
    Dir::create_ambient_dir_all(&nested, ambient_authority()).expect("create subdirectory");
    sweep_via_expired_read(&dir).expect("expired read");

    let mut expected: Vec<String> = decoys.iter().map(|name| (*name).to_owned()).collect();
    expected.push("pr_00000000000000cd.json".to_owned());
    expected.sort();
    assert_eq!(entry_names(&dir).expect("list directory"), expected);
}

#[rstest]
fn sweep_leaves_malformed_owned_entries_for_the_typed_error_path(workspace: Workspace) {
    let (_temp_dir, dir) = workspace.expect("workspace");
    let malformed =
        seed_raw(&dir, "pr_00000000000000ef.json", "{ not json").expect("seed malformed entry");
    sweep_via_expired_read(&dir).expect("expired read");
    assert_eq!(
        entry_names(&dir).expect("list directory"),
        vec!["pr_00000000000000ef.json"],
        "an unparseable file is not provably dbar's to delete"
    );

    // Reading a malformed entry directly still surfaces the typed parse error
    // rather than silently discarding the file.
    let clock = DefaultClock;
    let error = load_cached_value(&malformed, &clock, CacheTtlSeconds::new(1))
        .expect_err("malformed JSON must fail the read");
    assert!(
        matches!(error, CacheError::Serde(_)),
        "expected a serde error, got {error:?}"
    );
}

#[rstest]
fn sweep_leaves_oversized_owned_entries_alone(workspace: Workspace) {
    let (_temp_dir, dir) = workspace.expect("workspace");
    let payload = padded_entry(MAX_ENTRY_BYTES + 1, 0);
    seed_raw(&dir, "pr_00000000000000ef.json", &payload).expect("seed oversized entry");
    sweep_via_expired_read(&dir).expect("expired read");
    assert_eq!(
        entry_names(&dir).expect("list directory"),
        vec!["pr_00000000000000ef.json"],
        "a file too large to be one dbar wrote is not provably ours to delete"
    );
}

#[rstest]
fn sweep_removes_no_more_than_the_per_run_bound(workspace: Workspace) {
    let (_temp_dir, dir) = workspace.expect("workspace");
    let seeded = SWEEP_REMOVAL_LIMIT + 4;
    assert!(
        seeded <= SWEEP_INSPECT_LIMIT,
        "the removal bound, not the inspection bound, must be the binding one"
    );
    for index in 0..seeded {
        seed_entry(&dir, &format!("pr_{index:016x}.json"), 0).expect("seed expired entry");
    }
    // The target is one of the seeded entries, so an unbounded sweep would
    // empty the directory; the bound must leave the surplus for a later run.
    sweep_via_expired_read(&dir).expect("expired read");
    assert_eq!(
        entry_names(&dir).expect("list directory").len(),
        seeded - SWEEP_REMOVAL_LIMIT
    );
}
