//! Bounded reclamation of expired dbar cache entries.
//!
//! Entries are keyed by a hash of the project directory and branch, so a
//! long-lived checkout accumulates one file per branch it has ever had.
//! [`sweep_cache_dir`] is the explicit entry point that reclaims them; it is
//! also what the expired-read path in [`super::load_cached_value`] calls, so a
//! backlog is cleared without needing a scheduled job.

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs_utf8::{Dir, DirEntry};
use mockable::Clock;

use crate::types::CacheTtlSeconds;

use super::{CacheEntry, CacheError, read_bounded, to_epoch_seconds};

/// Names listed from the cache directory in one retention sweep.
///
/// Listing names is cheap (a handful of `getdents` calls), so this is set well
/// above any plausible dbar cache size.
pub(super) const SWEEP_LIST_LIMIT: usize = 256;

/// Entries opened and parsed in one retention sweep.
pub(super) const SWEEP_INSPECT_LIMIT: usize = 16;

/// Entries removed in one retention sweep.
pub(super) const SWEEP_REMOVAL_LIMIT: usize = 8;

/// The prefix `status::cache_key` gives every PR cache file name.
const OWNED_NAME_PREFIX: &str = "pr_";

/// The extension `status::cache_key` gives every PR cache file name.
const OWNED_NAME_SUFFIX: &str = ".json";

/// The number of hex digits in a PR cache file name (`{digest:016x}`).
const OWNED_DIGEST_LEN: usize = 16;

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

/// Reclaim expired dbar cache entries in `dir_path`, doing bounded work.
///
/// This is the explicit maintenance operation; see [`sweep_expired_entries`]
/// for the per-run bounds and the ownership predicate that decide what may be
/// removed. It is safe to call at any time: a directory dbar does not own, or
/// one holding files it did not write, is left untouched.
///
/// # Examples
///
/// ```rust,ignore
/// use camino::Utf8Path;
/// use dbar::cache::sweep_cache_dir;
/// use dbar::types::CacheTtlSeconds;
/// use mockable::DefaultClock;
///
/// sweep_cache_dir(Utf8Path::new("."), &DefaultClock, CacheTtlSeconds::new(60))?;
/// # Ok::<(), dbar::cache::CacheError>(())
/// ```
pub fn sweep_cache_dir(
    dir_path: &Utf8Path,
    clock: &dyn Clock,
    ttl: CacheTtlSeconds,
) -> Result<(), CacheError> {
    let dir = Dir::open_ambient_dir(dir_path, ambient_authority())?;
    let now = to_epoch_seconds(clock.utc().timestamp())?;
    sweep_expired_entries(&SweepContext {
        dir: &dir,
        dir_path,
        now,
        ttl,
    })
}

/// Reclaim expired dbar cache entries, doing bounded work.
///
/// Called from [`sweep_cache_dir`] and from the expired-read path, which is
/// already committed to a fresh upstream lookup, so the common cache-hit path
/// never scans the directory. One run lists at most [`SWEEP_LIST_LIMIT`] names,
/// opens at most [`SWEEP_INSPECT_LIMIT`] of them, and removes at most
/// [`SWEEP_REMOVAL_LIMIT`]; a backlog is therefore cleared over successive runs
/// rather than in one long pass on the status line's hot path.
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

/// Read a swept entry's contents, or `None` if it is not removable.
///
/// A file that vanished under us, and one that is too large to be an entry
/// dbar wrote, both yield `None`: neither is provably ours to delete, and
/// neither is worth failing the caller's fresh lookup over.
fn read_swept_entry(
    context: &SweepContext<'_>,
    path: &Utf8Path,
    name: &str,
) -> Result<Option<String>, CacheError> {
    match read_bounded(context.dir, name, path) {
        Ok(contents) => Ok(Some(contents)),
        Err(CacheError::EntryTooLarge { .. }) => Ok(None),
        Err(CacheError::Io(err)) if is_vanished(&err) => Ok(None),
        Err(CacheError::Io(err)) => Err(retention_error(path, err)),
        Err(err) => Err(err),
    }
}

fn is_vanished(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::NotFound
}
