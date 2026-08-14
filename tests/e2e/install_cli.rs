//! Binary-level coverage for the `dbar install` subcommand.

use std::io;

use tempfile::TempDir;

use super::install_support::{
    MARKER_END, MARKER_START, backup_path, config_path, lock_path, read_config, seeded_config,
    write_config,
};

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

/// A dry run must not disturb a configuration that already exists.
///
/// The case above has nothing to lose: no config file is present, so "left the
/// filesystem untouched" reduces to "created nothing". This case seeds a real
/// user configuration whose managed block is stale, so a non-dry run would
/// rewrite the file and leave a backup behind, then asserts that the dry run
/// does neither.
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixtures with `?`; assertions remain the idiomatic failure mechanism"
)]
fn install_dry_run_preserves_existing_config() -> io::Result<()> {
    let temp_dir = TempDir::new().expect("temp dir");
    let config = config_path(&temp_dir)?;
    let seed = seeded_config();
    write_config(&config, &seed)?;
    let before = read_config(&config)?;
    assert_eq!(before, seed, "the seed must land verbatim");

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("dbar");
    cmd.args([
        "install",
        "--path",
        config.as_str(),
        "--position",
        "right",
        "--full",
        "--dry-run",
    ]);
    let output = cmd.assert().success().get_output().stdout.clone();
    assert_previews_full_right_snippet(&String::from_utf8_lossy(&output));

    assert_eq!(
        read_config(&config)?,
        before,
        "a dry run must leave the existing config byte-identical"
    );
    assert_no_side_files(&config);
    Ok(())
}

/// Assert the dry run previewed the requested variant, not the seeded block.
#[track_caller]
fn assert_previews_full_right_snippet(text: &str) {
    assert!(text.contains("Dry run for"));
    assert!(text.contains(MARKER_START));
    assert!(text.contains(MARKER_END));
    assert!(text.contains("--client-width"), "`--full` was requested");
    assert!(
        text.contains("status-right"),
        "`--position right` was requested"
    );
}

/// Assert a dry run left no backup and no lock file beside `config`.
#[track_caller]
fn assert_no_side_files(config: &camino::Utf8Path) {
    assert!(
        !backup_path(config).as_std_path().exists(),
        "a dry run must not write a backup"
    );
    // A dry run deliberately takes no lock, so it must not create the lock file
    // either; doing so would breach the "mutates nothing" guarantee.
    assert!(
        !lock_path(config).as_std_path().exists(),
        "a dry run must not create the install lock file"
    );
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
