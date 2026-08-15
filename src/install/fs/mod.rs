//! The filesystem and locking layer behind an install transaction.
//!
//! Everything here is capability-based: paths are resolved to a `cap_std`
//! `Dir` for their parent, and writes land by rename rather than in place.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs_utf8::{Dir, File, OpenOptions};

use super::InstallError;

/// How many times the exclusive lock is attempted before giving up.
///
/// The retry budget is deliberately bounded — `LOCK_ATTEMPTS` tries spaced
/// `LOCK_RETRY_DELAY` apart, about ten seconds in total — but it is generous,
/// because the thing being waited on is always a live process. `flock` is
/// released by the kernel when its holder exits, so a crashed install cannot
/// leave a lock behind for this budget to rescue us from; the only reason to
/// wait is that someone else is genuinely mid-transaction. Each transaction now
/// flushes both the backup and the config to disk before returning, which is
/// far slower than the string work around it, so a handful of installs racing
/// on one config can legitimately queue for seconds. A budget of one second
/// turned that queue into spurious "config is locked" failures.
const LOCK_ATTEMPTS: u32 = 200;

/// How long to wait between lock attempts.
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Disambiguates temp-file names for concurrent writers within one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The single, fixed backup path for a config file.
///
/// `.dbar.bak` always holds the config's contents immediately before the most
/// recent successful install; each install overwrites it rather than keeping a
/// history. Concurrent installs are safe only because the per-config lock makes
/// each backup-then-write pair atomic as a unit, so the backup is never a mix
/// of two runs.
pub(super) fn backup_path_for(path: &Utf8Path) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{}.dbar.bak", path.as_str()))
}

/// The sibling lock file guarding a config file's install transaction.
fn lock_path_for(path: &Utf8Path) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{}.dbar.lock", path.as_str()))
}

/// Take the exclusive install lock for `config_path`, retrying briefly.
///
/// Returns the locked file, which must be held for the duration of the
/// transaction: the lock is released when it is dropped. Returns
/// [`InstallError::Locked`] once the retry budget is exhausted.
pub(super) fn acquire_lock(config_path: &Utf8Path) -> Result<File, InstallError> {
    let file = open_lock_file(config_path)?;
    let mut remaining = LOCK_ATTEMPTS;
    while remaining > 0 {
        if try_lock_exclusive(&file)? {
            return Ok(file);
        }
        remaining -= 1;
        if remaining > 0 {
            std::thread::sleep(LOCK_RETRY_DELAY);
        }
    }
    Err(InstallError::Locked)
}

/// Open — creating if absent — the lock file beside the config.
///
/// The lock file is never removed. `flock` locks an inode rather than a path,
/// so unlinking it would reintroduce a TOCTOU race: a second process could
/// create and lock a fresh inode for the same path while the first still holds
/// the old one, and both would then believe they hold the config exclusively.
/// An empty sibling file is a cheap price for that guarantee.
fn open_lock_file(config_path: &Utf8Path) -> Result<File, InstallError> {
    let lock_path = lock_path_for(config_path);
    let (dir, file_name) = open_parent_for_write(&lock_path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    Ok(dir.open_with(file_name, &options)?)
}

/// Attempt a non-blocking exclusive `flock`, reporting whether it was taken.
#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> Result<bool, InstallError> {
    use rustix::fs::{FlockOperation, flock};
    use rustix::io::Errno;

    match flock(file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(true),
        // `EWOULDBLOCK` and `EAGAIN` share a value on Linux but not everywhere,
        // so both are matched by comparison rather than by pattern.
        Err(err) if err == Errno::WOULDBLOCK || err == Errno::AGAIN => Ok(false),
        Err(err) => Err(InstallError::Io(io::Error::from(err))),
    }
}

/// Platforms without `flock` fall back to no synchronization, matching the
/// process-group precedent in `crate::command`.
#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature must match the unix implementation"
)]
fn try_lock_exclusive(_file: &File) -> Result<bool, InstallError> {
    Ok(true)
}

/// The largest tmux configuration the installer will read.
///
/// The whole config is held in memory, edited, and written back, so an
/// unbounded read is an unbounded allocation. The cap is deliberately generous
/// rather than tight: this is the user's own file, read only on an explicit
/// `dbar install`, and refusing a large-but-legitimate config would be a
/// regression. The largest configs in the wild — generated ones, or
/// `oh-my-tmux` and friends with their comments intact — are a few hundred
/// kilobytes, so 8 MiB is roughly an order of magnitude beyond anything a
/// person has written while still bounding the damage a runaway or hostile
/// file can do. Exceeding it is refused outright: silently truncating a config
/// and then rewriting it would destroy the user's data.
const MAX_CONFIG_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn read_to_string(path: &Utf8Path) -> Result<String, InstallError> {
    // Reads must never create directories: a missing parent surfaces as a
    // `NotFound` error that the caller treats as "no existing config", so a
    // `--dry-run` never mutates the filesystem.
    let (dir, file_name) = open_parent_for_read(path)?;
    read_bounded(&dir, file_name)
}

