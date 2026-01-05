//! Snapshot coverage for the status line output.

use camino::Utf8PathBuf;
use tempfile::TempDir;

#[test]
fn status_snapshot_without_git() {
    let temp_dir = TempDir::new().unwrap_or_else(|err| panic!("temp dir: {err}"));
    let temp_dir_path = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf())
        .unwrap_or_else(|_| panic!("temp dir path is not utf8"));
    let project_dir = temp_dir_path.join("project");
    cap_std::fs_utf8::Dir::open_ambient_dir(temp_dir_path.as_path(), cap_std::ambient_authority())
        .and_then(|dir| dir.create_dir("project"))
        .unwrap_or_else(|err| panic!("create project dir: {err}"));

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("dbar");
    cmd.args([
        "status",
        "--project-dir",
        project_dir.as_str(),
        "--show-pr",
        "false",
        "--session",
        "demo",
        "--window",
        "1",
        "--pane",
        "%0",
        "--socket",
        "/tmp/tmux-demo",
    ]);
    let output = cmd.assert().success().get_output().stdout.clone();
    let text = String::from_utf8_lossy(&output).trim().to_owned();
    insta::assert_snapshot!(text);
}
