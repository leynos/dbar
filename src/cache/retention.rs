//! Bounded reclamation of expired dbar cache entries.
//!
//! Entries are keyed by a hash of the project directory and branch, so a
//! long-lived checkout accumulates one file per branch it has ever had.
//! [`sweep_cache_dir`] is the sole entry point that reclaims them. It never
//! runs behind a read: [`super::load_cached_value`] only reports expiry, and
//! the caller invokes the sweep explicitly once it has decided on a fresh
//! lookup, so a backlog is cleared without needing a scheduled job and every
//! deletion is visible at a call site.

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs_utf8::{Dir, DirEntry};
use mockable::Clock;

use crate::types::CacheTtlSeconds;

use super::{CacheEntry, CacheError, acquire_mutation_lock, read_bounded, to_epoch_seconds};

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

/// The suffix [`super::temp_name`] gives every in-progress write.
const TEMP_NAME_SUFFIX: &str = ".tmp";

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
        self.is_stale(entry.updated_at)
    }

    /// Report whether something last touched at `at` has outlived the TTL.
    const fn is_stale(&self, at: u64) -> bool {
        self.now.saturating_sub(at) > self.ttl.value()
    }
}

/// What kind of dbar-owned file the sweep is looking at.
enum OwnedKind {
    /// A completed cache entry, named `pr_<16 lowercase hex>.json`.
    Entry,
    /// An abandoned write temp file, named `<entry>.<pid>.<counter>.tmp`.
    Temp,
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
/// The `cache` module is crate-internal — `dbar` exposes only `run` and
/// `DbarError` — so this example is written against the in-crate path.
/// rustdoc collects doctests only from the public API, so an example on a
/// private item such as this one is never compiled; it is rendered as text
/// rather than run.
///
/// ```text
/// use camino::Utf8Path;
/// use crate::cache::retention::sweep_cache_dir;
/// use crate::types::CacheTtlSeconds;
/// use mockable::DefaultClock;
///
/// sweep_cache_dir(Utf8Path::new("."), &DefaultClock, CacheTtlSeconds::new(60))?;
/// ```
pub fn sweep_cache_dir(
    dir_path: &Utf8Path,
    clock: &dyn Clock,
    ttl: CacheTtlSeconds,
) -> Result<(), CacheError> {
    let dir = Dir::open_ambient_dir(dir_path, ambient_authority())?;
    let _lock = acquire_mutation_lock(&dir)?;
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
/// Called from [`sweep_cache_dir`], which callers invoke only once they are
/// already committed to a fresh upstream lookup, so the common cache-hit path
/// never scans the directory. One run lists at most [`SWEEP_LIST_LIMIT`] names,
/// opens at most [`SWEEP_INSPECT_LIMIT`] of them, and removes at most
/// [`SWEEP_REMOVAL_LIMIT`]; a backlog is therefore cleared over successive runs
/// rather than in one long pass on the status line's hot path.
///
/// The cache directory may be shared, so a completed entry is only removed
/// when it is dbar-owned on all three counts: its name matches `pr_<16
/// lowercase hex digits>.json`, it is a regular file whose contents parse as a
/// [`CacheEntry`], and that entry's own recorded timestamp puts it past `ttl`.
/// Anything else is left alone.
///
/// A write interrupted between creating its temp file and renaming it leaves
/// that temp file behind, which no rename will ever clear; those are reclaimed
/// too, by [`remove_if_stale_temp`], on the narrower evidence available for a
/// file that carries no parseable entry.
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
        let Some(kind) = owned_kind(&entry) else {
            continue;
        };
        inspected += 1;
        let reclaimed = match kind {
            OwnedKind::Entry => remove_if_expired(context, &entry)?,
            OwnedKind::Temp => remove_if_stale_temp(context, &entry)?,
        };
        if reclaimed {
            removed += 1;
        }
    }
    Ok(())
}

/// Classify `entry`, or `None` when it is not a file dbar wrote.
fn owned_kind(entry: &DirEntry) -> Option<OwnedKind> {
    // A name that will not decode as UTF-8 cannot be one dbar wrote, and a
    // file type that cannot be read belongs to something else mid-change. The
    // type check also excludes a symlink named like an entry, which is not
    // ours to follow or unlink.
    if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
        return None;
    }
    let name = entry.file_name().ok()?;
    if is_owned_name(&name) {
        return Some(OwnedKind::Entry);
    }
    if is_owned_temp_name(&name) {
        return Some(OwnedKind::Temp);
    }
    None
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

/// Report whether `name` matches [`super::temp_name`]'s
/// `pr_<16 lowercase hex>.json.<digits>.<digits>.tmp` shape.
///
/// The predicate is exact on every component, because the only thing standing
/// between a stranger's file in a shared cache directory and deletion is this
/// function agreeing that dbar wrote it.
fn is_owned_temp_name(name: &str) -> bool {
    let Some(rest) = name.strip_suffix(TEMP_NAME_SUFFIX) else {
        return false;
    };
    let Some((head, counter)) = rest.rsplit_once('.') else {
        return false;
    };
    let Some((entry_name, pid)) = head.rsplit_once('.') else {
        return false;
    };
    is_decimal(pid) && is_decimal(counter) && is_owned_name(entry_name)
}

