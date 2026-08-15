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
    /// Kept alive purely so the temporary directory survives the scenario.
    _temp_dir: TempDir,
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
        _temp_dir: temp_dir,
        repo_dir,
        output: None,
    }
}

// The parameter name is the fixture name that `#[scenario]` exposes to the
// steps, so it must stay `world`: renaming it to `_world` renames the fixture
// and every step binding fails to resolve at run time.
#[scenario("tests/rstest_bdd/status.feature")]
fn status_scenarios(world: World) {}

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

fn init_repo(world: &World, is_dirty: bool) -> io::Result<()> {
    // An empty template keeps the developer's `~/.git-templates` hooks and
    // description out of the fixture repository.
    run_git(world, ["init", "-b", "main", "--template="])?;
    // Pin the repository-local settings the steps depend on rather than
    // inheriting whatever the ambient configuration would have supplied.
    run_git(world, ["config", "core.hooksPath", "/dev/null"])?;
    if is_dirty {
        write_repo_file(world, "seed")?;
        run_git(world, ["add", "demo.txt"])?;
        write_repo_file(world, "seeded")?;
    }
    Ok(())
}

fn write_repo_file(world: &World, contents: &str) -> io::Result<()> {
    cap_std::fs_utf8::Dir::open_ambient_dir(world.repo_dir.as_path(), cap_std::ambient_authority())
        .and_then(|dir| dir.write("demo.txt", contents))
}

/// Run git in the scenario's repository, isolated from ambient configuration.
///
/// A developer's global or system configuration can otherwise reach into the
/// fixture — `init.defaultBranch`, `core.hooksPath`, `init.templateDir` and
/// the like — and change what the steps observe. Pointing both configuration
/// files at `/dev/null` and setting `GIT_CONFIG_NOSYSTEM` makes the repository
/// depend only on the arguments passed here.
fn run_git(world: &World, args: impl IntoIterator<Item = &'static str>) -> io::Result<()> {
    let git_args: Vec<&'static str> = args.into_iter().collect();
    let output = Command::new("git")
        .args(&git_args)
        .current_dir(world.repo_dir.as_std_path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        // Carry the invocation and git's own diagnosis into the error: a bare
        // "git command failed" says nothing about which step broke or why.
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::other(format!(
            "`git {}` failed with {}: {}",
            git_args.join(" "),
            output.status,
            stderr.trim()
        )))
    }
}
