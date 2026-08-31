//! Cache ports and adapter-neutral outcomes used by status assembly.

use camino::{Utf8Path, Utf8PathBuf};
use mockable::Clock;

use crate::types::CacheTtlSeconds;

/// A cache failure expressed without exposing a filesystem adapter error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheFailure {
    /// The cache directory could not be resolved.
    DirectoryUnavailable,
    /// Loading or reclaiming cache entries failed.
    Read,
    /// Persisting a cache entry failed.
    Write,
}

/// A cache value as seen through the status application's read port.
#[derive(Debug)]
pub(crate) enum CachedValue {
    /// A current entry supplied this value.
    Fresh(String),
    /// An entry exists but must be refreshed before use.
    Expired,
    /// No entry exists at the requested key.
    Missing,
}

/// Read-only cache operations needed while rendering a status line.
pub(crate) trait CacheReader {
    /// Resolve the cache root for an optional configuration override.
    fn resolve_dir(&self, override_dir: Option<Utf8PathBuf>) -> Result<Utf8PathBuf, CacheFailure>;
    /// Load a cache entry without changing it.
    fn load(
        &self,
        path: &Utf8Path,
        clock: &dyn Clock,
        ttl: CacheTtlSeconds,
    ) -> Result<CachedValue, CacheFailure>;
}

/// Cache mutations reserved for the explicit refresh operation.
pub(crate) trait CacheWriter {
    /// Reclaim expired entries after refresh has committed to a live lookup.
    fn sweep(
        &self,
        dir: &Utf8Path,
        clock: &dyn Clock,
        ttl: CacheTtlSeconds,
    ) -> Result<(), CacheFailure>;
    /// Persist one refreshed value.
    fn store(&self, path: &Utf8Path, clock: &dyn Clock, value: String) -> Result<(), CacheFailure>;
}

/// Complete cache storage needed by an explicit refresh.
pub(crate) trait CacheStorage: CacheReader + CacheWriter {}

impl<T: CacheReader + CacheWriter> CacheStorage for T {}
