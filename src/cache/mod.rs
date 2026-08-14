//! Cache helpers for expensive lookups.
//!
//! Entries are keyed by a hash of the project directory and branch, so a
//! long-lived checkout accumulates one file per branch it has ever had. To
//! keep that bounded, a read that finds an expired entry also runs a bounded
//! retention sweep over the containing directory; see
//! [`sweep_expired_entries`] for the policy.

use std::sync::atomic::{AtomicU64, Ordering};

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs_utf8::{Dir, DirEntry};
use directories::ProjectDirs;
use mockable::Clock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::CacheTtlSeconds;

/// Disambiguates temp-file names for concurrent writers within one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
/// Errors produced while reading or writing cached data.
pub enum CacheError {
    /// No cache directory could be resolved for this platform.
    #[error("cache directory is unavailable")]
    MissingBaseDir,
    /// The cache path could not be converted to UTF-8.
    #[error("cache path is not valid UTF-8")]
    InvalidUtf8,
    /// The cache path does not include a final file name.
    #[error("cache path is missing a file name")]
    MissingFileName,
    /// The system clock returned a value before the Unix epoch.
    #[error("cache entry is older than the Unix epoch")]
    ClockSkew,
    /// Serialization or deserialization failed.
    #[error("cache serialization failed: {0}")]
    Serde(#[from] serde_json::Error),
    /// File system operations failed.
    #[error("cache IO failed: {0}")]
    Io(#[from] std::io::Error),
    /// Reclaiming expired cache entries failed.
    ///
    /// This is reported instead of `Ok(None)` on a cache miss caused by
    /// expiry, so the cached value itself is never at stake: callers may log
    /// it and carry on with a fresh lookup rather than failing the status
    /// line.
    #[error("cache retention sweep failed for {path}: {source}")]
    Retention {
        /// The directory or entry the sweep was working on.
        path: Utf8PathBuf,
        /// The underlying file system failure.
        source: std::io::Error,
    },
}

/// Names listed from the cache directory in one retention sweep.
///
/// Listing names is cheap (a handful of `getdents` calls), so this is set well
/// above any plausible dbar cache size.
const SWEEP_LIST_LIMIT: usize = 256;

/// Entries opened and parsed in one retention sweep.
const SWEEP_INSPECT_LIMIT: usize = 16;

/// Entries removed in one retention sweep.
const SWEEP_REMOVAL_LIMIT: usize = 8;

/// The prefix `status::cache_key` gives every PR cache file name.
const OWNED_NAME_PREFIX: &str = "pr_";

/// The extension `status::cache_key` gives every PR cache file name.
const OWNED_NAME_SUFFIX: &str = ".json";

/// The number of hex digits in a PR cache file name (`{digest:016x}`).
const OWNED_DIGEST_LEN: usize = 16;

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    value: String,
    updated_at: u64,
}

/// Resolve the cache directory, allowing an optional override.
///
/// # Examples
///
/// ```rust,ignore
/// use dbar::cache::resolve_cache_dir;
///
/// let dir = resolve_cache_dir(None)?;
/// # Ok::<(), dbar::cache::CacheError>(())
/// ```
pub fn resolve_cache_dir(override_dir: Option<Utf8PathBuf>) -> Result<Utf8PathBuf, CacheError> {
    if let Some(path) = override_dir {
        return Ok(path);
    }
    let dirs = ProjectDirs::from("com", "dbar", "dbar").ok_or(CacheError::MissingBaseDir)?;
    Utf8PathBuf::from_path_buf(dirs.cache_dir().to_path_buf()).map_err(|_| CacheError::InvalidUtf8)
}

/// Load a cached value if it is still within its TTL.
///
/// Finding an expired entry also triggers a bounded retention sweep of the
/// containing directory (see [`sweep_expired_entries`]), because that path
/// already implies a fresh upstream lookup. A sweep failure is reported as
/// [`CacheError::Retention`] in place of the `Ok(None)` the expiry would
/// otherwise produce.
///
/// # Examples
///
/// ```rust,ignore
/// use camino::Utf8Path;
/// use dbar::cache::load_cached_value;
/// use mockable::DefaultClock;
/// use dbar::types::CacheTtlSeconds;
///
/// let clock = DefaultClock;
/// let value = load_cached_value(Utf8Path::new("cache.json"), &clock, CacheTtlSeconds::new(60))?;
/// assert!(value.is_none());
/// # Ok::<(), dbar::cache::CacheError>(())
/// ```
pub fn load_cached_value(
    path: &Utf8Path,
    clock: &dyn Clock,
    ttl: CacheTtlSeconds,
) -> Result<Option<String>, CacheError> {
    let contents = match read_to_string(path) {
        Ok(value) => value,
        Err(CacheError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(err) => return Err(err),
    };
    let entry: CacheEntry = serde_json::from_str(&contents)?;
    let now = to_epoch_seconds(clock.utc().timestamp())?;
    let age = now.saturating_sub(entry.updated_at);
    if age > ttl.value() {
        let (dir, _) = open_parent(path)?;
        sweep_expired_entries(&SweepContext {
            dir: &dir,
            dir_path: parent_of(path),
            now,
            ttl,
        })?;
        return Ok(None);
    }
    Ok(Some(entry.value))
}

/// Store a cache entry, replacing any existing value.
///
/// # Examples
///
/// ```rust,ignore
/// use camino::Utf8Path;
/// use dbar::cache::store_cached_value;
/// use mockable::DefaultClock;
///
/// let clock = DefaultClock;
/// store_cached_value(Utf8Path::new("cache.json"), &clock, "123")?;
/// # Ok::<(), dbar::cache::CacheError>(())
/// ```
pub fn store_cached_value(
    path: &Utf8Path,
    clock: &dyn Clock,
    value: impl Into<String>,
) -> Result<(), CacheError> {
    if let Some(parent) = path.parent() {
        Dir::create_ambient_dir_all(parent, ambient_authority())?;
    }
    let entry = CacheEntry {
        value: value.into(),
        updated_at: to_epoch_seconds(clock.utc().timestamp())?,
    };
    let payload = serde_json::to_string(&entry)?;
    write(path, &payload)?;
    Ok(())
}

fn read_to_string(path: &Utf8Path) -> Result<String, CacheError> {
    let (dir, file_name) = open_parent(path)?;
    Ok(dir.read_to_string(file_name)?)
}

fn write(path: &Utf8Path, payload: &str) -> Result<(), CacheError> {
    let (dir, file_name) = open_parent(path)?;
    // Write to a uniquely named temp file in the same directory, then rename it
    // over the target. Rename is atomic on the same filesystem, so a concurrent
    // reader (or another writer) never observes a partially written entry.
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!("{file_name}.{}.{unique}.tmp", std::process::id());
    let result = dir
        .write(tmp_name.as_str(), payload.as_bytes())
        .and_then(|()| dir.rename(tmp_name.as_str(), &dir, file_name));
    if result.is_err() {
        // Best-effort cleanup; surface the original error, not the removal's.
        dir.remove_file(tmp_name.as_str()).ok();
    }
    result?;
    Ok(())
}

fn open_parent(path: &Utf8Path) -> Result<(Dir, &str), CacheError> {
    let file_name = path.file_name().ok_or(CacheError::MissingFileName)?;
    let dir = Dir::open_ambient_dir(parent_of(path), ambient_authority())?;
    Ok((dir, file_name))
}

fn parent_of(path: &Utf8Path) -> &Utf8Path {
    path.parent().unwrap_or_else(|| Utf8Path::new("."))
}

fn retention_error(path: impl Into<Utf8PathBuf>, source: std::io::Error) -> CacheError {
    CacheError::Retention {
        path: path.into(),
        source,
    }
}

/// The directory, clock reading, and TTL one retention sweep works against.
struct SweepContext<'a> {
    /// An open handle on the directory being swept.
    dir: &'a Dir,
    /// The same directory's path, used only to label errors.
    dir_path: &'a Utf8Path,
    /// The epoch seconds every entry's recorded timestamp is compared to.
    now: u64,
    /// The age past which an entry is reclaimable.
    ttl: CacheTtlSeconds,
}

impl SweepContext<'_> {
    const fn is_expired(&self, entry: &CacheEntry) -> bool {
        self.now.saturating_sub(entry.updated_at) > self.ttl.value()
    }
}

