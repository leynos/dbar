//! Cache helpers for expensive lookups.
//!
//! Entries are keyed by a hash of the project directory and branch, so a
//! long-lived checkout accumulates one file per branch it has ever had. To
//! keep that bounded, a read that finds an expired entry also runs a bounded
//! retention sweep over the containing directory; see [`sweep_cache_dir`] for
//! the policy and for the explicit maintenance entry point.

use std::sync::atomic::{AtomicU64, Ordering};

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;
use directories::ProjectDirs;
use mockable::Clock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::CacheTtlSeconds;

mod retention;

use retention::sweep_cache_dir;

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
    /// A cache file was larger than any entry dbar writes.
    ///
    /// The file is neither buffered nor parsed; callers treat it as a miss and
    /// perform a fresh lookup.
    #[error("cache entry {path} exceeds the {limit}-byte read limit")]
    EntryTooLarge {
        /// The entry that was refused.
        path: Utf8PathBuf,
        /// The configured byte ceiling that was exceeded.
        limit: usize,
    },
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

/// The largest cache file that will be read into memory.
///
/// An entry dbar writes is one small JSON object: a short status string and a
/// timestamp, tens of bytes in practice. 64 KiB is therefore hundreds of times
/// the largest entry the writer above can produce, so no legitimate entry is
/// ever refused — while a corrupt, truncated, or foreign file in a shared
/// cache directory cannot be buffered on the status line's hot path, which
/// reads this file on every refresh.
const MAX_ENTRY_BYTES: usize = 64 * 1024;

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

/// Load a cached value if it is still within its TTL, reclaiming expired
/// entries on the way.
///
/// This is a read-through with maintenance, not a pure read: finding an
/// expired entry also runs [`sweep_cache_dir`] over the containing directory,
/// which removes expired dbar-owned entries. That path is already committed to
/// a fresh upstream lookup, so the cost lands where a lookup was going to be
/// paid anyway; an ordinary cache hit touches nothing. A sweep failure is
/// reported as [`CacheError::Retention`] in place of the `Ok(None)` the expiry
/// would otherwise produce.
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
        // The one write this read performs, named at the call site rather than
        // hidden behind the load: expiry implies a fresh upstream lookup, so
        // the reclamation is paid here or not at all.
        sweep_cache_dir(parent_of(path), clock, ttl)?;
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
    read_bounded(&dir, file_name, path)
}

/// Read `dir/name`, refusing anything past [`MAX_ENTRY_BYTES`].
///
/// One byte beyond the ceiling is read so that overrunning the limit is
/// distinguishable from exactly reaching it, matching the shape of
/// `command::spawn_reader`. An oversized payload is dropped rather than
/// returned: truncating it would hand the parser a corrupt entry, and keeping
/// it would defeat the bound.
fn read_bounded(dir: &Dir, name: &str, path: &Utf8Path) -> Result<String, CacheError> {
    use std::io::Read as _;

    let file = dir.open(name)?;
    let ceiling = u64::try_from(MAX_ENTRY_BYTES)
        .map_err(|_| std::io::Error::other("cache entry limit does not fit in a byte count"))?;
    let mut buffer = Vec::new();
    file.take(ceiling + 1).read_to_end(&mut buffer)?;
    if buffer.len() > MAX_ENTRY_BYTES {
        drop(buffer);
        return Err(CacheError::EntryTooLarge {
            path: path.to_owned(),
            limit: MAX_ENTRY_BYTES,
        });
    }
    String::from_utf8(buffer)
        .map_err(|err| CacheError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err)))
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

const fn to_epoch_seconds(timestamp: i64) -> Result<u64, CacheError> {
    if timestamp < 0 {
        return Err(CacheError::ClockSkew);
    }
    Ok(timestamp.unsigned_abs())
}

#[cfg(test)]
mod tests;
