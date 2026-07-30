//! tmux configuration installation helpers.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;
use thiserror::Error;

use crate::types::StatusPosition;

const MARKER_START: &str = "# dbar: begin";
const MARKER_END: &str = "# dbar: end";

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
    /// IO failures while reading or writing the config file.
    #[error("failed to read tmux config: {0}")]
    Io(#[from] std::io::Error),
}

/// Install the tmux snippet into the specified configuration file.
///
/// # Examples
///
/// ```rust,ignore
/// use dbar::install::install;
/// use dbar::types::StatusPosition;
///
/// let outcome = install(Some("~/.tmux.conf".into()), StatusPosition::Right, true, false)?;
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
        write(&backup, &existing)?;
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
    let (dir, file_name) = open_parent_for_write(path)?;
    Ok(dir.write(file_name, contents.as_bytes())?)
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
mod tests {
    //! Behavioural tests for tmux snippet installation, idempotence, and layout.
    use super::*;
    use camino::Utf8PathBuf;
    use rstest::rstest;
    use tempfile::TempDir;

    /// Create a temporary directory and the `tmux.conf` path within it.
    fn workspace() -> Result<(TempDir, Utf8PathBuf), InstallError> {
        let temp_dir = TempDir::new().map_err(InstallError::Io)?;
        let path = Utf8PathBuf::from_path_buf(temp_dir.path().join("tmux.conf"))
            .map_err(|_| InstallError::MissingFileName)?;
        Ok((temp_dir, path))
    }

    #[rstest]
    fn install_writes_snippet() {
        let (_temp_dir, path) = workspace().expect("workspace");
        let initial = "set -g status on\n";
        write(&path, initial).expect("write config");

        let outcome = install(Some(path.clone()), StatusPosition::Right, false, false)
            .expect("install snippet");
        assert!(outcome.updated);
        assert!(outcome.backup_path.is_some());

        let contents = read_to_string(&path).expect("read config");
        assert!(contents.contains(MARKER_START));
        assert!(contents.contains(MARKER_END));
    }

    #[rstest]
    fn install_is_idempotent() {
        let (_temp_dir, path) = workspace().expect("workspace");
        let _ = install(Some(path.clone()), StatusPosition::Right, false, false)
            .expect("install snippet");
        let second = install(Some(path.clone()), StatusPosition::Right, false, false)
            .expect("install snippet");
        assert!(!second.updated);
    }

    #[rstest]
    fn install_full_adds_client_width() {
        let (_temp_dir, path) = workspace().expect("workspace");
        let outcome =
            install(Some(path), StatusPosition::Left, true, true).expect("install snippet");
        assert!(outcome.snippet.contains("--client-width #{q:client_width}"));
        assert!(outcome.snippet.contains("status-left-length 999"));
    }

    #[rstest]
    fn install_right_enables_clock() {
        let (_temp_dir, path) = workspace().expect("workspace");
        let outcome =
            install(Some(path), StatusPosition::Right, true, false).expect("install snippet");
        assert!(outcome.snippet.contains("--show-clock true"));
        assert!(outcome.snippet.contains("status-right"));
    }

    #[rstest]
    fn install_left_omits_clock() {
        let (_temp_dir, path) = workspace().expect("workspace");
        let outcome =
            install(Some(path), StatusPosition::Left, true, false).expect("install snippet");
        assert!(!outcome.snippet.contains("--show-clock true"));
    }

    #[rstest]
    fn install_dry_run_leaves_missing_parent_absent() {
        let (temp_dir, _) = workspace().expect("workspace");
        let missing_parent = Utf8PathBuf::from_path_buf(temp_dir.path().join("missing"))
            .expect("missing parent path");
        let config = missing_parent.join("tmux.conf");
        let outcome =
            install(Some(config), StatusPosition::Left, true, false).expect("dry run install");
        assert!(outcome.dry_run);
        // The parent directory must not have been created by the dry run.
        assert!(Dir::open_ambient_dir(missing_parent.as_path(), ambient_authority()).is_err());
    }

    #[rstest]
    fn install_snippet_shell_quotes_tmux_formats() {
        let snippet = build_snippet(StatusPosition::Left, true);
        for token in [
            "#{q:pane_current_path}",
            "#{q:session_name}",
            "#{q:window_index}",
            "#{q:pane_id}",
            "#{q:socket_path}",
            "#{q:client_width}",
        ] {
            assert!(snippet.contains(token), "snippet missing {token}");
        }
        // The unquoted forms that permitted shell injection must be gone.
        assert!(!snippet.contains("\"#{pane_current_path}\""));
        assert!(!snippet.contains("\"#{client_width}\""));
    }
}
