//! tmux configuration installation helpers.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

use camino::Utf8Path;
use camino::Utf8PathBuf;
use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;
use thiserror::Error;

use crate::types::StatusPosition;

const MARKER_START: &str = "# dbar: begin";
const MARKER_END: &str = "# dbar: end";

/// Disambiguates temp-file names for concurrent writers within one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
/// Summary of an install operation.
pub struct InstallOutcome {
    /// The config file that was targeted.
    pub path: Utf8PathBuf,
    /// Optional backup path when a file was overwritten.
    pub backup_path: Option<Utf8PathBuf>,
    /// Whether the file contents changed.
    pub updated: bool,
    /// Whether the install was a dry run.
    pub dry_run: bool,
    /// The snippet that would be or was written.
    pub snippet: String,
}

#[derive(Debug, Error)]
/// Errors reported while installing tmux configuration.
pub enum InstallError {
    /// A path was not supplied for editing.
    #[error("missing tmux configuration path; pass --path")]
    MissingPath,
    /// The path does not include a file name.
    #[error("tmux config path is missing a file name")]
    MissingFileName,
    /// Existing markers are missing a closing delimiter.
    #[error("tmux config markers are incomplete")]
    IncompleteMarkers,
    /// IO failures while reading or writing the config file, including backups.
    #[error("failed to read or write tmux config: {0}")]
    Io(#[from] std::io::Error),
}

/// Install the tmux snippet into the specified configuration file.
///
/// The path is used verbatim: `install` does not expand a leading `~`. Callers
/// must resolve it to an absolute path first (`config::default_tmux_config_path`
/// does this), otherwise a literal `~` directory is created relative to the
/// working directory.
///
/// # Examples
///
/// ```rust,ignore
/// use dbar::install::install;
/// use dbar::types::StatusPosition;
///
/// // Already resolved; `~` is not expanded by `install`.
/// let path = dbar::config::default_tmux_config_path();
/// let outcome = install(Some(path), StatusPosition::Right, true, false)?;
/// assert!(outcome.dry_run);
/// # Ok::<(), dbar::install::InstallError>(())
/// ```
pub fn install(
    config_path_opt: Option<Utf8PathBuf>,
    position: StatusPosition,
    dry_run: bool,
    full: bool,
) -> Result<InstallOutcome, InstallError> {
    let config_path = config_path_opt.ok_or(InstallError::MissingPath)?;
    let snippet = build_snippet(position, full);

    let existing = match read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(InstallError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };

    let (updated, contents) = apply_snippet(&existing, &snippet)?;
    let backup_path = if should_back_up(updated, dry_run, &existing) {
        let backup = backup_path_for(&config_path);
        // The backup holds the config's contents, so it must inherit the
        // config's mode rather than the backup path's own (absent) mode.
        let (_, config_file_name) = split_parent(&config_path)?;
        write_inheriting(&backup, &existing, config_file_name)?;
        Some(backup)
    } else {
        None
    };

    if updated && !dry_run {
        write(&config_path, &contents)?;
    }

    Ok(InstallOutcome {
        path: config_path,
        backup_path,
        updated,
        dry_run,
        snippet,
    })
}

/// Report whether the existing config warrants a backup before writing.
///
/// A backup is only written when the snippet actually changes the file, the
/// run is not a dry run, and there is prior content worth preserving.
const fn should_back_up(updated: bool, dry_run: bool, existing: &str) -> bool {
    updated && !dry_run && !existing.is_empty()
}

fn apply_snippet(existing: &str, snippet: &str) -> Result<(bool, String), InstallError> {
    if let Some((before, after_start)) = existing.split_once(MARKER_START) {
        let Some((between, after_marker_end)) = after_start.split_once(MARKER_END) else {
            return Err(InstallError::IncompleteMarkers);
        };
        let (line_break, after_end) = after_marker_end
            .strip_prefix('\n')
            .map_or(("", after_marker_end), |rest| ("\n", rest));
        let current = format!("{MARKER_START}{between}{MARKER_END}{line_break}");
        if current == snippet {
            return Ok((false, existing.to_owned()));
        }
        let mut next = String::new();
        next.push_str(before);
        next.push_str(snippet);
        next.push_str(after_end);
        return Ok((true, next));
    }

    let mut next = String::from(existing);
    if !next.ends_with('\n') && !next.is_empty() {
        next.push('\n');
    }
    next.push_str(snippet);
    Ok((true, next))
}

fn build_snippet(position: StatusPosition, full: bool) -> String {
    let target = match position {
        StatusPosition::Left => "status-left",
        StatusPosition::Right => "status-right",
    };
    // Use tmux's `#{q:...}` modifier so each value is shell-quoted by tmux
    // before it is spliced into the `#(...)` command that tmux runs via
    // `/bin/sh`. Bare double quotes still permit command substitution, so an
    // unescaped path or session name could otherwise inject shell commands.
    let mut command = String::from(concat!(
        "dbar status --project-dir #{q:pane_current_path} ",
        "--session #{q:session_name} ",
        "--window #{q:window_index} ",
        "--pane #{q:pane_id} ",
        "--socket #{q:socket_path}"
    ));
    if matches!(position, StatusPosition::Right) {
        command.push_str(" --show-clock true");
    }
    if full {
        command.push_str(" --client-width #{q:client_width}");
    }
    let length_line = if full {
        format!("set -g {target}-length 999\n")
    } else {
        String::new()
    };
    format!("{MARKER_START}\nset -g {target} '#({command})'\n{length_line}{MARKER_END}\n")
}

fn backup_path_for(path: &Utf8Path) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{}.dbar.bak", path.as_str()))
}

