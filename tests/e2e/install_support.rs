//! Shared fixtures for the binary-level `dbar install` tests.
//!
//! The install tests are split across two modules — the single-process
//! behaviour in `install_cli` and the multi-process race in
//! `install_concurrency` — but both seed the same shaped configuration file and
//! inspect the same sibling paths, so those pieces live here.

use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use tempfile::TempDir;

/// Opening marker of the block `dbar install` manages.
pub const MARKER_START: &str = "# dbar: begin";
/// Closing marker of the block `dbar install` manages.
pub const MARKER_END: &str = "# dbar: end";

/// User-authored configuration preceding the managed block.
pub const PROLOGUE: &str = "# user prologue\nset -g mouse on\n";
/// How many filler lines pad the epilogue. See [`epilogue`].
const EPILOGUE_FILLER_LINES: usize = 4096;

/// The line repeated to pad the epilogue.
const EPILOGUE_FILLER_LINE: &str = "# filler padding\n";

/// User-authored configuration following the managed block.
///
/// The bulk is deliberate. Racing installs only contend if their
/// read-modify-write cycles overlap, and a config large enough to take a
/// measurable moment to read and rewrite widens that window enough for the
/// concurrency test to be a real guard rather than a coin toss.
pub fn epilogue() -> String {
    let mut text = String::from("# user epilogue\nset -g history-limit 50000\n");
    text.extend(std::iter::repeat_n(
        EPILOGUE_FILLER_LINE,
        EPILOGUE_FILLER_LINES,
    ));
    text
}

/// A token that appears only in the seeded, pre-install managed block.
///
/// No snippet `dbar install` generates contains it, so its presence in a file
/// identifies that file as the untouched seed rather than any install's output.
pub const SEEDED_TOKEN: &str = "seeded-marker";

/// A configuration carrying user content around a stale managed block.
///
/// The stale block matches no snippet `dbar install` can produce, so every
/// install against this seed reports an update and writes a backup.
pub fn seeded_config() -> String {
    format!(
        "{PROLOGUE}{MARKER_START}\nset -g status-left '#(dbar status --{SEEDED_TOKEN})'\n\
         {MARKER_END}\n{}",
        epilogue()
    )
}

/// The tmux configuration path inside a temporary directory.
pub fn config_path(temp_dir: &TempDir) -> io::Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(temp_dir.path().join("tmux.conf"))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "temp dir path is not utf8"))
}

/// The backup `dbar install` writes beside `path` before overwriting it.
pub fn backup_path(path: &Utf8Path) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{path}.dbar.bak"))
}

/// The lock file `dbar install` takes beside `path` for a real (non-dry) run.
pub fn lock_path(path: &Utf8Path) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{path}.dbar.lock"))
}

/// Read a configuration file as UTF-8.
pub fn read_config(path: &Utf8Path) -> io::Result<String> {
    let (dir, file_name) = open_parent(path)?;
    dir.read_to_string(file_name)
}

/// Write `contents` to a configuration file, creating it if absent.
pub fn write_config(path: &Utf8Path, contents: &str) -> io::Result<()> {
    let (dir, file_name) = open_parent(path)?;
    dir.write(file_name, contents)
}

/// Count non-overlapping occurrences of `needle` in `haystack`.
pub fn count_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

fn open_parent(path: &Utf8Path) -> io::Result<(cap_std::fs_utf8::Dir, &str)> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing file name"))?;
    let dir = cap_std::fs_utf8::Dir::open_ambient_dir(parent, cap_std::ambient_authority())?;
    Ok((dir, file_name))
}
