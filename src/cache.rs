//! Cache helpers for expensive lookups.

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;
use directories::ProjectDirs;
use mockable::Clock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::CacheTtlSeconds;

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
    Ok(dir.write(file_name, payload.as_bytes())?)
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
    use super::*;
    use camino::Utf8PathBuf;
    use mockable::DefaultClock;
    use rstest::fixture;
    use rstest::rstest;
    use tempfile::TempDir;

    #[fixture]
    fn temp_dir() -> TempDir {
        TempDir::new().expect("temp dir")
    }

    #[rstest]
    fn cache_round_trip(temp_dir: TempDir) {
        let path = Utf8PathBuf::from_path_buf(temp_dir.path().join("cache.json"))
            .map_err(|_| CacheError::InvalidUtf8)
            .expect("cache path");
        let clock = DefaultClock;
        store_cached_value(&path, &clock, "123").expect("write cache");
        let value = load_cached_value(&path, &clock, CacheTtlSeconds::new(60)).expect("read cache");
        assert_eq!(value.as_deref(), Some("123"));
    }

    #[rstest]
    fn cache_expires_when_ttl_passed(temp_dir: TempDir) {
        let path = Utf8PathBuf::from_path_buf(temp_dir.path().join("expired.json"))
            .map_err(|_| CacheError::InvalidUtf8)
            .expect("cache path");
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
}