/// Report whether `text` is a non-empty run of ASCII decimal digits.
fn is_decimal(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Remove `entry` when it is a write temp file that has outlived `ttl`.
///
/// A temp file holds a raw payload with no recorded timestamp — it may not
/// even be complete — so its own mtime is the only age available, and the only
/// discriminator that distinguishes an orphan from a live writer's file.
///
/// That is safe against a live writer because [`super::write`] creates the
/// temp file, fills it, and renames it over the target within a single call:
/// a temp file whose writer is still running was created microseconds ago, so
/// it cannot have aged past any TTL dbar accepts. Only a writer killed between
/// the create and the rename leaves one behind to grow old, and that file will
/// never be renamed by anyone.
fn remove_if_stale_temp(context: &SweepContext<'_>, entry: &DirEntry) -> Result<bool, CacheError> {
    let name = entry
        .file_name()
        .map_err(|err| retention_error(context.dir_path, err))?;
    let path = context.dir_path.join(&name);
    let modified = match modified_epoch_seconds(entry) {
        Ok(seconds) => seconds,
        // Vanished under us: the writer finished its rename, or another dbar
        // process reclaimed it.
        Err(err) if is_vanished(&err) => return Ok(false),
        Err(err) => return Err(retention_error(path, err)),
    };
    if !context.is_stale(modified) {
        return Ok(false);
    }
    remove_reclaimed(context, &name, path)
}

/// The mtime of `entry` in whole seconds since the Unix epoch.
fn modified_epoch_seconds(entry: &DirEntry) -> std::io::Result<u64> {
    entry
        .metadata()?
        .modified()?
        .into_std()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs())
        .map_err(|_| std::io::Error::other("file modification time precedes the Unix epoch"))
}

/// Remove `name`, treating an already-vanished file as a successful removal.
fn remove_reclaimed(
    context: &SweepContext<'_>,
    name: &str,
    path: Utf8PathBuf,
) -> Result<bool, CacheError> {
    match context.dir.remove_file(name) {
        Ok(()) => Ok(true),
        // Another dbar process reclaimed the same file first.
        Err(err) if is_vanished(&err) => Ok(true),
        Err(err) => Err(retention_error(path, err)),
    }
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
    remove_reclaimed(context, &name, path)
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

#[cfg(test)]
mod tests {
    //! Coverage pinning the sweep's ownership predicate to the writer that
    //! actually mints cache-file names.
    use super::{is_owned_name, is_owned_temp_name};
    use crate::status::cache_key::pr_cache_path;
    use camino::Utf8Path;
    use rstest::rstest;

    /// The sweep only removes names [`is_owned_name`] accepts, and those names
    /// are produced in another module. If either side's format drifts the sweep
    /// silently stops reclaiming anything, so the two are pinned together here
    /// rather than left to agree by inspection.
    #[rstest]
    #[case::plain("/projects/demo", "main")]
    #[case::slashed_branch("/projects/demo", "feature/a")]
    #[case::punctuated("/projects/a_b.c-d", "release/1.2")]
    #[case::non_ascii("/projects/démo", "ветка")]
    #[case::empty_branch("/projects/demo", "")]
    fn sweep_owns_every_name_the_cache_key_writer_mints(
        #[case] project_dir: &str,
        #[case] branch: &str,
    ) {
        let path = pr_cache_path(Utf8Path::new("/cache"), Utf8Path::new(project_dir), branch);
        let name = path.file_name().expect("cache path ends in a file name");
        assert!(
            is_owned_name(name),
            "the retention sweep would skip {name}, which pr_cache_path minted"
        );
    }

    /// The temp-name predicate is what stands between a stranger's file in a
    /// shared cache directory and deletion, so every component is pinned: an
    /// entry name dbar mints, then a decimal pid, a decimal counter, and the
    /// literal extension. `write` mints exactly this and nothing else.
    #[rstest]
    #[case::minted("pr_0123456789abcdef.json.4321.0.tmp", true)]
    #[case::wide_counter("pr_00000000000000ab.json.1.18446744073709551615.tmp", true)]
    #[case::uppercase_digest("pr_0123456789ABCDEF.json.1.2.tmp", false)]
    #[case::short_digest("pr_deadbeef.json.1.2.tmp", false)]
    #[case::missing_prefix("0123456789abcdef.json.1.2.tmp", false)]
    #[case::missing_extension("pr_0123456789abcdef.1.2.tmp", false)]
    #[case::non_decimal_pid("pr_0123456789abcdef.json.abc.2.tmp", false)]
    #[case::hex_counter("pr_0123456789abcdef.json.1.0x2.tmp", false)]
    #[case::empty_pid("pr_0123456789abcdef.json..2.tmp", false)]
    #[case::one_field("pr_0123456789abcdef.json.1.tmp", false)]
    #[case::three_fields("pr_0123456789abcdef.json.1.2.3.tmp", false)]
    #[case::wrong_suffix("pr_0123456789abcdef.json.1.2.tmpx", false)]
    #[case::bare_suffix("pr_0123456789abcdef.json.tmp", false)]
    #[case::completed_entry("pr_0123456789abcdef.json", false)]
    fn the_temp_predicate_accepts_only_names_the_writer_mints(
        #[case] name: &str,
        #[case] owned: bool,
    ) {
        assert_eq!(is_owned_temp_name(name), owned, "for {name}");
    }
}
