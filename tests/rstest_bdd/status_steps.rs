//! Behavioural steps for the tmux status line scenarios.

use std::io;
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
    // A fixture cannot return `Result` (its value is injected by type), and
    // Clippy forbids `expect` outside `#[test]` functions, so a failed setup
    // aborts the scenario with an explicit panic.
    let Ok(temp_dir) = TempDir::new() else {
        panic!("failed to create temp dir");
    };
    let Ok(repo_dir) = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()) else {
        panic!("temp dir path is not utf8");
    };
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
fn clean_repo(world: &mut World) -> io::Result<()> {
    init_repo(world, false)
}

#[given("a dirty git repository")]
fn dirty_repo(world: &mut World) -> io::Result<()> {
    init_repo(world, true)
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
fn contains_branch(world: &World) -> Result<(), String> {
    assert!(require_output(world)?.contains("main"));
    Ok(())
}

#[then("the status line contains the clean glyph")]
fn contains_clean_glyph(world: &World) -> Result<(), String> {
    assert!(require_output(world)?.contains(CLEAN_GLYPH));
    Ok(())
}

#[then("the status line contains the dirty glyph")]
fn contains_dirty_glyph(world: &World) -> Result<(), String> {
    assert!(require_output(world)?.contains(DIRTY_GLYPH));
    Ok(())
}

#[then("the status line contains the staged glyph")]
fn contains_staged_glyph(world: &World) -> Result<(), String> {
    assert!(require_output(world)?.contains(STAGED_GLYPH));
    Ok(())
}

fn require_output(world: &World) -> Result<&String, String> {
    world
        .output
        .as_ref()
        .ok_or_else(|| "status output was not captured".to_owned())
}

fn init_repo(world: &World, dirty: bool) -> io::Result<()> {
    run_git(world, ["init", "-b", "main"])?;
    if dirty {
        write_repo_file(world, "seed")?;
        run_git(world, ["add", "demo.txt"])?;
        write_repo_file(world, "seeded")?;
    }
    Ok(())
}

fn write_repo_file(world: &World, contents: &str) -> io::Result<()> {
    cap_std::fs_utf8::Dir::open_ambient_dir(world.repo_dir.clone(), cap_std::ambient_authority())
        .and_then(|dir| dir.write("demo.txt", contents))
}

fn run_git(world: &World, args: impl IntoIterator<Item = &'static str>) -> io::Result<()> {
    let status = Command::new("git")
        .args(args)
        .current_dir(world.repo_dir.as_std_path())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("git command failed"))
    }
}
