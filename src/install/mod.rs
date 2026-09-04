//! tmux configuration installation helpers.

use camino::Utf8PathBuf;
use cap_std::fs_utf8::File;
use thiserror::Error;

use crate::types::StatusPosition;

mod fs;
mod snippet;

use fs::{acquire_lock, backup_path_for, read_to_string, split_parent, write, write_inheriting};
use snippet::{apply_snippet, build_snippet};

/// Whether an install applies its result or only previews it.
///
/// This and [`Width`] used to be adjacent `bool` parameters of [`install`].
/// Two adjacent booleans of the same type can be transposed without the
/// compiler noticing, and transposing these two turns a preview into a write of
/// the wrong variant — the one mistake this function must never make silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Report what would change, touching nothing.
    DryRun,
    /// Take the lock, write the backup, and update the config.
    Write,
}

impl RunMode {
    /// Build the mode from a `--dry-run` flag.
    ///
    /// # Examples
    ///
    /// ```
    /// # use dbar::install::RunMode;
    /// assert_eq!(RunMode::from_dry_run(true), RunMode::DryRun);
    /// assert_eq!(RunMode::from_dry_run(false), RunMode::Write);
    /// ```
    #[must_use]
    pub const fn from_dry_run(is_dry_run: bool) -> Self {
        if is_dry_run {
            Self::DryRun
        } else {
            Self::Write
        }
    }

    /// Report whether this run only previews its result.
    #[must_use]
    pub const fn is_dry_run(self) -> bool {
        matches!(self, Self::DryRun)
    }
}

/// Whether the snippet claims the full width of the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    /// Pass the client width through and lift tmux's status-line length cap.
    Full,
    /// Leave tmux's own length limit in place.
    Plain,
}

impl Width {
    /// Build the width from a `--full` flag.
    ///
    /// # Examples
    ///
    /// ```
    /// # use dbar::install::Width;
    /// assert_eq!(Width::from_full(true), Width::Full);
    /// assert_eq!(Width::from_full(false), Width::Plain);
    /// ```
    #[must_use]
    pub const fn from_full(is_full: bool) -> Self {
        if is_full { Self::Full } else { Self::Plain }
    }

    /// Report whether the full-width variant was requested.
    #[must_use]
    pub const fn is_full(self) -> bool {
        matches!(self, Self::Full)
    }
}

#[derive(Debug)]
/// Summary of an install operation.
pub struct InstallOutcome {
    /// The config file that was targeted.
    pub path: Utf8PathBuf,
    /// Optional backup path when a file was overwritten.
    pub backup_path: Option<Utf8PathBuf>,
    /// Whether the file contents changed.
    pub is_updated: bool,
    /// Whether the install was a dry run.
    pub is_dry_run: bool,
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
    /// The config holds more than one managed block.
    #[error("tmux config holds more than one dbar block; remove the extras")]
    DuplicateMarkers,
    /// Another install is already updating this configuration file.
    #[error("tmux config is locked by another install; try again")]
    Locked,
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
/// ```text
/// // Already resolved; `~` is not expanded by `install`.
/// let path = dbar::config::default_tmux_config_path();
/// let outcome = install(Some(path), StatusPosition::Right, RunMode::DryRun, Width::Plain)?;
/// assert!(outcome.is_dry_run);
/// ```
pub fn install(
    config_path_opt: Option<Utf8PathBuf>,
    position: StatusPosition,
    mode: RunMode,
    width: Width,
) -> Result<InstallOutcome, InstallError> {
    let config_path = config_path_opt.ok_or(InstallError::MissingPath)?;
    let snippet = build_snippet(position, width);

    // Serialize the whole read-modify-write against competing installs. The
    // guard is bound for the rest of the function so the read, the backup, and
    // the final rename form one transaction; it is released when the file is
    // dropped on return.
    //
    // A dry run takes no lock: it mutates nothing, and because every write
    // lands by atomic rename it can only ever observe a whole file, never a
    // torn one. Locking would also create the lock file — and its parent
    // directory — for a run that promises to leave the filesystem untouched.
    let _lock: Option<File> = match mode {
        RunMode::DryRun => None,
        RunMode::Write => Some(acquire_lock(&config_path)?),
    };

    let existing = match read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(InstallError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };

    let (is_updated, contents) = apply_snippet(&existing, &snippet)?;
    let backup_path = if should_back_up(is_updated, mode, &existing) {
        let backup = backup_path_for(&config_path);
        // The backup holds the config's contents, so it must inherit the
        // config's mode rather than the backup path's own (absent) mode.
        let (_, config_file_name) = split_parent(&config_path)?;
        write_inheriting(&backup, &existing, config_file_name)?;
        Some(backup)
    } else {
        None
    };

    if is_updated && !mode.is_dry_run() {
        write(&config_path, &contents)?;
    }

    Ok(InstallOutcome {
        path: config_path,
        backup_path,
        is_updated,
        is_dry_run: mode.is_dry_run(),
        snippet,
    })
}

/// Report whether the existing config warrants a backup before writing.
///
/// A backup is only written when the snippet actually changes the file, the
/// run is not a dry run, and there is prior content worth preserving.
const fn should_back_up(is_updated: bool, mode: RunMode, existing: &str) -> bool {
    is_updated && !mode.is_dry_run() && !existing.is_empty()
}

#[cfg(test)]
mod property_tests;
#[cfg(test)]
mod quoting_tests;
#[cfg(test)]
mod tests;
