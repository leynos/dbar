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
/// let outcome = install(Some("~/.tmux.conf".into()), StatusPosition::Right, true)?;
/// assert!(outcome.dry_run);
/// # Ok::<(), dbar::install::InstallError>(())
/// ```
pub fn install(
    config_path_opt: Option<Utf8PathBuf>,
    position: StatusPosition,
    dry_run: bool,
) -> Result<InstallOutcome, InstallError> {
    let config_path = config_path_opt.ok_or(InstallError::MissingPath)?;
    let snippet = build_snippet(position);

    let existing = match read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(InstallError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };

    let (updated, contents) = apply_snippet(&existing, &snippet)?;
    let backup_path = if updated && !dry_run && !existing.is_empty() {
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

fn build_snippet(position: StatusPosition) -> String {
    let target = match position {
        StatusPosition::Left => "status-left",
        StatusPosition::Right => "status-right",
    };
    let command = concat!(
        "dbar status --session \"#{session_name}\" ",
        "--window \"#{window_index}\" ",
        "--pane \"#{pane_id}\" ",
        "--socket \"#{socket_path}\""
    );
    format!("{MARKER_START}\nset -g {target} '#({command})'\n{MARKER_END}\n")
}

fn backup_path_for(path: &Utf8Path) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{}.dbar.bak", path.as_str()))
}

fn read_to_string(path: &Utf8Path) -> Result<String, InstallError> {
    let (dir, file_name) = open_parent(path)?;
    Ok(dir.read_to_string(file_name)?)
}

fn write(path: &Utf8Path, contents: &str) -> Result<(), InstallError> {
    let (dir, file_name) = open_parent(path)?;
    Ok(dir.write(file_name, contents.as_bytes())?)
}

fn open_parent(path: &Utf8Path) -> Result<(Dir, &str), InstallError> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let file_name = path.file_name().ok_or(InstallError::MissingFileName)?;
    Dir::create_ambient_dir_all(parent, ambient_authority())?;
    let dir = Dir::open_ambient_dir(parent, ambient_authority())?;
    Ok((dir, file_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use rstest::fixture;
    use rstest::rstest;
    use tempfile::TempDir;

    #[fixture]
    fn temp_dir() -> TempDir {
        TempDir::new().expect("temp dir")
    }

    #[rstest]
    fn install_writes_snippet(temp_dir: TempDir) {
        let path = Utf8PathBuf::from_path_buf(temp_dir.path().join("tmux.conf"))
            .map_err(|_| InstallError::MissingFileName)
            .expect("tmux config path");
        let initial = "set -g status on\n";
        write(&path, initial).expect("write config");

        let outcome =
            install(Some(path.clone()), StatusPosition::Right, false).expect("install snippet");
        assert!(outcome.updated);
        assert!(outcome.backup_path.is_some());

        let contents = read_to_string(&path).expect("read config");
        assert!(contents.contains(MARKER_START));
        assert!(contents.contains(MARKER_END));
    }

    #[rstest]
    fn install_is_idempotent(temp_dir: TempDir) {
        let path = Utf8PathBuf::from_path_buf(temp_dir.path().join("tmux.conf"))
            .map_err(|_| InstallError::MissingFileName)
            .expect("tmux config path");
        let _ = install(Some(path.clone()), StatusPosition::Right, false).expect("install snippet");
        let second =
            install(Some(path.clone()), StatusPosition::Right, false).expect("install snippet");
        assert!(!second.updated);
    }
}