/// Read `dir/name`, refusing anything past [`MAX_CONFIG_BYTES`].
///
/// One byte beyond the ceiling is read so that overrunning the limit is
/// distinguishable from exactly reaching it, matching the shape of
/// `command::spawn_reader`. An overrun is reported as an
/// [`io::ErrorKind::FileTooLarge`] error, which the caller's `NotFound` arm
/// deliberately does not absorb: an oversized config must fail the install
/// rather than be mistaken for an absent one and overwritten.
fn read_bounded(dir: &Dir, name: &str) -> Result<String, InstallError> {
    use std::io::Read as _;

    let file = dir.open(name)?;
    let ceiling = u64::try_from(MAX_CONFIG_BYTES)
        .map_err(|_| io::Error::other("config size limit does not fit in a byte count"))?;
    let mut buffer = Vec::new();
    file.take(ceiling + 1).read_to_end(&mut buffer)?;
    if buffer.len() > MAX_CONFIG_BYTES {
        drop(buffer);
        return Err(InstallError::Io(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("tmux config exceeds the {MAX_CONFIG_BYTES}-byte read limit"),
        )));
    }
    String::from_utf8(buffer)
        .map_err(|err| InstallError::Io(io::Error::new(io::ErrorKind::InvalidData, err)))
}

pub(super) fn write(path: &Utf8Path, contents: &str) -> Result<(), InstallError> {
    let (_, file_name) = split_parent(path)?;
    write_inheriting(path, contents, file_name)
}

/// Write `contents` to `path`, taking permissions from `permissions_from` — a
/// file name in the same parent directory.
///
/// A backup must inherit the mode of the config it copies rather than that of
/// its own (absent) destination, otherwise a `0600` config yields a
/// world-readable `0644` backup of the same content.
pub(super) fn write_inheriting(
    path: &Utf8Path,
    contents: &str,
    permissions_from: &str,
) -> Result<(), InstallError> {
    let (dir, file_name) = open_parent_for_write(path)?;
    // Write to a uniquely named temp file, then rename it over the target.
    // `Dir::write` truncates in place, so an interrupted write would otherwise
    // leave a half-written tmux config behind.
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!("{file_name}.{}.{unique}.tmp", std::process::id());
    let result = fill_then_rename(
        &dir,
        &TempSwap {
            tmp_name: tmp_name.as_str(),
            file_name,
            permissions_from,
        },
        contents,
    );
    if result.is_err() {
        // Best-effort cleanup; surface the original error, not the removal's.
        dir.remove_file(tmp_name.as_str()).ok();
    }
    result?;
    Ok(())
}

/// Create, harden, fill and flush the temp file, then swap it over the target.
///
/// The order is the whole point. Permissions are copied onto the file while it
/// is still empty, so a `0600` config's bytes are never momentarily readable
/// through a `0644` temp file. The contents are then flushed with `sync_all`
/// before the rename, so the new name can never point at an empty or
/// half-written file.
///
/// The directory itself is deliberately not synced after the rename. That
/// would harden the rename against a power loss, but `cap_std` exposes no
/// `Dir::sync_all`, and fsyncing the descriptor it wraps fails with `EBADF`
/// because `cap_primitives` opens directories with `O_PATH`. Reaching around
/// the capability with `std::fs` is both banned here and the very thing
/// `cap_std` exists to prevent, so the rename's durability is left to the
/// filesystem. The atomicity the installer actually promises — a reader sees
/// either the old config or the new one — comes from the rename itself and
/// holds regardless.
fn fill_then_rename(dir: &Dir, names: &TempSwap<'_>, contents: &str) -> io::Result<()> {
    use std::io::Write as _;

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = dir.open_with(names.tmp_name, &options)?;
    inherit_permissions_from(dir, names.tmp_name, names.permissions_from)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    drop(file);
    dir.rename(names.tmp_name, dir, names.file_name)
}

/// The three file names the swap juggles, kept together so that three
/// same-typed `&str` arguments cannot be supplied in the wrong order.
struct TempSwap<'a> {
    /// The uniquely named file the contents are written to first.
    tmp_name: &'a str,
    /// The name the temp file is renamed onto once it is complete.
    file_name: &'a str,
    /// The existing file whose mode the temp file inherits.
    permissions_from: &'a str,
}

/// Copy the target's permissions onto the freshly written temp file.
///
/// `Dir::write` creates the temp file with default (umask-derived) permissions,
/// so renaming it over the target would otherwise widen a hardened config such
/// as a `0600` `tmux.conf`. A missing target leaves the defaults in place.
fn inherit_permissions_from(dir: &Dir, tmp_name: &str, source: &str) -> io::Result<()> {
    match dir.metadata(source) {
        Ok(metadata) => dir.set_permissions(tmp_name, metadata.permissions()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Split a config path into the directory to open and the file name within it.
///
/// A bare relative path such as `tmux.conf` has an *empty* parent rather than
/// no parent, and opening `""` as a directory fails, so the empty parent is
/// mapped to `.` alongside the absent one.
pub(super) fn split_parent(path: &Utf8Path) -> Result<(&Utf8Path, &str), InstallError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_str().is_empty())
        .unwrap_or_else(|| Utf8Path::new("."));
    let file_name = path.file_name().ok_or(InstallError::MissingFileName)?;
    Ok((parent, file_name))
}

pub(super) fn open_parent_for_read(path: &Utf8Path) -> Result<(Dir, &str), InstallError> {
    let (parent, file_name) = split_parent(path)?;
    let dir = Dir::open_ambient_dir(parent, ambient_authority())?;
    Ok((dir, file_name))
}

pub(super) fn open_parent_for_write(path: &Utf8Path) -> Result<(Dir, &str), InstallError> {
    let (parent, file_name) = split_parent(path)?;
    Dir::create_ambient_dir_all(parent, ambient_authority())?;
    let dir = Dir::open_ambient_dir(parent, ambient_authority())?;
    Ok((dir, file_name))
}

#[cfg(test)]
mod tests;
