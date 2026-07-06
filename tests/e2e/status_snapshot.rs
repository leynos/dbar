//! Snapshot coverage for the status line output.

use std::process::Command;

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

#[test]
fn status_renders_configured_clock() {
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
        "--show-clock",
        "true",
        "--clock-format",
        "clock",
        "--session",
        "demo",
        "--window",
        "1",
        "--pane",
        "%0",
    ]);
    let output = cmd.assert().success().get_output().stdout.clone();
    let text = String::from_utf8_lossy(&output).trim().to_owned();
    assert!(text.contains("\u{f017} clock"));
}

#[test]
fn status_snapshot_clean_git_full_width_with_pr() {
    let temp_dir = TempDir::new().unwrap_or_else(|err| panic!("temp dir: {err}"));
    let repo_dir = create_repo_dir(&temp_dir);
    init_repo(&repo_dir);

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("dbar");
    cmd.args([
        "status",
        "--project-dir",
        repo_dir.as_str(),
        "--show-pr",
        "true",
        "--github-mock-pr",
        "42",
        "--client-width",
        "80",
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
    let text = String::from_utf8_lossy(&output).trim_end().to_owned();
    insta::assert_snapshot!(text);
}

#[test]
fn status_snapshot_dirty_git_default_width() {
    let temp_dir = TempDir::new().unwrap_or_else(|err| panic!("temp dir: {err}"));
    let repo_dir = create_repo_dir(&temp_dir);
    init_repo(&repo_dir);
    mark_repo_dirty(&repo_dir);

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("dbar");
    cmd.args([
        "status",
        "--project-dir",
        repo_dir.as_str(),
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
    let text = String::from_utf8_lossy(&output).trim_end().to_owned();
    insta::assert_snapshot!(text);
}

fn init_repo(path: &Utf8PathBuf) {
    run_git(path, ["init", "-b", "main"]);
    write_file(path, "README.md", "seed");
    run_git(path, ["add", "README.md"]);
    run_git(
        path,
        [
            "-c",
            "user.name=dbar",
            "-c",
            "user.email=dbar@example.com",
            "commit",
            "-m",
            "init",
        ],
    );
}

fn create_repo_dir(temp_dir: &TempDir) -> Utf8PathBuf {
    let temp_dir_path = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf())
        .unwrap_or_else(|_| panic!("temp dir path is not utf8"));
    let repo_dir = temp_dir_path.join("project");
    cap_std::fs_utf8::Dir::open_ambient_dir(temp_dir_path.as_path(), cap_std::ambient_authority())
        .and_then(|dir| dir.create_dir("project"))
        .unwrap_or_else(|err| panic!("create repo dir: {err}"));
    repo_dir
}

fn mark_repo_dirty(path: &Utf8PathBuf) {
    write_file(path, "README.md", "updated");
    run_git(path, ["add", "README.md"]);
    write_file(path, "README.md", "dirty");
}

fn write_file(path: &Utf8PathBuf, name: &str, contents: &str) {
    cap_std::fs_utf8::Dir::open_ambient_dir(path.as_path(), cap_std::ambient_authority())
        .and_then(|dir| dir.write(name, contents))
        .unwrap_or_else(|err| panic!("write file: {err}"));
}

fn run_git(path: &Utf8PathBuf, args: impl IntoIterator<Item = &'static str>) {
    let status = Command::new("git")
        .args(args)
        .current_dir(path.as_std_path())
        .status()
        .unwrap_or_else(|err| panic!("git command: {err}"));
    assert!(status.success());
}
