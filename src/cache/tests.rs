//! Round-trip, TTL-expiry, and retention-sweep tests for the PR cache layer.
//!
//! Concurrency coverage lives in the sibling `concurrency_tests` module.
use super::retention::{SWEEP_INSPECT_LIMIT, SWEEP_REMOVAL_LIMIT};
use super::*;
use camino::Utf8PathBuf;
use mockable::{DefaultClock, MockClock};
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
    let found = load_cached_value(&path, &clock, CacheTtlSeconds::new(60)).expect("read cache");
    assert!(
        matches!(&found, CacheLookup::Fresh(value) if value == "123"),
        "expected the stored value, got {found:?}"
    );
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
    let found = load_cached_value(&path, &clock, CacheTtlSeconds::new(1)).expect("read cache");
    assert!(
        matches!(found, CacheLookup::Expired),
        "expected an expiry report, got {found:?}"
    );
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

    let found = load_cached_value(&path, &clock, CacheTtlSeconds::new(600))
        .expect("an entry at the ceiling must be readable");
    let CacheLookup::Fresh(value) = found else {
        panic!("an entry at the ceiling must read as fresh, got {found:?}");
    };
    assert_eq!(value.len(), MAX_ENTRY_BYTES - padded_entry(0, now).len());
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

/// The TTL the sweep tests read and sweep against.
const SWEEP_TTL: CacheTtlSeconds = CacheTtlSeconds::new(1);

/// Names a sweep must leave alone however stale they are.
///
/// Each is seeded holding an expired payload, so only the ownership predicate
/// stands between it and removal. The second group probes the temp-file
/// predicate specifically: each one is a near miss for the
/// `pr_<16 hex>.json.<pid>.<counter>.tmp` shape the writer mints.
const DECOYS: [&str; 12] = [
    "pr_notahexdigits0.json",   // right length, not hex
    "pr_0123456789ABCDEF.json", // uppercase hex
    "pr_deadbeef.json",         // too few digits
    "pr_0123456789abcdef.json.tmp",
    "pr_0123456789abcdef.jsonx",
    "0123456789abcdef.json", // missing prefix
    "README.md",
    "pr_0123456789abcdef.json.4321.tmp",    // one field short
    "pr_0123456789abcdef.json.abc.7.tmp",   // non-decimal pid
    "pr_0123456789abcdef.json.4321.7.tmpx", // wrong extension
    "pr_0123456789ABCDEF.json.4321.7.tmp",  // uppercase digest
    "cache.json.4321.7.tmp",                // a temp file for a name dbar never mints
];

/// A clock reading far past anything this suite writes, so every seeded file
/// is unambiguously stale by its own mtime.
const DISTANT_FUTURE: &str = "2200-01-01T00:00:00+00:00";

/// A temp file name of the shape [`temp_name`] mints.
const ORPHAN_NAME: &str = "pr_00000000000000ab.json.4321.7.tmp";

/// A payload a killed writer could plausibly have left behind: the raw entry
/// bytes, truncated mid-write, with no timestamp to judge the file by.
const PARTIAL_PAYLOAD: &str = "{\"value\":\"1\",\"upd";

/// Seed an expired target entry, read it, then sweep the directory explicitly.
///
/// This mirrors the shape of the production call site in `status`: the read
/// only reports expiry, and the caller — having decided on a fresh lookup —
/// invokes retention itself. The read outcome is returned so callers can
/// confirm the expiry that justified the sweep.
fn read_then_sweep(dir: &Utf8Path) -> Result<CacheLookup, CacheError> {
    let clock = DefaultClock;
    let target = seed_entry(dir, TARGET_NAME, 0)?;
    let found = load_cached_value(&target, &clock, SWEEP_TTL)?;
    sweep_cache_dir(dir, &clock, SWEEP_TTL)?;
    Ok(found)
}

#[rstest]
fn load_never_removes_an_expired_entry(workspace: Workspace) {
    let (_temp_dir, dir) = workspace.expect("workspace");
    let target = seed_entry(&dir, TARGET_NAME, 0).expect("seed expired entry");
    let clock = DefaultClock;

    // Read it repeatedly: a read is not a maintenance operation, so no number
    // of reads may reclaim anything.
    for _ in 0..3 {
        let found = load_cached_value(&target, &clock, SWEEP_TTL).expect("expired read");
        assert!(
            matches!(found, CacheLookup::Expired),
            "expected an expiry report, got {found:?}"
        );
    }
    assert_eq!(
        entry_names(&dir).expect("list directory"),
        vec![TARGET_NAME],
        "load_cached_value must leave an expired entry on disk"
    );

    // The same entry is reclaimed once retention is invoked explicitly.
    sweep_cache_dir(&dir, &clock, SWEEP_TTL).expect("sweep");
    assert_eq!(
        entry_names(&dir).expect("list directory"),
        Vec::<String>::new()
    );
}

