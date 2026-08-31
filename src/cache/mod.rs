//! Cache helpers for expensive lookups.
//!
//! Entries are keyed by a hash of the project directory and branch, so a
//! long-lived checkout accumulates one file per branch it has ever had. To
//! keep that bounded, expired entries are reclaimed by [`sweep_cache_dir`].
//!
//! Reads never delete. [`load_cached_value`] reports expiry as
//! [`CacheLookup::Expired`] and leaves the file in place; the caller — which
//! has just learned it must perform a fresh lookup anyway — invokes the sweep
//! itself, so the deletion is visible at the call site rather than hidden
//! behind a `load_*` name.

use std::sync::atomic::{AtomicU64, Ordering};

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs_utf8::{Dir, OpenOptions};
use directories::ProjectDirs;
use mockable::Clock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::status::cache::{CacheReader, CacheWriter, CachedValue};
use crate::status::pr::CacheFailure;
use crate::types::CacheTtlSeconds;

mod retention;

pub use retention::sweep_cache_dir;

/// Filesystem-backed implementation of the status cache ports.
pub(crate) struct FileCacheStorage;

impl CacheReader for FileCacheStorage {
    fn resolve_dir(&self, override_dir: Option<Utf8PathBuf>) -> Result<Utf8PathBuf, CacheFailure> {
        resolve_cache_dir(override_dir).map_err(directory_failure)
    }

    fn load(
        &self,
        path: &Utf8Path,
        clock: &dyn Clock,
        ttl: CacheTtlSeconds,
    ) -> Result<CachedValue, CacheFailure> {
        load_cached_value(path, clock, ttl)
            .map(|lookup| match lookup {
                CacheLookup::Fresh(cached) => CachedValue::Fresh(cached),
                CacheLookup::Expired => CachedValue::Expired,
                CacheLookup::Missing => CachedValue::Missing,
            })
            .map_err(read_failure)
    }
}

impl CacheWriter for FileCacheStorage {
    fn sweep(
        &self,
        dir: &Utf8Path,
        clock: &dyn Clock,
        ttl: CacheTtlSeconds,
    ) -> Result<(), CacheFailure> {
        sweep_cache_dir(dir, clock, ttl).map_err(read_failure)
    }

    fn store(&self, path: &Utf8Path, clock: &dyn Clock, value: String) -> Result<(), CacheFailure> {
        store_cached_value(path, clock, value).map_err(write_failure)
    }
}

/// Classify cache-directory resolution without retaining adapter errors.
fn directory_failure(_error: CacheError) -> CacheFailure {
    CacheFailure::DirectoryUnavailable
}

/// Classify every failed cache read and retention sweep as a read failure.
fn read_failure(_error: CacheError) -> CacheFailure {
    CacheFailure::Read
}

/// Classify every failed cache persistence operation as a write failure.
fn write_failure(_error: CacheError) -> CacheFailure {
    CacheFailure::Write
}

/// Disambiguates temp-file names for concurrent writers within one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Temp names tried before a write gives up.
///
/// The temp file is created exclusively, so a name that is already taken is an
/// error rather than a silent truncation. A stale temp file left behind by a
/// crashed writer would otherwise wedge that cache key permanently, and the
/// pid-and-counter pair — while improbable — is not a uniqueness proof either.
/// Each attempt draws a fresh counter value, and [`sweep_cache_dir`] reclaims
/// stale temp files in the background, so exhausting this budget means
/// something is wrong with the directory rather than merely unlucky.
const TEMP_NAME_ATTEMPTS: usize = 4;

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
    /// Only [`sweep_cache_dir`] produces this; the cached value itself is
    /// never at stake, so callers may log it and carry on with the fresh
    /// lookup that prompted the sweep rather than failing the status line.
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
/// ```text
/// use crate::cache::resolve_cache_dir;
///
/// let dir = resolve_cache_dir(None)?;
/// ```
pub fn resolve_cache_dir(override_dir: Option<Utf8PathBuf>) -> Result<Utf8PathBuf, CacheError> {
    if let Some(path) = override_dir {
        return Ok(path);
    }
    let dirs = ProjectDirs::from("com", "dbar", "dbar").ok_or(CacheError::MissingBaseDir)?;
    Utf8PathBuf::from_path_buf(dirs.cache_dir().to_path_buf()).map_err(|_| CacheError::InvalidUtf8)
}

