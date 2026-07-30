//! Cache helpers for expensive lookups.

use std::sync::atomic::{AtomicU64, Ordering};

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;
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
    /// Serialisation or deserialisation failed.
    #[error("cache serialization failed: {0}")]
    Serde(#[from] serde_json::Error),
    /// File system operations failed.
    #[error("cache IO failed: {0}")]
    Io(#[from] std::io::Error),
}

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
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let file_name = path.file_name().ok_or(CacheError::MissingFileName)?;
    let dir = Dir::open_ambient_dir(parent, ambient_authority())?;
    Ok((dir, file_name))
}

const fn to_epoch_seconds(timestamp: i64) -> Result<u64, CacheError> {
    if timestamp < 0 {
        return Err(CacheError::ClockSkew);
    }
    Ok(timestamp.unsigned_abs())
}

#[cfg(test)]
mod tests {
    //! Round-trip and TTL-expiry tests for the PR cache layer.
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
}