/// Reclaim expired dbar cache entries, doing bounded work.
///
/// The sweep runs only from the expired-read path, which is already committed
/// to a fresh upstream lookup, so the common cache-hit path never scans the
/// directory. One run lists at most [`SWEEP_LIST_LIMIT`] names, opens at most
/// [`SWEEP_INSPECT_LIMIT`] of them, and removes at most
/// [`SWEEP_REMOVAL_LIMIT`]; a backlog is therefore cleared over successive
/// runs rather than in one long pass on the status line's hot path.
///
/// The cache directory may be shared, so a file is only removed when it is
/// dbar-owned on all three counts: its name matches `pr_<16 lowercase hex
/// digits>.json`, it is a regular file whose contents parse as a
/// [`CacheEntry`], and that entry's own recorded timestamp puts it past `ttl`.
/// Anything else is left alone.
fn sweep_expired_entries(context: &SweepContext<'_>) -> Result<(), CacheError> {
    let listing = context
        .dir
        .entries()
        .map_err(|err| retention_error(context.dir_path, err))?;
    let mut inspected = 0_usize;
    let mut removed = 0_usize;
    for listed in listing.take(SWEEP_LIST_LIMIT) {
        if inspected >= SWEEP_INSPECT_LIMIT || removed >= SWEEP_REMOVAL_LIMIT {
            break;
        }
        let entry = listed.map_err(|err| retention_error(context.dir_path, err))?;
        if !is_owned_entry(&entry) {
            continue;
        }
        inspected += 1;
        if remove_if_expired(context, &entry)? {
            removed += 1;
        }
    }
    Ok(())
}

