//! Tests for the bounded config read behind an install transaction.
use camino::Utf8PathBuf;
use rstest::{fixture, rstest};
use tempfile::TempDir;

use super::{MAX_CONFIG_BYTES, backup_path_for, open_parent_for_read, read_to_string, write};
use crate::install::{InstallError, RunMode, Width, install};
use crate::types::StatusPosition;

/// A temporary directory and the `tmux.conf` path within it.
type Workspace = Result<(TempDir, Utf8PathBuf), InstallError>;

/// Create a temporary directory and the `tmux.conf` path within it.
#[fixture]
fn workspace() -> Workspace {
    let temp_dir = TempDir::new().map_err(InstallError::Io)?;
    let path = Utf8PathBuf::from_path_buf(temp_dir.path().join("tmux.conf"))
        .map_err(|_| InstallError::MissingFileName)?;
    Ok((temp_dir, path))
}

/// A config one byte past the ceiling, built from a valid tmux comment line.
fn oversized_config() -> String {
    let mut contents = String::with_capacity(MAX_CONFIG_BYTES + 1);
    contents.push('#');
    while contents.len() <= MAX_CONFIG_BYTES {
        contents.push('a');
    }
    contents
}

#[rstest]
fn read_refuses_a_config_past_the_ceiling(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    let contents = oversized_config();
    assert!(contents.len() > MAX_CONFIG_BYTES);
    write(&path, &contents).expect("write oversized config");

    let error = read_to_string(&path).expect_err("an oversized config must be refused");
    let InstallError::Io(io_error) = error else {
        panic!("expected an IO error, got {error:?}");
    };
    assert_eq!(io_error.kind(), std::io::ErrorKind::FileTooLarge);
}

#[rstest]
fn read_accepts_a_config_at_the_ceiling(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    let mut contents = oversized_config();
    contents.truncate(MAX_CONFIG_BYTES);
    write(&path, &contents).expect("write config at the ceiling");

    let observed = read_to_string(&path).expect("a config at the ceiling must be readable");
    assert_eq!(observed.len(), MAX_CONFIG_BYTES);
}

#[rstest]
fn install_refuses_an_oversized_config_without_touching_it(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    let contents = oversized_config();
    write(&path, &contents).expect("write oversized config");

    let error = install(
        Some(path.clone()),
        StatusPosition::Right,
        RunMode::Write,
        Width::Plain,
    )
    .expect_err("an oversized config must fail the install");
    let InstallError::Io(io_error) = error else {
        panic!("expected an IO error, got {error:?}");
    };
    assert_eq!(io_error.kind(), std::io::ErrorKind::FileTooLarge);

    // The refusal must leave the user's file exactly as it was: an oversized
    // config is never truncated, rewritten, or mistaken for an absent one.
    let backup = backup_path_for(&path);
    let (dir, backup_name) = open_parent_for_read(&backup).expect("open parent");
    assert!(
        dir.metadata(backup_name).is_err(),
        "a refused install must not write a backup"
    );
    // The backup is a sibling of the config, so one directory handle serves
    // both lookups.
    let config_name = path.file_name().expect("config file name");
    let surviving = dir.metadata(config_name).expect("config metadata");
    assert_eq!(
        usize::try_from(surviving.len()).expect("config length"),
        contents.len(),
        "a refused install must leave the config byte-for-byte intact"
    );
}
