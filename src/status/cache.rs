//! Cache ports owned by the status application boundary.
//!
//! The status query receives only [`CacheReader`], while the explicit refresh
//! operation receives [`CacheStorage`]. This keeps a status render unable to
//! mutate cache state by construction.

use camino::{Utf8Path, Utf8PathBuf};
use mockable::Clock;

use super::pr::CacheFailure;
use crate::config::StatusArgs;
use crate::types::CacheTtlSeconds;

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

/// Inputs to the cache read performed while rendering status.
pub(crate) struct CachedPrRead<'a> {
    /// Parsed status settings.
    pub(crate) args: &'a StatusArgs,
    /// Clock used to evaluate TTL expiry.
    pub(crate) clock: &'a dyn Clock,
    /// Directory that scopes the cache key.
    pub(crate) project_dir: &'a Utf8Path,
    /// Branch that scopes the cache key.
    pub(crate) branch: &'a str,
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