/// Report whether `entry` is a regular file named like a dbar cache entry.
fn is_owned_entry(entry: &DirEntry) -> bool {
    // A name that will not decode as UTF-8 cannot be one dbar wrote, and a
    // file type that cannot be read belongs to something else mid-change.
    let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
    is_file && entry.file_name().is_ok_and(|name| is_owned_name(&name))
}

/// Report whether `name` matches `status::cache_key`'s `pr_{digest:016x}.json` shape.
fn is_owned_name(name: &str) -> bool {
    name.strip_prefix(OWNED_NAME_PREFIX)
        .and_then(|rest| rest.strip_suffix(OWNED_NAME_SUFFIX))
        .is_some_and(|digest| {
            digest.len() == OWNED_DIGEST_LEN
                && digest
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
}

/// Remove `entry` when it parses as a cache entry that has outlived `ttl`.
///
/// Returns whether a removal was performed. A file that vanishes between the
/// listing and the removal counts as success: another dbar process reclaimed
/// it first.
fn remove_if_expired(context: &SweepContext<'_>, entry: &DirEntry) -> Result<bool, CacheError> {
    let name = entry
        .file_name()
        .map_err(|err| retention_error(context.dir_path, err))?;
    let path = context.dir_path.join(&name);
    let Some(contents) = read_swept_entry(context, &path, &name)? else {
        return Ok(false);
    };
    // Judge expiry by the entry's own recorded timestamp rather than by file
    // metadata, so a concurrent writer's refreshed entry is seen as fresh. The
    // remaining window between this read and the removal below can at worst
    // discard a just-written entry, costing one extra upstream lookup.
    let Ok(parsed) = serde_json::from_str::<CacheEntry>(&contents) else {
        return Ok(false);
    };
    if !context.is_expired(&parsed) {
        return Ok(false);
    }
    match context.dir.remove_file(&name) {
        Ok(()) => Ok(true),
        // Another dbar process reclaimed the same expired entry first.
        Err(err) if is_vanished(&err) => Ok(true),
        Err(err) => Err(retention_error(path, err)),
    }
}

/// Read a swept entry's contents, or `None` if it vanished under us.
fn read_swept_entry(
    context: &SweepContext<'_>,
    path: &Utf8Path,
    name: &str,
) -> Result<Option<String>, CacheError> {
    match context.dir.read_to_string(name) {
        Ok(contents) => Ok(Some(contents)),
        Err(err) if is_vanished(&err) => Ok(None),
        Err(err) => Err(retention_error(path, err)),
    }
}

fn is_vanished(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::NotFound
}

const fn to_epoch_seconds(timestamp: i64) -> Result<u64, CacheError> {
    if timestamp < 0 {
        return Err(CacheError::ClockSkew);
    }
    Ok(timestamp.unsigned_abs())
}

#[cfg(test)]
mod tests;