fn read_to_string(path: &Utf8Path) -> Result<String, InstallError> {
    // Reads must never create directories: a missing parent surfaces as a
    // `NotFound` error that the caller treats as "no existing config", so a
    // `--dry-run` never mutates the filesystem.
    let (dir, file_name) = open_parent_for_read(path)?;
    Ok(dir.read_to_string(file_name)?)
}

fn write(path: &Utf8Path, contents: &str) -> Result<(), InstallError> {
    let (_, file_name) = split_parent(path)?;
    write_inheriting(path, contents, file_name)
}

/// Write `contents` to `path`, taking permissions from `permissions_from` — a
/// file name in the same parent directory.
///
/// A backup must inherit the mode of the config it copies rather than that of
/// its own (absent) destination, otherwise a `0600` config yields a
/// world-readable `0644` backup of the same content.
fn write_inheriting(
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
    let result = dir
        .write(tmp_name.as_str(), contents.as_bytes())
        .and_then(|()| inherit_permissions_from(&dir, tmp_name.as_str(), permissions_from))
        .and_then(|()| dir.rename(tmp_name.as_str(), &dir, file_name));
    if result.is_err() {
        // Best-effort cleanup; surface the original error, not the removal's.
        dir.remove_file(tmp_name.as_str()).ok();
    }
    result?;
    Ok(())
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

fn split_parent(path: &Utf8Path) -> Result<(&Utf8Path, &str), InstallError> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let file_name = path.file_name().ok_or(InstallError::MissingFileName)?;
    Ok((parent, file_name))
}

fn open_parent_for_read(path: &Utf8Path) -> Result<(Dir, &str), InstallError> {
    let (parent, file_name) = split_parent(path)?;
    let dir = Dir::open_ambient_dir(parent, ambient_authority())?;
    Ok((dir, file_name))
}

fn open_parent_for_write(path: &Utf8Path) -> Result<(Dir, &str), InstallError> {
    let (parent, file_name) = split_parent(path)?;
    Dir::create_ambient_dir_all(parent, ambient_authority())?;
    let dir = Dir::open_ambient_dir(parent, ambient_authority())?;
    Ok((dir, file_name))
}

#[cfg(test)]
mod tests;
