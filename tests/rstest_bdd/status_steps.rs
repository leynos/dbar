//! Behavioural steps for the tmux status line scenarios.

use std::process::Command;

use camino::Utf8PathBuf;
use rstest::fixture;
use rstest_bdd_macros::{given, scenario, then, when};
use tempfile::TempDir;

const CLEAN_GLYPH: &str = "\u{f42e}";
const DIRTY_GLYPH: &str = "\u{f444}";
const STAGED_GLYPH: &str = "\u{f457}";

#[derive(Debug)]
struct World {
    temp_dir: TempDir,
    repo_dir: Utf8PathBuf,
    output: Option<String>,
}

#[fixture]
fn world() -> World {
    let temp_dir = TempDir::new().unwrap_or_else(|err| panic!("temp dir: {err}"));
    let repo_dir = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf())
        .unwrap_or_else(|_| panic!("utf8 temp path"));
    World {
        temp_dir,
        repo_dir,
        output: None,
    }
}

#[scenario("tests/rstest_bdd/status.feature")]
fn status_scenarios(world: World) {
    let _ = world;
}

#[given("a clean git repository")]
fn clean_repo(world: &mut World) {
    init_repo(world, false);
}

#[given("a dirty git repository")]
fn dirty_repo(world: &mut World) {
    init_repo(world, true);
}

#[when("I run dbar status")]
fn run_status(world: &mut World) {
    let _temp_dir_path = world.temp_dir.path();
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("dbar");
    cmd.args([
        "status",
        "--project-dir",
        world.repo_dir.as_str(),
        "--show-pr",
        "false",
        "--session",
        "demo",
        "--window",
        "1",
        "--pane",
        "%0",
    ]);
    let output = cmd.assert().success().get_output().stdout.clone();
    world.output = Some(String::from_utf8_lossy(&output).trim().to_owned());
}

#[then("the status line contains the branch name \"main\"")]
fn contains_branch(world: &World) {
    let output = world.output.as_ref().unwrap_or_else(|| panic!("output"));
    assert!(output.contains("main"));
}

#[then("the status line contains the clean glyph")]
fn contains_clean_glyph(world: &World) {
    let output = world.output.as_ref().unwrap_or_else(|| panic!("output"));
    assert!(output.contains(CLEAN_GLYPH));
}

#[then("the status line contains the dirty glyph")]
fn contains_dirty_glyph(world: &World) {
    let output = world.output.as_ref().unwrap_or_else(|| panic!("output"));
    assert!(output.contains(DIRTY_GLYPH));
}

#[then("the status line contains the staged glyph")]
fn contains_staged_glyph(world: &World) {
    let output = world.output.as_ref().unwrap_or_else(|| panic!("output"));
    assert!(output.contains(STAGED_GLYPH));
}

fn init_repo(world: &World, dirty: bool) {
    let _temp_dir_path = world.temp_dir.path();
    run_git(world, ["init", "-b", "main"]);
    if dirty {
        cap_std::fs_utf8::Dir::open_ambient_dir(
            world.repo_dir.clone(),
            cap_std::ambient_authority(),
        )
        .and_then(|dir| dir.write("demo.txt", "seed"))
        .unwrap_or_else(|err| panic!("write file: {err}"));
        run_git(world, ["add", "demo.txt"]);
        cap_std::fs_utf8::Dir::open_ambient_dir(
            world.repo_dir.clone(),
            cap_std::ambient_authority(),
        )
        .and_then(|dir| dir.write("demo.txt", "seeded"))
        .unwrap_or_else(|err| panic!("write file: {err}"));
    }
}

fn run_git(world: &World, args: impl IntoIterator<Item = &'static str>) {
    let status = Command::new("git")
        .args(args)
        .current_dir(world.repo_dir.as_std_path())
        .status()
        .unwrap_or_else(|err| panic!("git command: {err}"));
    assert!(status.success());
}
