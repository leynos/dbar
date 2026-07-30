//! Binary-level coverage for the `dbar install` subcommand.

use std::io;

use camino::Utf8PathBuf;
use tempfile::TempDir;

const MARKER_START: &str = "# dbar: begin";
const MARKER_END: &str = "# dbar: end";

#[test]
fn install_dry_run_previews_without_writing() {
    let temp_dir = TempDir::new().expect("temp dir");
    let config = config_path(&temp_dir).expect("config path");

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("dbar");
    cmd.args(["install", "--path", config.as_str(), "--dry-run"]);
    let output = cmd.assert().success().get_output().stdout.clone();
    let text = String::from_utf8_lossy(&output);

    assert!(text.contains("Dry run for"));
    assert!(text.contains(MARKER_START));
    // A dry run must leave the filesystem untouched.
    assert!(!config.as_std_path().exists());
}

#[test]
fn install_writes_and_is_idempotent() {
    let temp_dir = TempDir::new().expect("temp dir");
    let config = config_path(&temp_dir).expect("config path");

    let mut first = assert_cmd::cargo::cargo_bin_cmd!("dbar");
    first.args(["install", "--path", config.as_str()]);
    let first_out = first.assert().success().get_output().stdout.clone();
    assert!(String::from_utf8_lossy(&first_out).contains("Updated tmux config"));

    let contents = read_config(&config).expect("read config");
    assert!(contents.contains(MARKER_START));
    assert!(contents.contains(MARKER_END));
    // The snippet must shell-quote every tmux format it interpolates.
    assert!(contents.contains("#{q:pane_current_path}"));
    assert!(!contents.contains("\"#{pane_current_path}\""));

    let mut second = assert_cmd::cargo::cargo_bin_cmd!("dbar");
    second.args(["install", "--path", config.as_str()]);
    let second_out = second.assert().success().get_output().stdout.clone();
    assert!(String::from_utf8_lossy(&second_out).contains("already up to date"));

    let after = read_config(&config).expect("read config again");
    assert_eq!(contents, after);
}

fn config_path(temp_dir: &TempDir) -> io::Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(temp_dir.path().join("tmux.conf"))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "temp dir path is not utf8"))
}

fn read_config(path: &Utf8PathBuf) -> io::Result<String> {
    let parent = path.parent().unwrap_or_else(|| camino::Utf8Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing file name"))?;
    cap_std::fs_utf8::Dir::open_ambient_dir(parent, cap_std::ambient_authority())
        .and_then(|dir| dir.read_to_string(file_name))
}