/// What a cache read found at a path.
///
/// Expiry is reported rather than acted on: nothing here removes a file, so a
/// caller that wants an expired entry reclaimed calls [`sweep_cache_dir`]
/// explicitly.
#[derive(Debug)]
pub enum CacheLookup {
    /// An entry that is still within its TTL, with its cached value.
    Fresh(String),
    /// An entry that exists but has outlived its TTL. It is left on disk.
    Expired,
    /// No entry exists at the path.
    Missing,
}

/// Load a cached value if it is still within its TTL.
///
/// This is a pure read: no file is created, modified, or removed on any path
/// through it. An entry past its TTL yields [`CacheLookup::Expired`] and stays
/// on disk; reclaiming it is [`sweep_cache_dir`]'s job, invoked by whoever
/// decided to do the fresh lookup.
///
/// # Examples
///
/// ```text
/// use camino::Utf8Path;
/// use crate::cache::{CacheLookup, load_cached_value};
/// use mockable::DefaultClock;
/// use crate::types::CacheTtlSeconds;
///
/// let clock = DefaultClock;
/// let found = load_cached_value(Utf8Path::new("cache.json"), &clock, CacheTtlSeconds::new(60))?;
/// assert!(matches!(found, CacheLookup::Missing));
/// ```
///
/// # Errors
///
/// Returns an error if the entry cannot be read, exceeds
/// [`MAX_ENTRY_BYTES`], or does not deserialize.
pub fn load_cached_value(
    path: &Utf8Path,
    clock: &dyn Clock,
    ttl: CacheTtlSeconds,
) -> Result<CacheLookup, CacheError> {
    let contents = match read_to_string(path) {
        Ok(value) => value,
        Err(CacheError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CacheLookup::Missing);
        }
        Err(err) => return Err(err),
    };
    let entry: CacheEntry = serde_json::from_str(&contents)?;
    let now = to_epoch_seconds(clock.utc().timestamp())?;
    let age = now.saturating_sub(entry.updated_at);
    if age > ttl.value() {
        return Ok(CacheLookup::Expired);
    }
    Ok(CacheLookup::Fresh(entry.value))
}

/// Store a cache entry, replacing any existing value.
///
/// # Examples
///
/// ```text
/// use camino::Utf8Path;
/// use crate::cache::store_cached_value;
/// use mockable::DefaultClock;
///
/// let clock = DefaultClock;
/// store_cached_value(Utf8Path::new("cache.json"), &clock, "123")?;
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
    let mut attempt = 1_usize;
    loop {
        let tmp_name = temp_name(file_name);
        match create_and_fill(&dir, tmp_name.as_str(), payload) {
            Ok(()) => return finish_write(&dir, tmp_name.as_str(), file_name),
            // The name was taken. Nothing was opened, so nothing needs cleaning
            // up; draw another name and try again.
            Err(err)
                if err.kind() == std::io::ErrorKind::AlreadyExists
                    && attempt < TEMP_NAME_ATTEMPTS =>
            {
                attempt += 1;
            }
            Err(err) => return Err(CacheError::Io(err)),
        }
    }
}

/// Mint a temp name for `file_name`, unique across this process's writers.
fn temp_name(file_name: &str) -> String {
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{file_name}.{}.{unique}.tmp", std::process::id())
}

/// Create `dir/tmp_name` exclusively and write `payload` into it.
///
/// `create_new` is what makes the temp file the writer's own: an existing name
/// fails with `AlreadyExists` instead of being truncated, and a symlink
/// planted at that name is refused rather than followed, so the payload can
/// only ever land in a file this call has just created. `cap_std` already
/// refuses to traverse a symlink out of the directory the [`Dir`] capability
/// was opened on, so the redirection this closes is the one within that
/// directory — but it also matches what `install::fs` does for the tmux
/// config, and turns "silently overwrite whatever is there" into an error.
fn create_and_fill(dir: &Dir, tmp_name: &str, payload: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = dir.open_with(tmp_name, &options)?;
    file.write_all(payload.as_bytes())
}

/// Rename the filled temp file over `file_name`, cleaning up on failure.
fn finish_write(dir: &Dir, tmp_name: &str, file_name: &str) -> Result<(), CacheError> {
    let result = dir.rename(tmp_name, dir, file_name);
    if result.is_err() {
        // Best-effort cleanup; surface the original error, not the removal's.
        dir.remove_file(tmp_name).ok();
    }
    result.map_err(CacheError::Io)
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
mod concurrency_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod write_tests;