#[rstest]
fn sweep_removes_expired_owned_entries(workspace: Workspace) {
    let (_temp_dir, dir) = workspace.expect("workspace");
    seed_entry(&dir, "pr_00000000000000ab.json", 0).expect("seed expired entry");
    let found = read_then_sweep(&dir).expect("expired read and sweep");
    assert!(
        matches!(found, CacheLookup::Expired),
        "an epoch-stamped entry must read as expired, got {found:?}"
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
    read_then_sweep(&dir).expect("expired read and sweep");
    assert_eq!(
        entry_names(&dir).expect("list directory"),
        vec!["pr_00000000000000ab.json"]
    );
}

#[rstest]
fn sweep_leaves_unrelated_files_untouched(workspace: Workspace) {
    let (_temp_dir, dir) = workspace.expect("workspace");
    for name in DECOYS {
        seed_entry(&dir, name, 0).expect("seed decoy");
    }
    // A directory named exactly like an owned entry must survive too.
    let nested = dir.join("pr_00000000000000cd.json");
    Dir::create_ambient_dir_all(&nested, ambient_authority()).expect("create subdirectory");
    read_then_sweep(&dir).expect("expired read and sweep");

    let mut expected: Vec<String> = DECOYS.iter().map(|name| (*name).to_owned()).collect();
    expected.push("pr_00000000000000cd.json".to_owned());
    expected.sort();
    assert_eq!(entry_names(&dir).expect("list directory"), expected);
}

#[rstest]
fn sweep_leaves_malformed_owned_entries_for_the_typed_error_path(workspace: Workspace) {
    let (_temp_dir, dir) = workspace.expect("workspace");
    let malformed =
        seed_raw(&dir, "pr_00000000000000ef.json", "{ not json").expect("seed malformed entry");
    read_then_sweep(&dir).expect("expired read and sweep");
    assert_eq!(
        entry_names(&dir).expect("list directory"),
        vec!["pr_00000000000000ef.json"],
        "an unparseable file is not provably dbar's to delete"
    );

    // Reading a malformed entry directly still surfaces the typed parse error
    // rather than silently discarding the file.
    let clock = DefaultClock;
    let error = load_cached_value(&malformed, &clock, SWEEP_TTL)
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
    read_then_sweep(&dir).expect("expired read and sweep");
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
    read_then_sweep(&dir).expect("expired read and sweep");
    assert_eq!(
        entry_names(&dir).expect("list directory").len(),
        seeded - SWEEP_REMOVAL_LIMIT
    );
}

#[rstest]
fn sweep_reclaims_a_stale_orphaned_temp_file(workspace: Workspace) {
    let (_temp_dir, dir) = workspace.expect("workspace");
    seed_raw(&dir, ORPHAN_NAME, PARTIAL_PAYLOAD).expect("seed orphaned temp file");
    for name in DECOYS {
        seed_entry(&dir, name, 0).expect("seed decoy");
    }

    // The orphan carries no parseable timestamp, so the sweep judges it by its
    // mtime. Reading the clock from the far future is what makes the file old
    // without having to backdate it.
    let mut clock = MockClock::new();
    let future = DISTANT_FUTURE.parse().expect("the distant future parses");
    clock.expect_utc().returning(move || future);
    sweep_cache_dir(&dir, &clock, SWEEP_TTL).expect("sweep");

    let mut expected: Vec<String> = DECOYS.iter().map(|name| (*name).to_owned()).collect();
    expected.sort();
    assert_eq!(
        entry_names(&dir).expect("list directory"),
        expected,
        "the sweep must reclaim the orphaned temp file and nothing else"
    );
}

#[rstest]
fn sweep_keeps_an_in_flight_temp_file(workspace: Workspace) {
    let (_temp_dir, dir) = workspace.expect("workspace");
    seed_raw(&dir, ORPHAN_NAME, PARTIAL_PAYLOAD).expect("seed in-flight temp file");
    // The real clock is the point: a temp file belonging to a writer that is
    // still running was created moments ago, and removing it would corrupt
    // that write. Age is the only discriminator, so it must hold here.
    read_then_sweep(&dir).expect("expired read and sweep");
    assert_eq!(
        entry_names(&dir).expect("list directory"),
        vec![ORPHAN_NAME],
        "a temp file younger than the TTL may belong to a live writer"
    );
}
